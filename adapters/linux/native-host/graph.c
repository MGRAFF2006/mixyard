// SPDX-License-Identifier: MIT
// A private-graph qualification client, not a physical-device backend.
#include <errno.h>
#include <inttypes.h>
#include <math.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>
#include <pipewire/pipewire.h>
#include <pipewire/filter.h>
#include <pipewire/data-loop.h>
#include <spa/param/audio/format-utils.h>
#include <spa/buffer/meta.h>
#include <json-c/json.h>
#include "mixyard_host.h"
#include "timeline.h"
#include "destination.h"

#define ROUTES 3
#define FRAMES 128
#define STRIDE (2 * sizeof(double))
#define LIMIT (FRAMES * STRIDE)
#define MAX_GLOBALS 256

enum test_mode { NORMAL, OUTPUT_FAILURE, TAIL_LOSS };
struct app;
struct endpoint {
    struct app *app;
    struct pw_filter *filter;
    struct spa_hook listener;
    unsigned role; // 0 generator; 1 shared-engine capture; 2 actual consumer
    void *in[ROUTES], *out[ROUTES];
};
struct port { _Atomic bool format_ok; };
struct global { uint32_t id, node; uint64_t serial; char type[32], name[96], direction[8]; };
struct app {
    struct pw_main_loop *loop;
    struct pw_context *context;
    struct pw_data_loop *data_loop;
    struct pw_core *core;
    struct pw_registry *registry;
    struct spa_hook registry_hook, core_hook;
    struct spa_source *timer;
    struct endpoint nodes[3];
    struct global globals[MAX_GLOBALS];
    size_t global_count;
    struct pw_proxy *links[ROUTES * 2];
    struct mx_host *host; // exclusively accessed by graph data-loop owner
    enum test_mode mode;
    _Atomic bool source_missing, source_removed;
    uint32_t removed_source_port;
    _Atomic uint64_t epoch_frame, overflow_frame, loss_frame, inactive[ROUTES];
    uint64_t failure_silence;
    _Atomic uint32_t clock_id;
    _Atomic unsigned command, command_done, phase, error;
    _Atomic uint64_t delivered[ROUTES], silence[ROUTES], mismatches, rendered, dropped, heap_ops;
    uint64_t generation, quiet_rendered;
    mx_graph_timeline timeline;
    uint64_t quiet_silence[ROUTES];
    _Atomic uint32_t role_clock[3];
    unsigned ticks, stage, last_ack;
    uint64_t baseline[ROUTES];
    bool quiet_started;
    bool topology_written, linked, expected_loss, cancelled;
    const char *directory;
};

// Linker wrapping measures heap operations in this client and the static Rust host.
// The distro libpipewire shared object's internals are outside this measurement.
static _Thread_local struct app *callback_owner;
void *__real_malloc(size_t); void *__real_calloc(size_t, size_t);
void *__real_realloc(void *, size_t); void __real_free(void *);
int __real_posix_memalign(void **, size_t, size_t);
void *__real_aligned_alloc(size_t, size_t);
static void heap_operation(void) {
    if (callback_owner) atomic_fetch_add_explicit(&callback_owner->heap_ops, 1, memory_order_relaxed);
}
void *__wrap_malloc(size_t n) { heap_operation(); return __real_malloc(n); }
void *__wrap_calloc(size_t n, size_t size) { heap_operation(); return __real_calloc(n, size); }
void *__wrap_realloc(void *p, size_t n) { heap_operation(); return __real_realloc(p, n); }
void __wrap_free(void *p) { heap_operation(); __real_free(p); }
int __wrap_posix_memalign(void **p, size_t alignment, size_t n) { heap_operation(); return __real_posix_memalign(p, alignment, n); }
void *__wrap_aligned_alloc(size_t alignment, size_t n) { heap_operation(); return __real_aligned_alloc(alignment, n); }
static double source_value(unsigned source, uint64_t frame, unsigned channel) {
    // Distinct channels, Float64 low bits and signed zero exercise native bytes.
    if (source == 0) {
        if ((frame & 31) == 0) return channel ? -0.0 : 0.0;
        return (channel ? -0.125 : 0.25) + (double)(frame % 7) * 0x1p-42;
    }
    return source == 1 ? (channel ? 0.375 : 0.5) : 0.875;
}
static double program_value(struct app *a, uint64_t frame, unsigned channel) {
    return frame >= atomic_load_explicit(&a->overflow_frame, memory_order_acquire) ? 0x1p1023 : source_value(0, frame, channel);
}
static double oracle(struct app *a, unsigned route, uint64_t frame, unsigned channel) {
    if (a->mode == TAIL_LOSS) {
        uint64_t epoch = atomic_load_explicit(&a->epoch_frame, memory_order_acquire);
        uint64_t loss = atomic_load_explicit(&a->loss_frame, memory_order_acquire);
        if (frame >= loss) {
            return route == 1 && frame - loss >= FRAMES ? source_value(1, frame-FRAMES, channel) * 0.125 : 0.0;
        }
        if (frame < epoch || frame - epoch < FRAMES) return 0.0;
        frame -= FRAMES;
    }
    double program = program_value(a, frame, channel);
    // Private source has no sends. Call-return excluded from Call by policy.
    if (route == 0) return program;
    if (route == 1) return (program * 0.25 + source_value(1, frame, channel) * 0.125) * (a->mode == OUTPUT_FAILURE ? 16.0 : 1.0);
    return program * 0.5;
}
static void fail(struct app *a, unsigned error) {
    unsigned zero = 0;
    atomic_compare_exchange_strong(&a->error, &zero, error);
}
static struct spa_meta_header *header(struct pw_buffer *b) {
    return spa_buffer_find_meta_data(b->buffer, SPA_META_Header, sizeof(struct spa_meta_header));
}
static double *samples(struct app *a, struct pw_buffer *b, bool input, uint32_t frames) {
    if (b->buffer->n_datas != 1 || !b->buffer->datas[0].data || frames > FRAMES) {
        fail(a, 10); return NULL;
    }
    struct spa_data *d = &b->buffer->datas[0];
    if (!d->chunk || d->maxsize < frames * STRIDE) { fail(a, 11); return NULL; }
    if (input && (d->chunk->offset != 0 || d->chunk->size != frames * STRIDE ||
                  d->chunk->stride != (int32_t)STRIDE || d->chunk->flags != 0)) {
        fail(a, 12); return NULL;
    }
    return d->data;
}
static void submit(void *port, struct pw_buffer *b, uint32_t frames,
                   const struct spa_io_position *pos, bool silent) {
    struct spa_data *d = &b->buffer->datas[0];
    d->chunk->offset = 0; d->chunk->stride = STRIDE;
    d->chunk->size = frames * STRIDE; d->chunk->flags = 0;
    struct spa_meta_header *h = header(b);
    if (h) *h = (struct spa_meta_header){ .pts = (int64_t)pos->clock.nsec,
        .seq = pos->clock.position, .flags = silent ? SPA_META_HEADER_FLAG_GAP : 0 };
    b->size = frames;
    pw_filter_queue_buffer(port, b);
}
static void process_inner(void *userdata, struct spa_io_position *pos) {
    struct endpoint *e = userdata;
    struct app *a = e->app;
    uint32_t n = pos->clock.duration;
    atomic_store_explicit(&a->role_clock[e->role], pos->clock.id, memory_order_release);
    if (pos->clock.duration != FRAMES || pos->clock.rate.num != 1 || pos->clock.rate.denom != 48000 ||
        pos->clock.id == SPA_ID_INVALID || pos->clock.nsec > INT64_MAX) { fail(a, 20); return; }
    if (e->role == 0) {
        if (atomic_load_explicit(&a->phase, memory_order_relaxed) == 3 && atomic_load(&a->overflow_frame) == UINT64_MAX)
            atomic_store_explicit(&a->overflow_frame, pos->clock.position, memory_order_release);
        for (unsigned s = 0; s < ROUTES; ++s) {
            if (s == 0 && atomic_load_explicit(&a->source_missing, memory_order_acquire)) continue;
            struct pw_buffer *b = pw_filter_dequeue_buffer(e->out[s]);
            if (!b) continue;
            double *p = samples(a, b, false, n);
            if (!p) { pw_filter_queue_buffer(e->out[s], b); continue; }
            if (p) for (unsigned f = 0; f < n; ++f)
                for (unsigned ch = 0; ch < 2; ++ch) p[2*f+ch] = s == 0 && atomic_load_explicit(&a->phase, memory_order_relaxed) == 2 ? NAN : s == 0 ? program_value(a, pos->clock.position+f, ch) : source_value(s, pos->clock.position+f, ch);
            if (!header(b)) fail(a, 21);
            submit(e->out[s], b, n, pos, !p);
        }
        return;
    }
    if (e->role == 2) {
        for (unsigned r = 0; r < ROUTES; ++r) {
            // Deliberately stop draining one route. Other native ports must progress.
            if (r == 2 && atomic_load_explicit(&a->phase, memory_order_relaxed) == 1) continue;
            struct pw_buffer *b = pw_filter_dequeue_buffer(e->in[r]);
            if (!b) continue;
            double *p = samples(a, b, true, n);
            struct spa_meta_header *h = header(b);
            if (!h) fail(a, 21);
            else if (p && (h->flags & SPA_META_HEADER_FLAG_GAP)) {
                for (unsigned f = 0; f < 2*n; ++f) if (p[f] != 0.0) fail(a, 22);
                atomic_fetch_add_explicit(&a->silence[r], 1, memory_order_relaxed);
            } else if (p && h) {
                for (unsigned f = 0; f < n; ++f) for (unsigned ch = 0; ch < 2; ++ch) {
                    double expected = oracle(a, r, h->seq+f, ch);
                    // Canonical mixing may normalize signed zero. Nonzero low bits must survive.
                    if (p[2*f+ch] != expected) atomic_fetch_add_explicit(&a->mismatches, 1, memory_order_relaxed);
                }
                atomic_fetch_add_explicit(&a->delivered[r], 1, memory_order_relaxed);
            }
            pw_filter_queue_buffer(e->in[r], b);
        }
        return;
    }
    atomic_store_explicit(&a->clock_id, pos->clock.id, memory_order_release);
    unsigned command = atomic_load_explicit(&a->command, memory_order_acquire);
    if (command != a->last_ack) {
        if (a->host && (command == 2 || command == 4)) mx_host_revoke(a->host);
        a->last_ack = command;
        atomic_store_explicit(&a->command_done, command, memory_order_release);
    }
    struct pw_buffer *input[ROUTES] = {0};
    mx_input inputs[ROUTES] = {0};
    bool fresh = atomic_load_explicit(&a->role_clock[0], memory_order_acquire) == pos->clock.id;
    for (unsigned r = 0; r < ROUTES; ++r) {
        struct port *in_port = e->in[r], *out_port = e->out[r];
        if ((!atomic_load_explicit(&in_port->format_ok, memory_order_acquire) &&
             !(r == 0 && atomic_load_explicit(&a->source_missing, memory_order_acquire))) ||
            !atomic_load_explicit(&out_port->format_ok, memory_order_acquire)) fresh = false;
    }
    mx_stamp stamp = {0};
    if (a->host && command == 1 && fresh && !mx_graph_stamp(&a->timeline,
        pos->clock.id, pos->clock.position, pos->clock.nsec, n, a->generation, &stamp)) {
        fresh = false; fail(a, 25);
    }
    for (unsigned s = 0; s < ROUTES; ++s) {
        inputs[s] = (mx_input){ .encoding = MX_F64LE, .sample_rate_hz = 48000, .channels = 2, .stamp = stamp };
        if (s == 0 && atomic_load_explicit(&a->source_missing, memory_order_acquire)) {
            if (atomic_load(&a->loss_frame) == UINT64_MAX) atomic_store_explicit(&a->loss_frame, pos->clock.position, memory_order_release);
            continue;
        }
        input[s] = pw_filter_dequeue_buffer(e->in[s]);
        if (!input[s]) { fresh = false; continue; }
        double *p = samples(a, input[s], true, n);
        struct spa_meta_header *h = header(input[s]);
        if (!h || h->flags || h->seq != pos->clock.position || h->pts != (int64_t)pos->clock.nsec || !p) {
            fresh = false; continue;
        }
        inputs[s].bytes = (const uint8_t *)p; inputs[s].byte_count = n * STRIDE;
    }
    bool rendered = false;
    if (a->host && command == 1 && !fresh) {
        mx_host_revoke(a->host);
        atomic_store_explicit(&a->command, 4, memory_order_release);
    }
    if (a->host && command == 1 && fresh && !atomic_load_explicit(&a->error, memory_order_relaxed)) {
        if (mx_host_process(a->host, pos->clock.nsec, stamp, n, inputs, ROUTES) == 0) {
            if (atomic_load(&a->epoch_frame) == UINT64_MAX) atomic_store_explicit(&a->epoch_frame, a->timeline.epoch_frame, memory_order_release);
            rendered = true; atomic_fetch_add_explicit(&a->rendered, 1, memory_order_relaxed);
        } else if (atomic_load_explicit(&a->phase, memory_order_relaxed) == 2) {
            mx_host_revoke(a->host); atomic_store_explicit(&a->command, 3, memory_order_release);
        } else fail(a, 23);
    }
    for (unsigned r = 0; r < ROUTES; ++r) {
        struct pw_buffer *b = pw_filter_dequeue_buffer(e->out[r]);
        if (!b) { atomic_fetch_add_explicit(&a->dropped, 1, memory_order_relaxed); continue; }
        double *p = samples(a, b, false, n);
        if (!p) { pw_filter_queue_buffer(e->out[r], b); continue; }
        bool silent;
        int result = mx_destination_copy(a->host, r, rendered, p, 2*n, &silent);
        if (result == MX_INACTIVE) atomic_fetch_add_explicit(&a->inactive[r], 1, memory_order_relaxed);
        else if (result != MX_OK) fail(a, 24);
        if (!header(b)) fail(a, 21);
        submit(e->out[r], b, n, pos, silent);
    }
    for (unsigned s = 0; s < ROUTES; ++s) if (input[s]) pw_filter_queue_buffer(e->in[s], input[s]);
}
static void process(void *userdata, struct spa_io_position *pos) {
    struct endpoint *e = userdata;
    // PipeWire 1.0 has one context data loop. Check the actual callback owner
    // before touching Rust state; replacement invokes this same loop with ack.
    if (!pw_data_loop_in_thread(e->app->data_loop)) { fail(e->app, 77); return; }
    callback_owner = e->app;
    process_inner(userdata, pos);
    callback_owner = NULL;
}
static void param_changed(void *userdata, void *port, uint32_t id, const struct spa_pod *param) {
    struct endpoint *e = userdata;
    if (!port || id != SPA_PARAM_Format) return;
    struct port *p = port;
    struct spa_audio_info_raw info = {0};
    bool ok = param && spa_format_audio_raw_parse(param, &info) == 0 &&
        info.format == SPA_AUDIO_FORMAT_F64_LE && info.rate == 48000 && info.channels == 2 &&
        info.position[0] == SPA_AUDIO_CHANNEL_FL && info.position[1] == SPA_AUDIO_CHANNEL_FR;
    atomic_store_explicit(&p->format_ok, ok, memory_order_release);
    if (param && !ok) fail(e->app, 30);
}
static void state_changed(void *userdata, enum pw_filter_state old, enum pw_filter_state state, const char *error) {
    (void)old; (void)error;
    struct endpoint *e = userdata;
    if (state == PW_FILTER_STATE_ERROR) fail(e->app, 31);
}
static const struct pw_filter_events filter_events = {
    PW_VERSION_FILTER_EVENTS, .process = process, .param_changed = param_changed, .state_changed = state_changed
};
static void *add_port(struct endpoint *e, enum pw_direction dir, unsigned index) {
    char name[32]; snprintf(name, sizeof(name), "%s-%u", dir == PW_DIRECTION_INPUT ? "in" : "out", index);
    uint8_t storage[2048]; struct spa_pod_builder b = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw info = { .format = SPA_AUDIO_FORMAT_F64_LE, .rate = 48000,
        .channels = 2, .position = {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR} };
    const struct spa_pod *params[3];
    params[0] = spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &info);
    params[1] = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
        SPA_PARAM_BUFFERS_size, SPA_POD_Int(LIMIT),
        SPA_PARAM_BUFFERS_stride, SPA_POD_Int(STRIDE));
    params[2] = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta,
        SPA_PARAM_META_type, SPA_POD_Id(SPA_META_Header),
        SPA_PARAM_META_size, SPA_POD_Int(sizeof(struct spa_meta_header)));
    return pw_filter_add_port(e->filter, dir, PW_FILTER_PORT_FLAG_MAP_BUFFERS, sizeof(struct port),
        pw_properties_new(PW_KEY_PORT_NAME, name, NULL), params, 3);
}
static bool number(const char *s, uint64_t *out) {
    if (!s || !*s || *s == '-') return false;
    char *end; errno = 0; unsigned long long v = strtoull(s, &end, 10);
    if (errno || *end) return false;
    *out = v; return true;
}
static void global_added(void *userdata, uint32_t id, uint32_t permissions, const char *type,
                         uint32_t version, const struct spa_dict *props) {
    (void)permissions; (void)version;
    struct app *a = userdata;
    if (strcmp(type, PW_TYPE_INTERFACE_Node) && strcmp(type, PW_TYPE_INTERFACE_Port) &&
        strcmp(type, PW_TYPE_INTERFACE_Device) && strcmp(type, PW_TYPE_INTERFACE_Module)) return;
    if (!props || a->global_count == MAX_GLOBALS) { fail(a, 40); return; }
    if (!strcmp(type, PW_TYPE_INTERFACE_Device)) { fail(a, 41); return; }
    const char *name = spa_dict_lookup(props, PW_KEY_NODE_NAME);
    if (!strcmp(type, PW_TYPE_INTERFACE_Node) &&
        (!name || strncmp(name, "mixyard-private-", 16))) { fail(a, 42); return; }
    if (!strcmp(type, PW_TYPE_INTERFACE_Module)) {
        const char *module = spa_dict_lookup(props, PW_KEY_MODULE_NAME);
        if (!module || (strcmp(module, "libpipewire-module-protocol-native") &&
            strcmp(module, "libpipewire-module-client-node") && strcmp(module, "libpipewire-module-access") &&
            strcmp(module, "libpipewire-module-spa-node-factory") && strcmp(module, "libpipewire-module-link-factory"))) fail(a, 43);
        return;
    }
    struct global *g = &a->globals[a->global_count++];
    g->id = id; g->node = SPA_ID_INVALID;
    snprintf(g->type, sizeof(g->type), "%s", !strcmp(type, PW_TYPE_INTERFACE_Node) ? "node" : "port");
    if (!number(spa_dict_lookup(props, PW_KEY_OBJECT_SERIAL), &g->serial)) fail(a, 44);
    if (name) snprintf(g->name, sizeof(g->name), "%s", name);
    const char *pn = spa_dict_lookup(props, PW_KEY_PORT_NAME);
    if (pn) snprintf(g->name, sizeof(g->name), "%s", pn);
    const char *direction = spa_dict_lookup(props, PW_KEY_PORT_DIRECTION);
    if (direction) snprintf(g->direction, sizeof(g->direction), "%s", direction);
    uint64_t node;
    if (number(spa_dict_lookup(props, PW_KEY_NODE_ID), &node) && node <= UINT32_MAX) g->node = (uint32_t)node;
}
static void global_removed(void *userdata, uint32_t id) {
    struct app *a = userdata;
    for (size_t i = 0; i < a->global_count; ++i) if (a->globals[i].id == id) {
        if (a->linked && !a->expected_loss && id != a->removed_source_port) fail(a, 45);
        a->globals[i].id = SPA_ID_INVALID;
        if (id == a->removed_source_port) atomic_store_explicit(&a->source_removed, true, memory_order_release);
        if (a->linked && id != a->removed_source_port) atomic_store_explicit(&a->command, 4, memory_order_release);
    }
}
static const struct pw_registry_events registry_events = {
    PW_VERSION_REGISTRY_EVENTS, .global = global_added, .global_remove = global_removed
};
static void core_error(void *userdata, uint32_t id, int seq, int res, const char *message) {
    (void)id; (void)seq;
    fprintf(stderr, "native core error %d: %s\n", res, message);
    fail(userdata, 46);
}
static const struct pw_core_events core_events = { PW_VERSION_CORE_EVENTS, .error = core_error };
static struct global *find_port(struct app *a, unsigned node, const char *direction, unsigned index) {
    char name[32]; snprintf(name, sizeof(name), "%s-%u", direction, index);
    struct global *found = NULL;
    for (size_t i = 0; i < a->global_count; ++i) {
        struct global *g = &a->globals[i];
        if (g->id != SPA_ID_INVALID && !strcmp(g->type, "port") && g->node == node &&
            !strcmp(g->direction, direction) && !strcmp(g->name, name)) {
            if (found) { fail(a, 47); return NULL; } found = g;
        }
    }
    return found;
}
static bool link_graph(struct app *a) {
    for (unsigned step = 0; step < 2; ++step) for (unsigned r = 0; r < ROUTES; ++r) {
        uint32_t from = pw_filter_get_node_id(a->nodes[step].filter);
        uint32_t to = pw_filter_get_node_id(a->nodes[step+1].filter);
        struct global *out = find_port(a, from, "out", r), *in = find_port(a, to, "in", r);
        if (!out || !in) return false;
    }
    for (unsigned step = 0; step < 2; ++step) for (unsigned r = 0; r < ROUTES; ++r) {
        uint32_t from = pw_filter_get_node_id(a->nodes[step].filter);
        uint32_t to = pw_filter_get_node_id(a->nodes[step+1].filter);
        struct global *out = find_port(a, from, "out", r), *in = find_port(a, to, "in", r);
        struct pw_properties *p = pw_properties_new(NULL, NULL);
        pw_properties_setf(p, PW_KEY_LINK_OUTPUT_NODE, "%u", from);
        pw_properties_setf(p, PW_KEY_LINK_OUTPUT_PORT, "%u", out->id);
        pw_properties_setf(p, PW_KEY_LINK_INPUT_NODE, "%u", to);
        pw_properties_setf(p, PW_KEY_LINK_INPUT_PORT, "%u", in->id);
        pw_properties_set(p, PW_KEY_OBJECT_LINGER, "false");
        a->links[step*ROUTES+r] = pw_core_create_object(a->core, "link-factory", PW_TYPE_INTERFACE_Link,
            PW_VERSION_LINK, &p->dict, 0);
        pw_properties_free(p);
        if (!a->links[step*ROUTES+r]) { fail(a, 48); return false; }
    }
    a->linked = true;
    return true;
}
static void observed(struct app *a, unsigned node, const char *direction, unsigned r, char *out, size_t capacity) {
    uint32_t id = pw_filter_get_node_id(a->nodes[node].filter);
    struct global *p = find_port(a, id, direction, r), *n = NULL;
    for (size_t i = 0; i < a->global_count; ++i) if (a->globals[i].id == id) n = &a->globals[i];
    if (!p || !n) { fail(a, 50); out[0] = 0; return; }
    snprintf(out, capacity, "pipewire:node:%u:serial:%" PRIu64 ":port:%u:serial:%" PRIu64,
        id, n->serial, p->id, p->serial);
}
static void write_topology(struct app *a) {
    json_object *root = json_object_new_object(), *sources = json_object_new_array(), *outputs = json_object_new_array();
    json_object_object_add(root, "clockId", json_object_new_int64(atomic_load_explicit(&a->clock_id, memory_order_acquire)));
    json_object_object_add(root, "sources", sources); json_object_object_add(root, "outputs", outputs);
    for (unsigned r = 0; r < ROUTES; ++r) {
        char id[192]; observed(a, 0, "out", r, id, sizeof(id));
        json_object_array_add(sources, json_object_new_string(id));
        observed(a, 2, "in", r, id, sizeof(id));
        json_object_array_add(outputs, json_object_new_string(id));
    }
    char path[512], complete[512]; snprintf(path, sizeof(path), "%s/topology.new", a->directory);
    snprintf(complete, sizeof(complete), "%s/topology.json", a->directory);
    FILE *f = fopen(path, "wx");
    if (!f || fprintf(f, "%s\n", json_object_to_json_string_ext(root, JSON_C_TO_STRING_PLAIN)) < 0 || fclose(f) || rename(path, complete)) fail(a, 51);
    json_object_put(root);
    a->topology_written = true;
}
struct replacement { struct app *a; mx_host *next, *old; uint64_t generation; };
static int replace_host(struct spa_loop *loop, bool async, uint32_t seq, const void *data, size_t size, void *userdata) {
    (void)loop; (void)async; (void)seq; (void)data; (void)size;
    struct replacement *r = userdata;
    r->old = r->a->host; r->a->host = r->next;
    r->a->generation = r->generation; r->a->last_ack = 0;
    atomic_store_explicit(&r->a->epoch_frame, UINT64_MAX, memory_order_release);
    r->a->timeline = (mx_graph_timeline){ .clock_id = atomic_load_explicit(&r->a->clock_id, memory_order_acquire) };
    atomic_store_explicit(&r->a->command_done, 0, memory_order_release);
    atomic_store_explicit(&r->a->command, r->next ? 1 : 0, memory_order_release);
    return 0;
}
static bool compiled_spec(mx_host *host, bool output, size_t index, json_object *expected) {
    uint8_t text[1024]; size_t needed = 0;
    int rc = output ? mx_host_output_info(host, index, text, sizeof(text), &needed) :
                      mx_host_source_info(host, index, text, sizeof(text), &needed);
    if (rc != MX_OK || needed >= sizeof(text)) return false;
    text[needed] = 0;
    json_object *actual = json_tokener_parse((const char *)text);
    bool valid = actual && json_object_equal(actual, expected);
    if (actual) json_object_put(actual);
    return valid;
}
static bool install_host(struct app *a, uint64_t generation) {
    char path[512]; snprintf(path, sizeof(path), "%s/request.json", a->directory);
    json_object *request = json_object_from_file(path);
    if (!request) return false;
    json_object_object_add(request, "generation", json_object_new_int64(generation));
    json_object *sources = NULL, *outputs = NULL;
    bool valid = json_object_object_get_ex(request, "sources", &sources) &&
        json_object_object_get_ex(request, "outputs", &outputs) &&
        json_object_array_length(sources) == ROUTES && json_object_array_length(outputs) == ROUTES;
    for (unsigned r = 0; valid && r < ROUTES; ++r) for (unsigned side = 0; side < 2; ++side) {
        char id[192]; observed(a, side ? 2 : 0, side ? "in" : "out", r, id, sizeof(id));
        json_object *port = json_object_array_get_idx(side ? outputs : sources, r), *value;
        valid = json_object_object_get_ex(port, "observedId", &value) && !strcmp(json_object_get_string(value), id);
        struct port *native_port = side ? a->nodes[2].in[r] : a->nodes[0].out[r];
        valid = valid && atomic_load_explicit(&native_port->format_ok, memory_order_acquire);
    }
    if (!valid) { json_object_put(request); fail(a, 52); return false; }
    const char *text = json_object_to_json_string_ext(request, JSON_C_TO_STRING_PLAIN);
    mx_host *host = NULL; char error[1024] = {0};
    if (mx_host_abi_version() != MX_HOST_ABI_VERSION) { json_object_put(request); fail(a, 59); return false; }
    int result = mx_host_create((const uint8_t *)text, strlen(text), &host, error, sizeof(error));
    if (result || !host) { fprintf(stderr, "shared engine create: %s (%d)\n", error, result); json_object_put(request); fail(a, 53); return false; }
    if (mx_host_source_count(host) != ROUTES || mx_host_output_count(host) != ROUTES) {
        mx_host_destroy(host); json_object_put(request); fail(a, 54); return false;
    }
    for (unsigned r = 0; r < ROUTES; ++r) {
        if (!compiled_spec(host, false, r, json_object_array_get_idx(sources, r)) ||
            !compiled_spec(host, true, r, json_object_array_get_idx(outputs, r))) {
            mx_host_destroy(host); json_object_put(request); fail(a, 56); return false;
        }
    }
    json_object_put(request);
    struct replacement replacement = { .a = a, .next = host, .generation = generation };
    int rc = pw_data_loop_invoke(a->data_loop, replace_host, 0, NULL, 0, true, &replacement);
    if (rc < 0) { mx_host_destroy(host); fail(a, 55); return false; }
    if (replacement.old) mx_host_destroy(replacement.old);
    return true;
}
static void baseline(struct app *a) {
    for (unsigned r = 0; r < ROUTES; ++r) a->baseline[r] = atomic_load_explicit(&a->delivered[r], memory_order_relaxed);
    a->ticks = 0;
}
static bool progressed(struct app *a, unsigned r, unsigned count) {
    return atomic_load_explicit(&a->delivered[r], memory_order_relaxed) >= a->baseline[r] + count;
}
static bool verify_quiet(struct app *a) {
    if (!a->quiet_started) {
        baseline(a);
        a->quiet_rendered = atomic_load_explicit(&a->rendered, memory_order_relaxed);
        for (unsigned r = 0; r < ROUTES; ++r) a->quiet_silence[r] = atomic_load_explicit(&a->silence[r], memory_order_relaxed);
        a->quiet_started = true;
        return false;
    }
    if (a->ticks < 10) return false;
    if (atomic_load_explicit(&a->rendered, memory_order_relaxed) != a->quiet_rendered) fail(a, 74);
    for (unsigned r = 0; r < ROUTES; ++r) {
        if (atomic_load_explicit(&a->delivered[r], memory_order_relaxed) > a->baseline[r] + 8 ||
            atomic_load_explicit(&a->silence[r], memory_order_relaxed) < a->quiet_silence[r] + 32) fail(a, 75);
    }
    a->quiet_started = false;
    return true;
}
static int mark_program_lost(struct spa_loop *loop, bool async, uint32_t seq, const void *data, size_t size, void *userdata) {
    (void)loop; (void)async; (void)seq; (void)data; (void)size;
    struct app *a = userdata;
    atomic_store_explicit(&a->source_missing, true, memory_order_release);
    return 0;
}
static bool remove_program_port(struct app *a) {
    struct global *port = find_port(a, pw_filter_get_node_id(a->nodes[0].filter), "out", 0);
    if (!port) { fail(a, 78); return false; }
    a->removed_source_port = port->id;
    // Acknowledge that no callback can touch this port before removing it.
    if (pw_data_loop_invoke(a->data_loop, mark_program_lost, 0, NULL, 0, true, a) < 0) { fail(a, 79); return false; }
    if (pw_filter_remove_port(a->nodes[0].out[0]) < 0) { fail(a, 80); return false; }
    a->nodes[0].out[0] = NULL;
    return true;
}
static bool publish_marker(struct app *a, const char *name) {
    char staging[512], complete[512];
    snprintf(staging, sizeof(staging), "%s/%s.new", a->directory, name);
    snprintf(complete, sizeof(complete), "%s/%s", a->directory, name);
    FILE *f = fopen(staging, "wx");
    if (!f) return false;
    bool ok = fputs("true\n", f) >= 0;
    if (fclose(f)) ok = false;
    return ok && rename(staging, complete) == 0;
}
static void timer(void *userdata, uint64_t expirations) {
    (void)expirations;
    struct app *a = userdata;
    if (atomic_load(&a->heap_ops)) fail(a, 76);
    if (atomic_load_explicit(&a->error, memory_order_acquire) || atomic_load(&a->mismatches)) {
        pw_main_loop_quit(a->loop); return;
    }
    if (++a->ticks > 200) { fail(a, 60 + a->stage); pw_main_loop_quit(a->loop); return; }
    switch (a->stage) {
    case 0:
        if (link_graph(a)) { a->stage = 1; a->ticks = 0; }
        break;
    case 1:
        if (!atomic_load_explicit(&a->clock_id, memory_order_acquire)) break;
        for (unsigned node = 0; node < 3; ++node)
            if (atomic_load_explicit(&a->role_clock[node], memory_order_acquire) != atomic_load_explicit(&a->clock_id, memory_order_acquire)) return;
        if (!a->topology_written) write_topology(a);
        if (install_host(a, 1)) { a->stage = 2; baseline(a);
            if (!publish_marker(a, "admitted")) fail(a, 57); }
        break;
    case 2:
        if (progressed(a, 0, 64) && progressed(a, 1, 64) && progressed(a, 2, 64)) {
            if (!publish_marker(a, "flowing")) { fail(a, 58); break; }
            baseline(a);
            if (a->mode == OUTPUT_FAILURE) {
                a->failure_silence = atomic_load(&a->silence[1]);
                atomic_store_explicit(&a->phase, 3, memory_order_release); a->stage = 11;
            } else if (a->mode == TAIL_LOSS) {
                if (remove_program_port(a)) a->stage = 12;
            } else { atomic_store(&a->phase, 1); a->stage = 3; }
        }
        break;
    case 3:
        if (a->ticks < 10) break;
        if (!progressed(a, 0, 32) || !progressed(a, 1, 32) || !atomic_load(&a->dropped)) { fail(a, 70); break; }
        atomic_store(&a->phase, 0); baseline(a); a->stage = 4;
        break;
    case 4:
        if (!progressed(a, 2, 32)) break;
        atomic_store_explicit(&a->command, 2, memory_order_release); a->stage = 5; a->ticks = 0;
        break;
    case 5:
        if (atomic_load_explicit(&a->command_done, memory_order_acquire) != 2 || !verify_quiet(a)) break;
        if (install_host(a, 2)) { baseline(a); a->stage = 6; }
        break;
    case 6:
        if (!progressed(a, 0, 64) || !progressed(a, 1, 64) || !progressed(a, 2, 64)) break;
        atomic_store(&a->phase, 2); a->stage = 7; a->ticks = 0;
        break;
    case 7:
        if (atomic_load_explicit(&a->command_done, memory_order_acquire) != 3 || !verify_quiet(a)) break;
        atomic_store(&a->phase, 0);
        if (install_host(a, 3)) { baseline(a); a->stage = 8; }
        break;
    case 8:
        if (!progressed(a, 0, 64) || !progressed(a, 1, 64)) break;
        a->expected_loss = true;
        pw_filter_destroy(a->nodes[0].filter); a->nodes[0].filter = NULL;
        a->stage = 9; a->ticks = 0;
        break;
    case 9:
        if (atomic_load_explicit(&a->command_done, memory_order_acquire) != 4 || !verify_quiet(a)) break;
        a->stage = 10; pw_main_loop_quit(a->loop);
        break;
    case 11:
        if (!progressed(a, 0, 128) || !progressed(a, 2, 128) || atomic_load(&a->inactive[1]) < 128 ||
            atomic_load(&a->silence[1]) < a->failure_silence + 128) break;
        if (progressed(a, 1, 9) || atomic_load(&a->inactive[0]) || atomic_load(&a->inactive[2])) { fail(a, 81); break; }
        a->stage = 10; pw_main_loop_quit(a->loop);
        break;
    case 12:
        if (!progressed(a, 0, 128) || !progressed(a, 1, 128) || !progressed(a, 2, 128) ||
            atomic_load_explicit(&a->loss_frame, memory_order_acquire) == UINT64_MAX ||
            !atomic_load_explicit(&a->source_removed, memory_order_acquire)) break;
        if (atomic_load(&a->inactive[0]) || atomic_load(&a->inactive[1]) || atomic_load(&a->inactive[2])) { fail(a, 82); break; }
        a->stage = 10; pw_main_loop_quit(a->loop);
        break;
    }
}
static void quit(void *userdata, int signal) {
    (void)signal; struct app *a = userdata; a->cancelled = true; pw_main_loop_quit(a->loop);
}
int main(int argc, char **argv) {
    if (argc != 3 || !getenv("MIXYARD_PRIVATE_GRAPH") || strcmp(getenv("MIXYARD_PRIVATE_GRAPH"), "hosted-owned-only")) {
        fprintf(stderr, "requires disposable hosted private graph and private control directory\n"); return 2;
    }
    struct stat info;
    if (lstat(argv[1], &info) || !S_ISDIR(info.st_mode) || info.st_uid != getuid() || (info.st_mode & 077)) return 2;
    struct app a = { .directory = argv[1], .removed_source_port = SPA_ID_INVALID };
    if (!strcmp(argv[2], "normal")) a.mode = NORMAL;
    else if (!strcmp(argv[2], "output-failure")) a.mode = OUTPUT_FAILURE;
    else if (!strcmp(argv[2], "tail-loss")) a.mode = TAIL_LOSS;
    else return 2;
    atomic_init(&a.epoch_frame, UINT64_MAX); atomic_init(&a.overflow_frame, UINT64_MAX); atomic_init(&a.loss_frame, UINT64_MAX);
    pw_init(&argc, &argv);
    a.loop = pw_main_loop_new(NULL);
    a.context = pw_context_new(pw_main_loop_get_loop(a.loop), NULL, 0);
    a.data_loop = pw_context_get_data_loop(a.context);
    if (!a.data_loop) return 2;
    a.core = pw_context_connect(a.context, pw_properties_new(PW_KEY_REMOTE_NAME, "mixyard-private", NULL), 0);
    if (!a.core) return 2;
    pw_core_add_listener(a.core, &a.core_hook, &core_events, &a);
    a.registry = pw_core_get_registry(a.core, PW_VERSION_REGISTRY, 0);
    pw_registry_add_listener(a.registry, &a.registry_hook, &registry_events, &a);
    const char *names[] = {"mixyard-private-generator", "mixyard-private-engine", "mixyard-private-consumer"};
    for (unsigned node = 0; node < 3; ++node) {
        struct endpoint *e = &a.nodes[node]; e->app = &a; e->role = node;
        struct pw_properties *props = pw_properties_new(PW_KEY_NODE_NAME, names[node],
            PW_KEY_NODE_GROUP, "mixyard-private", PW_KEY_NODE_ALWAYS_PROCESS, "true",
            PW_KEY_NODE_AUTOCONNECT, "false", "node.dont-reconnect", "true",
            "mixyard.test.provenance", node == 0 ? "owned-synthetic-generator" : node == 1 ? "mixyard-engine-output" : "owned-test-consumer",
            NULL);
        e->filter = pw_filter_new(a.core, names[node], props);
        if (!e->filter) return 2;
        pw_filter_add_listener(e->filter, &e->listener, &filter_events, e);
        for (unsigned r = 0; r < ROUTES; ++r) {
            if (node > 0) e->in[r] = add_port(e, PW_DIRECTION_INPUT, r);
            if (node < 2) e->out[r] = add_port(e, PW_DIRECTION_OUTPUT, r);
            if ((node > 0 && !e->in[r]) || (node < 2 && !e->out[r])) return 2;
        }
        if (pw_filter_connect(e->filter, PW_FILTER_FLAG_RT_PROCESS, NULL, 0) < 0) return 2;
    }
    struct pw_loop *loop = pw_main_loop_get_loop(a.loop);
    pw_loop_add_signal(loop, SIGINT, quit, &a); pw_loop_add_signal(loop, SIGTERM, quit, &a);
    a.timer = pw_loop_add_timer(loop, timer, &a);
    struct timespec interval = { .tv_nsec = 50000000 };
    pw_loop_update_timer(loop, a.timer, &interval, &interval, false);
    pw_main_loop_run(a.loop);
    // Serialize pointer withdrawal on the owning data loop before destroying Rust state.
    struct replacement replacement = { .a = &a };
    if (pw_data_loop_invoke(a.data_loop, replace_host, 0, NULL, 0, true, &replacement) < 0) return 2;
    if (replacement.old) mx_host_destroy(replacement.old);
    a.expected_loss = true;
    for (unsigned l = 0; l < ROUTES*2; ++l) if (a.links[l]) pw_proxy_destroy(a.links[l]);
    for (unsigned n = 0; n < 3; ++n) if (a.nodes[n].filter) pw_filter_destroy(a.nodes[n].filter);
    pw_proxy_destroy((struct pw_proxy *)a.registry);
    pw_core_disconnect(a.core); pw_context_destroy(a.context); pw_main_loop_destroy(a.loop); pw_deinit();
    printf("{\"stage\":%u,\"error\":%u,\"sampleMismatches\":%" PRIu64 ",\"rendered\":%" PRIu64
        ",\"delivered\":[%" PRIu64 ",%" PRIu64 ",%" PRIu64 "],\"dropped\":%" PRIu64 ",\"cancelled\":%s,\"callbackHeapOperations\":%" PRIu64 ",\"inactiveOutputs\":[%" PRIu64 ",%" PRIu64 ",%" PRIu64 "],\"lossFrame\":%" PRIu64 "}\n",
        a.stage, atomic_load(&a.error), atomic_load(&a.mismatches), atomic_load(&a.rendered),
        atomic_load(&a.delivered[0]), atomic_load(&a.delivered[1]), atomic_load(&a.delivered[2]),
        atomic_load(&a.dropped), a.cancelled ? "true" : "false", atomic_load(&a.heap_ops),
        atomic_load(&a.inactive[0]), atomic_load(&a.inactive[1]), atomic_load(&a.inactive[2]), atomic_load(&a.loss_frame));
    return (a.stage == 10 || a.cancelled) && !atomic_load(&a.error) && !atomic_load(&a.mismatches) ? 0 : 1;
}

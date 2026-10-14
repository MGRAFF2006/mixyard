/* SPDX-License-Identifier: MIT
 * PRIVATE fixture runtime. No production plan parser or host apply path. */
#include <errno.h>
#include <math.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <pipewire/pipewire.h>
#include <pipewire/filter.h>
#include <spa/param/audio/format-utils.h>

#define FRAMES 2048u
#define SLOTS 8u
#define TRACE_SLOTS 32768u
struct trace { uint64_t frame, ticks, position, epoch, now, wanted; unsigned frames, depth, flags, revision; };
struct app;
struct block { uint64_t position, epoch, output_epoch; uint32_t frames; float pcm[FRAMES * 2]; };
struct queue { _Atomic unsigned write, read; _Atomic uint64_t drops, underruns;
    struct block blocks[SLOTS]; struct trace trace[TRACE_SLOTS]; _Atomic unsigned traced; _Atomic uint64_t trace_overflow; };
struct object { uint32_t id, node, on, op, in, ip; uint64_t serial;
    char *type, *name, *direction, *channel; struct object *next; };
struct binding { struct app *app; uint32_t id; uint64_t serial;
    struct pw_proxy *proxy; struct spa_hook hook; bool valid; bool port; struct pw_node_info *node_info; struct pw_port_info *port_info; };
struct owned_link { struct app *app; struct pw_proxy *proxy; struct spa_hook hook, link_hook; struct pw_link_info *info;
    uint32_t id, on, op, in, ip; bool failed; struct owned_link *next; };
struct source { struct app *app; char *name; struct pw_filter *filter;
    struct spa_hook hook; void *ports[2]; _Atomic bool gate;
    _Atomic uint64_t epoch, cycles, first_frame; _Atomic unsigned quantum; _Atomic uint64_t position;
    uint32_t node, selected; uint64_t serial; bool retired;
    struct binding binds[3]; };
struct mix { struct app *app; char *name; struct pw_stream *stream; struct spa_hook hook;
    unsigned index; _Atomic uint64_t ack, ack_frame, frames, stale, bad_quantum;
    _Atomic bool stall; _Atomic uint64_t epoch; _Atomic unsigned quantum; };
struct app { struct pw_main_loop *loop; struct pw_context *context; struct pw_core *core;
    struct pw_registry *registry; struct spa_hook core_hook, registry_hook;
    struct object *objects; struct owned_link *links;
    struct source *sources; struct mix *mixes; struct queue *queues;
    float *gains[2]; unsigned ns, nm; _Atomic unsigned revision;
    unsigned sync_seq; enum { IDLE, LINKED, ACTIVATED, OPEN } action;
    _Atomic uint64_t last_trigger; bool triggered;
    bool fatal, observer_delayed; struct spa_source *delay_timer; };

static void die(const char *message) { fprintf(stderr, "%s\n", message); exit(2); }
static unsigned number(const char *s) {
    char *end; errno = 0; unsigned long v = strtoul(s, &end, 10);
    if (errno || !*s || *end || v > UINT32_MAX) die("invalid integer");
    return v;
}
static uint32_t prop_id(const struct spa_dict *p, const char *key) {
    const char *s = spa_dict_lookup(p, key); return s ? number(s) : PW_ID_ANY;
}
static char *prop_copy(const struct spa_dict *p, const char *key) {
    const char *s = spa_dict_lookup(p, key); return strdup(s ? s : "");
}
static struct object *object(struct app *a, uint32_t id) {
    for (struct object *o = a->objects; o; o = o->next) if (o->id == id) return o;
    return NULL;
}
static void close_source(struct source *s) {
    if (!s->retired) fprintf(stderr, "retire %s selected=%u\n", s->name, s->selected);
    atomic_store_explicit(&s->gate, false, memory_order_release);
    atomic_fetch_add(&s->epoch, 1); s->retired = true;
}
static void close_all(struct app *a) {
    for (unsigned i = 0; i < a->ns; i++) close_source(&a->sources[i]);
}
static void quit(void *data, int sig) {
    (void)sig; struct app *a = data; close_all(a); pw_main_loop_quit(a->loop);
}
static struct queue *queue(struct app *a, unsigned s, unsigned m) {
    return &a->queues[(size_t)s * a->nm + m];
}
static void ingress(void *data, struct spa_io_position *p) {
    struct source *s = data; struct app *a = s->app;
    uint32_t n = p->clock.duration;
    atomic_store(&s->quantum, n);
    atomic_fetch_add_explicit(&s->cycles, 1, memory_order_relaxed);
    if (n > FRAMES) { atomic_store(&s->gate, false); return; }
    float *in[2];
    for (unsigned c = 0; c < 2; c++) in[c] = pw_filter_get_dsp_buffer(s->ports[c], n);
    bool open = atomic_load_explicit(&s->gate, memory_order_acquire);
    uint64_t epoch = atomic_load(&s->epoch);
    if (open && in[0] && in[1]) {
        uint64_t unset = UINT64_MAX;
        atomic_compare_exchange_strong(&s->first_frame, &unset, p->clock.position);
    }
    unsigned index = (unsigned)(s - a->sources);
    for (unsigned m = 0; m < a->nm; m++) {
        struct queue *q = queue(a, index, m);
        unsigned w = atomic_load_explicit(&q->write, memory_order_relaxed);
        unsigned r = atomic_load_explicit(&q->read, memory_order_acquire);
        if (w - r == SLOTS) { atomic_fetch_add(&q->drops, n); continue; }
        struct block *b = &q->blocks[w % SLOTS];
        b->frames = n; b->position = p->clock.position; b->epoch = epoch;
        b->output_epoch = atomic_load(&a->mixes[m].epoch);
        for (unsigned f = 0; f < n; f++) for (unsigned c = 0; c < 2; c++)
            b->pcm[f * 2 + c] = open && in[c] ? in[c][f] : 0.0f;
        atomic_store_explicit(&q->write, w + 1, memory_order_release);
    }
    atomic_store_explicit(&s->position, p->clock.position, memory_order_release);
    if (a->triggered) {
        bool ready = true;
        for (unsigned i = 0; i < a->ns; i++) if (atomic_load_explicit(&a->sources[i].position, memory_order_acquire) != p->clock.position) ready = false;
        uint64_t previous = atomic_load(&a->last_trigger);
        if (ready && previous != p->clock.position && atomic_compare_exchange_strong(&a->last_trigger, &previous, p->clock.position))
            for (unsigned m = 0; m < a->nm; m++) pw_stream_trigger_process(a->mixes[m].stream);
    }
}
static void egress(void *data) {
    struct mix *m = data; struct app *a = m->app;
    struct pw_buffer *pb = pw_stream_dequeue_buffer(m->stream);
    if (!pb) return;
    struct spa_data *d = &pb->buffer->datas[0];
    uint32_t n = pb->requested ? pb->requested : 256;
    n = SPA_MIN(n, d->maxsize / (2 * sizeof(float)));
    atomic_store(&m->quantum, n);
    if (n > FRAMES || !d->data) {
        atomic_fetch_add(&m->bad_quantum, 1); d->chunk->size = 0;
        pw_stream_queue_buffer(m->stream, pb); return;
    }
    float *out = d->data;
    memset(out, 0, n * 2 * sizeof(float));
    unsigned revision = atomic_load_explicit(&a->revision, memory_order_acquire);
    struct pw_time time = {0}; pw_stream_get_time_n(m->stream, &time, sizeof(time));
    for (unsigned s = 0; s < a->ns; s++) {
        struct queue *q = queue(a, s, m->index); unsigned traced = atomic_load(&q->traced);
        struct trace trace = {.frame = atomic_load(&m->frames), .ticks = time.ticks, .position = UINT64_MAX,
            .wanted = atomic_load(&a->last_trigger), .now = time.now, .epoch = atomic_load(&a->sources[s].epoch), .frames = n, .revision = revision};
        if (!atomic_load(&m->stall)) {
            unsigned r = atomic_load_explicit(&q->read, memory_order_relaxed);
            unsigned w = atomic_load_explicit(&q->write, memory_order_acquire);
            /* Timestamp freshness, not extra buffering or sample replacement.
             * A missed output callback must not turn into permanent FIFO lag.
             * Every discarded block is counted and marks this trace as loss. */
            if (a->triggered) for (unsigned skip = 0; skip < SLOTS && r != w; skip++) {
                struct block *old = &q->blocks[r % SLOTS];
                if (old->position >= trace.wanted) break;
                atomic_fetch_add(&q->drops, old->frames); r++; trace.flags |= 8;
                atomic_store_explicit(&q->read, r, memory_order_release);
            }
            trace.depth = w - r;
            if (r == w) { atomic_fetch_add(&q->underruns, n); trace.flags |= 1; }
            else {
            struct block *b = &q->blocks[r % SLOTS]; trace.position = b->position;
            bool valid = (!a->triggered || b->position == trace.wanted) && b->frames == n && b->output_epoch == atomic_load(&m->epoch) && b->epoch == atomic_load(&a->sources[s].epoch)
                && atomic_load_explicit(&a->sources[s].gate, memory_order_acquire);
            if (!valid) { atomic_fetch_add(&m->stale, n); trace.flags |= 2; }
            float gain = a->gains[revision % 2][m->index * a->ns + s];
            if (valid) for (unsigned f = 0; f < n * 2; f++) out[f] += gain * b->pcm[f];
            atomic_store_explicit(&q->read, r + 1, memory_order_release);
            }
        } else trace.flags |= 4;
        if (traced < TRACE_SLOTS) { q->trace[traced] = trace; atomic_store_explicit(&q->traced, traced + 1, memory_order_release); }
        else atomic_fetch_add(&q->trace_overflow, 1);
    }
    uint64_t frame = atomic_fetch_add(&m->frames, n);
    /* Acknowledged before publishing the block, after applying its coefficient. */
    if (atomic_load(&m->ack) != revision) {
        atomic_store(&m->ack_frame, frame);
        atomic_store_explicit(&m->ack, revision, memory_order_release);
    }
    d->chunk->offset = 0; d->chunk->stride = 2 * sizeof(float);
    d->chunk->size = n * 2 * sizeof(float);
    pb->size = n; pw_stream_queue_buffer(m->stream, pb);
}
static const struct pw_filter_events filter_events = {
    PW_VERSION_FILTER_EVENTS, .process = ingress };
static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS, .process = egress };

static void global(void *data, uint32_t id, uint32_t permissions, const char *type,
        uint32_t version, const struct spa_dict *props) {
    (void)permissions; (void)version; struct app *a = data;
    if (!props) return;
    struct object *o = calloc(1, sizeof(*o)); if (!o) die("out of memory");
    o->id = id; o->type = strdup(type); o->name = prop_copy(props, PW_KEY_NODE_NAME);
    o->direction = prop_copy(props, PW_KEY_PORT_DIRECTION);
    o->channel = prop_copy(props, PW_KEY_AUDIO_CHANNEL);
    const char *serial = spa_dict_lookup(props, PW_KEY_OBJECT_SERIAL);
    o->serial = serial ? strtoull(serial, NULL, 10) : 0;
    o->node = prop_id(props, PW_KEY_NODE_ID);
    o->on = prop_id(props, PW_KEY_LINK_OUTPUT_NODE); o->op = prop_id(props, PW_KEY_LINK_OUTPUT_PORT);
    o->in = prop_id(props, PW_KEY_LINK_INPUT_NODE); o->ip = prop_id(props, PW_KEY_LINK_INPUT_PORT);
    o->next = a->objects; a->objects = o;
    for (unsigned i = 0; i < a->ns; i++) {
        struct source *s = &a->sources[i];
        if (!a->observer_delayed && s->selected != PW_ID_ANY && id != s->selected && !strcmp(o->name, s->name)) close_source(s);
        if (!a->observer_delayed && o->in == pw_filter_get_node_id(s->filter) && atomic_load(&s->gate)) {
            bool owned = false;
            for (struct owned_link *l = a->links; l; l = l->next) if (l->id == id) owned = true;
            if (!owned) close_source(s);
        }
    }
}
static void removed(void *data, uint32_t id) {
    struct app *a = data;
    for (unsigned s = 0; s < a->ns; s++) {
        struct source *src = &a->sources[s];
        bool selected = src->selected == id;
        for (unsigned j = 0; j < 3; j++) selected |= src->binds[j].id == id;
        for (struct owned_link *l = a->links; l; l = l->next)
            if (l->in == pw_filter_get_node_id(src->filter) && l->id == id) selected = true;
        if (selected) {
            fprintf(stderr, "%s identity/link %u\n", a->observer_delayed ? "deferred" : "removed", id);
            if (!a->observer_delayed) close_source(src);
        }
    }
    struct object **ptr = &a->objects;
    while (*ptr) { struct object *o = *ptr;
        if (o->id == id) { *ptr = o->next; free(o->type); free(o->name);
            free(o->direction); free(o->channel); free(o); break; }
        ptr = &o->next;
    }
}
static const struct pw_registry_events registry_events = {
    PW_VERSION_REGISTRY_EVENTS, .global = global, .global_remove = removed };
static void node_info(void *data, const struct pw_node_info *update) {
    struct binding *b = data;
    b->node_info = pw_node_info_update(b->node_info, update);
    const struct pw_node_info *info = b->node_info;
    struct object *o = object(b->app, b->id);
    const char *serial = info->props ? spa_dict_lookup(info->props, PW_KEY_OBJECT_SERIAL) : NULL;
    const char *name = info->props ? spa_dict_lookup(info->props, PW_KEY_NODE_NAME) : NULL;
    b->valid = o && info->id == b->id && serial && strtoull(serial, NULL, 10) == b->serial
        && name && !strcmp(name, o->name);
    if (!b->valid) { fprintf(stderr, "binding invalid id=%u port=%d serial=%s props=", b->id, b->port, serial ? serial : "missing");
        for (unsigned i = 0; i < info->props->n_items; i++) fprintf(stderr, "%s=%s ", info->props->items[i].key, info->props->items[i].value);
        fprintf(stderr, "\n"); close_all(b->app); }
}
static void port_info(void *data, const struct pw_port_info *update) {
    struct binding *b = data;
    b->port_info = pw_port_info_update(b->port_info, update);
    const struct pw_port_info *info = b->port_info;
    struct object *o = object(b->app, b->id);
    const char *serial = info->props ? spa_dict_lookup(info->props, PW_KEY_OBJECT_SERIAL) : NULL;
    const char *node = info->props ? spa_dict_lookup(info->props, PW_KEY_NODE_ID) : NULL;
    const char *channel = info->props ? spa_dict_lookup(info->props, PW_KEY_AUDIO_CHANNEL) : NULL;
    b->valid = o && info->id == b->id && serial && strtoull(serial, NULL, 10) == b->serial
        && node && number(node) == o->node && channel && !strcmp(channel, o->channel)
        && info->direction == PW_DIRECTION_OUTPUT;
    if (!b->valid) { fprintf(stderr, "binding invalid id=%u port=%d serial=%s props=", b->id, b->port, serial ? serial : "missing");
        for (unsigned i = 0; i < info->props->n_items; i++) fprintf(stderr, "%s=%s ", info->props->items[i].key, info->props->items[i].value);
        fprintf(stderr, "\n"); close_all(b->app); }
}
static const struct pw_node_events node_events = { PW_VERSION_NODE_EVENTS, .info = node_info };
static const struct pw_port_events port_events = { PW_VERSION_PORT_EVENTS, .info = port_info };
static void bind_object(struct app *a, struct binding *b, struct object *o, bool port) {
    b->app = a; b->id = o->id; b->serial = o->serial; b->port = port;
    b->proxy = pw_registry_bind(a->registry, o->id, o->type,
        port ? PW_VERSION_PORT : PW_VERSION_NODE, 0);
    if (!b->proxy) die("bind failed");
    if (port) pw_port_add_listener((struct pw_port *)b->proxy, &b->hook, &port_events, b);
    else pw_node_add_listener((struct pw_node *)b->proxy, &b->hook, &node_events, b);
}
static void link_bound(void *data, uint32_t id) { ((struct owned_link *)data)->id = id; }
static void link_error(void *data, int seq, int res, const char *message) {
    (void)seq; (void)res; (void)message; struct owned_link *l = data;
    l->failed = true; close_all(l->app);
}
static const struct pw_proxy_events proxy_events = {
    PW_VERSION_PROXY_EVENTS, .bound = link_bound, .error = link_error };
static void link_info(void *data, const struct pw_link_info *update) {
    struct owned_link *l = data;
    l->info = pw_link_info_update(l->info, update);
    if (l->info->output_node_id != l->on || l->info->output_port_id != l->op
            || l->info->input_node_id != l->in || l->info->input_port_id != l->ip
            || l->info->state == PW_LINK_STATE_ERROR) {
        l->failed = true; close_all(l->app);
    }
}
static const struct pw_link_events link_events = { PW_VERSION_LINK_EVENTS, .info = link_info };
static void link_ports(struct app *a, struct object *out, struct object *in) {
    struct owned_link *l = calloc(1, sizeof(*l)); if (!l) die("out of memory");
    l->app = a; l->id = PW_ID_ANY; l->on = out->node; l->op = out->id;
    l->in = in->node; l->ip = in->id; l->next = a->links; a->links = l;
    struct pw_properties *p = pw_properties_new(PW_KEY_OBJECT_LINGER, "false", NULL);
    pw_properties_setf(p, PW_KEY_LINK_OUTPUT_NODE, "%u", l->on);
    pw_properties_setf(p, PW_KEY_LINK_OUTPUT_PORT, "%u", l->op);
    pw_properties_setf(p, PW_KEY_LINK_INPUT_NODE, "%u", l->in);
    pw_properties_setf(p, PW_KEY_LINK_INPUT_PORT, "%u", l->ip);
    l->proxy = pw_core_create_object(a->core, "link-factory", PW_TYPE_INTERFACE_Link,
        PW_VERSION_LINK, &p->dict, 0);
    pw_properties_free(p); if (!l->proxy) die("link creation failed");
    pw_proxy_add_listener(l->proxy, &l->hook, &proxy_events, l);
    pw_link_add_listener((struct pw_link *)l->proxy, &l->link_hook, &link_events, l);
}
static struct object *port(struct app *a, uint32_t node, const char *direction, const char *channel) {
    struct object *found = NULL;
    for (struct object *o = a->objects; o; o = o->next)
        if (o->node == node && !strcmp(o->direction, direction) && !strcmp(o->channel, channel)) {
            if (found) return NULL;
            found = o;
        }
    return found;
}
static bool valid(struct app *a) {
    for (unsigned i = 0; i < a->ns; i++) {
        struct source *s = &a->sources[i]; unsigned matches = 0;
        if (s->retired) return false;
        for (struct object *o = a->objects; o; o = o->next)
            if (!strcmp(o->type, PW_TYPE_INTERFACE_Node) && !strcmp(o->name, s->name)) {
                matches++; if (o->id != s->selected || o->serial != s->serial) return false;
            }
        if (matches != 1) return false;
        for (unsigned j = 0; j < 3; j++) {
            struct binding *b = &s->binds[j]; struct object *o = object(a, b->id);
            if (!b->valid || !o || o->serial != b->serial) return false;
        }
        uint32_t node = pw_filter_get_node_id(s->filter); unsigned count = 0;
        for (struct object *o = a->objects; o; o = o->next) if (o->in == node) {
            bool owned = false;
            for (struct owned_link *l = a->links; l; l = l->next)
                if (!l->failed && l->info && l->id == o->id && l->on == s->selected && l->op == o->op
                        && l->in == o->in && l->ip == o->ip) owned = true;
            if (!owned) return false;
            count++;
        }
        if (count != 2) return false;
    }
    return true;
}
static void resume_observer(void *data, uint64_t expirations) {
    (void)expirations; struct app *a = data; a->observer_delayed = false;
    if (!valid(a)) close_all(a);
    puts("observer resumed; identity revalidated"); fflush(stdout);
}
static void barrier(struct app *a) { a->sync_seq = pw_core_sync(a->core, PW_ID_CORE, 0); }
static void done(void *data, uint32_t id, int seq) {
    struct app *a = data; if (id != PW_ID_CORE || (unsigned)seq != a->sync_seq) return;
    bool ok = valid(a);
    if (a->action == ACTIVATED && ok) {
        for (unsigned s = 0; s < a->ns; s++) if (pw_filter_set_active(a->sources[s].filter, true) < 0) ok = false;
    } else if (a->action == OPEN && ok) {
        for (unsigned s = 0; s < a->ns; s++) if (atomic_load(&a->sources[s].cycles) < 2) ok = false;
        if (ok) for (unsigned s = 0; s < a->ns; s++) {
            atomic_fetch_add(&a->sources[s].epoch, 1); /* discard staged blocks */
            atomic_store_explicit(&a->sources[s].gate, true, memory_order_release);
        }
    }
    if (!ok) close_all(a);
    printf("barrier %u %s\n", a->action, ok ? "validated" : "blocked"); fflush(stdout);
    a->action = IDLE;
}
static void core_error(void *data, uint32_t id, int seq, int res, const char *message) {
    (void)seq; struct app *a = data; close_all(a);
    fprintf(stderr, "core error id=%u res=%d %s\n", id, res, message);
    if (id == PW_ID_CORE) { a->fatal = true; pw_main_loop_quit(a->loop); }
}
static const struct pw_core_events core_events = { PW_VERSION_CORE_EVENTS, .done = done, .error = core_error };
static void connect_sources(struct app *a) {
    for (unsigned s = 0; s < a->ns; s++) {
        struct source *src = &a->sources[s]; struct object *selected = NULL;
        if (src->retired || src->selected != PW_ID_ANY) { puts("blocked retired/already bound"); return; }
        for (struct object *o = a->objects; o; o = o->next)
            if (!strcmp(o->type, PW_TYPE_INTERFACE_Node) && !strcmp(o->name, src->name)) {
                if (selected) { close_all(a); puts("blocked duplicate selector"); return; }
                selected = o;
            }
        if (!selected || !selected->serial) { close_all(a); puts("blocked missing identity"); return; }
        src->selected = selected->id; src->serial = selected->serial;
        bind_object(a, &src->binds[0], selected, false);
        for (unsigned c = 0; c < 2; c++) {
            struct object *out = port(a, selected->id, "out", c ? "FR" : "FL");
            struct object *in = port(a, pw_filter_get_node_id(src->filter), "in", c ? "FR" : "FL");
            if (!out || !in) { close_all(a); puts("blocked missing/duplicate channel"); return; }
            bind_object(a, &src->binds[c + 1], out, true); link_ports(a, out, in);
        }
        printf("identity %s node=%u serial=%llu epoch=%llu\n", src->name, src->selected,
            (unsigned long long)src->serial, (unsigned long long)atomic_load(&src->epoch));
    }
    a->action = LINKED; barrier(a);
}
static void trace_dump(struct app *a) {
    for (unsigned s = 0; s < a->ns; s++) for (unsigned m = 0; m < a->nm; m++) {
        struct queue *q = queue(a, s, m);
        unsigned count = atomic_load_explicit(&q->traced, memory_order_acquire);
        for (unsigned i = 0; i < count; i++) {
            struct trace *t = &q->trace[i];
            printf("trace %u %u %llu %llu %llu %llu %u %u %u %u %llu %llu\n", s, m,
                (unsigned long long)t->frame, (unsigned long long)t->ticks, (unsigned long long)t->position,
                (unsigned long long)t->epoch, t->frames, t->depth, t->flags, t->revision, (unsigned long long)t->now, (unsigned long long)t->wanted);
        }
        printf("trace-overflow %u %u %llu\n", s, m, (unsigned long long)atomic_load(&q->trace_overflow));
    }
}
static void status(struct app *a) {
    printf("status revision=%u valid=%d\n", atomic_load(&a->revision), valid(a));
    for (unsigned s = 0; s < a->ns; s++) printf("source %u gate=%d retired=%d epoch=%llu first=%llu cycles=%llu quantum=%u\n", s,
        atomic_load(&a->sources[s].gate), a->sources[s].retired,
        (unsigned long long)atomic_load(&a->sources[s].epoch),
        (unsigned long long)atomic_load(&a->sources[s].first_frame),
        (unsigned long long)atomic_load(&a->sources[s].cycles), atomic_load(&a->sources[s].quantum));
    for (unsigned m = 0; m < a->nm; m++) {
        printf("mix %u ack=%llu frame=%llu frames=%llu stale=%llu bad_quantum=%llu quantum=%u\n", m,
            (unsigned long long)atomic_load(&a->mixes[m].ack), (unsigned long long)atomic_load(&a->mixes[m].ack_frame),
            (unsigned long long)atomic_load(&a->mixes[m].frames), (unsigned long long)atomic_load(&a->mixes[m].stale),
            (unsigned long long)atomic_load(&a->mixes[m].bad_quantum), atomic_load(&a->mixes[m].quantum));
        for (unsigned s = 0; s < a->ns; s++) {
            struct queue *q = queue(a, s, m);
            printf("queue %u %u depth=%u drops=%llu underruns=%llu\n", s, m,
                atomic_load(&q->write) - atomic_load(&q->read),
                (unsigned long long)atomic_load(&q->drops), (unsigned long long)atomic_load(&q->underruns));
        }
    }
}
static void command(void *data, int fd, uint32_t mask) {
    (void)mask; struct app *a = data; char line[256];
    /* Control thread only; one newline per write from the harness. */
    if (!fgets(line, sizeof(line), stdin)) { quit(a, 0); return; }
    unsigned m, s, mute; float gain; char extra;
    if (!strcmp(line, "bind\n")) connect_sources(a);
    else if (!strcmp(line, "activate\n") || !strcmp(line, "open\n")) {
        if (a->action != IDLE) puts("blocked pending barrier");
        else { a->action = line[0] == 'a' ? ACTIVATED : OPEN; barrier(a); }
    } else if (!strcmp(line, "status\n")) status(a);
    else if (!strcmp(line, "trace\n")) trace_dump(a);
    else if (!strcmp(line, "delay-observer\n") && valid(a) && !a->observer_delayed) {
        /* PRIVATE fault injection: delay only loss observations, not protocol
         * processing. This is a simulated notification lag, not a host exploit. */
        a->observer_delayed = true; struct timespec delay = { .tv_sec = 2 };
        pw_loop_update_timer(pw_main_loop_get_loop(a->loop), a->delay_timer, &delay, NULL, false);
        puts("observer delay injected for 2 seconds");
    }
    else if (!strcmp(line, "hold-control\n")) {
        puts("holding control for 2 seconds; RT continues"); fflush(stdout);
        usleep(2000000);
    }
    else if (!strcmp(line, "close\n")) close_all(a);
    else if (!strcmp(line, "quit\n")) quit(a, 0);
    else if (sscanf(line, "set %u %u %f %u %c", &m, &s, &gain, &mute, &extra) == 4) {
        unsigned rev = atomic_load(&a->revision); bool ready = true;
        for (unsigned i = 0; i < a->nm; i++) if (atomic_load_explicit(&a->mixes[i].ack, memory_order_acquire) != rev) ready = false;
        if (!ready || m >= a->nm || s >= a->ns || !isfinite(gain) || gain < 0 || gain > 4 || mute > 1) puts("blocked revision/value");
        else {
            memcpy(a->gains[(rev + 1) % 2], a->gains[rev % 2], a->ns * a->nm * sizeof(float));
            a->gains[(rev + 1) % 2][m * a->ns + s] = mute ? 0 : gain;
            atomic_store_explicit(&a->revision, rev + 1, memory_order_release);
            if (a->triggered && atomic_load(&a->sources[0].cycles) == 0)
                for (unsigned i = 0; i < a->nm; i++) pw_stream_trigger_process(a->mixes[i].stream);
            printf("prepared %u\n", rev + 1);
        }
    } else if (sscanf(line, "stall %u %u %c", &m, &s, &extra) == 2 && m < a->nm && s <= 1) {
        atomic_fetch_add(&a->mixes[m].epoch, 1);
        atomic_store(&a->mixes[m].stall, s); puts("stall prepared");
    } else puts("blocked command");
    (void)fd; fflush(stdout);
}
int main(int argc, char **argv) {
    if (!getenv("MIXYARD_PRIVATE_PROOF") || strcmp(getenv("MIXYARD_PRIVATE_PROOF"), "1")
            || !getenv("PIPEWIRE_REMOTE") || strcmp(getenv("PIPEWIRE_REMOTE"), "mixyard-private"))
        die("PRIVATE proof only: explicit private environment required; production apply unavailable");
    if (argc < 5) die("usage: mixyard-pw SOURCE... -- MIX...");
    struct app a = {0}; unsigned split = 0;
    a.triggered = !getenv("MIXYARD_TRIGGERED") || strcmp(getenv("MIXYARD_TRIGGERED"), "0");
    atomic_init(&a.last_trigger, UINT64_MAX);
    for (int i = 1; i < argc; i++) if (!strcmp(argv[i], "--")) { if (split) die("duplicate separator"); split = i; }
    if (split < 2 || split >= (unsigned)argc - 1) die("sources and mixes required");
    a.ns = split - 1; a.nm = argc - split - 1;
    if ((size_t)a.ns * a.nm > SIZE_MAX / sizeof(struct queue)) die("fixture too large");
    a.sources = calloc(a.ns, sizeof(*a.sources)); a.mixes = calloc(a.nm, sizeof(*a.mixes));
    a.queues = calloc((size_t)a.ns * a.nm, sizeof(*a.queues));
    for (unsigned i = 0; i < 2; i++) a.gains[i] = calloc((size_t)a.ns * a.nm, sizeof(float));
    if (!a.sources || !a.mixes || !a.queues || !a.gains[0] || !a.gains[1]) die("allocation failed");
    if (!atomic_is_lock_free(&a.revision) || !atomic_is_lock_free(&a.mixes[0].ack)
            || !atomic_is_lock_free(&a.sources[0].gate)) die("lock-free atomics required");
    for (size_t i = 0; i < (size_t)a.ns * a.nm; i++) {
        atomic_init(&a.queues[i].write, 0); atomic_init(&a.queues[i].read, 0);
        atomic_init(&a.queues[i].traced, 0); atomic_init(&a.queues[i].trace_overflow, 0);
        atomic_init(&a.queues[i].drops, 0); atomic_init(&a.queues[i].underruns, 0);
    }
    for (unsigned i = 1; i < (unsigned)argc; i++) if (i != split) {
        if (!*argv[i] || strlen(argv[i]) > 100) die("invalid fixture name");
        for (unsigned j = 1; j < i; j++) if (j != split && !strcmp(argv[i], argv[j])) die("duplicate fixture name");
        for (unsigned j = 1; j < split; j++) {
            char guard[128]; snprintf(guard, sizeof(guard), "guard.%s", argv[j]);
            if (!strcmp(argv[i], guard)) die("fixture name collides with owned ingress");
        }
    }
    sigset_t signals; sigemptyset(&signals); sigaddset(&signals, SIGTERM); sigaddset(&signals, SIGINT);
    if (sigprocmask(SIG_BLOCK, &signals, NULL) < 0) return 2;
    pw_init(&argc, &argv); a.loop = pw_main_loop_new(NULL);
    struct pw_loop *loop = pw_main_loop_get_loop(a.loop);
    a.context = pw_context_new(loop, NULL, 0); a.core = pw_context_connect(a.context, NULL, 0);
    if (!a.core) die("private connection failed");
    pw_core_add_listener(a.core, &a.core_hook, &core_events, &a);
    a.registry = pw_core_get_registry(a.core, PW_VERSION_REGISTRY, 0);
    pw_registry_add_listener(a.registry, &a.registry_hook, &registry_events, &a);
    for (unsigned i = 0; i < a.ns; i++) {
        struct source *s = &a.sources[i]; s->app = &a; s->name = argv[i + 1]; s->selected = PW_ID_ANY;
        atomic_init(&s->position, UINT64_MAX); atomic_init(&s->quantum, 0); atomic_init(&s->gate, false); atomic_init(&s->cycles, 0);
        atomic_init(&s->epoch, 1); atomic_init(&s->first_frame, UINT64_MAX);
        for (unsigned j = 0; j < 3; j++) s->binds[j].id = PW_ID_ANY;
        char name[128]; snprintf(name, sizeof(name), "guard.%s", s->name);
        s->filter = pw_filter_new(a.core, name, pw_properties_new(PW_KEY_NODE_NAME, name,
            PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, "Filter", PW_KEY_MEDIA_ROLE, "DSP",
            PW_KEY_NODE_AUTOCONNECT, "false", PW_KEY_NODE_ALWAYS_PROCESS, "true", "node.want-driver", "true", NULL));
        pw_filter_add_listener(s->filter, &s->hook, &filter_events, s);
        for (unsigned c = 0; c < 2; c++) s->ports[c] = pw_filter_add_port(s->filter, PW_DIRECTION_INPUT,
            PW_FILTER_PORT_FLAG_MAP_BUFFERS, 1, pw_properties_new(PW_KEY_FORMAT_DSP, "32 bit float mono audio",
                PW_KEY_PORT_NAME, c ? "in_FR" : "in_FL", PW_KEY_AUDIO_CHANNEL, c ? "FR" : "FL",
                "port.exclusive", "true", NULL), NULL, 0);
        if (!s->ports[0] || !s->ports[1] || pw_filter_connect(s->filter,
                PW_FILTER_FLAG_RT_PROCESS | PW_FILTER_FLAG_INACTIVE, NULL, 0) < 0) die("ingress creation failed");
    }
    for (unsigned i = 0; i < a.nm; i++) {
        struct mix *m = &a.mixes[i]; m->app = &a; m->index = i; m->name = argv[split + 1 + i];
        atomic_init(&m->ack, 0); atomic_init(&m->ack_frame, 0); atomic_init(&m->frames, 0);
        atomic_init(&m->stale, 0); atomic_init(&m->bad_quantum, 0);
        atomic_init(&m->quantum, 0); atomic_init(&m->stall, false); atomic_init(&m->epoch, 0);
        m->stream = pw_stream_new(a.core, m->name, pw_properties_new(PW_KEY_NODE_NAME, m->name,
            PW_KEY_NODE_DESCRIPTION, m->name, PW_KEY_MEDIA_CLASS, "Audio/Source", PW_KEY_MEDIA_TYPE, "Audio",
            PW_KEY_MEDIA_CATEGORY, "Capture", PW_KEY_NODE_VIRTUAL, "true", PW_KEY_NODE_AUTOCONNECT, "false",
            PW_KEY_NODE_ALWAYS_PROCESS, "true", "node.want-driver", "true", "node.pause-on-idle", "false", NULL));
        pw_stream_add_listener(m->stream, &m->hook, &stream_events, m);
        uint8_t buffer[1024]; struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
        const struct spa_pod *param = spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat,
            &SPA_AUDIO_INFO_RAW_INIT(.format = SPA_AUDIO_FORMAT_F32, .rate = 48000, .channels = 2,
                .position = {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR}));
        if (pw_stream_connect(m->stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
            PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS | (a.triggered ? PW_STREAM_FLAG_TRIGGER : 0), &param, 1) < 0) die("egress creation failed");
    }
    a.delay_timer = pw_loop_add_timer(loop, resume_observer, &a);
    if (!a.delay_timer) die("timer creation failed");
    pw_loop_add_signal(loop, SIGINT, quit, &a); pw_loop_add_signal(loop, SIGTERM, quit, &a);
    setvbuf(stdin, NULL, _IONBF, 0); pw_loop_add_io(loop, STDIN_FILENO, SPA_IO_IN | SPA_IO_HUP, false, command, &a);
    printf("private runtime sdk=%s sources=%u mixes=%u queue_slots=%u max_frames=%u\n", pw_get_library_version(), a.ns, a.nm, SLOTS, FRAMES); fflush(stdout);
    pw_main_loop_run(a.loop); status(&a); trace_dump(&a);
    while (a.links) { struct owned_link *l = a.links; a.links = l->next;
        pw_proxy_destroy(l->proxy); if (l->info) pw_link_info_free(l->info); free(l); }
    for (unsigned i = 0; i < a.ns; i++) {
        for (unsigned j = 0; j < 3; j++) { struct binding *b = &a.sources[i].binds[j];
            if (b->proxy) pw_proxy_destroy(b->proxy);
            if (b->port_info) pw_port_info_free(b->port_info);
            if (b->node_info) pw_node_info_free(b->node_info);
        }
        pw_filter_destroy(a.sources[i].filter);
    }
    for (unsigned i = 0; i < a.nm; i++) pw_stream_destroy(a.mixes[i].stream);
    pw_proxy_destroy((struct pw_proxy *)a.registry); pw_core_disconnect(a.core);
    pw_context_destroy(a.context); pw_main_loop_destroy(a.loop); pw_deinit();
    while (a.objects) { struct object *o = a.objects; a.objects = o->next;
        free(o->type); free(o->name); free(o->direction); free(o->channel); free(o); }
    free(a.sources); free(a.mixes); free(a.queues); free(a.gains[0]); free(a.gains[1]);
    return a.fatal ? 2 : 0;
}

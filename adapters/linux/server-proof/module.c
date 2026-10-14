/* SPDX-License-Identifier: MIT
 * Copyright (c) 2026 Mathis Graff
 * PRIVATE proof: compiled against the exact PipeWire 1.6.9 server source/build.
 */
#include <config.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#include <spa/node/utils.h>
#include <spa/node/io.h>
#include <spa/node/command.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/buffers.h>
#include <spa/pod/filter.h>
#include <pipewire/private.h>

#define FRAMES 4096u
#define QUEUE 8u
#define BUFFERS 16u
#define CONSENT "MIXYARD_PRIVATE_SERVER_PROOF_V1\n"
#define INSECURE_CONSENT "MIXYARD_INSECURE_NEGATIVE_CONTROL_V1\n"

struct proof;
struct peer { struct proof *proof; int fd; struct spa_source *source; };
struct block { uint64_t generation; uint32_t frames, offset; float pcm[2][FRAMES]; };
struct node {
    struct proof *proof;
    unsigned kind; /* 0 ingress, 1 public output, 2/3 controlled fixtures */
    struct spa_node spa;
    struct spa_hook_list hooks;
    struct spa_callbacks callbacks;
    struct spa_node_info info;
    struct spa_port_info port_info;
    struct spa_param_info params[4];
    struct pw_impl_node *impl;
    struct pw_impl_port *port;
    struct spa_hook node_hook, port_hook;
    struct spa_io_buffers *io;
    struct spa_io_position *position;
    struct spa_buffer *buffers[BUFFERS];
    bool outstanding[BUFFERS];
    uint32_t n_buffers;
    bool formatted, started;
    atomic_uint_fast64_t processed, position_frames;
};
struct proof {
    struct pw_impl_module *module;
    struct pw_context *context;
    struct pw_loop *mainloop, *rtloop;
    struct spa_hook module_hook;
    struct node nodes[4];
    struct pw_impl_link *link;
    struct pw_impl_node *selected_node;
    struct pw_impl_port *selected_port;
    struct spa_hook link_hook, selected_node_hook, selected_port_hook, owner_hook;
    struct block queue[QUEUE];
    uint32_t read, write; /* only accessed while holding the single shared data loop */
    atomic_uint_fast64_t generation, format_errors, discarded, captured, emitted;
    uint64_t next_generation;
    bool insecure, creating, retiring, stopping, fixtures;
    int fd;
    struct spa_source *socket_source;
    struct spa_source *fault_timer;
    struct peer peers[8];
    dev_t socket_dev;
    ino_t socket_ino;
    char socket_path[sizeof(((struct sockaddr_un *)0)->sun_path)];
};

static void retire(struct proof *p);
static void remove_hook(struct spa_hook *hook)
{
    spa_hook_remove(hook);
    memset(hook, 0, sizeof(*hook));
}
static void fault(struct proof *p)
{
    atomic_store_explicit(&p->generation, 0, memory_order_release);
    atomic_fetch_add_explicit(&p->format_errors, 1, memory_order_relaxed);
}
static enum spa_direction direction(struct node *n)
{ return n->kind == 0 ? SPA_DIRECTION_INPUT : SPA_DIRECTION_OUTPUT; }
static bool valid_port(struct node *n, enum spa_direction d, uint32_t id)
{ return d == direction(n) && id == 0; }
static struct spa_audio_info_raw format(void)
{
    return (struct spa_audio_info_raw) { .format = SPA_AUDIO_FORMAT_F32P,
        .rate = 48000, .channels = 2,
        .position = { SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR } };
}
static bool fixed_format(const struct spa_pod *pod)
{
    uint32_t type, subtype;
    struct spa_audio_info_raw f = {0};
    return pod && spa_format_parse(pod, &type, &subtype) >= 0 &&
        type == SPA_MEDIA_TYPE_audio && subtype == SPA_MEDIA_SUBTYPE_raw &&
        spa_format_audio_raw_parse(pod, &f) >= 0 && f.format == SPA_AUDIO_FORMAT_F32P &&
        f.rate == 48000 && f.channels == 2 && !(f.flags & SPA_AUDIO_FLAG_UNPOSITIONED) &&
        f.position[0] == SPA_AUDIO_CHANNEL_FL && f.position[1] == SPA_AUDIO_CHANNEL_FR;
}
static void port_emit(struct node *n)
{
    n->port_info.change_mask = SPA_PORT_CHANGE_MASK_PARAMS | SPA_PORT_CHANGE_MASK_RATE;
    spa_node_emit_port_info(&n->hooks, direction(n), 0, &n->port_info);
    n->port_info.change_mask = 0;
}
static int add_listener(void *data, struct spa_hook *h, const struct spa_node_events *e, void *ud)
{
    struct node *n = data;
    struct spa_hook_list saved;
    spa_hook_list_isolate(&n->hooks, &saved, h, e, ud);
    n->info.change_mask = SPA_NODE_CHANGE_MASK_FLAGS | SPA_NODE_CHANGE_MASK_PROPS;
    spa_node_emit_info(&n->hooks, &n->info);
    n->info.change_mask = 0;
    n->port_info.change_mask = SPA_PORT_CHANGE_MASK_FLAGS | SPA_PORT_CHANGE_MASK_PROPS |
        SPA_PORT_CHANGE_MASK_PARAMS | SPA_PORT_CHANGE_MASK_RATE;
    spa_node_emit_port_info(&n->hooks, direction(n), 0, &n->port_info);
    n->port_info.change_mask = 0;
    spa_hook_list_join(&n->hooks, &saved);
    return 0;
}
static int callbacks(void *data, const struct spa_node_callbacks *cb, void *ud)
{ ((struct node *)data)->callbacks = SPA_CALLBACKS_INIT(cb, ud); return 0; }
static int sync_node(void *data, int seq)
{ spa_node_emit_result(&((struct node *)data)->hooks, seq, 0, 0, NULL); return 0; }
static int enum_node(void *data, int seq, uint32_t id, uint32_t start, uint32_t num, const struct spa_pod *filter)
{ return -ENOENT; }
static int set_node(void *data, uint32_t id, uint32_t flags, const struct spa_pod *pod)
{ return -ENOENT; }
static int node_io(void *data, uint32_t id, void *io, size_t size)
{
    struct node *n = data;
    if (id == SPA_IO_Position) {
        if (io && size < sizeof(struct spa_io_position)) return -ENOSPC;
        n->position = io; return 0;
    }
    return -ENOENT;
}
static int command(void *data, const struct spa_command *cmd)
{
    struct node *n = data;
    switch (SPA_NODE_COMMAND_ID(cmd)) {
    case SPA_NODE_COMMAND_Start:
        if (!n->formatted || !n->n_buffers || !n->io) return -EIO;
        n->started = true; return 0;
    case SPA_NODE_COMMAND_Pause:
    case SPA_NODE_COMMAND_Suspend: n->started = false; return 0;
    default: return -ENOTSUP;
    }
}
static int add_port(void *data, enum spa_direction d, uint32_t id, const struct spa_dict *props)
{ return -ENOTSUP; }
static int remove_port(void *data, enum spa_direction d, uint32_t id)
{ return -ENOTSUP; }
static int enum_port(void *data, int seq, enum spa_direction d, uint32_t port,
        uint32_t id, uint32_t start, uint32_t num, const struct spa_pod *filter)
{
    struct node *n = data;
    uint8_t bytes[2048];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(bytes, sizeof(bytes));
    struct spa_pod *pod;
    struct spa_result_node_params r = { .id = id, .index = start, .next = start + 1 };
    if (!valid_port(n, d, port) || !num) return -EINVAL;
    if (start) return 0;
    switch (id) {
    case SPA_PARAM_EnumFormat:
    case SPA_PARAM_Format: {
        struct spa_audio_info_raw f = format();
        if (id == SPA_PARAM_Format && !n->formatted) return -EIO;
        pod = spa_format_audio_raw_build(&b, id, &f); break;
    }
    case SPA_PARAM_Buffers:
        if (!n->formatted) return -EIO;
        pod = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamBuffers, id,
            SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, BUFFERS),
            SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(2),
            SPA_PARAM_BUFFERS_size, SPA_POD_CHOICE_RANGE_Int(FRAMES * 4, 4, FRAMES * 4),
            SPA_PARAM_BUFFERS_stride, SPA_POD_Int(4)); break;
    case SPA_PARAM_IO:
        pod = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamIO, id,
            SPA_PARAM_IO_id, SPA_POD_Id(SPA_IO_Buffers),
            SPA_PARAM_IO_size, SPA_POD_Int(sizeof(struct spa_io_buffers))); break;
    default: return -ENOENT;
    }
    if (spa_pod_filter(&b, &r.param, pod, filter) >= 0)
        spa_node_emit_result(&n->hooks, seq, 0, SPA_RESULT_TYPE_NODE_PARAMS, &r);
    return 0;
}
static int set_port(void *data, enum spa_direction d, uint32_t port,
        uint32_t id, uint32_t flags, const struct spa_pod *pod)
{
    struct node *n = data;
    if (!valid_port(n, d, port)) return -EINVAL;
    if (id != SPA_PARAM_Format) return -ENOENT;
    if (pod && !fixed_format(pod)) { fault(n->proof); return -EINVAL; }
    if (!(flags & SPA_NODE_PARAM_FLAG_TEST_ONLY)) {
        if (atomic_load(&n->proof->generation) && n->kind < 2 && !pod) fault(n->proof);
        n->formatted = pod != NULL;
        if (!pod) { n->started = false; n->n_buffers = 0; }
        n->params[1] = SPA_PARAM_INFO(SPA_PARAM_Format,
            SPA_PARAM_INFO_WRITE | (n->formatted ? SPA_PARAM_INFO_READ : 0));
        n->params[2] = SPA_PARAM_INFO(SPA_PARAM_Buffers, n->formatted ? SPA_PARAM_INFO_READ : 0);
        port_emit(n);
    }
    return 0;
}
static bool buffer_valid(struct spa_buffer *b)
{
    if (!b || b->n_datas != 2) return false;
    for (unsigned c = 0; c < 2; c++)
        if (!b->datas[c].data || !b->datas[c].chunk || b->datas[c].maxsize < 4 ||
            b->datas[c].maxsize > FRAMES * 4 || b->datas[c].maxsize % 4) return false;
    return true;
}
static int use_buffers(void *data, enum spa_direction d, uint32_t port, uint32_t flags,
        struct spa_buffer **buffers, uint32_t count)
{
    struct node *n = data;
    if (!valid_port(n, d, port) || count > BUFFERS || (count && !n->formatted)) return -EINVAL;
    for (uint32_t i = 0; i < count; i++) if (!buffer_valid(buffers[i])) { fault(n->proof); return -EINVAL; }
    n->n_buffers = count;
    n->started = false;
    memset(n->outstanding, 0, sizeof(n->outstanding));
    for (uint32_t i = 0; i < count; i++) n->buffers[i] = buffers[i];
    return 0;
}
static int port_io(void *data, enum spa_direction d, uint32_t port, uint32_t id, void *io, size_t size)
{
    struct node *n = data;
    if (!valid_port(n, d, port)) return -EINVAL;
    if (id != SPA_IO_Buffers) return -ENOENT;
    if (io && size < sizeof(struct spa_io_buffers)) return -ENOSPC;
    n->io = io; return 0;
}
static int reuse(void *data, uint32_t port, uint32_t buffer)
{
    struct node *n = data;
    if (port || buffer >= n->n_buffers) return -EINVAL;
    n->outstanding[buffer] = false; return 0;
}
static int process(void *data)
{
    struct node *n = data;
    struct proof *p = n->proof;
    struct spa_io_buffers *io = n->io;
    uint64_t gen = atomic_load_explicit(&p->generation, memory_order_acquire);
    if (!n->started || !n->formatted || !io) return -EIO;
    uint32_t duration = n->position ? n->position->clock.duration : 0;
    if (!duration || duration > FRAMES || !n->position ||
        n->position->clock.rate.num != 1 || n->position->clock.rate.denom != 48000) {
        fault(p); return -EINVAL;
    }
    atomic_store_explicit(&n->position_frames, n->position->clock.position, memory_order_relaxed);
    if (n->kind == 0) {
        if (io->status != SPA_STATUS_HAVE_DATA) return SPA_STATUS_NEED_DATA;
        if (io->buffer_id >= n->n_buffers) { fault(p); return -EINVAL; }
        struct spa_buffer *b = n->buffers[io->buffer_id];
        uint32_t size = b->datas[0].chunk->size;
        bool valid = size && size % 4 == 0 && size <= FRAMES * 4;
        for (unsigned c = 0; c < 2; c++) {
            struct spa_data *s = &b->datas[c];
            valid = valid && s->chunk->size == size && s->chunk->stride == 4 &&
                s->chunk->offset % 4 == 0 && s->chunk->offset <= s->maxsize &&
                size <= s->maxsize - s->chunk->offset;
        }
        if (!valid) { fault(p); io->status = SPA_STATUS_NEED_DATA; return -EINVAL; }
        uint32_t frames = size / 4;
        if (gen && p->write - p->read < QUEUE) {
            struct block *q = &p->queue[p->write % QUEUE];
            q->generation = gen; q->frames = frames; q->offset = 0;
            for (unsigned c = 0; c < 2; c++)
                memcpy(q->pcm[c], SPA_PTROFF(b->datas[c].data, b->datas[c].chunk->offset, void), size);
            p->write++;
            atomic_fetch_add_explicit(&p->captured, frames, memory_order_relaxed);
        } else atomic_fetch_add_explicit(&p->discarded, frames, memory_order_relaxed);
        atomic_fetch_add_explicit(&n->processed, frames, memory_order_relaxed);
        io->status = SPA_STATUS_NEED_DATA;
        return SPA_STATUS_NEED_DATA;
    }
    if (io->status == SPA_STATUS_HAVE_DATA) return SPA_STATUS_HAVE_DATA;
    if (io->buffer_id < n->n_buffers) n->outstanding[io->buffer_id] = false;
    uint32_t id;
    for (id = 0; id < n->n_buffers; id++) if (!n->outstanding[id]) break;
    if (id == n->n_buffers) return -EPIPE;
    struct spa_buffer *b = n->buffers[id];
    for (unsigned c = 0; c < 2; c++) {
        if (duration * 4 > b->datas[c].maxsize) { fault(p); return -ENOSPC; }
        memset(b->datas[c].data, 0, duration * 4);
        b->datas[c].chunk->offset = 0; b->datas[c].chunk->size = duration * 4; b->datas[c].chunk->stride = 4;
    }
    uint32_t produced = 0;
    if (n->kind >= 2) {
        float value = n->kind == 2 ? 0.125f : 0.75f;
        for (uint32_t f = 0; f < duration; f++) {
            ((float *)b->datas[0].data)[f] = value;
            ((float *)b->datas[1].data)[f] = -value;
        }
    } else if (gen) {
        while (produced < duration && p->read != p->write) {
            struct block *q = &p->queue[p->read % QUEUE];
            if (q->generation != gen) {
                atomic_fetch_add_explicit(&p->discarded, q->frames - q->offset, memory_order_relaxed);
                p->read++; continue;
            }
            uint32_t count = SPA_MIN(duration - produced, q->frames - q->offset);
            for (unsigned c = 0; c < 2; c++)
                memcpy((float *)b->datas[c].data + produced, q->pcm[c] + q->offset, count * 4);
            q->offset += count; produced += count;
            if (q->offset == q->frames) p->read++;
        }
        /* Retirement may start on the mainloop while this block is copied. */
        if (atomic_load_explicit(&p->generation, memory_order_acquire) != gen) {
            for (unsigned c = 0; c < 2; c++) memset(b->datas[c].data, 0, duration * 4);
            atomic_fetch_add_explicit(&p->discarded, produced, memory_order_relaxed); produced = 0;
        }
        atomic_fetch_add_explicit(&p->emitted, produced, memory_order_relaxed);
    }
    atomic_fetch_add_explicit(&n->processed, duration, memory_order_relaxed);
    n->outstanding[id] = true; io->buffer_id = id; io->status = SPA_STATUS_HAVE_DATA;
    return SPA_STATUS_HAVE_DATA;
}
static const struct spa_node_methods methods = {
    SPA_VERSION_NODE_METHODS, .add_listener = add_listener, .set_callbacks = callbacks,
    .sync = sync_node, .enum_params = enum_node, .set_param = set_node, .set_io = node_io,
    .send_command = command, .add_port = add_port, .remove_port = remove_port,
    .port_enum_params = enum_port, .port_set_param = set_port, .port_use_buffers = use_buffers,
    .port_set_io = port_io, .port_reuse_buffer = reuse, .process = process,
};

static int clear_rt(struct spa_loop *loop, bool async, uint32_t seq,
        const void *data, size_t size, void *ud)
{
    struct proof *p = ud;
    for (uint32_t i = p->read; i != p->write; i++) {
        struct block *q = &p->queue[i % QUEUE];
        atomic_fetch_add(&p->discarded, q->frames - q->offset);
    }
    p->read = p->write = 0;
    memset(p->queue, 0, sizeof(p->queue));
    for (unsigned k = 0; k < 2; k++) {
        struct node *n = &p->nodes[k];
        for (unsigned i = 0; i < n->n_buffers; i++)
            for (unsigned c = 0; c < 2; c++)
                memset(n->buffers[i]->datas[c].data, 0, n->buffers[i]->datas[c].maxsize);
        if (n->io) { n->io->buffer_id = SPA_ID_INVALID; n->io->status = SPA_STATUS_NEED_DATA; }
        memset(n->outstanding, 0, sizeof(n->outstanding));
    }
    return 0;
}
static void retire(struct proof *p)
{
    atomic_store_explicit(&p->generation, 0, memory_order_release);
    if (p->retiring) return;
    p->retiring = true;
    if (p->rtloop) pw_loop_locked(p->rtloop, clear_rt, 0, NULL, 0, p);
    remove_hook(&p->selected_node_hook);
    remove_hook(&p->selected_port_hook);
    remove_hook(&p->owner_hook);
    remove_hook(&p->link_hook);
    p->selected_node = NULL; p->selected_port = NULL;
    struct pw_impl_link *link = p->link;
    p->link = NULL;
    if (link) pw_impl_link_destroy(link);
    p->retiring = false;
}
static void port_initialized(void *data)
{
    struct node *n = data;
    if (n->kind == 0 && !n->proof->insecure) {
        if (!n->port || !n->port->global || n->port->global->registered) { fault(n->proof); return; }
        n->port->global->permission_mask = 0;
    }
}
static void port_destroyed(void *data)
{
    struct node *n = data;
    remove_hook(&n->port_hook);
    n->port = NULL;
    if (!n->proof->stopping && n->kind < 2) retire(n->proof);
}
static const struct pw_impl_port_events own_port_events = {
    PW_VERSION_IMPL_PORT_EVENTS, .initialized = port_initialized, .destroy = port_destroyed,
};
static void node_initialized(void *data)
{
    struct node *n = data;
    if (n->kind == 0 && !n->proof->insecure) {
        struct pw_global *g = pw_impl_node_get_global(n->impl);
        if (!g || g->registered) { fault(n->proof); return; }
        g->permission_mask = 0;
    }
}
static void node_port_init(void *data, struct pw_impl_port *port)
{
    struct node *n = data;
    /* port_added is too late: its global is already registered. */
    if (n->port || pw_impl_port_get_id(port) != 0 ||
        pw_impl_port_get_direction(port) != (enum pw_direction)direction(n)) {
        fault(n->proof); return;
    }
    n->port = port;
    pw_impl_port_add_listener(port, &n->port_hook, &own_port_events, n);
}
static void own_destroyed(void *data)
{
    struct node *n = data;
    remove_hook(&n->node_hook);
    n->impl = NULL;
    if (!n->proof->stopping) retire(n->proof);
}
static const struct pw_impl_node_events own_node_events = {
    PW_VERSION_IMPL_NODE_EVENTS, .initialized = node_initialized,
    .port_init = node_port_init, .destroy = own_destroyed,
};
static int make_node(struct proof *p, unsigned kind)
{
    static const char *names[] = { "mixyard.proof.ingress", "mixyard.proof.output",
        "mixyard.proof.allowed", "mixyard.proof.forbidden" };
    static const struct spa_dict_item port_items[] = {
        { "audio.channel", "FL,FR" }, { "port.name", "stereo" },
        { "audio.channels", "2" }, { "audio.position", "[ FL FR ]" },
    };
    static const struct spa_dict port_props = SPA_DICT_INIT_ARRAY(port_items);
    struct node *n = &p->nodes[kind];
    n->proof = p; n->kind = kind;
    atomic_init(&n->processed, 0); atomic_init(&n->position_frames, 0);
    spa_hook_list_init(&n->hooks);
    n->spa.iface = SPA_INTERFACE_INIT(SPA_TYPE_INTERFACE_Node, SPA_VERSION_NODE, &methods, n);
    n->info = SPA_NODE_INFO_INIT();
    n->info.flags = SPA_NODE_FLAG_RT;
    n->info.max_input_ports = kind == 0 ? 1 : 0;
    n->info.max_output_ports = kind == 0 ? 0 : 1;
    n->params[0] = SPA_PARAM_INFO(SPA_PARAM_EnumFormat, SPA_PARAM_INFO_READ);
    n->params[1] = SPA_PARAM_INFO(SPA_PARAM_Format, SPA_PARAM_INFO_WRITE);
    n->params[2] = SPA_PARAM_INFO(SPA_PARAM_Buffers, 0);
    n->params[3] = SPA_PARAM_INFO(SPA_PARAM_IO, SPA_PARAM_INFO_READ);
    n->port_info = SPA_PORT_INFO_INIT();
    n->port_info.flags = SPA_PORT_FLAG_NO_REF;
    n->port_info.rate = SPA_FRACTION(1, 48000);
    n->port_info.props = &port_props;
    n->port_info.params = n->params; n->port_info.n_params = SPA_N_ELEMENTS(n->params);
    struct pw_properties *props = pw_properties_new(PW_KEY_NODE_NAME, names[kind],
        PW_KEY_MEDIA_CLASS, kind == 0 ? "Audio/Sink" : "Audio/Source",
        "node.virtual", "true", "node.want-driver", "true", "node.pause-on-idle", "false",
        "node.always-process", "true", "node.loop.name", "data-loop.0", NULL);
    if (!props) return -ENOMEM;
    n->impl = pw_context_create_node(p->context, props, 0);
    if (!n->impl) return -errno;
    if (!p->rtloop) p->rtloop = n->impl->data_loop;
    /* ponytail: single shared data loop instead of a cross-loop queue protocol. */
    if (p->rtloop != n->impl->data_loop) return -ENOTSUP;
    pw_impl_node_add_listener(n->impl, &n->node_hook, &own_node_events, n);
    int res = pw_impl_node_set_implementation(n->impl, &n->spa);
    if (res < 0) return res;
    if ((res = pw_impl_node_register(n->impl, NULL)) < 0) return res;
    if (!n->port || !n->port->global || !n->port->global->registered) return -EIO;
    if (kind == 0 && !p->insecure &&
        (n->impl->global->permission_mask != 0 || n->port->global->permission_mask != 0)) return -EPERM;
    return pw_impl_node_set_active(n->impl, true);
}
static void endpoint_destroy(void *data)
{ retire(data); }
static void selected_info(void *data, const struct pw_node_info *info)
{
    struct proof *p = data;
    if (!p->creating && (info->change_mask & PW_NODE_CHANGE_MASK_PROPS)) retire(p);
}
static void selected_port_info(void *data, const struct pw_port_info *info)
{
    struct proof *p = data;
    if (!p->creating && (info->change_mask & PW_PORT_CHANGE_MASK_PROPS)) retire(p);
}
static void selected_param(void *data, uint32_t id)
{
    struct proof *p = data;
    /* Initial link negotiation installs the exact filtered format. Once the
     * link has buffers, any subsequent format mutation revokes selection. */
    bool initial_format = id == SPA_PARAM_Format && p->link &&
        p->link->info.state >= PW_LINK_STATE_INIT && p->link->info.state <= PW_LINK_STATE_ALLOCATING;
    if (!p->creating && !initial_format &&
        (id == SPA_PARAM_Format || id == SPA_PARAM_EnumFormat)) retire(p);
}
static const struct pw_impl_node_events selected_node_events = {
    PW_VERSION_IMPL_NODE_EVENTS, .destroy = endpoint_destroy, .info_changed = selected_info,
};
static const struct pw_impl_port_events selected_port_events = {
    PW_VERSION_IMPL_PORT_EVENTS, .destroy = endpoint_destroy,
    .info_changed = selected_port_info, .param_changed = selected_param,
};
static const struct pw_global_events owner_events = {
    PW_VERSION_GLOBAL_EVENTS, .destroy = endpoint_destroy,
};
static void link_initialized(void *data)
{
    struct proof *p = data;
    struct pw_global *g = pw_impl_link_get_global(p->link);
    if (!g || g->registered) { fault(p); return; }
    if (!p->insecure) g->permission_mask = 0;
}
static void link_destroyed(void *data)
{
    struct proof *p = data;
    remove_hook(&p->link_hook);
    p->link = NULL;
    retire(p);
}
static void link_unlinked(void *data, struct pw_impl_port *port)
{ atomic_store_explicit(&((struct proof *)data)->generation, 0, memory_order_release); }
static void link_state(void *data, enum pw_link_state old, enum pw_link_state state, const char *error)
{
    /* Defer destruction out of the link's own state transition stack. */
    if (state == PW_LINK_STATE_ERROR || state == PW_LINK_STATE_UNLINKED)
        atomic_store_explicit(&((struct proof *)data)->generation, 0, memory_order_release);
}
static const struct pw_impl_link_events link_events = {
    PW_VERSION_IMPL_LINK_EVENTS, .initialized = link_initialized, .destroy = link_destroyed,
    .port_unlinked = link_unlinked, .state_changed = link_state,
};
struct format_check { bool found, bad; };
static void check_format(void *data, int seq, int res, uint32_t type, const void *result)
{
    struct format_check *c = data;
    const struct spa_result_node_params *r = result;
    if (seq != 0) return;
    if (res != 0 || type != SPA_RESULT_TYPE_NODE_PARAMS || !r ||
        r->id != SPA_PARAM_EnumFormat || !fixed_format(r->param)) c->bad = true;
    else c->found = true;
}
static int check_port_format(struct pw_impl_port *port)
{
    /* 1.6.9 impl-port.c queries the node implementation for EnumFormat;
     * port->mix / SPA_ID_INVALID are only the SPA_PARAM_IO query target. */
    struct spa_node *node = pw_impl_node_get_implementation(port->node);
    struct spa_hook listener = {0};
    struct format_check c = {0};
    static const struct spa_node_events events = {
        SPA_VERSION_NODE_EVENTS, .result = check_format,
    };
    if (!node) return -ENOTSUP;
    int res = spa_node_add_listener(node, &listener, &events, &c);
    if (res < 0) return -ENOTSUP;
    if (res == 0)
        res = spa_node_port_enum_params(node, 0, port->direction, port->port_id,
            SPA_PARAM_EnumFormat, 0, 16, NULL);
    spa_hook_remove(&listener);
    /* Never await or accept an async result, even if a synchronous pod arrived. */
    return res == 0 && c.found && !c.bad ? 0 : -ENOTSUP;
}
static struct pw_global *exact(struct proof *p, uint32_t id, uint64_t serial, const char *type)
{
    struct pw_global *g = pw_context_find_global(p->context, id);
    return g && g->registered && !g->destroyed && g->serial == serial &&
        pw_global_is_type(g, type) && g->object ? g : NULL;
}
static int select_endpoint(struct proof *p, uint32_t node_id, uint64_t node_serial,
        uint32_t port_id, uint64_t port_serial, uint32_t owner_id, uint64_t owner_serial)
{
    /* This socket callback is a server mainloop entry, never a PW client method. */
    if (p->context->current_client) return -EPERM;
    retire(p);
    struct pw_global *ng = exact(p, node_id, node_serial, PW_TYPE_INTERFACE_Node);
    struct pw_global *pg = exact(p, port_id, port_serial, PW_TYPE_INTERFACE_Port);
    if (!ng || !pg) return -ESTALE;
    struct pw_impl_node *node = ng->object;
    struct pw_impl_port *port = pg->object;
    if (port->node != node || port->direction != PW_DIRECTION_OUTPUT ||
        !p->nodes[0].port || !p->nodes[1].impl || node == p->nodes[1].impl) return -EINVAL;
    const char *channel = pw_properties_get(port->properties, "audio.channel");
    /* Raw pw_stream ports omit the DSP channel property. The fixed raw format
     * below is authoritative for both channel identities; reject contradictions. */
    if (channel && strcmp(channel, "FL,FR")) return -EINVAL;
    const char *client = pw_properties_get(node->properties, PW_KEY_CLIENT_ID);
    struct pw_global *owner = NULL;
    if (!client) {
        if (owner_id != SPA_ID_INVALID || owner_serial != 0 ||
            (node != p->nodes[2].impl && node != p->nodes[3].impl)) return -EPERM;
    } else {
        uint32_t id;
        if (!spa_atou32(client, &id, 10) || id != owner_id ||
            !(owner = exact(p, owner_id, owner_serial, PW_TYPE_INTERFACE_Client))) return -EPERM;
    }
    int res = check_port_format(port);
    if (res < 0) return res;
    /* No yield from identity/format resolution through exact-pointer link creation. */
    p->creating = true;
    p->selected_node = node; p->selected_port = port;
    pw_impl_node_add_listener(node, &p->selected_node_hook, &selected_node_events, p);
    pw_impl_port_add_listener(port, &p->selected_port_hook, &selected_port_events, p);
    if (owner) pw_global_add_listener(owner, &p->owner_hook, &owner_events, p);
    uint8_t bytes[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(bytes, sizeof(bytes));
    struct spa_audio_info_raw f = format();
    struct spa_pod *filter = spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &f);
    struct pw_properties *props = pw_properties_new("link.passive", "false", "object.linger", "false", NULL);
    if (!props) { res = -ENOMEM; goto fail; }
    p->link = pw_context_create_link(p->context, port, p->nodes[0].port, filter, props, 0);
    if (!p->link) { res = -errno; goto fail; }
    pw_impl_link_add_listener(p->link, &p->link_hook, &link_events, p);
    res = pw_impl_link_register(p->link, NULL);
    if (res < 0 || !p->link || p->link->info.state == PW_LINK_STATE_ERROR ||
        p->link->info.state == PW_LINK_STATE_UNLINKED) { if (res >= 0) res = -EIO; goto fail; }
    if (!p->insecure && p->link->global->permission_mask != 0) { res = -EPERM; goto fail; }
    if (++p->next_generation == 0) { res = -EOVERFLOW; goto fail; }
    atomic_store_explicit(&p->generation, p->next_generation, memory_order_release);
    p->creating = false;
    return 0;
fail:
    p->creating = false; retire(p); return res;
}

static size_t node_status(struct node *n, char *buf, size_t size)
{
    struct pw_global *g = n->impl ? n->impl->global : NULL;
    struct pw_global *pg = n->port ? n->port->global : NULL;
    int len = snprintf(buf, size,
        "{\"node_id\":%u,\"node_serial\":%"PRIu64",\"port_id\":%u,\"port_serial\":%"PRIu64
        ",\"node_mask\":%u,\"port_mask\":%u,\"processed_frames\":%"PRIuFAST64
        ",\"position_frames\":%"PRIuFAST64"}",
        g ? g->id : SPA_ID_INVALID, g ? g->serial : 0,
        pg ? pg->id : SPA_ID_INVALID, pg ? pg->serial : 0,
        g ? g->permission_mask : 0, pg ? pg->permission_mask : 0,
        atomic_load(&n->processed), atomic_load(&n->position_frames));
    return len > 0 && (size_t)len < size ? (size_t)len : 0;
}
static void status(struct proof *p, char *reply, size_t size)
{
    if (p->link && !atomic_load(&p->generation)) retire(p);
    char nodes[4][384];
    for (unsigned i = 0; i < 4; i++) node_status(&p->nodes[i], nodes[i], sizeof(nodes[i]));
    struct pw_global *link = p->link ? p->link->global : NULL;
    snprintf(reply, size,
        "{\"status\":\"control_done\",\"proof\":\"UNVERIFIED\",\"insecure_negative_control\":%s,"
        "\"generation\":%"PRIuFAST64",\"ingress\":%s,\"output\":%s,\"fixtures\":[%s,%s],"
        "\"link_id\":%u,\"link_serial\":%"PRIu64",\"link_mask\":%u,"
        "\"captured_frames\":%"PRIuFAST64",\"emitted_frames\":%"PRIuFAST64
        ",\"format_errors\":%"PRIuFAST64",\"discarded_frames\":%"PRIuFAST64"}\n",
        p->insecure ? "true" : "false", atomic_load(&p->generation),
        nodes[0], nodes[1], nodes[2], nodes[3], link ? link->id : SPA_ID_INVALID,
        link ? link->serial : 0, link ? link->permission_mask : 0,
        atomic_load(&p->captured), atomic_load(&p->emitted), atomic_load(&p->format_errors),
        atomic_load(&p->discarded));
}
static int number(const char *s, uint64_t max, uint64_t *out)
{
    if (!s || !*s) return -EINVAL;
    for (const char *c = s; *c; c++) if (*c < '0' || *c > '9') return -EINVAL;
    char *end;
    errno = 0;
    unsigned long long v = strtoull(s, &end, 10);
    if (errno || *end || v > max) return -EINVAL;
    *out = v; return 0;
}
static int control(struct proof *p, char *request, char *reply, size_t size)
{
    if (p->context->current_client || p->stopping) return -EPERM;
    size_t len = strlen(request);
    if (len && request[len - 1] == '\n') request[--len] = 0;
    if (!strcmp(request, "STATUS")) { status(p, reply, size); return 0; }
    if (!strcmp(request, "RETIRE") || !strcmp(request, "DESTROY_LINK")) {
        if (!strcmp(request, "DESTROY_LINK") && p->link) pw_impl_link_destroy(p->link);
        else retire(p);
    } else if (!strncmp(request, "DESTROY_SOURCE ", 15)) {
        uint64_t index;
        if (number(request + 15, 1, &index) < 0 || !p->nodes[index + 2].impl) return -EINVAL;
        pw_impl_node_destroy(p->nodes[index + 2].impl);
    } else {
        char *save = NULL;
        char *token = strtok_r(request, " ", &save);
        if (!token || strcmp(token, "SELECT")) return -EINVAL;
        uint64_t values[6];
        for (unsigned i = 0; i < 6; i++) {
            token = strtok_r(NULL, " ", &save);
            if (number(token, i % 2 ? UINT64_MAX : UINT32_MAX, &values[i]) < 0) return -EINVAL;
        }
        if (strtok_r(NULL, " ", &save)) return -EINVAL;
        int res = select_endpoint(p, values[0], values[1], values[2], values[3], values[4], values[5]);
        if (res < 0) return res;
    }
    snprintf(reply, size, "{\"status\":\"control_done\",\"generation\":%"PRIuFAST64"}\n",
        atomic_load(&p->generation));
    return 0;
}
static void close_peer(struct peer *peer)
{
    if (peer->source) pw_loop_destroy_source(peer->proof->mainloop, peer->source);
    peer->source = NULL;
    if (peer->fd >= 0) close(peer->fd);
    peer->fd = -1;
}
static void peer_ready(void *data, int fd, uint32_t mask)
{
    struct peer *peer = data;
    char request[513], reply[2400];
    ssize_t len = recv(fd, request, sizeof(request) - 1, MSG_DONTWAIT | MSG_TRUNC);
    if (len < 0 && errno == EAGAIN && !(mask & (SPA_IO_ERR | SPA_IO_HUP))) return;
    int res = -EINVAL;
    if (len > 0 && len <= 512 && !memchr(request, 0, len)) {
        request[len] = 0;
        res = control(peer->proof, request, reply, sizeof(reply));
    }
    if (res < 0) snprintf(reply, sizeof(reply), "{\"status\":\"error\",\"errno\":%d}\n", -res);
    /* A reply acknowledges only this control transaction, not processing. */
    send(fd, reply, strlen(reply), MSG_DONTWAIT | MSG_NOSIGNAL);
    close_peer(peer);
}
static void accept_ready(void *data, int fd, uint32_t mask)
{
    struct proof *p = data;
    /* At most eight accepts per mainloop callback; never wait for an external peer. */
    for (unsigned attempt = 0; attempt < SPA_N_ELEMENTS(p->peers); attempt++) {
        int client = accept4(fd, NULL, NULL, SOCK_NONBLOCK | SOCK_CLOEXEC);
        if (client < 0) return;
        struct ucred cred;
        socklen_t size = sizeof(cred);
        if (getsockopt(client, SOL_SOCKET, SO_PEERCRED, &cred, &size) < 0 ||
            size != sizeof(cred) || cred.uid != geteuid()) { close(client); continue; }
        struct peer *peer = NULL;
        for (unsigned i = 0; i < SPA_N_ELEMENTS(p->peers); i++)
            if (p->peers[i].fd < 0) { peer = &p->peers[i]; break; }
        if (!peer) { close(client); continue; }
        peer->fd = client;
        peer->source = pw_loop_add_io(p->mainloop, client, SPA_IO_IN | SPA_IO_ERR | SPA_IO_HUP,
            false, peer_ready, peer);
        if (!peer->source) close_peer(peer);
    }
}
static int private_parent(const char *path)
{
    if (!path || path[0] != '/' || strlen(path) >= 4096) return -EINVAL;
    char parent[4096];
    strcpy(parent, path);
    char *slash = strrchr(parent, '/');
    if (!slash || !slash[1] || slash == parent) return -EINVAL;
    *slash = 0;
    int fd = open(parent, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return -errno;
    struct stat st;
    int res = fstat(fd, &st) < 0 ? -errno : 0;
    if (!res && (st.st_uid != geteuid() || (st.st_mode & 07777) != 0700)) res = -EPERM;
    close(fd); return res;
}
static int consent(const char *path, bool insecure)
{
    int res = private_parent(path);
    if (res < 0) return res;
    int fd = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0) return -errno;
    struct stat st;
    const char *expected = insecure ? INSECURE_CONSENT : CONSENT;
    char bytes[128];
    if (fstat(fd, &st) < 0) res = -errno;
    else if (!S_ISREG(st.st_mode) || st.st_uid != geteuid() ||
        (st.st_mode & 07777) != 0600 || st.st_nlink != 1) res = -EPERM;
    else {
        ssize_t len = read(fd, bytes, sizeof(bytes));
        if (len < 0) res = -errno;
        else if ((size_t)len != strlen(expected) || memcmp(bytes, expected, len)) res = -EPERM;
    }
    close(fd); return res;
}
static void fault_poll(void *data, uint64_t expirations)
{
    struct proof *p = data;
    if (p->link && !atomic_load_explicit(&p->generation, memory_order_acquire)) retire(p);
}
static void destroy_module(void *data)
{
    struct proof *p = data;
    p->stopping = true;
    remove_hook(&p->module_hook);
    for (unsigned i = 0; i < SPA_N_ELEMENTS(p->peers); i++) close_peer(&p->peers[i]);
    if (p->socket_source) pw_loop_destroy_source(p->mainloop, p->socket_source);
    if (p->fault_timer) pw_loop_destroy_source(p->mainloop, p->fault_timer);
    if (p->fd >= 0) close(p->fd);
    struct stat st;
    if (*p->socket_path && lstat(p->socket_path, &st) == 0 &&
        st.st_dev == p->socket_dev && st.st_ino == p->socket_ino && S_ISSOCK(st.st_mode))
        unlink(p->socket_path);
    retire(p);
    for (unsigned i = 0; i < 4; i++) {
        struct node *n = &p->nodes[i];
        if (n->impl) pw_impl_node_destroy(n->impl);
        remove_hook(&n->port_hook); remove_hook(&n->node_hook);
    }
    free(p);
}
static const struct pw_impl_module_events module_events = {
    PW_VERSION_IMPL_MODULE_EVENTS, .destroy = destroy_module,
};
SPA_EXPORT
int pipewire__module_init(struct pw_impl_module *module, const char *args)
{
    /* pw_impl_module_get_context is the daemon's context, not a client context. */
    struct pw_context *ctx = pw_impl_module_get_context(module);
    if (ctx->current_client || strcmp(pw_get_library_version(), "1.6.9")) return -EPERM;
    struct pw_properties *props = pw_properties_new_string(args ? args : "");
    if (!props) return -ENOMEM;
    bool insecure = pw_properties_get_bool(props, "negative.control", false);
    bool fixtures = pw_properties_get_bool(props, "fixture.sources", false);
    const char *path = pw_properties_get(props, "control.path");
    int res = consent(pw_properties_get(props, "consent.path"), insecure);
    if (res < 0) goto done;
    if (!path || strlen(path) >= sizeof(((struct sockaddr_un *)0)->sun_path)) { res = -EINVAL; goto done; }
    if ((res = private_parent(path)) < 0) goto done;
    struct proof *p = calloc(1, sizeof(*p));
    if (!p) { res = -ENOMEM; goto done; }
    p->fd = -1; p->context = ctx; p->module = module;
    p->insecure = insecure; p->fixtures = fixtures;
    p->mainloop = pw_context_get_main_loop(ctx);
    for (unsigned i = 0; i < SPA_N_ELEMENTS(p->peers); i++) {
        p->peers[i].proof = p; p->peers[i].fd = -1;
    }
    atomic_init(&p->generation, 0); atomic_init(&p->format_errors, 0);
    atomic_init(&p->discarded, 0); atomic_init(&p->captured, 0); atomic_init(&p->emitted, 0);
    for (unsigned i = 0; i < 4; i++) {
        p->nodes[i].proof = p;
        atomic_init(&p->nodes[i].processed, 0); atomic_init(&p->nodes[i].position_frames, 0);
    }
    if (!atomic_is_lock_free(&p->generation) || !atomic_is_lock_free(&p->format_errors)) {
        res = -ENOTSUP; goto fail;
    }
    if (insecure) pw_log_warn("MIXYARD INSECURE NEGATIVE CONTROL: ingress protection disabled; NEVER production enablement");
    for (unsigned i = 0; i < (fixtures ? 4u : 2u); i++)
        if ((res = make_node(p, i)) < 0) goto fail;
    p->fd = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (p->fd < 0) { res = -errno; goto fail; }
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    strcpy(address.sun_path, path);
    mode_t old_umask = umask(0077);
    int bound = bind(p->fd, (struct sockaddr *)&address, sizeof(address));
    umask(old_umask);
    if (bound < 0) { res = -errno; goto fail; }
    strcpy(p->socket_path, path);
    struct stat st;
    if (lstat(path, &st) < 0) { res = -errno; goto fail; }
    p->socket_dev = st.st_dev; p->socket_ino = st.st_ino;
    if (listen(p->fd, 8) < 0) { res = -errno; goto fail; }
    p->socket_source = pw_loop_add_io(p->mainloop, p->fd, SPA_IO_IN, false, accept_ready, p);
    p->fault_timer = pw_loop_add_timer(p->mainloop, fault_poll, p);
    if (!p->socket_source || !p->fault_timer) { res = -ENOMEM; goto fail; }
    struct timespec tick = { .tv_nsec = 10000000 };
    if ((res = pw_loop_update_timer(p->mainloop, p->fault_timer, &tick, &tick, false)) < 0) goto fail;
    pw_impl_module_add_listener(module, &p->module_hook, &module_events, p);
    res = 0; goto done;
fail:
    destroy_module(p);
done:
    pw_properties_free(props);
    return res;
}

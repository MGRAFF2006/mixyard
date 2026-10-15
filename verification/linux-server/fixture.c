/* SPDX-License-Identifier: MIT
 * Standalone PipeWire 1.6.9 graph-substitution proof fixture.
 *
 * capture NAME PATH             writes interleaved F32 to PATH, PATH.json trace
 * produce NAME {0.125|0.75} PATH writes the same artifacts for generated PCM
 * attack [--self-regrant] OUT_NODE OUT_PORT IN_NODE IN_PORT
 *
 * All modes remain alive until SIGINT/SIGTERM (or an asynchronous error).
 * Stream exit 0 means recorded, never acceptance. Exit 2 means BLOCKED evidence.
 * Packet offsets/counts are stereo frames, not float indices or bytes.
 * Clock positions are the graph driver's clock, not an invented source timestamp.
 */
#include <errno.h>
#include <inttypes.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>

#include <spa/buffer/meta.h>
#include <spa/param/audio/format-utils.h>
#include <spa/utils/result.h>
#include <pipewire/pipewire.h>

#define RATE 48000u
#define FRAME_CAPACITY (RATE * 120u)
#define PACKET_CAPACITY 65536u
#define MAX_PACKET_FRAMES 65536u

enum mode { CAPTURE, PRODUCE, ATTACK };
enum packet_flags {
    NO_CLOCK = 1u, NO_BUFFER = 2u, BAD_BUFFER = 4u,
    STORAGE_FULL = 8u, BAD_FORMAT = 16u, QUEUE_ERROR = 32u,
};

struct packet {
    struct spa_io_clock clock;
    struct spa_meta_header header;
    uint64_t offset;
    uint32_t frames, source_frames, flags, chunk_flags[2];
    uint32_t n_datas, chunk_size[2];
    int32_t chunk_stride[2];
    bool has_header;
};

struct fixture {
    enum mode mode;
    const char *name, *path;
    float value;
    bool regrant, failed, stopping, started, bound;
    int sync_seq;
    uint32_t node_id;
    const char *endpoints[4];
    struct pw_main_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_stream *stream;
    struct pw_proxy *link;
    struct spa_hook core_hook, client_proxy_hook, client_hook;
    struct spa_hook stream_hook, link_proxy_hook, link_hook;
    struct spa_source *signals[2];
    _Atomic(struct spa_io_position *) position;
    atomic_bool format_ok;
    struct spa_audio_info_raw format;
    unsigned format_events, format_clears, format_errors;
    float *pcm;
    struct packet *packets;
    uint64_t frames, calls, no_buffer, bad_buffer, no_clock, bad_format;
    uint64_t dropped_frames, trace_drops, storage_drops, queue_errors;
    uint32_t n_packets;
};

static void json_string(FILE *file, const char *value)
{
    const unsigned char *p = (const unsigned char *)(value ? value : "");
    fputc('"', file);
    for (; *p; p++) {
        if (*p == '"' || *p == '\\')
            fprintf(file, "\\%c", *p);
        else if (*p < 32)
            fprintf(file, "\\u%04x", *p);
        else
            fputc(*p, file);
    }
    fputc('"', file);
}

static void fail(struct fixture *f, const char *where, int result, const char *message)
{
    printf("{\"event\":\"error\",\"where\":");
    json_string(stdout, where);
    printf(",\"result\":%d,\"message\":", result);
    json_string(stdout, message);
    puts("}");
    f->failed = true;
    if (f->loop)
        pw_main_loop_quit(f->loop);
}

static void on_signal(void *data, int signal_number)
{
    struct fixture *f = data;
    printf("{\"event\":\"signal\",\"signal\":%d}\n", signal_number);
    f->stopping = true;
    pw_main_loop_quit(f->loop);
}

static void on_io(void *data, uint32_t id, void *area, uint32_t size)
{
    struct fixture *f = data;
    if (id == SPA_IO_Position)
        atomic_store(&f->position,
            area && size >= sizeof(struct spa_io_position) ? area : NULL);
}

static void on_format(void *data, uint32_t id, const struct spa_pod *param)
{
    struct fixture *f = data;
    struct spa_audio_info_raw info = { 0 };
    if (id != SPA_PARAM_Format)
        return;
    if (!param) {
        f->format_clears++;
        atomic_store(&f->format_ok, false);
        return;
    }
    f->format_events++;
    if (spa_format_audio_raw_parse(param, &info) < 0 ||
        info.format != SPA_AUDIO_FORMAT_F32P || info.rate != RATE ||
        info.channels != 2 || info.position[0] != SPA_AUDIO_CHANNEL_FL ||
        info.position[1] != SPA_AUDIO_CHANNEL_FR ||
        (info.flags & SPA_AUDIO_FLAG_UNPOSITIONED)) {
        f->format_errors++;
        atomic_store(&f->format_ok, false);
        fail(f, "negotiated-format", -EINVAL, "required F32P/48000/FL,FR");
        return;
    }
    f->format = info;
    atomic_store(&f->format_ok, true);
    puts("{\"event\":\"format\",\"format\":\"F32P\",\"rate\":48000,"
         "\"channels\":[\"FL\",\"FR\"]}");
}

/* RT owns all PCM/packet/counter writes; main reads only after stream destruction.
 * ponytail: fixed 120-second storage; overflow blocks evidence instead of wrapping.
 */
static void on_process(void *data)
{
    struct fixture *f = data;
    struct packet scratch = { 0 }, *p = &scratch;
    struct spa_io_position *position = atomic_load(&f->position);
    struct pw_buffer *b;
    struct spa_buffer *buffer;
    uint32_t count = 0, offsets[2] = { 0 }, strides[2] = { 0 };
    bool valid = true;

    f->calls++;
    if (f->n_packets < PACKET_CAPACITY)
        p = &f->packets[f->n_packets++];
    else
        f->trace_drops++;
    p->offset = f->frames;
    if (position)
        p->clock = position->clock;
    if (!position || !p->clock.duration || !p->clock.rate.num || !p->clock.rate.denom) {
        p->flags |= NO_CLOCK;
        f->no_clock++;
    }
    if (!atomic_load(&f->format_ok)) {
        p->flags |= BAD_FORMAT;
        f->bad_format++;
        valid = false;
    }
    b = pw_stream_dequeue_buffer(f->stream);
    if (!b) {
        p->flags |= NO_BUFFER;
        f->no_buffer++;
        f->dropped_frames += p->clock.duration;
        return;
    }
    buffer = b->buffer;
    p->n_datas = buffer->n_datas;
    for (unsigned c = 0; c < buffer->n_datas && c < 2; c++) {
        if (buffer->datas[c].chunk) {
            p->chunk_size[c] = buffer->datas[c].chunk->size;
            p->chunk_stride[c] = buffer->datas[c].chunk->stride;
        }
    }
    if (buffer->n_datas != 2) {
        valid = false;
    } else {
        for (unsigned c = 0; c < 2; c++) {
            const struct spa_data *plane = &buffer->datas[c];
            const struct spa_chunk *chunk = plane->chunk;
            uint32_t plane_frames;
            if (!plane->data || !chunk) {
                valid = false;
                break;
            }
            p->chunk_flags[c] = chunk->flags;
            if (f->mode == PRODUCE) {
                offsets[c] = 0;
                strides[c] = sizeof(float);
                plane_frames = plane->maxsize / sizeof(float);
                if (c == 0 || plane_frames < count)
                    count = plane_frames;
            } else {
                offsets[c] = chunk->offset;
                strides[c] = chunk->stride == 0 ? sizeof(float) : (uint32_t)chunk->stride;
                if (chunk->stride < 0 || strides[c] < sizeof(float) ||
                    offsets[c] > plane->maxsize ||
                    chunk->size > plane->maxsize - offsets[c] ||
                    chunk->size % strides[c] != 0) {
                    valid = false;
                    break;
                }
                plane_frames = chunk->size / strides[c];
                if (c == 0)
                    count = plane_frames;
                else if (plane_frames != count)
                    valid = false;
                if (chunk->flags & SPA_CHUNK_FLAG_CORRUPTED) {
                    p->flags |= BAD_BUFFER;
                    f->bad_buffer++;
                }
            }
        }
    }
    if (f->mode == PRODUCE) {
        uint64_t requested = b->requested ? b->requested : p->clock.duration;
        if (!requested || requested > count || requested > MAX_PACKET_FRAMES)
            valid = false;
        else
            count = (uint32_t)requested;
    }
    p->source_frames = count;
    if (count > MAX_PACKET_FRAMES || !count)
        valid = false;
    if (!valid) {
        p->flags |= BAD_BUFFER;
        f->bad_buffer++;
        f->dropped_frames += count ? count : p->clock.duration;
        if (f->mode == PRODUCE) {
            for (uint32_t c = 0; c < buffer->n_datas && c < 2; c++)
                if (buffer->datas[c].chunk)
                    buffer->datas[c].chunk->size = 0;
            b->size = 0;
        }
    } else {
        struct spa_meta_header *header = spa_buffer_find_meta_data(buffer,
            SPA_META_Header, sizeof(struct spa_meta_header));
        bool store = p != &scratch && count <= FRAME_CAPACITY - f->frames;
        if (header) {
            p->has_header = true;
            p->header = *header;
            if (header->flags & SPA_META_HEADER_FLAG_CORRUPTED) {
                p->flags |= BAD_BUFFER;
                f->bad_buffer++;
            }
        }
        if (!store) {
            p->flags |= STORAGE_FULL;
            f->storage_drops++;
            f->dropped_frames += count;
        }
        for (uint32_t i = 0; i < count; i++) {
            for (unsigned c = 0; c < 2; c++) {
                float sample;
                uint8_t *address = (uint8_t *)buffer->datas[c].data +
                    offsets[c] + (size_t)i * strides[c];
                if (f->mode == PRODUCE) {
                    sample = c == 0 ? f->value : -f->value;
                    memcpy(address, &sample, sizeof(sample));
                } else {
                    memcpy(&sample, address, sizeof(sample));
                }
                if (store)
                    f->pcm[(f->frames + i) * 2 + c] = sample;
            }
        }
        if (store) {
            p->frames = count;
            f->frames += count;
        }
        if (f->mode == PRODUCE) {
            for (unsigned c = 0; c < 2; c++) {
                struct spa_chunk *chunk = buffer->datas[c].chunk;
                chunk->offset = 0;
                chunk->stride = sizeof(float);
                chunk->size = count * sizeof(float);
                chunk->flags = 0;
            }
            b->size = count;
        }
    }
    if (pw_stream_queue_buffer(f->stream, b) < 0) {
        p->flags |= QUEUE_ERROR;
        f->queue_errors++;
    }
}

static void on_state(void *data, enum pw_stream_state old,
    enum pw_stream_state state, const char *error)
{
    struct fixture *f = data;
    uint32_t node_id;
    (void)old;
    printf("{\"event\":\"state\",\"state\":");
    json_string(stdout, pw_stream_state_as_string(state));
    node_id = pw_stream_get_node_id(f->stream);
    if (node_id != PW_ID_ANY)
        f->node_id = node_id;
    printf(",\"node_id\":%u}\n", node_id);
    if (state == PW_STREAM_STATE_ERROR && !f->stopping)
        fail(f, "stream", -EIO, error);
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .state_changed = on_state, .io_changed = on_io,
    .param_changed = on_format, .process = on_process,
};

static void on_proxy_error(void *data, int seq, int res, const char *message)
{
    (void)seq;
    fail(data, "proxy", res, message);
}

static void on_link_bound(void *data, uint32_t id)
{
    struct fixture *f = data;
    f->bound = true;
    printf("{\"event\":\"link_bound\",\"id\":%u}\n", id);
}

static void on_link_removed(void *data)
{
    struct fixture *f = data;
    if (!f->stopping)
        fail(f, "link-removed", -ENOENT, "server removed link proxy");
}

static void on_link_info(void *data, const struct pw_link_info *info)
{
    printf("{\"event\":\"link_state\",\"state\":");
    json_string(stdout, pw_link_state_as_string(info->state));
    printf(",\"output_node\":%u,\"output_port\":%u,\"input_node\":%u,"
           "\"input_port\":%u}\n", info->output_node_id, info->output_port_id,
           info->input_node_id, info->input_port_id);
    if (info->state == PW_LINK_STATE_ERROR)
        fail(data, "link-state", -EIO, info->error);
}

static const struct pw_proxy_events link_proxy_events = {
    PW_VERSION_PROXY_EVENTS, .bound = on_link_bound,
    .removed = on_link_removed, .error = on_proxy_error,
};
static const struct pw_link_events link_events = {
    PW_VERSION_LINK_EVENTS, .info = on_link_info,
};

static void start(struct fixture *f)
{
    struct pw_properties *props;
    int result;
    f->started = true;
    if (f->mode == ATTACK) {
        props = pw_properties_new(
            PW_KEY_LINK_OUTPUT_NODE, f->endpoints[0],
            PW_KEY_LINK_OUTPUT_PORT, f->endpoints[1],
            PW_KEY_LINK_INPUT_NODE, f->endpoints[2],
            PW_KEY_LINK_INPUT_PORT, f->endpoints[3],
            PW_KEY_OBJECT_LINGER, "false", NULL);
        if (!props) {
            fail(f, "link-properties", -ENOMEM, "allocation failed");
            return;
        }
        f->link = pw_core_create_object(f->core, "link-factory",
            PW_TYPE_INTERFACE_Link, PW_VERSION_LINK, &props->dict, 0);
        pw_properties_free(props);
        if (!f->link) {
            fail(f, "create-link", -errno, strerror(errno));
            return;
        }
        pw_proxy_add_listener(f->link, &f->link_proxy_hook, &link_proxy_events, f);
        pw_proxy_add_object_listener(f->link, &f->link_hook, &link_events, f);
        printf("{\"event\":\"link_requested\",\"port_resolution\":\"global-id-then-local-id\","
               "\"output_node\":%s,\"output_port\":%s,\"input_node\":%s,\"input_port\":%s}\n",
               f->endpoints[0], f->endpoints[1], f->endpoints[2], f->endpoints[3]);
    } else {
        uint8_t storage[1024];
        struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
        const struct spa_pod *params[1];
        struct spa_audio_info_raw info = SPA_AUDIO_INFO_RAW_INIT(
            .format = SPA_AUDIO_FORMAT_F32P, .rate = RATE, .channels = 2,
            .position = { SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR });
        props = pw_properties_new(
            PW_KEY_NODE_NAME, f->name, PW_KEY_MEDIA_NAME, f->name,
            PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_ROLE, "Music",
            PW_KEY_MEDIA_CATEGORY, f->mode == PRODUCE ? "Playback" : "Capture",
            PW_KEY_MEDIA_CLASS, f->mode == PRODUCE ? "Stream/Output/Audio" : "Stream/Input/Audio",
            PW_KEY_NODE_AUTOCONNECT, "false", PW_KEY_NODE_WANT_DRIVER, "true",
            /* No session manager in the diagnostic daemon. Keep raw stereo. */
            "adapter.auto-port-config", "{ mode = passthrough position = preserve }",
            PW_KEY_NODE_RATE, "1/48000", NULL);
        if (!props) {
            fail(f, "stream-properties", -ENOMEM, "allocation failed");
            return;
        }
        f->stream = pw_stream_new(f->core, f->name, props);
        if (!f->stream) {
            fail(f, "new-stream", -errno, strerror(errno));
            return;
        }
        pw_stream_add_listener(f->stream, &f->stream_hook, &stream_events, f);
        params[0] = spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info);
        result = pw_stream_connect(f->stream,
            f->mode == PRODUCE ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT,
            PW_ID_ANY, PW_STREAM_FLAG_NO_CONVERT | PW_STREAM_FLAG_MAP_BUFFERS |
            PW_STREAM_FLAG_RT_PROCESS, params, 1);
        if (result < 0)
            fail(f, "connect-stream", result, spa_strerror(result));
    }
}

static void on_done(void *data, uint32_t id, int seq)
{
    struct fixture *f = data;
    if (id == PW_ID_CORE && seq == f->sync_seq && !f->started && !f->failed) {
        puts("{\"event\":\"client_roundtrip_done\",\"permission_escalation_proven\":false}");
        start(f);
    }
}

static void on_core_error(void *data, uint32_t id, int seq, int res, const char *message)
{
    printf("{\"event\":\"core_error_id\",\"id\":%u,\"seq\":%d}\n", id, seq);
    fail(data, "core", res, message);
}

static const struct pw_core_events core_events = {
    PW_VERSION_CORE_EVENTS, .done = on_done, .error = on_core_error,
};

static void on_permissions(void *data, uint32_t index, uint32_t count,
    const struct pw_permission *permissions)
{
    (void)data;
    printf("{\"event\":\"permissions\",\"index\":%u,\"entries\":[", index);
    for (uint32_t i = 0; i < count; i++)
        printf("%s{\"id\":%u,\"permissions\":%u}", i ? "," : "",
            permissions[i].id, permissions[i].permissions);
    puts("]}");
}

static const struct pw_client_events client_events = {
    PW_VERSION_CLIENT_EVENTS, .permissions = on_permissions,
};

static void on_client_bound(void *data, uint32_t id)
{
    struct fixture *f = data;
    struct pw_client *client = pw_core_get_client(f->core);
    int result;
    printf("{\"event\":\"own_client_bound\",\"id\":%u}\n", id);
    if (f->regrant) {
        struct pw_permission permission = PW_PERMISSION_INIT(PW_ID_ANY, PW_PERM_ALL);
        result = pw_client_update_permissions(client, 1, &permission);
        printf("{\"event\":\"self_regrant_requested\",\"id\":%u,\"permissions\":%u,"
               "\"result\":%d,\"permission_escalation_proven\":false}\n",
            permission.id, permission.permissions, result);
        if (result < 0) {
            fail(f, "self-regrant", result, spa_strerror(result));
            return;
        }
        result = pw_client_get_permissions(client, 0, UINT32_MAX);
        if (result < 0) {
            fail(f, "get-permissions", result, spa_strerror(result));
            return;
        }
    }
    f->sync_seq = pw_core_sync(f->core, PW_ID_CORE, 0);
    if (f->sync_seq < 0)
        fail(f, "client-sync", f->sync_seq, spa_strerror(f->sync_seq));
}

static const struct pw_proxy_events client_proxy_events = {
    PW_VERSION_PROXY_EVENTS, .bound = on_client_bound, .error = on_proxy_error,
};

static void *allocate_prefaulted(size_t size)
{
    unsigned char *memory = calloc(1, size);
    if (memory) {
        volatile unsigned char *touch = memory;
        for (size_t i = 0; i < size; i += 4096)
            touch[i] = 0;
        touch[size - 1] = 0;
    }
    return memory;
}

static bool close_output(FILE *file)
{
    bool good = !ferror(file);
    if (fflush(file) != 0)
        good = false;
    if (fclose(file) != 0)
        good = false;
    return good;
}

static int write_capture(struct fixture *f)
{
    FILE *file;
    char *metadata;
    uint16_t endian = 1;
    bool blocked = f->failed || !f->frames || !f->format_events || f->format_errors ||
        f->no_buffer || f->bad_buffer || f->no_clock || f->bad_format ||
        f->trace_drops || f->storage_drops || f->queue_errors;

    file = fopen(f->path, "wb");
    if (!file) {
        fail(f, "open-pcm", -errno, strerror(errno));
        return 1;
    }
    bool written = fwrite(f->pcm, sizeof(float) * 2, f->frames, file) == f->frames;
    if (!close_output(file))
        written = false;
    if (!written) {
        fail(f, "write-pcm", -EIO, "incomplete PCM artifact");
        blocked = true;
    }
    metadata = malloc(strlen(f->path) + sizeof(".json"));
    if (!metadata) {
        fail(f, "metadata-path", -ENOMEM, "allocation failed");
        return 1;
    }
    strcpy(metadata, f->path);
    strcat(metadata, ".json");
    file = fopen(metadata, "w");
    free(metadata);
    if (!file) {
        fail(f, "open-metadata", -errno, strerror(errno));
        return 1;
    }
    fprintf(file, "{\"schema\":\"mixyard-pw-fixture-v1\",\"status\":\"%s\",\"name\":",
        blocked ? "BLOCKED" : "RECORDED");
    json_string(file, f->name);
    fprintf(file, ",\"mode\":\"%s\",\"pcm_path\":", f->mode == PRODUCE ? "produce" : "capture");
    json_string(file, f->path);
    fprintf(file, ",\"format\":\"F32%s\",\"wire_format\":\"F32P\",\"rate\":48000,"
        "\"channels\":[\"FL\",\"FR\"],\"interleaved\":true,\"offset_unit\":\"frames\","
        "\"clock_scope\":\"graph-driver\",\"node_id\":%u,\"frames\":%"PRIu64","
        "\"frame_capacity\":%u,\"packet_capacity\":%u,\"format_events\":%u,"
        "\"format_clears\":%u,\"format_errors\":%u,\"value\":%.9g,"
        "\"drops\":{\"process_calls\":%"PRIu64",\"no_buffer\":%"PRIu64","
        "\"bad_buffer\":%"PRIu64",\"no_clock\":%"PRIu64",\"bad_format\":%"PRIu64","
        "\"frames\":%"PRIu64",\"trace\":%"PRIu64",\"storage\":%"PRIu64","
        "\"queue_errors\":%"PRIu64"},\"packets\":[\n",
        *(uint8_t *)&endian ? "LE" : "BE", f->node_id, f->frames,
        FRAME_CAPACITY, PACKET_CAPACITY, f->format_events, f->format_clears,
        f->format_errors, (double)f->value, f->calls, f->no_buffer, f->bad_buffer,
        f->no_clock, f->bad_format, f->dropped_frames, f->trace_drops,
        f->storage_drops, f->queue_errors);
    for (uint32_t i = 0; i < f->n_packets; i++) {
        const struct packet *p = &f->packets[i];
        fprintf(file, "%s{\"position\":%"PRIu64",\"duration\":%"PRIu64","
            "\"offset\":%"PRIu64",\"frames\":%u,\"source_frames\":%u,"
            "\"clock_id\":%u,\"nsec\":%"PRIu64",\"clock_rate_num\":%u,"
            "\"clock_rate_denom\":%u,\"clock_flags\":%u,\"clock_cycle\":%u,"
            "\"clock_delay\":%"PRId64",\"clock_xrun\":%"PRIu64",\"flags\":%u,"
            "\"chunk_flags\":[%u,%u],\"n_datas\":%u,\"chunk_size\":[%u,%u],"
            "\"chunk_stride\":[%d,%d],\"header\":",
            i ? ",\n" : "", p->clock.position, p->clock.duration,
            p->offset, p->frames, p->source_frames, p->clock.id, p->clock.nsec,
            p->clock.rate.num, p->clock.rate.denom, p->clock.flags, p->clock.cycle,
            p->clock.delay, p->clock.xrun, p->flags, p->chunk_flags[0], p->chunk_flags[1],
            p->n_datas, p->chunk_size[0], p->chunk_size[1], p->chunk_stride[0], p->chunk_stride[1]);
        if (p->has_header)
            fprintf(file, "{\"pts\":%"PRId64",\"seq\":%"PRIu64",\"offset\":%u,\"flags\":%u}",
                p->header.pts, p->header.seq, p->header.offset, p->header.flags);
        else
            fputs("null", file);
        fputc('}', file);
    }
    fputs("\n]}\n", file);
    if (!close_output(file)) {
        fail(f, "write-metadata", -EIO, "incomplete JSON artifact");
        return 1;
    }
    printf("{\"event\":\"recording_stopped\",\"status\":\"%s\",\"frames\":%"PRIu64","
           "\"packets\":%u}\n", blocked ? "BLOCKED" : "RECORDED", f->frames, f->n_packets);
    return blocked ? 2 : 0;
}

static bool numeric_id(const char *text)
{
    char *end;
    unsigned long long value;
    if (!*text)
        return false;
    for (const char *p = text; *p; p++)
        if (*p < '0' || *p > '9')
            return false;
    errno = 0;
    value = strtoull(text, &end, 10);
    return !errno && !*end && value < UINT32_MAX;
}

int main(int argc, char **argv)
{
    struct fixture f = { .node_id = PW_ID_ANY, .sync_seq = -1 };
    struct pw_client *client;
    int result = 1;
    atomic_init(&f.position, NULL);
    atomic_init(&f.format_ok, false);
    setvbuf(stdout, NULL, _IOLBF, 0);
    if (argc == 4 && strcmp(argv[1], "capture") == 0) {
        f.mode = CAPTURE; f.name = argv[2]; f.path = argv[3];
    } else if (argc == 5 && strcmp(argv[1], "produce") == 0) {
        char *end;
        f.mode = PRODUCE; f.name = argv[2]; f.path = argv[4];
        errno = 0;
        f.value = strtof(argv[3], &end);
        if (errno || *end || (f.value != 0.125f && f.value != 0.75f))
            goto usage;
    } else if ((argc == 6 || argc == 7) && strcmp(argv[1], "attack") == 0) {
        int first = 2;
        f.mode = ATTACK;
        if (argc == 7) {
            if (strcmp(argv[2], "--self-regrant") != 0)
                goto usage;
            f.regrant = true;
            first++;
        }
        for (unsigned i = 0; i < 4; i++) {
            f.endpoints[i] = argv[first + i];
            if (!numeric_id(f.endpoints[i]))
                goto usage;
        }
    } else {
        goto usage;
    }
    if (f.mode != ATTACK && (!*f.name || !*f.path))
        goto usage;
    if (!atomic_is_lock_free(&f.position) || !atomic_is_lock_free(&f.format_ok)) {
        fputs("BLOCKED: RT atomics are not lock-free\n", stderr);
        return 2;
    }
    if (f.mode != ATTACK) {
        f.pcm = allocate_prefaulted((size_t)FRAME_CAPACITY * 2 * sizeof(float));
        f.packets = allocate_prefaulted((size_t)PACKET_CAPACITY * sizeof(struct packet));
        if (!f.pcm || !f.packets) {
            fputs("BLOCKED: capture preallocation failed\n", stderr);
            free(f.pcm); free(f.packets);
            return 2;
        }
    }
    pw_init(NULL, NULL);
    f.loop = pw_main_loop_new(NULL);
    if (!f.loop) {
        fail(&f, "main-loop", -errno, strerror(errno));
        goto cleanup;
    }
    f.signals[0] = pw_loop_add_signal(pw_main_loop_get_loop(f.loop), SIGINT, on_signal, &f);
    f.signals[1] = pw_loop_add_signal(pw_main_loop_get_loop(f.loop), SIGTERM, on_signal, &f);
    if (!f.signals[0] || !f.signals[1]) {
        fail(&f, "signal-source", -errno, strerror(errno));
        goto cleanup;
    }
    f.context = pw_context_new(pw_main_loop_get_loop(f.loop), NULL, 0);
    if (!f.context) {
        fail(&f, "context", -errno, strerror(errno));
        goto cleanup;
    }
    f.core = pw_context_connect(f.context, NULL, 0);
    if (!f.core) {
        fail(&f, "core-connect", -errno, strerror(errno));
        goto cleanup;
    }
    pw_core_add_listener(f.core, &f.core_hook, &core_events, &f);
    client = pw_core_get_client(f.core);
    if (!client) {
        fail(&f, "own-client", -ENOENT, "core did not provide client proxy");
        goto cleanup;
    }
    pw_client_add_listener(client, &f.client_hook, &client_events, &f);
    pw_proxy_add_listener((struct pw_proxy *)client, &f.client_proxy_hook, &client_proxy_events, &f);
    if (pw_proxy_get_bound_id((struct pw_proxy *)client) != PW_ID_ANY)
        on_client_bound(&f, pw_proxy_get_bound_id((struct pw_proxy *)client));
    if (!f.failed) {
        int run_result = pw_main_loop_run(f.loop);
        if (run_result < 0)
            fail(&f, "main-loop-run", run_result, spa_strerror(run_result));
    }
cleanup:
    f.stopping = true;
    /* Destruction synchronizes the data loop before main reads or frees RT data. */
    if (f.stream)
        pw_stream_destroy(f.stream);
    if (f.link)
        pw_proxy_destroy(f.link);
    if (f.core)
        pw_core_disconnect(f.core);
    if (f.context)
        pw_context_destroy(f.context);
    if (f.loop) {
        for (unsigned i = 0; i < 2; i++)
            if (f.signals[i])
                pw_loop_destroy_source(pw_main_loop_get_loop(f.loop), f.signals[i]);
        pw_main_loop_destroy(f.loop);
        f.loop = NULL;
    }
    if (f.mode != ATTACK)
        result = write_capture(&f);
    else
        result = f.failed ? 1 : f.bound ? 0 : 2;
    free(f.pcm);
    free(f.packets);
    pw_deinit();
    return result;
usage:
    fputs("usage: fixture capture NAME PATH\n"
          "       fixture produce NAME {0.125|0.75} PATH\n"
          "       fixture attack [--self-regrant] OUT_NODE OUT_PORT IN_NODE IN_PORT\n", stderr);
    return 1;
}

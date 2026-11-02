// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_CONSUMER_COPY_H
#define MIXYARD_NATIVE_CONSUMER_COPY_H
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <math.h>
#include "mixyard_host.h"
#define MX_CONSUMER_FRAMES 128
#define MX_CONSUMER_SAMPLES (2 * MX_CONSUMER_FRAMES)
#define MX_CONSUMER_SLOTS 8
// Exclusive graph DataLoop owner only. This is NOT an interthread/SPSC queue.
struct mx_consumer_metadata {
    mx_stamp stamp;
    uint64_t consumer_epoch, native_frame;
    uint32_t header_flags, header_offset;
    int64_t header_dts_offset;
    bool silent;
};
struct mx_consumer_packet {
    struct mx_consumer_metadata metadata;
    double pcm[MX_CONSUMER_SAMPLES];
};
struct mx_consumer_copy {
    struct mx_consumer_packet slots[MX_CONSUMER_SLOTS];
    uint64_t epoch, generation, clock_id, minimum_frame;
    uint64_t next_native_frame, next_engine_frame, next_sequence;
    unsigned read, count;
    bool active, started;
};
enum mx_consumer_result { MX_COPY_OK, MX_COPY_EMPTY, MX_COPY_CLOSED,
    MX_COPY_FULL, MX_COPY_STALE_BINDING, MX_COPY_INVALID, MX_COPY_EXPIRED };
static inline void mx_consumer_close(struct mx_consumer_copy *c) {
    // Preserve retired admission IDs to reject replay, never retained PCM/stamps.
    memset(c->slots, 0, sizeof(c->slots));
    c->read = c->count = 0; c->active = c->started = false;
    c->next_native_frame = c->next_engine_frame = c->next_sequence = 0;
}
static inline bool mx_consumer_open(struct mx_consumer_copy *c, uint64_t epoch,
        uint64_t generation, uint64_t clock_id, uint64_t minimum_frame) {
    if (!epoch || epoch <= c->epoch || !generation || generation < c->generation || !clock_id ||
        minimum_frame > UINT64_MAX - MX_CONSUMER_FRAMES) return false;
    mx_consumer_close(c);
    c->epoch = epoch; c->generation = generation; c->clock_id = clock_id;
    c->minimum_frame = minimum_frame; c->active = true;
    return true;
}
static inline bool mx_consumer_matches(const struct mx_consumer_copy *c,
        const struct mx_consumer_metadata *m) {
    return m->consumer_epoch == c->epoch && m->stamp.generation == c->generation &&
        m->stamp.revision == 1 && m->stamp.clock_id == c->clock_id;
}
static inline enum mx_consumer_result mx_consumer_push(struct mx_consumer_copy *c,
        uint64_t now, const struct mx_consumer_metadata *m, const double *pcm, size_t samples) {
    if (!mx_consumer_matches(c, m)) return MX_COPY_STALE_BINDING;
    if (!c->active) return MX_COPY_CLOSED;
    if (!pcm || samples != MX_CONSUMER_SAMPLES || m->native_frame < c->minimum_frame ||
        m->native_frame > UINT64_MAX - MX_CONSUMER_FRAMES ||
        m->stamp.first_frame > UINT64_MAX - MX_CONSUMER_FRAMES || m->stamp.sequence == UINT64_MAX ||
        m->stamp.captured_ns > now || m->stamp.deadline_ns < m->stamp.captured_ns ||
        m->stamp.deadline_ns - m->stamp.captured_ns > 100000000 ||
        (c->started && (m->native_frame != c->next_native_frame ||
            m->stamp.first_frame != c->next_engine_frame || m->stamp.sequence != c->next_sequence))) {
        mx_consumer_close(c); return MX_COPY_INVALID;
    }
    if (now > m->stamp.deadline_ns) { mx_consumer_close(c); return MX_COPY_EXPIRED; }
    for (size_t i = 0; i < samples; ++i) if (!isfinite(pcm[i]) || (m->silent && pcm[i] != 0.0)) {
        mx_consumer_close(c); return MX_COPY_INVALID;
    }
    if (c->count == MX_CONSUMER_SLOTS) { mx_consumer_close(c); return MX_COPY_FULL; }
    struct mx_consumer_packet *packet = &c->slots[(c->read + c->count) % MX_CONSUMER_SLOTS];
    packet->metadata = *m;
    memcpy(packet->pcm, pcm, sizeof(packet->pcm));
    ++c->count; c->started = true;
    c->next_native_frame = m->native_frame + MX_CONSUMER_FRAMES;
    c->next_engine_frame = m->stamp.first_frame + MX_CONSUMER_FRAMES;
    c->next_sequence = m->stamp.sequence + 1;
    return MX_COPY_OK;
}
static inline enum mx_consumer_result mx_consumer_front(struct mx_consumer_copy *c,
        uint64_t epoch, uint64_t generation, uint64_t clock_id, uint64_t now,
        const struct mx_consumer_packet **out) {
    *out = NULL;
    if (epoch != c->epoch || generation != c->generation || clock_id != c->clock_id)
        return MX_COPY_STALE_BINDING;
    if (!c->active) return MX_COPY_CLOSED;
    if (!c->count) return MX_COPY_EMPTY;
    const struct mx_consumer_packet *p = &c->slots[c->read];
    if (!mx_consumer_matches(c, &p->metadata)) { mx_consumer_close(c); return MX_COPY_INVALID; }
    if (now > p->metadata.stamp.deadline_ns) { mx_consumer_close(c); return MX_COPY_EXPIRED; }
    *out = p; return MX_COPY_OK;
}
static inline bool mx_consumer_pop(struct mx_consumer_copy *c, uint64_t epoch) {
    if (epoch != c->epoch || !c->active || !c->count) return false;
    memset(&c->slots[c->read], 0, sizeof(c->slots[c->read]));
    c->read = (c->read + 1) % MX_CONSUMER_SLOTS; --c->count;
    return true;
}
#endif

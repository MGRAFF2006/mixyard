// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_DIAGNOSTICS_H
#define MIXYARD_NATIVE_DIAGNOSTICS_H
#include <stdbool.h>
#include <stdio.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdatomic.h>
#include "buffer_diagnostics.h"
struct mx_native_observation {
    unsigned error, role, port, direction, phase, command;
    uint32_t frames, n_datas, data_type, data_flags, maxsize, mapoffset;
    bool chunk_present, header_present, owned_copy;
    uint32_t offset, size, flags;
    int32_t stride;
    uint64_t buffer_frames;
    uint32_t clock_id, rate_num, rate_denom;
    uint64_t clock_frame, clock_ns, clock_duration;
    uint64_t registration_id;
    struct mx_buffer_layout layout;
    uint64_t callback_monotonic_ns; // Separate OS CLOCK_MONOTONIC, not sample clock_ns.
    uint32_t header_flags;
    uint64_t header_seq;
    int64_t header_pts;
    uint32_t sample_index;
    uint64_t actual_bits, expected_bits;
};
struct mx_native_first_failure {
    _Atomic unsigned state; // 0 empty, 1 exclusive writer, 2 published
    struct mx_native_observation value;
};
static inline void mx_native_record_first(struct mx_native_first_failure *failure,
                                         const struct mx_native_observation *value) {
    unsigned empty = 0;
    if (!atomic_compare_exchange_strong_explicit(&failure->state, &empty, 1,
            memory_order_acquire, memory_order_relaxed)) return;
    failure->value = *value;
    atomic_store_explicit(&failure->state, 2, memory_order_release);
}
// Control only, after native teardown; never called by the process callback.
static inline void mx_native_print_observation(const struct mx_native_observation *r) {
    printf("{\"registrationId\":%" PRIu64 ",\"layout\":", r->registration_id);
    mx_print_buffer_layout(&r->layout);
    printf(",\"error\":%u,\"ownedCopy\":%s,\"role\":%u,\"port\":%u,\"direction\":%u,\"phase\":%u,\"command\":%u,\"frames\":%u,\"nDatas\":%u,\"dataType\":%u,\"dataFlags\":%u,\"maxsize\":%u,\"mapoffset\":%u,\"bufferFrames\":%" PRIu64
        ",\"chunkPresent\":%s,\"offset\":%u,\"size\":%u,\"stride\":%" PRId32 ",\"flags\":%u,\"clockId\":%u,\"clockFrame\":%" PRIu64 ",\"clockNs\":%" PRIu64 ",\"clockDuration\":%" PRIu64
        ",\"callbackMonotonicNs\":%" PRIu64 ",\"rateNum\":%u,\"rateDenom\":%u,\"headerPresent\":%s,\"headerFlags\":%u,\"headerSeq\":%" PRIu64 ",\"headerPts\":%" PRId64 ",\"sampleIndex\":%u,\"actualBits\":%" PRIu64 ",\"expectedBits\":%" PRIu64 "}",
        r->error, r->owned_copy ? "true" : "false", r->role, r->port, r->direction, r->phase, r->command, r->frames,
        r->n_datas, r->data_type, r->data_flags, r->maxsize, r->mapoffset, r->buffer_frames,
        r->chunk_present ? "true" : "false", r->offset, r->size, r->stride, r->flags,
        r->clock_id, r->clock_frame, r->clock_ns, r->clock_duration, r->callback_monotonic_ns, r->rate_num, r->rate_denom,
        r->header_present ? "true" : "false", r->header_flags, r->header_seq, r->header_pts,
        r->sample_index, r->actual_bits, r->expected_bits);
}
static inline void mx_native_print_first_failure(const char *name, const struct mx_native_first_failure *failure) {
    printf(",\"%s\":", name);
    if (atomic_load_explicit(&failure->state, memory_order_acquire) != 2) { printf("null"); return; }
    mx_native_print_observation(&failure->value);
}
#endif

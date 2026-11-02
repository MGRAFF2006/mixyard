// SPDX-License-Identifier: MIT
#ifndef MIXYARD_PIPEWIRE_TIMELINE_H
#define MIXYARD_PIPEWIRE_TIMELINE_H
#include <stdbool.h>
#include <stdint.h>
#include "mixyard_host.h"

// One evidenced native graph, explicitly rebased to this host's frame zero.
// Nanoseconds stay in the actual native timeline; no wall-clock offset is guessed.
typedef struct {
    uint64_t clock_id, epoch_frame, next_native_frame, sequence, next_frame;
    bool started;
} mx_graph_timeline;

static inline bool mx_graph_stamp(mx_graph_timeline *t, uint64_t clock_id,
        uint64_t native_frame, uint64_t captured_ns, uint64_t frames,
        uint64_t generation, mx_stamp *out) {
    if (clock_id != t->clock_id || frames == 0 || native_frame > UINT64_MAX - frames ||
        captured_ns > UINT64_MAX - 100000000 || t->sequence == UINT64_MAX ||
        t->next_frame > UINT64_MAX - frames ||
        (t->started && native_frame != t->next_native_frame)) return false;
    if (!t->started) { t->epoch_frame = native_frame; t->started = true; }
    *out = (mx_stamp){ .generation = generation, .revision = 1, .clock_id = clock_id,
        .sequence = t->sequence++, .first_frame = t->next_frame,
        .captured_ns = captured_ns, .deadline_ns = captured_ns + 100000000 };
    t->next_frame += frames;
    t->next_native_frame = native_frame + frames;
    return true;
}
#endif

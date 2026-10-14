// SPDX-License-Identifier: MIT
// Original delivery cursor used by the miniport. Caller serializes with Transport.
#pragma once
#include "../transport.hpp"
namespace mixyard_input {
struct Delivery {
    uint64_t generation = 0;
    uint64_t sequence = 0;
    uint32_t offset = 0;
    uint64_t lost_packets = 0;
    uint64_t copied_frames = 0;
    uint64_t silent_frames = 0;
    uint64_t revision = 0;
    uint64_t producer_frame = 0;

    void frame(const Transport& t, uint32_t* stereo) {
        stereo[0] = stereo[1] = 0;
        if (generation != t.generation) {
            generation = t.generation;
            sequence = 0;
            offset = 0;
            revision = producer_frame = 0;
        }
        if (!t.owner) { ++silent_frames; return; }
        const auto oldest = t.next_sequence > retained_packets ? t.next_sequence - retained_packets : 0;
        if (sequence < oldest) {
            lost_packets += oldest - sequence;
            sequence = oldest;
            offset = 0;
        }
        if (sequence >= t.next_sequence) { ++silent_frames; return; }
        const auto& p = t.packets[sequence % retained_packets];
        if (p.generation != generation || p.epoch != t.epoch || offset >= p.frames) {
            ++silent_frames;
            return;
        }
        stereo[0] = p.samples[offset * 2];
        stereo[1] = p.samples[offset * 2 + 1];
        revision = p.revision;
        producer_frame = p.first_frame + offset;
        ++copied_frames;
        if (++offset == p.frames) { ++sequence; offset = 0; }
    }
};
// Integer-only QPC conversion; no multiply of unbounded elapsed ticks.
inline uint64_t clock_frames(uint64_t ticks, uint64_t frequency) {
    return (ticks / frequency) * 48000 + ((ticks % frequency) * 48000) / frequency;
}
} // namespace mixyard_input

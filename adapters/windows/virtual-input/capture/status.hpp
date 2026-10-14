// SPDX-License-Identifier: MIT
#pragma once
#include "delivery.hpp"
namespace mixyard_input {
struct CaptureStatus {
    uint32_t version;
    uint32_t bytes;
    uint64_t generation;
    uint64_t producer_epoch;
    uint64_t next_sequence;
    uint64_t next_frame;
    uint64_t overwritten_packets;
    uint64_t stream_epoch;
    uint64_t endpoint_frames; // Committed cyclic-buffer frames including silence.
    uint64_t copied_frames;
    uint64_t silent_frames;
    uint64_t lost_packets;
    uint64_t missed_clock_frames;
    uint64_t revision; // Zero means no last copy for the current producer.
    uint64_t producer_frame; // Last copied source frame; NOT consumer acknowledgment.
    uint64_t endpoint_frame; // Corresponding last copied cyclic-buffer frame.
    uint64_t qpc; // QPC at the last copy, not at a later silence-only callback.
    uint64_t qpc_frequency;
    uint32_t running;
    uint32_t reserved;
};
static_assert(sizeof(CaptureStatus) == 144, "capture diagnostic ABI");
inline void record_capture_copy(CaptureStatus& last, const Transport& t, const Delivery& reader,
                                uint64_t endpoint_frame, uint64_t qpc) {
    last.generation = t.generation; last.producer_epoch = t.epoch;
    last.revision = reader.revision; last.producer_frame = reader.producer_frame;
    last.endpoint_frame = endpoint_frame; last.qpc = qpc;
}
// Caller serializes delivery and transport transitions. The stored tuple retains
// its copied identity; a status read must not relabel it after cleanup/admission.
inline CaptureStatus capture_status_snapshot(const CaptureStatus& last, const Transport& t) {
    auto snapshot = last;
    if (!t.owner || last.generation != t.generation || last.producer_epoch != t.epoch)
        snapshot.revision = snapshot.producer_frame = snapshot.endpoint_frame = snapshot.qpc = 0;
    snapshot.version = 1; snapshot.bytes = sizeof(snapshot);
    snapshot.generation = t.generation; snapshot.producer_epoch = t.epoch;
    snapshot.next_sequence = t.next_sequence; snapshot.next_frame = t.next_frame;
    snapshot.overwritten_packets = t.overwritten_packets;
    return snapshot;
}
} // namespace mixyard_input

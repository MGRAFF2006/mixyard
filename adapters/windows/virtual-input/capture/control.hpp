// SPDX-License-Identifier: MIT
#pragma once
#include "../driver/ioctl.hpp"
namespace mixyard_input {
constexpr uint32_t ioctl_capture_status = CTL_CODE(device_type, 0x802, METHOD_BUFFERED,
    FILE_READ_ACCESS | FILE_WRITE_ACCESS);
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
    uint64_t revision;
    uint64_t producer_frame; // Last copied source frame; NOT consumer acknowledgment.
    uint64_t endpoint_frame; // Corresponding last copied cyclic-buffer frame.
    uint64_t qpc;
    uint64_t qpc_frequency;
    uint32_t running;
    uint32_t reserved;
};
static_assert(sizeof(CaptureStatus) == 144, "capture diagnostic ABI");
}

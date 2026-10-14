// SPDX-License-Identifier: MIT
#pragma once
#include <stdint.h>

// Include ntddk.h (kernel) or Windows.h + winioctl.h (user mode) first.
namespace mixyard_input {
constexpr uint32_t device_type = 0x8000;
constexpr uint32_t ioctl_submit = CTL_CODE(device_type, 0x800, METHOD_BUFFERED,
                                          FILE_READ_ACCESS | FILE_WRITE_ACCESS);
constexpr uint32_t ioctl_status = CTL_CODE(device_type, 0x801, METHOD_BUFFERED,
                                          FILE_READ_ACCESS | FILE_WRITE_ACCESS);
constexpr uint32_t control_version = 1;

struct SubmitReply {
    uint32_t version;
    uint32_t bytes;
    uint32_t result; // mixyard_input::Result accepted / accepted_overwrite ONLY.
    uint32_t reserved;
};

// Queue diagnostics only. No producer identity/pointer or queued PCM is returned.
struct IngressStatus {
    uint32_t version;
    uint32_t bytes;
    uint64_t generation;
    uint64_t epoch;
    uint64_t next_frame;
    uint64_t next_sequence;
    uint64_t revision;
    uint64_t overwritten_packets;
    uint64_t delivered_frames; // Always zero: no WaveRT delivery exists.
    uint32_t endpoint_published; // Always zero: no PortCls registration exists.
    uint32_t reserved;
};
static_assert(sizeof(SubmitReply) == 16, "Submit reply ABI");
static_assert(sizeof(IngressStatus) == 72, "Status ABI");
} // namespace mixyard_input

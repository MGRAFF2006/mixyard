// SPDX-License-Identifier: MIT
#pragma once
#include "harness.hpp"
#include "pcm.hpp"
#include <memory>
#include <new>

namespace mixyard_producer {
// Separate non-realtime IOCTL worker boundary. It does not map the driver's
// global control interface to a PnP/MMDevice endpoint, provision/install a driver,
// pace its hardware clock, or acknowledge consumer delivery.
class StreamProducer {
    DeviceOps ops_;
    HANDLE file_ = INVALID_HANDLE_VALUE;
    std::unique_ptr<PcmEncoder> encoder_;
    bool used_ = false;
    bool status(CaptureStatus& s) {
        DWORD bytes = 0;
        return ops_.ioctl(file_, ioctl_capture_status, nullptr, 0, &s, sizeof(s), &bytes, nullptr) &&
            valid_status(s, bytes);
    }
public:
    explicit StreamProducer(DeviceOps ops) : ops_(ops) {}
    ~StreamProducer() { close(); }
    StreamProducer(const StreamProducer&) = delete;
    StreamProducer& operator=(const StreamProducer&) = delete;
    bool close() {
        if (encoder_) encoder_->revoke();
        if (file_ && file_ != INVALID_HANDLE_VALUE) {
            const HANDLE old = file_; file_ = INVALID_HANDLE_VALUE; return ops_.close(old) != FALSE;
        }
        return true;
    }
    // Explicit call by a separately authorized host only, never inventory.
    bool open(myw_format format, myw_binding binding, uint64_t clock_id) {
        if (used_) return false;
        used_ = true;
        if (!ops_.open || !ops_.ioctl || !ops_.close || !ingress_format(format) ||
            !binding.generation || !binding.revision || !clock_id) return false;
        file_ = ops_.open();
        if (!file_ || file_ == INVALID_HANDLE_VALUE) { file_ = INVALID_HANDLE_VALUE; return false; }
        CaptureStatus s{};
        if (!status(s) || s.next_sequence || s.next_frame || s.overwritten_packets || s.revision) { close(); return false; }
        encoder_.reset(new (std::nothrow) PcmEncoder(format, binding, clock_id, s.generation, s.producer_epoch));
        if (!encoder_ || !encoder_->active()) { close(); return false; }
        return true;
    }
    bool submit(myw_format format, const myw_block& block, const void* bytes, size_t length,
                 uint32_t offset, uint32_t frames) {
        if (!encoder_ || !encoder_->active() || !file_ || file_ == INVALID_HANDLE_VALUE) return false;
        CaptureStatus s{};
        const auto& previous = encoder_->state();
        if (!status(s) || s.generation != previous.generation || s.producer_epoch != previous.epoch ||
            s.next_sequence != previous.next_sequence || s.next_frame != previous.next_frame ||
            s.overwritten_packets != previous.overwritten_packets) { close(); return false; }
        PcmEncoder::Packet packet{};
        size_t size = 0;
        if (!encoder_->encode(format, block, bytes, length, offset, frames, packet, size)) { close(); return false; }
        const auto expected = encoder_->state().overwritten_packets ? Result::accepted_overwrite : Result::accepted;
        SubmitReply reply{};
        DWORD returned = 0;
        if (!ops_.ioctl(file_, ioctl_submit, packet.data(), static_cast<DWORD>(size), &reply, sizeof(reply),
                         &returned, nullptr) || !valid_reply(reply, returned, expected)) { close(); return false; }
        return true;
    }
};
}

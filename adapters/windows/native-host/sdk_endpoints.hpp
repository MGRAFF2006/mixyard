// SPDX-License-Identifier: MIT
#pragma once
// Windows SDK native servicing seams. SDK tests inject COM endpoints; production
// callers own already-authorized/initialized interfaces on this same thread.
// No discovery, activation, default selection or clock inference occurs here.
#ifdef _WIN32
#include "../worker_render.hpp"
#include "../virtual-input/producer/stream.hpp"
#include <audioclient.h>
#include <algorithm>

namespace mixyard_windows_host {
class CaptureEndpoint {
    myw_port* port_;
    IAudioCaptureClient* client_; // Borrowed on owning COM thread.
    myw_format format_{};
    myw_binding binding_{};
    uint64_t clock_ = 0, sequence_ = 0, first_frame_ = 0;
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> silence_{};
public:
    CaptureEndpoint(myw_port* port, IAudioCaptureClient* client) : port_(port), client_(client) {
        if (!client_ || myw_port_info(port_, &format_, &binding_, &clock_) != MYW_OK) myw_port_revoke(port_);
    }
    int service() {
        int result = myw_port_state(port_);
        if (result != MYW_OK) return result;
        UINT32 available = 0;
        HRESULT hr = client_->GetNextPacketSize(&available);
        if (FAILED(hr)) { myw_port_revoke(port_); return MYW_NATIVE; }
        if (!available) return MYW_EMPTY;
        BYTE* data = nullptr; UINT32 frames = 0; DWORD flags = 0; UINT64 device = 0, qpc = 0;
        hr = client_->GetBuffer(&data, &frames, &flags, &device, &qpc);
        if (hr == AUDCLNT_S_BUFFER_EMPTY) return MYW_EMPTY;
        if (FAILED(hr)) { myw_port_revoke(port_); return MYW_NATIVE; }
        if (!frames || frames > MYW_MAX_FRAMES * MYW_SLOTS ||
            (flags & ~AUDCLNT_BUFFERFLAGS_SILENT) || (!data && !(flags & AUDCLNT_BUFFERFLAGS_SILENT))) result = MYW_CAPTURE_FAULT;
        const size_t width = myw_frame_bytes(format_);
        for (uint32_t offset = 0; result == MYW_OK && offset < frames;) {
            const auto count = std::min<uint32_t>(frames - offset, MYW_MAX_FRAMES);
            myw_stamp stamp{binding_, clock_, sequence_, first_frame_, device, qpc, offset, flags};
            result = myw_port_push(port_, format_, stamp, data ? data + offset * width : silence_.data(), count, count * width);
            if (result == MYW_OK) { ++sequence_; first_frame_ += count; }
            offset += count;
        }
        if (FAILED(client_->ReleaseBuffer(frames))) result = MYW_NATIVE;
        if (result != MYW_OK) myw_port_revoke(port_);
        return result;
    }
};

class RenderEndpoint {
    myw_port* port_;
    IAudioRenderClient* client_;
    myw_format format_{};
    myw_binding binding_{};
    uint64_t clock_ = 0;
    mixyard_worker::RenderCursor cursor_{};
public:
    uint64_t underflow_frames = 0;
    RenderEndpoint(myw_port* port, IAudioRenderClient* client) : port_(port), client_(client) {
        if (!client_ || myw_port_info(port_, &format_, &binding_, &clock_) != MYW_OK) myw_port_revoke(port_);
    }
    int service(uint32_t frames) {
        if (!frames || frames > MYW_MAX_FRAMES) { myw_port_revoke(port_); return MYW_INVALID; }
        const auto state = myw_port_state(port_);
        if (state != MYW_OK) return state;
        BYTE* bytes = nullptr;
        if (FAILED(client_->GetBuffer(frames, &bytes))) { myw_port_revoke(port_); return MYW_NATIVE; }
        int result = bytes ? cursor_.fill(port_, binding_, myw_frame_bytes(format_), bytes, frames, underflow_frames) : MYW_NATIVE;
        if (result == MYW_OK) result = myw_port_state(port_);
        const auto hr = client_->ReleaseBuffer(frames, result == MYW_OK ? 0 : AUDCLNT_BUFFERFLAGS_SILENT);
        if (FAILED(hr)) result = MYW_NATIVE;
        if (result != MYW_OK) myw_port_revoke(port_);
        return result;
    }
};

// A separate consumer thread may attach a host output to the actual existing
// producer/IOCTL boundary. It must NOT also attach a render consumer to this port.
// The global driver ingress remains unmapped: this is never a selectable mic ID.
class IngressConsumer {
    myw_port* port_;
    myw_format format_{};
    myw_binding binding_{};
    uint64_t clock_ = 0;
    mixyard_producer::StreamProducer producer_;
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> bytes_{};
public:
    IngressConsumer(myw_port* port, mixyard_producer::DeviceOps ops) : port_(port), producer_(ops) {}
    bool open() { // Explicit authorized invocation, outside ANY PCM servicing.
        return myw_port_info(port_, &format_, &binding_, &clock_) == MYW_OK &&
            myw_port_state(port_) == MYW_OK && producer_.open(format_, binding_, clock_);
    }
    bool close() { myw_port_revoke(port_); return producer_.close(); }
    int service() { // Dedicated IOCTL worker: synchronous calls may block.
        myw_block block{};
        const auto result = myw_port_pop(port_, binding_, bytes_.data(), bytes_.size(), &block);
        if (result == MYW_EMPTY) return result;
        if (result != MYW_OK) { (void)close(); return result; }
        for (uint32_t offset = 0; offset < block.frames;) {
            const auto count = std::min<uint32_t>(block.frames - offset, mixyard_input::max_frames);
            if (myw_port_state(port_) != MYW_OK || !producer_.submit(format_, block, bytes_.data(),
                  block.frames * myw_frame_bytes(format_), offset, count)) { (void)close(); return MYW_NATIVE; }
            offset += count;
        }
        return MYW_OK;
    }
};
} // namespace mixyard_windows_host
#endif

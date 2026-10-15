// SPDX-License-Identifier: MIT
// Original disposable harness. Native operations are supplied only after CLI opt-in.
#pragma once
#include <windows.h>
#include <swdevice.h>
#include "../capture/control.hpp"
#include <array>
#include <chrono>
#include <condition_variable>
#include <cwchar>
#include <mutex>
#include <stdexcept>
#include <string>

namespace mixyard_producer {
using namespace mixyard_input;
inline constexpr wchar_t capture_path[] = L"\\\\.\\Global\\MixyardCaptureIngress";
inline constexpr wchar_t hardware_ids[] = L"SWD\\MixyardCapture\0"; // MULTI_SZ

inline bool valid_uuid(const std::wstring& id) {
    if (id.size() != 36) return false;
    bool nonzero = false;
    for (size_t i = 0; i < id.size(); ++i) {
        if (i == 8 || i == 13 || i == 18 || i == 23) {
            if (id[i] != L'-') return false;
        } else {
            if (!((id[i] >= L'0' && id[i] <= L'9') || (id[i] >= L'a' && id[i] <= L'f'))) return false;
            nonzero |= id[i] != L'0';
        }
    }
    return nonzero;
}

struct SoftwareOps {
    decltype(&SwDeviceCreate) create;
    decltype(&SwDeviceClose) close;
};

class Provision final {
    SoftwareOps ops_;
    HSWDEVICE handle_ = nullptr;
    bool used_ = false;
    std::mutex mutex_;
    std::condition_variable changed_;
    bool complete_ = false;
    HRESULT result_ = E_PENDING;
    HSWDEVICE callback_handle_ = nullptr;
    std::array<wchar_t, 1024> instance_{};

    static void CALLBACK completed(HSWDEVICE handle, HRESULT result, PVOID context, PCWSTR instance) {
        auto& self = *static_cast<Provision*>(context);
        std::lock_guard<std::mutex> lock(self.mutex_);
        self.callback_handle_ = handle;
        self.result_ = result;
        if (result == S_OK) {
            size_t n = 0;
            if (instance) while (n < self.instance_.size() && instance[n]) ++n;
            if (!instance || !n || n == self.instance_.size()) self.result_ = E_INVALIDARG;
            else std::wmemcpy(self.instance_.data(), instance, n + 1);
        }
        self.complete_ = true;
        self.changed_.notify_one();
    }
public:
    explicit Provision(SoftwareOps ops) : ops_(ops) {}
    ~Provision() { close(); } // Context remains alive until SwDeviceClose drains callbacks.
    Provision(const Provision&) = delete;
    Provision& operator=(const Provision&) = delete;
    HRESULT start(const std::wstring& uuid, const std::wstring& parent) {
        if (used_ || !valid_uuid(uuid) || parent.empty() || parent.size() >= 1024) return E_INVALIDARG;
        used_ = true;
        SW_DEVICE_CREATE_INFO info{};
        info.cbSize = sizeof(info);
        info.pszInstanceId = uuid.c_str();
        info.pszzHardwareIds = hardware_ids;
        info.CapabilityFlags = SWDeviceCapabilitiesRemovable | SWDeviceCapabilitiesDriverRequired;
        info.pszDeviceDescription = L"Mixyard disposable capture harness";
        const HRESULT hr = ops_.create(L"MixyardCapture", parent.c_str(), &info, 0, nullptr,
                                      completed, this, &handle_);
        if (hr != S_OK || !handle_) {
            close();
            return hr == S_OK ? E_UNEXPECTED : hr;
        }
        return S_OK; // Initiated only. Callback may already have completed.
    }
    HRESULT wait(unsigned milliseconds, std::wstring& actual_instance) {
        actual_instance.clear();
        if (!handle_ || !milliseconds || milliseconds > 30000) return E_INVALIDARG;
        std::unique_lock<std::mutex> lock(mutex_);
        if (!changed_.wait_for(lock, std::chrono::milliseconds(milliseconds), [this] { return complete_; }))
            return HRESULT_FROM_WIN32(WAIT_TIMEOUT);
        if (result_ != S_OK) return result_;
        if (callback_handle_ != handle_) return E_UNEXPECTED;
        actual_instance = instance_.data(); // Never construct a PnP or MMDevice ID.
        return S_OK;
    }
    void close() {
        if (handle_) {
            const auto handle = handle_;
            handle_ = nullptr;
            // Never hold mutex_ here: close may wait for completed() on another thread.
            ops_.close(handle);
        }
    }
};

inline void require(bool yes, const char* message) {
    if (!yes) throw std::runtime_error(message);
}
inline void put(uint8_t* p, uint64_t value, size_t bytes) {
    for (size_t i = 0; i < bytes; ++i) p[i] = static_cast<uint8_t>(value >> (i * 8));
}
using Packet = std::array<uint8_t, max_packet_bytes>;
inline Packet fixture_packet(const Transport& state, uint32_t frames, uint64_t revision) {
    require(frames && frames <= max_frames && revision && revision >= state.revision,
            "invalid fixture frames/revision");
    Packet p{};
    put(p.data(), magic, 4); put(p.data() + 4, version, 2); put(p.data() + 6, header_bytes, 2);
    put(p.data() + 8, header_bytes + frames * 8, 4); put(p.data() + 12, frames, 4);
    put(p.data() + 16, state.generation, 8); put(p.data() + 24, state.epoch, 8);
    put(p.data() + 32, revision, 8); put(p.data() + 40, state.next_frame, 8);
    put(p.data() + 48, 48000, 4); put(p.data() + 52, 2, 2); put(p.data() + 54, 1, 2);
    put(p.data() + 56, state.next_sequence, 8);
    for (size_t i = 0; i < frames; ++i) {
        put(p.data() + header_bytes + i * 8, 0x3e800000, 4); // L +0.25
        put(p.data() + header_bytes + i * 8 + 4, 0xbe000000, 4); // R -0.125
    }
    return p;
}

inline bool valid_status(const CaptureStatus& s, DWORD bytes) {
    return bytes == sizeof(s) && s.version == control_version && s.bytes == sizeof(s) &&
        !s.reserved && s.generation && s.producer_epoch && s.running <= 1 &&
        (!s.running || (s.stream_epoch && s.qpc_frequency)) &&
        (s.revision || (!s.producer_frame && !s.endpoint_frame && !s.qpc));
}
inline bool valid_reply(const SubmitReply& reply, DWORD bytes, Result expected) {
    return bytes == sizeof(reply) && reply.version == control_version && reply.bytes == sizeof(reply) &&
        !reply.reserved && reply.result == static_cast<uint32_t>(expected);
}

struct DeviceOps {
    HANDLE (*open)();
    BOOL (*ioctl)(HANDLE, DWORD, LPVOID, DWORD, LPVOID, DWORD, LPDWORD, LPOVERLAPPED);
    BOOL (*close)(HANDLE);
};
struct Enqueued { uint64_t generation, epoch, packets, frames; };
inline Enqueued produce(DeviceOps ops, uint64_t revision, unsigned packets) {
    require(revision && packets && packets <= retained_packets, "fixture requires revision>0 and 1..8 packets");
    struct File {
        DeviceOps ops;
        HANDLE handle;
        ~File() { if (handle != INVALID_HANDLE_VALUE && handle) (void)ops.close(handle); }
    } file{ops, ops.open()};
    require(file.handle && file.handle != INVALID_HANDLE_VALUE, "private capture open failed");
    CaptureStatus s{};
    DWORD bytes = 0;
    require(ops.ioctl(file.handle, ioctl_capture_status, nullptr, 0, &s, sizeof(s), &bytes, nullptr) != FALSE,
            "capture status IOCTL failed");
    require(valid_status(s, bytes), "malformed capture status");
    require(!s.next_sequence && !s.next_frame && !s.overwritten_packets && !s.revision,
            "admission not fresh; historical replay refused");
    Transport state{};
    state.initialize();
    require(state.begin(1, s.producer_epoch) == Result::accepted, "invalid producer epoch");
    state.generation = s.generation;
    for (unsigned i = 0; i < packets; ++i) {
        auto packet = fixture_packet(state, static_cast<uint32_t>(max_frames), revision);
        const auto expected = state.submit(1, packet.data(), packet.size()); // Same reviewed ABI validator.
        require(expected == Result::accepted, "fixture violates current transport");
        SubmitReply reply{};
        bytes = 0;
        require(ops.ioctl(file.handle, ioctl_submit, packet.data(), static_cast<DWORD>(packet.size()),
                          &reply, sizeof(reply), &bytes, nullptr) != FALSE, "submit IOCTL failed; no retry");
        require(valid_reply(reply, bytes, expected), "malformed submit reply; no retry");
    }
    // Closing revokes admission and clears queued/cyclic storage. No delivery claim.
    const auto closed = ops.close(file.handle);
    file.handle = INVALID_HANDLE_VALUE;
    require(closed != FALSE, "producer CloseHandle failed");
    return {s.generation, s.producer_epoch, packets, state.next_frame};
}
} // namespace mixyard_producer

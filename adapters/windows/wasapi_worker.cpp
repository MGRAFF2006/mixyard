// SPDX-License-Identifier: MIT
#include "worker.h"
#include "worker_render.hpp"
#ifdef _WIN32
#include <windows.h>
#include <audioclient.h>
#include <mmdeviceapi.h>
#include <mmreg.h>
#include <ks.h>
#include <ksmedia.h>
#include <avrt.h>
#include <wrl.h>
#include <algorithm>
#include <array>
#include <chrono>
#include <cstring>
#include <string>

namespace {
using Microsoft::WRL::ComPtr;
struct Apartment {
    HRESULT result = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    ~Apartment() { if (SUCCEEDED(result)) CoUninitialize(); }
};
struct Endpoint {
    ComPtr<IMMDeviceEnumerator> enumerator;
    ComPtr<IMMDevice> device;
    ComPtr<IAudioClient> client;
    WAVEFORMATEX* wave = nullptr;
    ~Endpoint() { CoTaskMemFree(wave); }
};
struct Event {
    HANDLE handle = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    ~Event() { if (handle) CloseHandle(handle); }
};
struct Scheduling {
    DWORD index = 0;
    HANDLE handle = AvSetMmThreadCharacteristicsW(L"Audio", &index);
    ~Scheduling() { if (handle) AvRevertMmThreadCharacteristics(handle); }
};
bool valid_endpoint(const uint16_t* id, size_t units, int direction) {
    if (!id || !units || units > 4096 || (direction != MYW_CAPTURE && direction != MYW_RENDER)) return false;
    for (size_t i = 0; i < units; ++i) if (!id[i]) return false;
    return true;
}
HRESULT describe(const WAVEFORMATEX& w, myw_format& f) {
    f = {};
    f.rate = w.nSamplesPerSec; f.channels = w.nChannels;
    f.container_bits = f.valid_bits = w.wBitsPerSample;
    bool pcm = w.wFormatTag == WAVE_FORMAT_PCM;
    bool floating = w.wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
    if (w.wFormatTag == WAVE_FORMAT_EXTENSIBLE) {
        if (w.cbSize < 22) return AUDCLNT_E_UNSUPPORTED_FORMAT;
        const auto& e = reinterpret_cast<const WAVEFORMATEXTENSIBLE&>(w);
        pcm = IsEqualGUID(e.SubFormat, KSDATAFORMAT_SUBTYPE_PCM) != FALSE;
        floating = IsEqualGUID(e.SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) != FALSE;
        f.valid_bits = e.Samples.wValidBitsPerSample;
        f.channel_mask = e.dwChannelMask; // A zero mask is ambiguous: reject.
    } else {
        // The base PCM/IEEE WAVE formats define canonical mono/stereo order.
        f.channel_mask = w.nChannels == 1 ? 4u : w.nChannels == 2 ? 3u : 0u;
    }
    f.floating = floating ? 1 : 0;
    const auto width = myw_frame_bytes(f);
    if ((!pcm && !floating) || !width || w.nBlockAlign != width ||
        w.nAvgBytesPerSec != f.rate * width) return AUDCLNT_E_UNSUPPORTED_FORMAT;
    return S_OK;
}
HRESULT open_endpoint(const uint16_t* id, size_t units, int direction, Endpoint& e, myw_format& format) {
    static_assert(sizeof(wchar_t) == sizeof(uint16_t));
    const std::wstring name(reinterpret_cast<const wchar_t*>(id), units);
    HRESULT hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                                  IID_PPV_ARGS(&e.enumerator));
    if (FAILED(hr)) return hr;
    hr = e.enumerator->GetDevice(name.c_str(), &e.device);
    if (FAILED(hr)) return hr;
    ComPtr<IMMEndpoint> endpoint;
    hr = e.device.As(&endpoint);
    if (FAILED(hr)) return hr;
    EDataFlow flow;
    hr = endpoint->GetDataFlow(&flow);
    if (FAILED(hr)) return hr;
    if (flow != (direction == MYW_CAPTURE ? eCapture : eRender)) return E_INVALIDARG;
    DWORD state = 0;
    hr = e.device->GetState(&state);
    if (FAILED(hr)) return hr;
    if (state != DEVICE_STATE_ACTIVE) return AUDCLNT_E_DEVICE_INVALIDATED;
    hr = e.device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr,
                            reinterpret_cast<void**>(e.client.GetAddressOf()));
    if (FAILED(hr)) return hr;
    hr = e.client->GetMixFormat(&e.wave);
    return FAILED(hr) ? hr : describe(*e.wave, format);
}
}
extern "C" int myw_endpoint_format(const uint16_t* id, size_t units, int direction,
                                   myw_format* format, int32_t* error) {
    if (format) *format = {};
    if (error) *error = 0;
    if (!format || !error || !valid_endpoint(id, units, direction)) return MYW_INVALID;
    try {
        Apartment apartment;
        HRESULT hr = apartment.result;
        if (SUCCEEDED(hr)) { Endpoint endpoint; hr = open_endpoint(id, units, direction, endpoint, *format); }
        *error = static_cast<int32_t>(hr);
        return SUCCEEDED(hr) ? MYW_OK : hr == AUDCLNT_E_UNSUPPORTED_FORMAT ? MYW_FORMAT : MYW_NATIVE;
    } catch (...) { *error = static_cast<int32_t>(E_OUTOFMEMORY); return MYW_EXHAUSTED; }
}
extern "C" int myw_worker_run(myw_port* port, const uint16_t* id, size_t units,
                              int direction, uint32_t duration_ms, myw_report* report) {
    if (report) { *report = {}; report->result = MYW_INVALID; }
    if (!port || !report || !valid_endpoint(id, units, direction) || !duration_ms || duration_ms > 600000) {
        myw_port_revoke(port);
        return MYW_INVALID;
    }
    myw_format expected{};
    myw_binding binding{};
    uint64_t clock_id = 0;
    (void)myw_port_info(port, &expected, &binding, &clock_id);
    int result = myw_port_state(port);
    try {
        Apartment apartment;
        Endpoint endpoint; // Released before apartment on every path.
        HRESULT hr = apartment.result;
        if (result == MYW_OK && SUCCEEDED(hr))
            hr = open_endpoint(id, units, direction, endpoint, report->negotiated);
        if (result == MYW_OK && SUCCEEDED(hr) && !myw_format_equal(expected, report->negotiated))
            result = MYW_FORMAT;
        Event event;
        if (result == MYW_OK && SUCCEEDED(hr) && !event.handle) hr = HRESULT_FROM_WIN32(GetLastError());
        if (result == MYW_OK && SUCCEEDED(hr))
            hr = endpoint.client->Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                                              0, 0, endpoint.wave, nullptr);
        UINT32 buffer_frames = 0;
        if (result == MYW_OK && SUCCEEDED(hr)) hr = endpoint.client->GetBufferSize(&buffer_frames);
        if (result == MYW_OK && SUCCEEDED(hr) && (!buffer_frames || buffer_frames > MYW_MAX_FRAMES * MYW_SLOTS))
            result = MYW_UNSUPPORTED;
        if (result == MYW_OK && SUCCEEDED(hr)) hr = endpoint.client->SetEventHandle(event.handle);
        ComPtr<IAudioCaptureClient> capture;
        ComPtr<IAudioRenderClient> render;
        ComPtr<IAudioClock> clock;
        if (result == MYW_OK && SUCCEEDED(hr)) {
            if (direction == MYW_CAPTURE) {
                hr = endpoint.client->GetService(IID_PPV_ARGS(&capture));
                report->clock_frequency = expected.rate;
            } else {
                hr = endpoint.client->GetService(IID_PPV_ARGS(&render));
                if (SUCCEEDED(hr)) hr = endpoint.client->GetService(IID_PPV_ARGS(&clock));
                if (SUCCEEDED(hr)) hr = clock->GetFrequency(&report->clock_frequency);
            }
        }
        mixyard_worker::RenderCursor cursor; // Allocate/touch before either stream starts.
        uint64_t sequence = 0;
        const size_t width = myw_frame_bytes(expected);
        bool started = false;
        if (result == MYW_OK && SUCCEEDED(hr) && direction == MYW_RENDER) {
            BYTE* bytes = nullptr;
            hr = render->GetBuffer(buffer_frames, &bytes);
            if (SUCCEEDED(hr)) hr = render->ReleaseBuffer(buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT);
            report->silent_frames = SUCCEEDED(hr) ? buffer_frames : 0; // Explicit startup preroll.
        }
        if (result == MYW_OK && SUCCEEDED(hr)) {
            result = myw_port_state(port);
            if (result == MYW_OK) { hr = endpoint.client->Start(); started = SUCCEEDED(hr); }
        }
        if (started) {
            Scheduling scheduling;
            report->mmcss = scheduling.handle ? 1u : 0u;
            const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(duration_ms);
            // No allocation, logging, locks, files or IOCTLs below. Only WASAPI,
            // event waits, bounded byte copies and lock-free queue operations.
            while (SUCCEEDED(hr) && result == MYW_OK && std::chrono::steady_clock::now() < until) {
                result = myw_port_state(port);
                if (result != MYW_OK) break;
                const auto wait = WaitForSingleObject(event.handle, 50);
                if (wait == WAIT_TIMEOUT) continue;
                if (wait != WAIT_OBJECT_0) { hr = HRESULT_FROM_WIN32(GetLastError()); break; }
                result = myw_port_state(port);
                if (result != MYW_OK) break;
                if (direction == MYW_CAPTURE) {
                    for (unsigned packet = 0; packet < MYW_SLOTS && SUCCEEDED(hr) && result == MYW_OK; ++packet) {
                        UINT32 available = 0;
                        hr = capture->GetNextPacketSize(&available);
                        if (FAILED(hr) || !available) break;
                        BYTE* data = nullptr; UINT32 frames = 0; DWORD flags = 0;
                        UINT64 position = 0, qpc = 0;
                        hr = capture->GetBuffer(&data, &frames, &flags, &position, &qpc);
                        if (FAILED(hr) || hr == AUDCLNT_S_BUFFER_EMPTY) break;
                        report->last_device_position = position; report->last_qpc_100ns = qpc; report->last_flags = flags;
                        if (!frames || frames > buffer_frames || (flags & ~AUDCLNT_BUFFERFLAGS_SILENT) ||
                            (!data && !(flags & AUDCLNT_BUFFERFLAGS_SILENT))) result = MYW_CAPTURE_FAULT;
                        for (UINT32 offset = 0; offset < frames && result == MYW_OK; ) {
                            const UINT32 count = std::min<UINT32>(frames - offset, MYW_MAX_FRAMES);
                            myw_stamp stamp{binding, clock_id, sequence, report->frames, position, qpc, offset, flags};
                            result = myw_port_push(port, expected, stamp, data ? data + offset * width : cursor.bytes.data(),
                                                   count, count * width);
                            if (result == MYW_OK) {
                                report->frames += count; ++sequence;
                                if (flags & AUDCLNT_BUFFERFLAGS_SILENT) report->silent_frames += count;
                            }
                            offset += count;
                        }
                        const auto released = capture->ReleaseBuffer(frames);
                        if (FAILED(released)) hr = released;
                    }
                } else {
                    UINT32 padding = 0;
                    hr = endpoint.client->GetCurrentPadding(&padding);
                    if (SUCCEEDED(hr) && padding > buffer_frames) hr = E_UNEXPECTED;
                    UINT32 remaining = SUCCEEDED(hr) ? buffer_frames - padding : 0;
                    for (unsigned packet = 0; remaining && packet < MYW_SLOTS && SUCCEEDED(hr) && result == MYW_OK; ++packet) {
                        const UINT32 count = std::min<UINT32>(remaining, MYW_MAX_FRAMES);
                        BYTE* data = nullptr;
                        hr = render->GetBuffer(count, &data);
                        if (FAILED(hr)) break;
                        if (!data) { result = MYW_NATIVE; hr = render->ReleaseBuffer(count, AUDCLNT_BUFFERFLAGS_SILENT); break; }
                        result = cursor.fill(port, binding, width, data, count, report->underflow_frames);
                        if (result == MYW_OK) result = myw_port_state(port);
                        hr = render->ReleaseBuffer(count, result == MYW_OK ? 0 : AUDCLNT_BUFFERFLAGS_SILENT);
                        if (SUCCEEDED(hr) && result == MYW_OK) report->frames += count;
                        remaining -= count;
                    }
                    if (SUCCEEDED(hr) && result == MYW_OK)
                        hr = clock->GetPosition(&report->last_device_position, &report->last_qpc_100ns);
                }
            }
            // Drop pending native data on revoke/failure/completion; no restart.
            const auto stop = endpoint.client->Stop();
            const auto reset = endpoint.client->Reset();
            if (SUCCEEDED(hr) && FAILED(stop)) hr = stop;
            if (SUCCEEDED(hr) && FAILED(reset)) hr = reset;
        }
        report->native_error = static_cast<int32_t>(hr);
        if (FAILED(hr)) result = hr == AUDCLNT_E_UNSUPPORTED_FORMAT ? MYW_FORMAT : MYW_NATIVE;
        if (result == MYW_OK && !report->frames) result = MYW_EMPTY;
        if (result == MYW_OK && report->underflow_frames) result = MYW_UNDERFLOW;
    } catch (...) { result = MYW_EXHAUSTED; report->native_error = static_cast<int32_t>(E_OUTOFMEMORY); }
    myw_port_revoke(port);
    report->result = result;
    return result;
}
#else
extern "C" int myw_endpoint_format(const uint16_t*, size_t, int, myw_format* format, int32_t* error) {
    if (format) *format = {};
    if (error) *error = 0;
    return MYW_UNSUPPORTED;
}
extern "C" int myw_worker_run(myw_port* port, const uint16_t*, size_t, int, uint32_t, myw_report* report) {
    if (report) { *report = {}; report->result = MYW_UNSUPPORTED; }
    myw_port_revoke(port);
    return MYW_UNSUPPORTED;
}
#endif

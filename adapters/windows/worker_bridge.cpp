// SPDX-License-Identifier: MIT
#include "worker.h"
#include <windows.h>
#include <array>
#include <chrono>
#include <iostream>
#include <memory>
#include <string>
#include <thread>

std::string utf8(const wchar_t* value);
std::string json(const std::string& value);

// Explicit probe flow, independent from runtime Apply/availability. This code
// performs no driver provisioning or virtual-input publication.
int worker_bridge(int argc, wchar_t** argv) {
    if (argc != 8 || std::wstring(argv[2]) != L"--mic" || std::wstring(argv[4]) != L"--render" ||
        std::wstring(argv[6]) != L"--seconds") {
        std::cerr << "bridge --mic EXACT_CAPTURE_MMDEVICE_ID --render EXACT_RENDER_MMDEVICE_ID --seconds 1..600\n";
        return 2;
    }
    const std::wstring source(argv[3]), destination(argv[5]), duration(argv[7]);
    if (duration.empty() || duration.find_first_not_of(L"0123456789") != std::wstring::npos) return 2;
    const auto seconds = std::stoull(duration);
    if (!seconds || seconds > 600 || source.empty() || destination.empty()) return 2;
    const auto* source_id = reinterpret_cast<const uint16_t*>(source.data());
    const auto* destination_id = reinterpret_cast<const uint16_t*>(destination.data());
    myw_format source_format{}, destination_format{};
    int32_t error = 0;
    int result = myw_endpoint_format(source_id, source.size(), MYW_CAPTURE, &source_format, &error);
    if (result == MYW_OK)
        result = myw_endpoint_format(destination_id, destination.size(), MYW_RENDER, &destination_format, &error);
    if (result != MYW_OK || !myw_format_equal(source_format, destination_format)) {
        std::cerr << "bridge setup rejected: result=" << result << ", HRESULT=" << error
                  << "; source and destination require identical rate/precision/ordered channels; no conversion\n";
        return 1;
    }
    using Port = std::unique_ptr<myw_port, decltype(&myw_port_destroy)>;
    const myw_binding binding{1, 1}; // This probe process is a new, nonpersistent host session.
    myw_port* raw_source = nullptr; myw_port* raw_destination = nullptr;
    result = myw_port_create(source_format, binding, 1, &raw_source);
    Port capture(raw_source, myw_port_destroy);
    if (result != MYW_OK) return 1;
    result = myw_port_create(destination_format, binding, 1, &raw_destination);
    Port render(raw_destination, myw_port_destroy);
    if (result != MYW_OK) return 1;
    myw_report capture_report{}, render_report{};
    const auto duration_ms = static_cast<uint32_t>(seconds * 1000);
    std::thread capture_thread([&] {
        (void)myw_worker_run(capture.get(), source_id, source.size(), MYW_CAPTURE, duration_ms, &capture_report);
    });
    // If another thread cannot be started, stop/join the first before unwinding.
    std::thread render_thread;
    try {
        render_thread = std::thread([&] {
            (void)myw_worker_run(render.get(), destination_id, destination.size(), MYW_RENDER, duration_ms, &render_report);
        });
    } catch (...) {
        myw_port_revoke(capture.get()); capture_thread.join(); throw;
    }
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> bytes{};
    const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(duration_ms);
    while (std::chrono::steady_clock::now() < until && myw_port_state(capture.get()) == MYW_OK &&
           myw_port_state(render.get()) == MYW_OK) {
        myw_block block{};
        result = myw_port_pop(capture.get(), binding, bytes.data(), bytes.size(), &block);
        if (result == MYW_EMPTY) { std::this_thread::sleep_for(std::chrono::milliseconds(1)); continue; }
        if (result != MYW_OK) break;
        result = myw_port_push(render.get(), source_format, block.stamp, bytes.data(), block.frames,
                               block.frames * myw_frame_bytes(source_format));
        if (result != MYW_OK) break;
    }
    myw_port_revoke(capture.get()); myw_port_revoke(render.get());
    capture_thread.join(); render_thread.join();
    std::cout << "{\"command\":\"bridge\",\"backendAvailable\":false,\"audioApplied\":false,"
                 "\"nativeAudioQualified\":false,\"virtualInputs\":false,\"sampleRate\":" << source_format.rate
              << ",\"captureEndpoint\":" << json(utf8(source.c_str()))
              << ",\"renderEndpoint\":" << json(utf8(destination.c_str()))
              << ",\"channels\":" << source_format.channels << ",\"channelMask\":" << source_format.channel_mask
              << ",\"containerBits\":" << source_format.container_bits << ",\"validBits\":" << source_format.valid_bits
              << ",\"floating\":" << source_format.floating << ",\"capturedFrames\":" << capture_report.frames
              << ",\"renderSubmittedFrames\":" << render_report.frames
              << ",\"renderUnderflowFrames\":" << render_report.underflow_frames
              << ",\"captureResult\":" << capture_report.result << ",\"captureHRESULT\":" << capture_report.native_error
              << ",\"renderResult\":" << render_report.result << ",\"renderHRESULT\":" << render_report.native_error << "}\n";
    // Revoking at the explicit deadline is expected, but no received/submitted
    // data, native fault, overflow or any underflow can be called a passing run.
    const auto ended = [](const myw_report& r) {
        return (r.result == MYW_OK || r.result == MYW_STALE) && r.native_error >= 0 && r.frames;
    };
    return ended(capture_report) && ended(render_report) && !render_report.underflow_frames &&
        (result == MYW_OK || result == MYW_EMPTY || result == MYW_STALE) ? 0 : 1;
}

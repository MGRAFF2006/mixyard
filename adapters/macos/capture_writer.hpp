// SPDX-License-Identifier: MIT
#pragma once

#include "capture_buffer.hpp"
#include <chrono>
#include <thread>

namespace mixyard {
// The worker owns all file I/O, including creation, finalization and destruction.
// On a stopAndWait timeout the caller must _Exit without releasing this or the queue.
class CaptureWriter {
    std::atomic<uint32_t> done{0}, completed{0};
    std::thread worker;

    void fail(const std::exception& e) {
        if (!failed.load(std::memory_order_relaxed)) {
            error = e.what();
            failed.store(1, std::memory_order_release);
        }
    }
public:
    std::atomic<uint32_t> failed{0}, writtenFrames{0};
    std::string error; // Read only after acquiring failed or joining the worker.
    bool opened = false, hasSignal = false; // Read only after successful stopAndWait.
    uint64_t frames = 0;

    CaptureWriter(CaptureQueue& queue, const char* path, uint32_t rate) {
        worker = std::thread([this, &queue, path = std::string(path), rate] {
            try {
                WavWriter wav(path.c_str(), rate);
                opened = true;
                try {
                    for (;;) {
                        if (const auto* chunk = queue.front()) {
                            wav.write(*chunk);
                            writtenFrames.fetch_add(chunk->frames, std::memory_order_relaxed);
                            queue.pop();
                        } else if (done.load(std::memory_order_acquire)) {
                            // The final producer publication precedes done.
                            if (!queue.front()) break;
                        } else std::this_thread::sleep_for(std::chrono::milliseconds(2));
                    }
                } catch (const std::exception& e) { fail(e); }
                try { wav.finish(); } catch (const std::exception& e) { fail(e); }
                frames = wav.frames();
                hasSignal = wav.hasSignal();
            } catch (const std::exception& e) { fail(e); }
            // Publish only after the WAV destructor (which may also perform I/O).
            completed.store(1, std::memory_order_release);
        });
    }

    bool stopAndWait(std::chrono::milliseconds timeout) {
        const auto deadline = std::chrono::steady_clock::now() + timeout;
        done.store(1, std::memory_order_release);
        while (!completed.load(std::memory_order_acquire)) {
            if (std::chrono::steady_clock::now() >= deadline) return false;
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
        }
        worker.join();
        return true;
    }
};
} // namespace mixyard

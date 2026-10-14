// SPDX-License-Identifier: MIT
#pragma once

#include <array>
#include <atomic>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <limits>
#include <stdexcept>
#include <string>
#include <unistd.h>

namespace mixyard {
constexpr uint32_t maxFrames = 4096;
constexpr uint32_t queueSlots = 32;
static_assert(std::atomic<uint32_t>::is_always_lock_free);
static_assert(sizeof(float) == 4 && std::numeric_limits<float>::is_iec559);
#if __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "The capture probe currently requires little-endian Float32 PCM."
#endif

struct Chunk {
    uint32_t frames = 0;
    std::array<float, maxFrames * 2> samples{};
};

// One HAL callback producer, one disk consumer. All storage is touched before start.
class CaptureQueue {
    std::array<Chunk, queueSlots> chunks{};
    alignas(64) std::atomic<uint32_t> writeIndex{0};
    alignas(64) std::atomic<uint32_t> readIndex{0};
public:
    std::atomic<uint32_t> dropped{0};
    std::atomic<uint32_t> invalid{0};

    bool push(const float* interleavedOrLeft, const float* right, uint32_t frames) noexcept {
        if (!interleavedOrLeft || frames == 0 || frames > maxFrames) {
            invalid.fetch_add(1, std::memory_order_relaxed);
            return false;
        }
        const auto w = writeIndex.load(std::memory_order_relaxed);
        if (w - readIndex.load(std::memory_order_acquire) == queueSlots) {
            dropped.fetch_add(1, std::memory_order_relaxed);
            return false;
        }
        auto& chunk = chunks[w % queueSlots];
        chunk.frames = frames;
        if (right) {
            for (uint32_t f = 0; f < frames; ++f) {
                chunk.samples[2 * f] = interleavedOrLeft[f];
                chunk.samples[2 * f + 1] = right[f];
            }
        } else {
            std::memcpy(chunk.samples.data(), interleavedOrLeft, frames * 2 * sizeof(float));
        }
        writeIndex.store(w + 1, std::memory_order_release);
        return true;
    }
    const Chunk* front() const noexcept {
        const auto r = readIndex.load(std::memory_order_relaxed);
        return r == writeIndex.load(std::memory_order_acquire) ? nullptr : &chunks[r % queueSlots];
    }
    void pop() noexcept {
        readIndex.store(readIndex.load(std::memory_order_relaxed) + 1, std::memory_order_release);
    }
};

class WavWriter {
    FILE* file = nullptr;
    uint32_t rate;
    uint64_t frameCount = 0;
    bool nonzero = false;
    static void put16(unsigned char* p, uint16_t n) {
        p[0] = n & 255; p[1] = n >> 8;
    }
    static void put32(unsigned char* p, uint32_t n) {
        for (unsigned i = 0; i < 4; ++i) p[i] = (n >> (8 * i)) & 255;
    }
    void header() {
        std::array<unsigned char, 56> h{};
        std::memcpy(h.data(), "RIFF", 4);
        put32(h.data() + 4, static_cast<uint32_t>(frameCount * 8 + 48));
        std::memcpy(h.data() + 8, "WAVEfmt ", 8);
        put32(h.data() + 16, 16);
        put16(h.data() + 20, 3); // IEEE Float32, stereo.
        put16(h.data() + 22, 2);
        put32(h.data() + 24, rate);
        put32(h.data() + 28, rate * 8);
        put16(h.data() + 32, 8);
        put16(h.data() + 34, 32);
        std::memcpy(h.data() + 36, "fact", 4);
        put32(h.data() + 40, 4);
        put32(h.data() + 44, static_cast<uint32_t>(frameCount));
        std::memcpy(h.data() + 48, "data", 4);
        put32(h.data() + 52, static_cast<uint32_t>(frameCount * 8));
        if (std::fwrite(h.data(), 1, h.size(), file) != h.size())
            throw std::runtime_error("WAV header write failed (check free space/permissions)");
    }
public:
    WavWriter(const char* path, uint32_t sampleRate) : rate(sampleRate) {
        if (rate < 8000 || rate > 192000) throw std::runtime_error("Unsupported WAV sample rate");
        const int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
        if (fd < 0) throw std::runtime_error("Cannot create WAV: " + std::string(std::strerror(errno)));
        file = fdopen(fd, "wb");
        if (!file) { close(fd); throw std::runtime_error("fdopen failed"); }
        try { header(); } catch (...) { std::fclose(file); file = nullptr; throw; }
    }
    ~WavWriter() { if (file) std::fclose(file); }
    WavWriter(const WavWriter&) = delete;
    WavWriter& operator=(const WavWriter&) = delete;
    void write(const Chunk& chunk) {
        if (chunk.frames == 0 || chunk.frames > maxFrames || frameCount + chunk.frames > (UINT32_MAX - 48) / 8)
            throw std::runtime_error("Invalid/oversize WAV data");
        for (uint32_t i = 0; i < chunk.frames * 2; ++i) {
            if (!std::isfinite(chunk.samples[i])) throw std::runtime_error("Nonfinite captured PCM");
            nonzero = nonzero || chunk.samples[i] != 0.0f;
        }
        const size_t bytes = chunk.frames * 8;
        if (std::fwrite(chunk.samples.data(), 1, bytes, file) != bytes)
            throw std::runtime_error("PCM disk write failed (check free space)");
        frameCount += chunk.frames;
    }
    void finish() {
        if (std::fflush(file) != 0 || std::fseek(file, 0, SEEK_SET) != 0)
            throw std::runtime_error("Cannot finalize WAV");
        header();
        const int result = std::fclose(file);
        file = nullptr;
        if (result != 0) throw std::runtime_error("WAV close failed");
    }
    uint64_t frames() const { return frameCount; }
    bool hasSignal() const { return nonzero; }
};
} // namespace mixyard

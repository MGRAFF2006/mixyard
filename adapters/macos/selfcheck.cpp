// SPDX-License-Identifier: MIT
#include "capture_buffer.hpp"
#include <cassert>
#include <chrono>
#include <fstream>
#include <iterator>
#include <memory>
#include <thread>
#include <vector>

static uint32_t u32(const std::vector<unsigned char>& b, size_t i) {
    return uint32_t(b[i]) | uint32_t(b[i + 1]) << 8 | uint32_t(b[i + 2]) << 16 | uint32_t(b[i + 3]) << 24;
}

int main(int argc, const char* argv[]) {
    assert(argc == 2);
    auto q = std::make_unique<mixyard::CaptureQueue>();
    const float left[] = {0.25f, -0.5f}, right[] = {-0.25f, 0.5f};
    assert(!q->front());
    assert(!q->push(nullptr, nullptr, 1));
    assert(!q->push(left, right, mixyard::maxFrames + 1));
    assert(q->invalid.load() == 2);
    for (uint32_t i = 0; i < mixyard::queueSlots; ++i) assert(q->push(left, right, 2));
    assert(!q->push(left, right, 2) && q->dropped.load() == 1);
    for (uint32_t i = 0; i < mixyard::queueSlots; ++i) {
        const auto* c = q->front(); assert(c && c->frames == 2);
        assert(c->samples[0] == 0.25f && c->samples[1] == -0.25f);
        assert(c->samples[2] == -0.5f && c->samples[3] == 0.5f);
        q->pop();
    }
    assert(!q->front());
    const float interleaved[] = {0.125f, -0.125f, 0.375f, -0.375f};
    assert(q->push(interleaved, nullptr, 2));
    mixyard::WavWriter wav(argv[1], 48000);
    bool refusedOverwrite = false;
    try { mixyard::WavWriter duplicate(argv[1], 48000); }
    catch (const std::runtime_error&) { refusedOverwrite = true; }
    assert(refusedOverwrite);
    wav.write(*q->front()); q->pop(); wav.finish();
    assert(wav.frames() == 2 && wav.hasSignal());
    std::ifstream input(argv[1], std::ios::binary);
    std::vector<unsigned char> bytes((std::istreambuf_iterator<char>(input)), {});
    assert(bytes.size() == 72 && u32(bytes, 4) == bytes.size() - 8);
    assert(std::memcmp(bytes.data(), "RIFF", 4) == 0 && std::memcmp(bytes.data() + 8, "WAVEfmt ", 8) == 0);
    assert(bytes[20] == 3 && bytes[22] == 2 && bytes[34] == 32);
    assert(u32(bytes, 24) == 48000 && u32(bytes, 28) == 384000);
    assert(std::memcmp(bytes.data() + 36, "fact", 4) == 0 && u32(bytes, 44) == 2);
    assert(std::memcmp(bytes.data() + 48, "data", 4) == 0 && u32(bytes, 52) == 16);
    assert(std::memcmp(bytes.data() + 56, interleaved, 16) == 0);

    // Two actual threads exercise publication/reuse; audio is synthetic.
    auto concurrent = std::make_unique<mixyard::CaptureQueue>();
    constexpr uint32_t count = 10000;
    std::thread consumer([&] {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        for (uint32_t i = 0; i < count;) {
            if (const auto* c = concurrent->front()) {
                assert(c->frames == 1 && c->samples[0] == float(i) && c->samples[1] == -float(i));
                concurrent->pop(); ++i;
            } else {
                assert(std::chrono::steady_clock::now() < deadline);
                std::this_thread::yield();
            }
        }
    });
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    for (uint32_t i = 0; i < count; ++i) {
        const float sample[] = {float(i), -float(i)};
        while (!concurrent->push(sample, nullptr, 1)) {
            assert(std::chrono::steady_clock::now() < deadline);
            std::this_thread::yield();
        }
    }
    consumer.join();
    assert(!concurrent->front());
    std::puts("PASS: synthetic queue bounds/overflow/planar interleave/threaded FIFO and Float32 WAV bytes/no-overwrite; no native Core Audio evidence");
}

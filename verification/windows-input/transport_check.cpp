// SPDX-License-Identifier: MIT
#include "../../adapters/windows/virtual-input/transport.hpp"
#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>

using namespace mixyard_input;
using Wire = std::array<uint8_t, max_packet_bytes>;
static void check(bool yes, const char* why) {
    if (!yes) { std::fprintf(stderr, "FAIL: %s\n", why); std::exit(1); }
}
static void put(Wire& b, size_t offset, uint64_t value, size_t bytes = 8) {
    for (size_t i = 0; i < bytes; ++i) b[offset + i] = static_cast<uint8_t>(value >> (i * 8));
}
static Wire packet(const Transport& t, uint32_t frames = 128, uint64_t rev = 1) {
    Wire b{};
    put(b, 0, magic, 4); put(b, 4, version, 2); put(b, 6, header_bytes, 2);
    put(b, 8, header_bytes + frames * 8, 4); put(b, 12, frames, 4);
    put(b, 16, t.generation); put(b, 24, t.epoch); put(b, 32, rev);
    put(b, 40, t.next_frame); put(b, 48, 48000, 4); put(b, 52, 2, 2);
    put(b, 54, 1, 2); put(b, 56, t.next_sequence);
    for (size_t i = 0; i < frames; ++i) {
        put(b, header_bytes + i * 8, 0x3e800000, 4); // +0.25 L
        put(b, header_bytes + i * 8 + 4, 0xbe000000, 4); // -0.125 R
    }
    return b;
}
static void rejected_unchanged(Transport& t, const Wire& b, size_t length, Result want) {
    const auto before = t;
    check(t.submit(41, b.data(), length) == want, "rejection status");
    check(std::memcmp(&t, &before, sizeof(t)) == 0, "rejected packet changed state");
}
int main() {
    Transport t{};
    t.initialize();
    check(t.begin(0, 1) == Result::malformed, "zero owner");
    check(t.begin(41, 0) == Result::malformed, "zero epoch");
    check(t.begin(41, 7) == Result::accepted, "admit owner");
    check(t.begin(42, 7) == Result::unauthorized, "exclusive owner");
    auto b = packet(t);
    check(t.submit(42, b.data(), b.size()) == Result::unauthorized, "other opened file denied");
    check(t.submit(41, nullptr, b.size()) == Result::malformed, "null input");
    for (size_t n = 0; n < b.size(); ++n) rejected_unchanged(t, b, n, Result::malformed);
    rejected_unchanged(t, b, b.size() + 1, Result::malformed);
    for (const size_t offset : {size_t(0), size_t(4), size_t(6), size_t(8), size_t(12), size_t(48), size_t(52), size_t(54)}) {
        auto bad = b; bad[offset] ^= 1;
        rejected_unchanged(t, bad, bad.size(), Result::malformed);
    }
    for (const size_t offset : {size_t(16), size_t(24)}) {
        auto bad = b; bad[offset] ^= 1;
        rejected_unchanged(t, bad, bad.size(), Result::stale);
    }
    for (const size_t offset : {size_t(32), size_t(40), size_t(56)}) {
        auto bad = b; put(bad, offset, offset == 32 ? 0 : 1);
        rejected_unchanged(t, bad, bad.size(), Result::out_of_order);
    }
    for (const uint32_t nonfinite : {0x7f800000u, 0xff800000u, 0x7f800001u, 0x7fc00000u, 0xffffffffu}) {
        auto bad = b; put(bad, max_packet_bytes - 4, nonfinite, 4);
        rejected_unchanged(t, bad, bad.size(), Result::malformed);
    }
    check(t.submit(41, b.data(), b.size()) == Result::accepted, "first queue acceptance");
    rejected_unchanged(t, b, b.size(), Result::out_of_order); // replay
    QueuedPacket a{}, slow{};
    check(t.copy_queued(t.generation, 0, &a) == Result::accepted, "reader A inspect");
    check(t.copy_queued(t.generation, 0, &slow) == Result::accepted, "reader B independent cursor");
    check(std::memcmp(&a, &slow, sizeof(a)) == 0, "inspection is nondestructive");
    check(a.first_frame == 0 && a.frames == 128 && a.revision == 1, "queued metadata");
    check(a.samples[0] == 0x3e800000 && a.samples[1] == 0xbe000000, "exact stereo bits");
    for (uint64_t i = 1; i <= retained_packets + 2; ++i) {
        b = packet(t, 1, 2);
        const auto want = i >= retained_packets ? Result::accepted_overwrite : Result::accepted;
        check(t.submit(41, b.data(), header_bytes + 8) == want, "bounded overwrite reported");
        check(t.copy_queued(t.generation, i, &a) == Result::accepted, "fast cursor survives slow reader");
        check(a.samples[2] == 0 && a.samples[max_frames * 2 - 1] == 0, "unused payload zeroed");
    }
    check(t.overwritten_packets == 3, "exact overwrite diagnostics");
    check(t.copy_queued(t.generation, 0, &slow) == Result::unavailable, "slow cursor loss explicit");
    check(slow.frames == 0 && slow.samples[0] == 0, "missing queue initializes zeros");
    check(t.copy_queued(t.generation, t.next_sequence, &slow) == Result::unavailable, "future cursor unavailable");
    b = packet(t, 1, 1);
    rejected_unchanged(t, b, header_bytes + 8, Result::out_of_order); // revision regression
    const auto old_generation = t.generation;
    check(t.cleanup(42) == Result::unauthorized && t.owner == 41, "foreign cleanup denied");
    check(t.cleanup(41) == Result::accepted, "owner cleanup");
    for (const auto& p : t.packets) {
        check(p.frames == 0 && p.generation == 0 && p.epoch == 0 && p.revision == 0, "metadata cleared");
        for (auto sample : p.samples) check(sample == 0, "producer loss zeroes retained samples");
    }
    check(t.copy_queued(old_generation, 0, &a) == Result::stale, "producer loss invalidates readers");
    check(t.begin(41, 7) == Result::accepted && t.generation == old_generation + 1, "same file key new generation");
    put(b, 16, old_generation);
    rejected_unchanged(t, b, header_bytes + 8, Result::stale);
    t.next_sequence = UINT64_MAX;
    b = packet(t, 1);
    rejected_unchanged(t, b, header_bytes + 8, Result::exhausted);
    t.next_sequence = 0; t.next_frame = UINT64_MAX;
    b = packet(t, 1);
    rejected_unchanged(t, b, header_bytes + 8, Result::exhausted);
    check(t.cleanup(41) == Result::accepted, "cleanup after exhausted");
    t.generation = UINT64_MAX;
    check(t.begin(41, 7) == Result::exhausted && t.owner == 0, "generation must never wrap");
    std::puts("PASS: ingress ABI/ownership/bounds/queue inspection/zeroing only; NO driver execution or PCM delivery");
}

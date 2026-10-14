// SPDX-License-Identifier: MIT
// Exercises the SAME cursor and integer clock conversion compiled into endpoint.cpp.
#include "../../adapters/windows/virtual-input/capture/delivery.hpp"
#include <array>
#include <cassert>
#include <cstdio>
using namespace mixyard_input;
static void put(uint8_t* p, uint64_t n, size_t bytes) {
    for (size_t i = 0; i < bytes; ++i) p[i] = static_cast<uint8_t>(n >> (i * 8));
}
static void submit(Transport& t, uint32_t frames, uint32_t marker) {
    std::array<uint8_t, max_packet_bytes> b{};
    put(b.data(), magic, 4); put(b.data()+4, version, 2); put(b.data()+6, header_bytes, 2);
    put(b.data()+8, header_bytes+frames*8, 4); put(b.data()+12, frames, 4);
    put(b.data()+16, t.generation, 8); put(b.data()+24, t.epoch, 8);
    put(b.data()+32, t.next_sequence+1, 8); put(b.data()+40, t.next_frame, 8);
    put(b.data()+48, 48000, 4); put(b.data()+52, 2, 2); put(b.data()+54, 1, 2);
    put(b.data()+56, t.next_sequence, 8);
    for (size_t i=0; i<frames; ++i) { put(b.data()+64+i*8, marker, 4); put(b.data()+68+i*8, marker|0x80000000u, 4); }
    const auto result = t.submit(7, b.data(), header_bytes+frames*8);
    assert(result == Result::accepted || result == Result::accepted_overwrite);
}
int main() {
    Transport t{}; t.initialize(); Delivery d{}; uint32_t stereo[2]{1,2};
    d.frame(t, stereo); assert(stereo[0] == 0 && stereo[1] == 0 && d.silent_frames == 1);
    assert(t.begin(7, 11) == Result::accepted);
    submit(t, 3, 0x3e800000); submit(t, 1, 0x3f000000);
    for (int i=0; i<3; ++i) { d.frame(t, stereo); assert(stereo[0] == 0x3e800000 && stereo[1] == 0xbe800000); assert(d.producer_frame == static_cast<uint64_t>(i)); }
    d.frame(t, stereo); assert(stereo[0] == 0x3f000000 && d.revision == 2 && d.producer_frame == 3);
    d.frame(t, stereo); assert(stereo[0] == 0 && stereo[1] == 0 && d.copied_frames == 4);
    for (int i=0; i<10; ++i) submit(t, 2, 0x3e000000);
    d.frame(t, stereo); assert(d.lost_packets == 2 && d.producer_frame == 8 && stereo[0] == 0x3e000000);
    assert(t.cleanup(7) == Result::accepted);
    d.frame(t, stereo); assert(stereo[0] == 0 && stereo[1] == 0);
    assert(t.begin(7, 12) == Result::accepted); submit(t, 1, 0x3d800000);
    d.frame(t, stereo); assert(d.producer_frame == 0 && d.sequence == 1 && stereo[0] == 0x3d800000);
    for (int i=0; i<1100; ++i) { d.frame(t, stereo); assert(stereo[0] == 0 && stereo[1] == 0); }
    // Independently evaluated exact integer rate, including non-integral QPC periods.
    for (uint64_t frequency : {10000000ULL, 3579545ULL}) {
        assert(clock_frames(0, frequency) == 0);
        assert(clock_frames(frequency, frequency) == 48000);
        assert(clock_frames(frequency*3600, frequency) == 172800000);
        uint64_t previous = 0;
        for (uint64_t ticks=0; ticks<frequency; ticks+=137) {
            auto frames=clock_frames(ticks,frequency);
            assert(frames == ticks*48000/frequency && frames >= previous); previous=frames;
        }
    }
    std::puts("PASS: shared delivery cursor/clock arithmetic only; NO native callback or consumer PCM proof");
}

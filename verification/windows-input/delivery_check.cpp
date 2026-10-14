// SPDX-License-Identifier: MIT
// Exercises the SAME cursor and integer clock conversion compiled into endpoint.cpp.
#include "../../adapters/windows/virtual-input/capture/delivery.hpp"
#include "../../adapters/windows/virtual-input/capture/buffer.hpp"
#include "../../adapters/windows/virtual-input/capture/status.hpp"
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
static void allocation_check() {
    // Inject allocation outcomes into the SAME allocation/free branch as the
    // miniport. These stubs are not native MDL or PortCls execution evidence.
    struct Mdl { uint32_t bytes; };
    struct Port {
        Mdl mdl{};
        uint32_t storage[16]{};
        bool allocation_fails = false, mapping_fails = false;
        unsigned maps = 0, frees = 0;
        Mdl* AllocatePagesForMdl(int, uint32_t) { return allocation_fails ? nullptr : &mdl; }
        void* MapAllocatedPages(Mdl* p, int) { assert(p == &mdl); ++maps; return mapping_fails ? nullptr : storage; }
        void FreePagesFromMdl(Mdl* p) { assert(p == &mdl); ++frees; }
    };
    const auto byte_count = [](Mdl* p) { return p->bytes; };
    for (uint32_t bytes : {0u, 32u, 63u}) {
        Port p{}; p.mdl.bytes = bytes;
        uint32_t* mapped = p.storage;
        assert(!allocate_capture_buffer<Mdl*>(&p, 0, 64, 0, byte_count, &mapped));
        assert(!mapped && p.maps == 0 && p.frees == 1);
        // A short allocation does not poison the next full allocation attempt.
        p.mdl.bytes = 64;
        assert(allocate_capture_buffer<Mdl*>(&p, 0, 64, 0, byte_count, &mapped) == &p.mdl);
        assert(mapped == p.storage && p.maps == 1 && p.frees == 1);
    }
    Port p{}; uint32_t* mapped = p.storage;
    p.allocation_fails = true;
    assert(!allocate_capture_buffer<Mdl*>(&p, 0, 64, 0, byte_count, &mapped));
    assert(!mapped && p.maps == 0 && p.frees == 0);
    p.allocation_fails = false; p.mapping_fails = true; p.mdl.bytes = 64;
    assert(!allocate_capture_buffer<Mdl*>(&p, 0, 64, 0, byte_count, &mapped));
    assert(!mapped && p.maps == 1 && p.frees == 1);
    p.mapping_fails = false; p.mdl.bytes = 4096; // Page-rounded capacity can exceed request.
    assert(allocate_capture_buffer<Mdl*>(&p, 0, 64, 0, byte_count, &mapped) == &p.mdl);
    assert(mapped == p.storage && p.maps == 2 && p.frees == 1);
}
static void provenance_check() {
    Transport t{}; t.initialize();
    assert(t.begin(7, 11) == Result::accepted); submit(t, 3, 0x3e800000);
    Delivery d{}; uint32_t stereo[2]{};
    d.frame(t, stereo); d.frame(t, stereo);
    CaptureStatus last{};
    record_capture_copy(last, t, d, 42, 12345);
    last.endpoint_frames = 43; last.stream_epoch = 8; last.qpc_frequency = 10000000;
    auto status = capture_status_snapshot(last, t);
    assert(status.revision == 1 && status.producer_frame == 1 && status.endpoint_frame == 42 && status.qpc == 12345);
    const auto invalid = [&] {
        const auto s = capture_status_snapshot(last, t);
        assert(s.generation == t.generation && s.producer_epoch == t.epoch);
        assert(s.revision == 0 && s.producer_frame == 0 && s.endpoint_frame == 0 && s.qpc == 0);
        assert(s.endpoint_frames == 43 && s.stream_epoch == 8 && s.qpc_frequency == 10000000);
        assert(s.version == 1 && s.bytes == sizeof(CaptureStatus));
    };
    // Cleanup and B's paused read before ANY subsequent frame/Tick.
    assert(t.cleanup(7) == Result::accepted); invalid();
    assert(t.begin(8, 12) == Result::accepted); invalid();
    assert(t.cleanup(7) == Result::unauthorized); invalid();
    // Malformed submit followed by the driver's admission-revocation action.
    assert(t.submit(8, nullptr, 0) == Result::malformed);
    assert(t.cleanup(8) == Result::accepted); invalid();
    assert(t.begin(9, 13) == Result::accepted); invalid();
    // Both producer generation and epoch are independently checked.
    last.generation = t.generation; invalid();
    last.producer_epoch = t.epoch;
    status = capture_status_snapshot(last, t);
    assert(status.revision == 1 && status.qpc == 12345);
    ++last.generation; invalid();
    assert(t.cleanup(9) == Result::accepted);
    assert(t.begin(7, 14) == Result::accepted); submit(t, 1, 0x3f000000);
    d.frame(t, stereo); assert(stereo[0] == 0x3f000000);
    record_capture_copy(last, t, d, 100, 67890);
    status = capture_status_snapshot(last, t);
    assert(status.generation == t.generation && status.producer_epoch == 14);
    assert(status.revision == 1 && status.producer_frame == 0 && status.endpoint_frame == 100 && status.qpc == 67890);
}
int main() {
    allocation_check(); provenance_check();
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
    std::puts("PASS: shared allocation/provenance/delivery/clock paths; NO native callback or consumer PCM proof");
}

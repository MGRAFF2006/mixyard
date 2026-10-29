// SPDX-License-Identifier: MIT
#include "worker.h"
#include "worker_render.hpp"
#include "virtual-input/producer/pcm.hpp"
#include <array>
#include <atomic>
#include <cstring>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <thread>

namespace {
void require(bool condition, const char* message) { if (!condition) throw std::runtime_error(message); }
const myw_format stereo{48000, 3, 2, 32, 32, 1};
const myw_binding binding{19, 7};
myw_stamp stamp(uint64_t sequence, uint64_t frame) {
    return {binding, 31, sequence, frame, 123456, 987654, 0, 0};
}
using Port = std::unique_ptr<myw_port, decltype(&myw_port_destroy)>;
Port port(myw_format format = stereo, myw_binding b = binding) {
    myw_port* p = nullptr;
    require(myw_port_create(format, b, 31, &p) == MYW_OK, "create exact port");
    return Port(p, myw_port_destroy);
}
void roundtrip(myw_format format) {
    auto p = port(format);
    const auto width = myw_frame_bytes(format);
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> data{}, out{};
    for (size_t i = 0; i < 5 * width; ++i) data[i] = static_cast<uint8_t>(i * 17 + 1);
    auto s = stamp(0, 0); s.packet_offset = 27;
    require(myw_port_push(p.get(), format, s, data.data(), 5, 5 * width) == MYW_OK, "push exact bytes");
    myw_block block{};
    require(myw_port_pop(p.get(), binding, out.data(), out.size(), &block) == MYW_OK, "pop exact bytes");
    require(block.frames == 5 && block.stamp.qpc_100ns == s.qpc_100ns &&
        block.stamp.device_frame == s.device_frame && block.stamp.packet_offset == 27, "retain original packet stamps");
    require(!std::memcmp(data.data(), out.data(), 5 * width), "precision/channel byte preservation");
    out.fill(255);
    require(myw_port_pop(p.get(), binding, out.data(), out.size(), &block) == MYW_EMPTY &&
        block.frames == 0, "empty does not replay");
    for (auto byte : out) require(!byte, "empty output zeroed");
}
void check_queue() {
    roundtrip(stereo);
    roundtrip({192000, 0x3f, 6, 64, 64, 1}); // Ordered 5.1 Float64, no f32 narrowing.
    roundtrip({384000, 0x63f, 8, 32, 24, 0}); // Eight-channel 24-in-32 valid bits.
    roundtrip({44100, 4, 1, 24, 24, 0}); // Packed PCM24.
    myw_port* invalid = reinterpret_cast<myw_port*>(1);
    require(myw_port_create({48000, 0, 2, 32, 32, 1}, binding, 1, &invalid) == MYW_FORMAT &&
        !invalid, "ambiguous channel mask rejects before allocation");
    require(myw_port_create(stereo, {0, 1}, 1, &invalid) == MYW_INVALID, "nonzero session identity required");
    auto p = port();
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> data{}, out{};
    myw_block block{};
    for (unsigned i = 0; i < MYW_SLOTS; ++i)
        require(myw_port_push(p.get(), stereo, stamp(i, i), data.data(), 1, 8) == MYW_OK, "fill bounded queue");
    require(myw_port_push(p.get(), stereo, stamp(MYW_SLOTS, MYW_SLOTS), data.data(), 1, 8) == MYW_FULL,
        "overflow terminally closes this output");
    out.fill(255);
    require(myw_port_pop(p.get(), binding, out.data(), out.size(), &block) == MYW_FULL && !out[0],
        "overflow cannot deliver historical pending data");
    auto sibling = port();
    require(myw_port_push(sibling.get(), stereo, stamp(0, 0), data.data(), 1, 8) == MYW_OK,
        "sibling output survives failure");
    auto wrong = stereo; wrong.valid_bits = 24;
    require(myw_port_push(sibling.get(), wrong, stamp(1, 1), data.data(), 1, 8) == MYW_FORMAT,
        "format change cannot silently narrow sibling");
    p = port();
    require(myw_port_push(p.get(), stereo, stamp(1, 0), data.data(), 1, 8) == MYW_ORDER,
        "sequence replay/gap fails closed");
    p = port();
    auto stale = stamp(0, 0); ++stale.binding.revision;
    require(myw_port_push(p.get(), stereo, stale, data.data(), 1, 8) == MYW_STALE, "stale revision rejected");
    p = port();
    auto fault = stamp(0, 0); fault.flags = 1;
    require(myw_port_push(p.get(), stereo, fault, data.data(), 1, 8) == MYW_CAPTURE_FAULT,
        "unknown-duration discontinuity never forwarded as contiguous data");
    p = port();
    data.fill(255);
    auto silent = stamp(0, 0); silent.flags = 2;
    require(myw_port_push(p.get(), stereo, silent, data.data(), 1, 8) == MYW_OK, "silent packet");
    require(myw_port_pop(p.get(), binding, out.data(), out.size(), &block) == MYW_OK && !out[0] && !out[7],
        "silent flag never forwards undefined WASAPI bytes");
    myw_port_revoke(p.get());
    require(myw_port_pop(p.get(), binding, out.data(), out.size(), &block) == MYW_STALE, "irreversible revoke");
    auto restart = port(stereo, {20, 7});
    require(myw_port_push(restart.get(), stereo, stamp(0, 0), data.data(), 1, 8) == MYW_STALE,
        "restart cannot replay old generation");
}
void check_spsc() {
    auto p = port();
    std::atomic<bool> good{true};
    std::atomic<uint64_t> acknowledged{0};
    std::thread producer([&] {
        std::array<uint8_t, 8> data{};
        // Consumer handshake keeps this test below queue capacity; FULL is
        // terminal in production, so this does not spin-retry a rejected push.
        for (uint64_t i = 0; i < 10000; ++i) {
            data[0] = static_cast<uint8_t>(i);
            // A separate handshake below owns scheduling; production queue is still exercised.
            if (myw_port_push(p.get(), stereo, stamp(i, i), data.data(), 1, data.size()) != MYW_OK) {
                good.store(false); return;
            }
            // Use a bounded external acknowledgement to avoid manufacturing overload.
            while (myw_port_state(p.get()) == MYW_OK && acknowledged.load(std::memory_order_acquire) <= i)
                std::this_thread::yield();
        }
    });
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> out{};
    for (uint64_t i = 0; i < 10000 && good.load(); ++i) {
        myw_block block{};
        int result;
        do { result = myw_port_pop(p.get(), binding, out.data(), out.size(), &block); } while (result == MYW_EMPTY);
        if (result != MYW_OK || block.stamp.sequence != i || out[0] != static_cast<uint8_t>(i)) {
            good.store(false); myw_port_revoke(p.get()); break;
        }
        acknowledged.store(i + 1, std::memory_order_release);
    }
    producer.join();
    require(good.load(), "real SPSC ordering and byte visibility");
}
void check_ingress() {
    using namespace mixyard_producer;
    std::array<uint8_t, 256 * 8> bytes{};
    const uint32_t samples[]{0x80000000u, 0x3f800001u, 0xbf000001u, 0x00000001u};
    std::memcpy(bytes.data(), samples, sizeof(samples));
    myw_block block{stamp(0, 0), 256};
    PcmEncoder encoder(stereo, binding, 31, 42, 98);
    PcmEncoder::Packet packet{};
    size_t length = 0;
    require(encoder.encode(stereo, block, bytes.data(), bytes.size(), 0, 128, packet, length),
        "real PCM bridge to existing driver ABI");
    require(length == mixyard_input::max_packet_bytes &&
        !std::memcmp(packet.data() + mixyard_input::header_bytes, bytes.data(), 128 * 8),
        "driver packet preserves signed zero/low bits/channel order");
    require(mixyard_input::load64(packet.data() + 16) == 42 &&
        mixyard_input::load64(packet.data() + 24) == 98 && mixyard_input::load64(packet.data() + 32) == 7,
        "driver epoch/generation and core revision remain distinct");
    require(encoder.encode(stereo, block, bytes.data(), bytes.size(), 128, 128, packet, length),
        "split second portion is contiguous");
    require(!encoder.encode(stereo, block, bytes.data(), bytes.size(), 0, 128, packet, length) &&
        !encoder.active(), "producer replay closes admission");
    auto wide = stereo; wide.container_bits = wide.valid_bits = 64;
    PcmEncoder unsupported(wide, binding, 31, 42, 98);
    require(!unsupported.active(), "limited driver rejects f64 without narrowing source route");
    PcmEncoder nonfinite(stereo, binding, 31, 42, 98);
    bytes[2] = 0x80; bytes[3] = 0x7f; // +infinity, prohibited by the existing ingress ABI.
    require(!nonfinite.encode(stereo, block, bytes.data(), bytes.size(), 0, 128, packet, length) && !length,
        "driver finite-only constraint fails closed with cleared packet");
}
void check_render() {
    auto p = port();
    mixyard_worker::RenderCursor cursor;
    const uint32_t words[]{0x3f800001u, 0x80000000u, 0xbf000001u, 0x00000001u};
    require(myw_port_push(p.get(), stereo, stamp(0, 0), words, 2, sizeof(words)) == MYW_OK, "render input");
    std::array<uint32_t, 6> out{};
    uint64_t missing = 0;
    require(cursor.fill(p.get(), binding, 8, reinterpret_cast<uint8_t*>(out.data()), 1, missing) == MYW_OK &&
        out[0] == words[0] && out[1] == words[1] && !missing, "partial render retains low bits/signed zero");
    out.fill(UINT32_MAX);
    require(cursor.fill(p.get(), binding, 8, reinterpret_cast<uint8_t*>(out.data()), 3, missing) == MYW_OK &&
        out[0] == words[2] && out[1] == words[3] && !out[2] && !out[5] && missing == 2,
        "remaining packet then initialized underflow silence with exact count");
    p = port();
    mixyard_worker::RenderCursor revoked;
    require(myw_port_push(p.get(), stereo, stamp(0, 0), words, 2, sizeof(words)) == MYW_OK, "revoked render input");
    require(revoked.fill(p.get(), binding, 8, reinterpret_cast<uint8_t*>(out.data()), 1, missing) == MYW_OK,
        "stage partial packet");
    myw_port_revoke(p.get());
    out.fill(UINT32_MAX);
    require(revoked.fill(p.get(), binding, 8, reinterpret_cast<uint8_t*>(out.data()), 1, missing) == MYW_STALE &&
        !out[0] && !out[1], "revoke silences even an already staged partial packet");
}
}
int main() {
    try {
        check_queue(); check_spsc(); check_ingress(); check_render();
        std::cout << "PASS exact PCM/SPSC/revoke/independent outputs and driver serialization; no native audio proof\n";
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}

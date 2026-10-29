// SPDX-License-Identifier: MIT
// Inspect actual production storage in the same translation unit. No substitute
// queue or debug-only clearing implementation can make this white-box check pass.
// Included by worker_queue.cpp only for MIXYARD_WINDOWS_SCRUB_CHECK builds.
#include "../worker_render.hpp"
#include <thread>
#include <memory>
#include <iostream>
#include <stdexcept>

namespace {
void require(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
void run() {
    const myw_format format{48000, 3, 2, 64, 64, 1};
    myw_port* raw = nullptr;
    require(myw_port_create(format, {17, 7}, 91, &raw) == MYW_OK, "actual queue setup");
    const auto cleanup = [](myw_port* p) { myw_port_revoke(p); myw_port_destroy(p); };
    std::unique_ptr<myw_port, decltype(cleanup)> port(raw, cleanup);
    require(myw_port_scrub_quiescent(port.get()) == MYW_INVALID, "open port scrub must reject");
    const std::array<double, 4> samples{{0.5, -0.25, 0.125, -0.5}};
    int pushed = MYW_INVALID;
    std::thread producer([&] { pushed = myw_port_push(port.get(), format, {{17, 7}, 91, 0, 0, 0, 0, 0, 0}, samples.data(), 2, sizeof(samples)); });
    producer.join(); // Actual producer quiescence before any scrub.
    require(pushed == MYW_OK, "actual queue has nonzero retained PCM");
    mixyard_worker::RenderCursor cursor;
    std::array<uint8_t, 16> output{}; uint64_t missing = 0;
    int filled = MYW_INVALID;
    std::thread consumer([&] { filled = cursor.fill(port.get(), {17, 7}, 16, output.data(), 1, missing); });
    consumer.join(); // Actual consumer quiescence, with a retained second frame.
    require(filled == MYW_OK, "stage actual packet");
    require(cursor.offset == 1 && cursor.block.frames == 2 &&
            std::any_of(cursor.bytes.begin(), cursor.bytes.end(), [](uint8_t b) { return b != 0; }), "nonzero partial cursor before scrub");
    require(cursor.scrub_quiescent(port.get()) == MYW_INVALID, "open cursor scrub must reject");
    myw_port_revoke(port.get());
    require(myw_port_scrub_quiescent(port.get()) == MYW_OK && cursor.scrub_quiescent(port.get()) == MYW_OK,
        "closed and actually joined storage scrubs");
    for (const auto& packet : port->packets) {
        for (auto byte : packet.bytes) require(!byte, "every production slot PCM byte scrubbed");
        require(!packet.block.frames && !packet.block.stamp.binding.generation, "packet metadata scrubbed");
    }
    for (auto byte : cursor.bytes) require(!byte, "every partial cursor byte scrubbed");
    require(!cursor.offset && !cursor.block.frames && myw_port_state(port.get()) == MYW_STALE, "scrub never rearms closed SPSC");
}
}
int main() {
    try { run(); std::cout << "PASS actual joined producer/consumer queue and partial cursor storage scrub\n"; }
    catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}

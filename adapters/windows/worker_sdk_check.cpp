// SPDX-License-Identifier: MIT
// Real SDK declarations, injected operations ONLY. Never calls a device API.
#include "virtual-input/producer/stream.hpp"
#include <cstring>
#include <iostream>
#include <stdexcept>

namespace {
using namespace mixyard_producer;
mixyard_input::Transport driver{};
unsigned opened = 0, closed = 0, submitted = 0;
bool malformed_reply = false;
HANDLE fake_open() {
    ++opened;
    require(driver.begin(1, 99) == Result::accepted, "fake exclusive admission");
    return reinterpret_cast<HANDLE>(static_cast<uintptr_t>(1));
}
BOOL fake_close(HANDLE) { ++closed; (void)driver.cleanup(1); return TRUE; }
BOOL fake_ioctl(HANDLE, DWORD code, LPVOID input, DWORD input_bytes,
                 LPVOID output, DWORD output_bytes, LPDWORD returned, LPOVERLAPPED) {
    if (code == ioctl_capture_status && !input && !input_bytes && output_bytes == sizeof(CaptureStatus)) {
        CaptureStatus status{};
        status.version = control_version; status.bytes = sizeof(status);
        status.generation = driver.generation; status.producer_epoch = driver.epoch;
        status.next_frame = driver.next_frame; status.next_sequence = driver.next_sequence;
        status.overwritten_packets = driver.overwritten_packets;
        std::memcpy(output, &status, sizeof(status)); *returned = sizeof(status); return TRUE;
    }
    if (code == ioctl_submit && output_bytes == sizeof(SubmitReply)) {
        ++submitted;
        const auto result = driver.submit(1, input, input_bytes);
        SubmitReply reply{};
        reply.version = control_version; reply.bytes = sizeof(reply);
        reply.result = static_cast<uint32_t>(result);
        if (malformed_reply) reply.reserved = 1;
        std::memcpy(output, &reply, sizeof(reply)); *returned = sizeof(reply); return TRUE;
    }
    return FALSE;
}
void run() {
    driver.initialize();
    const DeviceOps ops{fake_open, fake_ioctl, fake_close};
    const myw_format exact{48000, 3, 2, 32, 32, 1};
    const myw_binding binding{23, 8};
    const myw_block block{{binding, 3, 0, 0, 0, 0, 0, 0}, 1};
    const uint32_t words[]{0x3f000001u, 0x80000000u};
    StreamProducer producer(ops);
    require(producer.open(exact, binding, 3), "open actual production producer with injected driver");
    require(producer.submit(exact, block, words, sizeof(words), 0, 1), "submit real source bytes");
    require(driver.packets[0].samples[0] == words[0] && driver.packets[0].samples[1] == words[1],
        "exact samples reached production IOCTL serialization");
    ++driver.epoch; // Driver restart/revocation while the host still has its handle.
    const auto calls = submitted;
    auto next = block; next.stamp.first_frame = 1; next.stamp.sequence = 1;
    require(!producer.submit(exact, next, words, sizeof(words), 0, 1) && submitted == calls && closed == 1,
        "changed driver epoch closes before sending stale PCM");
    require(!producer.open(exact, binding, 3), "closed producer cannot silently reconnect");
    StreamProducer failed(ops);
    require(failed.open(exact, binding, 3), "fresh producer object");
    malformed_reply = true;
    require(!failed.submit(exact, block, words, sizeof(words), 0, 1) && closed == 2,
        "malformed reply closes/revokes driver admission");
    const auto before = opened;
    StreamProducer unsupported(ops);
    auto wrong = exact; wrong.rate = 44100;
    require(!unsupported.open(wrong, binding, 3) && opened == before,
        "format rejection precedes even opening driver");
}
}
int main() {
    try { run(); std::cout << "PASS SDK injected streaming producer; no native device execution\n"; }
    catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}

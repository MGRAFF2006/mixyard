// SPDX-License-Identifier: MIT
#include "worker_bridge.hpp"
#include "input-broker/packet.hpp"
#ifdef __APPLE__
#include "native_worker.hpp"
#endif
#include <cassert>
#include <chrono>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <new>
#include <thread>

static thread_local bool inCallback = false;
void* operator new(std::size_t size) {
    assert(!inCallback && "Callback allocated");
    if (void* pointer = std::malloc(size ? size : 1)) return pointer;
    throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { std::free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { std::free(pointer); }
void* operator new(std::size_t size, std::align_val_t alignment) {
    assert(!inCallback && "Callback allocated aligned storage");
    void* pointer = nullptr;
    if (posix_memalign(&pointer, static_cast<size_t>(alignment), size ? size : 1) != 0) throw std::bad_alloc();
    return pointer;
}
void* operator new[](std::size_t size, std::align_val_t alignment) { return ::operator new(size, alignment); }
// Keep replacement deletes out of callers: GCC otherwise diagnoses their
// inlined free as mismatched with the matching replacement aligned new above.
[[gnu::noinline]] void operator delete(void* pointer, std::align_val_t) noexcept { std::free(pointer); }
[[gnu::noinline]] void operator delete[](void* pointer, std::align_val_t) noexcept { std::free(pointer); }
[[gnu::noinline]] void operator delete(void* pointer, std::size_t, std::align_val_t) noexcept { std::free(pointer); }
[[gnu::noinline]] void operator delete[](void* pointer, std::size_t, std::align_val_t) noexcept { std::free(pointer); }
struct Callback {
    Callback() { assert(!inCallback); inCallback = true; }
    ~Callback() { inCallback = false; }
};
namespace w = mixyard::worker;
static std::unique_ptr<w::Queue> queue(w::Precision precision = w::Precision::Float64) {
    return std::make_unique<w::Queue>(w::Format{352800, {"FL", "FR"}, precision}, w::Binding{7, 11}, 19,
                                      w::Provenance{w::Origin::Synthetic, "synthetic:selected-source"});
}
static w::Stamp stamp(uint64_t sequence = 0, uint64_t first = 0) {
    return {{7, 11}, 19, sequence, first, 100, 1000};
}
static bool push(w::Queue& q, w::Stamp s, uint32_t frames, const void* data, uint32_t bytes) {
    const void* pointers[]{data}; const uint32_t sizes[]{bytes};
    Callback callback;
    return q.push(101, s, frames, pointers, sizes, 1, false);
}
static void silent(w::Queue& q) {
    std::array<unsigned char, 128> pcm; pcm.fill(0x5a);
    w::Stamp s; uint32_t frames = 10;
    Callback callback;
    assert(!q.pop(102, q.binding(), pcm.data(), pcm.size(), s, frames));
    assert(frames == 0 && s.binding.generation == 0);
    for (const auto byte : pcm) assert(byte == 0);
}

static void exactPlanar() {
    w::Queue q({384000, {"CA:1", "CA:2", "CA:3", "CA:4"}, w::Precision::Float64}, {7, 11}, 19,
               {w::Origin::Synthetic, "four-channel-file"});
    const double a[]{1.0 + 0x1p-40, -0.0, 0x1p-1074};
    const double b[]{-1.0 - 0x1p-40, 0.0, -0x1p-1074};
    const double c[]{0.125, 0.25, 0.375}, d[]{-0.125, -0.25, -0.375};
    const void* buffers[]{a, b, c, d}; const uint32_t sizes[]{24, 24, 24, 24};
    std::array<double, 12> pcm{};
    w::Stamp out; uint32_t frames = 0;
    {
        Callback callback;
        assert(q.push(101, stamp(), 3, buffers, sizes, 4, true));
        assert(q.pop(102, {7, 11}, pcm.data(), sizeof(pcm), out, frames));
    }
    assert(frames == 3 && out.clockId == 19 && out.binding.revision == 11);
    const double expected[]{a[0], b[0], c[0], d[0], a[1], b[1], c[1], d[1], a[2], b[2], c[2], d[2]};
    assert(std::memcmp(pcm.data(), expected, sizeof(expected)) == 0);
    assert(q.format().sampleRateHz == 384000 && q.format().channels[2] == "CA:3");
    silent(q); assert(q.fault() == w::Fault::None); // Empty pop never replays.
}

static void renderPeriods() {
    auto q = queue(w::Precision::Float32);
    const float first[]{0.25f, -0.25f, -0.0f, 0.0f, 0x1p-149f, -0x1p-149f};
    const float second[]{0.5f, -0.5f, 0.75f, -0.75f};
    assert(push(*q, stamp(0, 123), 3, first, sizeof(first)));
    assert(push(*q, stamp(1, 126), 2, second, sizeof(second)));
    w::Renderer render(*q);
    float output[8]{};
    {
        Callback callback;
        assert(render.render(102, 123, 1, output, 8));
        assert(std::memcmp(output, first, 8) == 0);
        assert(render.render(102, 124, 4, output, sizeof(output)));
    }
    const float expected[]{first[2], first[3], first[4], first[5], second[0], second[1], second[2], second[3]};
    assert(std::memcmp(output, expected, sizeof(expected)) == 0);
    // Missing destination frames are terminal, never delayed playback later.
    {
        Callback callback;
        assert(!render.render(102, 128, 4, output, sizeof(output)));
    }
    assert(q->fault() == w::Fault::Underflow);
    for (float value : output) assert(value == 0);
}

static void faultIsolation() {
    const double data[]{0.25, -0.25, 0.5, -0.5};
    auto sibling = queue();
    assert(push(*sibling, stamp(), 2, data, sizeof(data)));
    for (unsigned test = 0; test < 14; ++test) {
        auto q = queue();
        assert(push(*q, stamp(), 2, data, sizeof(data)));
        auto bad = stamp(1, 2);
        auto expected = w::Fault::Sequence;
        switch (test) {
            case 0: bad.binding.generation++; expected = w::Fault::Binding; break;
            case 1: bad.binding.revision++; expected = w::Fault::Binding; break;
            case 2: bad.clockId++; expected = w::Fault::Clock; break;
            case 3: bad.sequence = 0; break;
            case 4: bad.firstFrame++; break;
            case 5: bad.sequence = UINT64_MAX; break;
            case 6: bad.firstFrame = UINT64_MAX; break;
            case 7: bad.deadlineNs = 100; expected = w::Fault::Expired; break;
            case 8: bad.capturedNs = 102; expected = w::Fault::Expired; break;
            case 9: bad.capturedNs = 99; break;
            case 10: bad.deadlineNs = 99; expected = w::Fault::Expired; break;
            case 11: expected = w::Fault::Shape; break;
            case 12: expected = w::Fault::Nonfinite; break;
            case 13: expected = w::Fault::Shape; break;
        }
        const double nonfinite[]{std::numeric_limits<double>::infinity(), 0, 0, 0};
        assert(!push(*q, bad, test == 13 ? w::maxFrames + 1 : 2,
                     test == 12 ? nonfinite : data, test == 11 ? sizeof(data) - 1 : sizeof(data)));
        assert(q->fault() == expected); silent(*q);
        assert(!push(*q, stamp(1, 2), 2, data, sizeof(data))); // Terminal, not resettable.
    }
    auto full = queue();
    for (uint32_t i = 0; i < w::slots; ++i) assert(push(*full, stamp(i, i * 2), 2, data, sizeof(data)));
    assert(!push(*full, stamp(w::slots, w::slots * 2), 2, data, sizeof(data)));
    assert(full->fault() == w::Fault::Overflow); silent(*full);
    auto revoked = queue();
    assert(push(*revoked, stamp(), 2, data, sizeof(data)));
    revoked->revoke(); silent(*revoked);
    auto expired = queue();
    assert(push(*expired, stamp(), 2, data, sizeof(data)));
    double result[4]; w::Stamp out; uint32_t frames = 4;
    assert(!expired->pop(1001, expired->binding(), result, sizeof(result), out, frames));
    assert(expired->fault() == w::Fault::Expired); silent(*expired);
    // Fresh generation cannot accept a previous process/session's valid packet.
    w::Queue restarted({352800, {"FL", "FR"}, w::Precision::Float64}, {8, 11}, 19,
                        {w::Origin::Synthetic, "restarted-source"});
    assert(!push(restarted, stamp(), 2, data, sizeof(data)));
    assert(restarted.fault() == w::Fault::Binding); silent(restarted);
    // An independent destination retains all of its original precision/data.
    assert(sibling->pop(102, sibling->binding(), result, sizeof(result), out, frames));
    assert(frames == 2 && std::memcmp(result, data, sizeof(data)) == 0);
}

static void expiryDuringPartialRender() {
    auto q = queue(); const double data[]{0.25, -0.25, 0.5, -0.5};
    assert(push(*q, stamp(), 2, data, sizeof(data)));
    auto second = stamp(1, 2); second.deadlineNs = 101;
    assert(push(*q, second, 2, data, sizeof(data)));
    w::Renderer render(*q); std::array<double, 8> pcm; pcm.fill(1);
    { Callback callback; assert(!render.render(102, 0, 4, pcm.data(), sizeof(pcm))); }
    assert(q->fault() == w::Fault::Expired);
    for (double sample : pcm) assert(sample == 0); // No valid prefix leaks out.
    auto wrongClock = queue(); assert(push(*wrongClock, stamp(0, 7), 2, data, sizeof(data)));
    w::Renderer mismatch(*wrongClock);
    assert(!mismatch.render(102, 8, 2, pcm.data(), sizeof(data)));
    assert(wrongClock->fault() == w::Fault::Clock);
}

static void descriptorAndStorage() {
    for (auto channels : {std::vector<std::string>{}, std::vector<std::string>{"FL", "FL"},
                           std::vector<std::string>{""}}) {
        bool rejected = false;
        try { w::Queue invalid({48000, channels, w::Precision::Float64}, {1, 1}, 1, {w::Origin::Synthetic, "fixture"}); }
        catch (const std::invalid_argument&) { rejected = true; }
        assert(rejected);
    }
    auto q = queue();
    const double data[]{0.25, -0.25};
    assert(push(*q, stamp(), 1, data, sizeof(data)));
    double destination = 1; w::Stamp out; uint32_t frames;
    assert(!q->pop(102, q->binding(), &destination, sizeof(destination), out, frames));
    assert(destination == 0 && q->fault() == w::Fault::Shape);
    // Channel-rich blocks reject byte-budget overflow rather than truncating.
    std::vector<std::string> channels;
    for (unsigned i = 1; i <= 32; ++i) channels.push_back("CA:" + std::to_string(i));
    w::Queue wide({44100, channels, w::Precision::Float64}, {7, 11}, 19, {w::Origin::Synthetic, "wide"});
    assert(!push(wide, stamp(), w::maxFrames, data, sizeof(data)));
    assert(wide.fault() == w::Fault::Shape);
    auto backwards = queue();
    assert(push(*backwards, stamp(), 1, data, sizeof(data)));
    const void* pointers[]{data}; const uint32_t sizes[]{sizeof(data)};
    assert(!backwards->push(100, stamp(1, 1), 1, pointers, sizes, 1, false));
    assert(backwards->fault() == w::Fault::Clock); silent(*backwards);
    auto consumerClock = queue();
    assert(consumerClock->pop(103, consumerClock->binding(), &destination, sizeof(destination), out, frames) == false);
    assert(!consumerClock->pop(102, consumerClock->binding(), &destination, sizeof(destination), out, frames));
    assert(consumerClock->fault() == w::Fault::Clock && destination == 0);
}

static void concurrentFifo() {
    auto q = queue(); constexpr uint32_t count = 20000;
    std::thread reader([&] {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        for (uint32_t i = 0; i < count;) {
            double pcm[2]{1, 1}; w::Stamp out; uint32_t frames = 0;
            bool received;
            { Callback callback; received = q->pop(102, {7, 11}, pcm, sizeof(pcm), out, frames); }
            if (received) {
                assert(frames == 1 && out.sequence == i && out.firstFrame == i);
                assert(pcm[0] == i + 0x1p-32 && pcm[1] == -double(i)); ++i;
            } else {
                assert(q->fault() == w::Fault::None && pcm[0] == 0 && pcm[1] == 0);
                assert(std::chrono::steady_clock::now() < deadline); std::this_thread::yield();
            }
        }
    });
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    for (uint32_t i = 0; i < count; ++i) {
        while (!q->writable()) {
            assert(q->fault() == w::Fault::None && std::chrono::steady_clock::now() < deadline);
            std::this_thread::yield(); // Fixture scheduler only, never a native callback retry.
        }
        const double pcm[]{i + 0x1p-32, -double(i)};
        assert(push(*q, stamp(i, i), 1, pcm, sizeof(pcm)));
    }
    reader.join(); silent(*q); assert(q->fault() == w::Fault::None);
}

static void brokerAdmission() {
    mixyard::Header h; h.generation = 7; h.epoch = 19; h.revision = 11; h.position = 8; h.frames = 2; h.bytes = 16;
    const auto bytes = sizeof(h) + h.bytes;
    assert(mixyard::packetShape(h, bytes));
    for (unsigned test = 0; test < 12; ++test) {
        auto bad = h;
        switch (test) {
            case 0: bad.version++; break;
            case 1: bad.sampleRate = 44100; break;
            case 2: bad.channels++; break;
            case 3: bad.format++; break;
            case 4: bad.frames = 0; break;
            case 5: bad.frames = UINT32_MAX; break;
            case 6: bad.bytes++; break;
            case 7: bad.generation = 0; break;
            case 8: bad.epoch = 0; break;
            case 9: bad.revision = 0; break;
            case 10: bad.position = mixyard::maxPosition; break;
            case 11: bad.position = UINT64_MAX; break;
        }
        assert(!mixyard::packetShape(bad, bytes));
    }
    assert(!mixyard::packetShape(h, bytes - 1) && !mixyard::packetShape(h, SIZE_MAX));
    using A = mixyard::PacketAdmission;
    assert(mixyard::packetAdmission(h, 7, 19, 8, 11, 2) == A::Accepted);
    assert(mixyard::packetAdmission(h, 7, 19, 8, 11, 1) == A::Backpressure);
    assert(mixyard::packetAdmission(h, 8, 19, 8, 11, 2) == A::Stale);
    assert(mixyard::packetAdmission(h, 7, 20, 8, 11, 2) == A::Stale);
    assert(mixyard::packetAdmission(h, 7, 19, 9, 11, 2) == A::Stale);
    assert(mixyard::packetAdmission(h, 7, 19, 8, 12, 2) == A::Stale);
}

#ifdef __APPLE__
static void appleAbiOnly() {
    auto q = queue();
    AudioStreamBasicDescription actual{};
    actual.mFormatID = kAudioFormatLinearPCM;
    actual.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
    actual.mSampleRate = 352800; actual.mChannelsPerFrame = 2; actual.mBitsPerChannel = 64;
    actual.mBytesPerFrame = actual.mBytesPerPacket = 8; actual.mFramesPerPacket = 1;
    assert(w::exactFloat(actual, q->format()));
    auto bad = actual; bad.mBytesPerFrame = 4; assert(!w::exactFloat(bad, q->format()));
    w::CaptureIngress ingress(*q, actual, 1000000000);
    double left[]{1 + 0x1p-40, -0.0}, right[]{-1 - 0x1p-40, 0x1p-1074};
    struct { UInt32 count; AudioBuffer buffers[2]; } native{2, {{1, sizeof(left), left}, {1, sizeof(right), right}}};
    static_assert(offsetof(decltype(native), buffers) == offsetof(AudioBufferList, mBuffers));
    AudioTimeStamp now{}; now.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    now.mSampleTime = 17; now.mHostTime = mach_absolute_time();
    { Callback callback; assert(ingress.receive(&now, reinterpret_cast<AudioBufferList*>(&native), &now)); }
    w::HostClock clock; double pcm[4]; w::Stamp out; uint32_t frames;
    assert(q->pop(clock.now(), q->binding(), pcm, sizeof(pcm), out, frames));
    const double expected[]{left[0], right[0], left[1], right[1]};
    assert(frames == 2 && out.firstFrame == 17 && std::memcmp(pcm, expected, sizeof(pcm)) == 0);
    // No devices, default endpoints, permissions or IOProc are touched by this
    // malformed/synthetic open. All real output behavior remains unexecuted.
    bool rejected = false;
    try { auto output = w::OutputWorker::open("", *q); }
    catch (const std::invalid_argument&) { rejected = true; }
    assert(rejected);
    std::cout << "PASS: Apple ABI + synthetic ingress/timebase only; HAL/TCC/output BLOCKED\n";
}
#endif

int main() {
    exactPlanar(); renderPeriods(); faultIsolation(); expiryDuringPartialRender();
    descriptorAndStorage(); concurrentFifo(); brokerAdmission();
#ifdef __APPLE__
    appleAbiOnly();
#endif
    std::cout << "PASS: exact PCM/SPSC/lifetime/render/broker fixtures; backendAvailable=false audioApplied=false\n";
}

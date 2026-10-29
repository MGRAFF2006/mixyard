// SPDX-License-Identifier: MIT
#include "capture.hpp"
#include "../output-only/output_only.hpp"
#include "sdk_doubles.hpp"
#include <cstdlib>
#include <fstream>
#include <iostream>
#include <new>
#include <sstream>

void* operator new(std::size_t bytes) {
    assert(!sdk::callback && "Native callback allocated");
    if (auto* p = std::malloc(bytes ? bytes : 1)) return p;
    throw std::bad_alloc();
}
void* operator new[](std::size_t n) { return ::operator new(n); }
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t) noexcept { std::free(p); }
namespace m = mixyard::macos;
namespace w = mixyard::worker;
namespace out = mixyard::output;
namespace {
template<class F> void rejects(F function) {
    bool rejected = false; try { function(); } catch (const std::exception&) { rejected = true; }
    assert(rejected);
}
m::Selection selection() { return {101, 4242, true, {999}}; }
m::Timeline timeline() { return m::Timeline::injected(11); }
void sourceLifecycle() {
    sdk::reset();
    {
        auto capture = m::Capture::open(selection(), {7, 1}, timeline());
        capture->start(); assert(capture->close()); assert(capture->close());
        rejects([&] { capture->start(); });
    }
    const auto operations = sdk::calls;
    for (unsigned n = 1; n <= operations; ++n) {
        sdk::reset(); sdk::failAt = n;
        try {
            auto capture = m::Capture::open(selection(), {7, 1}, timeline());
            capture->start();
            if (!capture->close()) { sdk::failAt = 0; assert(capture->close()); }
        } catch (const std::exception&) {}
        assert(sdk::hits == 1); sdk::clean();
    }
    sdk::reset();
    {
        auto capture = m::Capture::open(selection(), {7, 1}, timeline()); capture->start();
        sdk::capture.stopFailures = 2;
        assert(!capture->revoke() && sdk::capture.running);
        assert(!capture->close() && sdk::capture.io && sdk::capture.tap);
        assert(capture->close());
    }
    sdk::reset();
    {
        auto s = selection(); s.consentToStereoTap = false;
        rejects([&] { m::Capture::open(s, {7, 1}, timeline()); }); assert(sdk::calls == 0);
        s = selection(); s.excludedProcesses.push_back(s.process);
        rejects([&] { m::Capture::open(s, {7, 1}, timeline()); }); assert(sdk::calls == 0);
        rejects([] { m::Timeline::physical(11); });
    }
    sdk::reset();
    {
        auto capture = m::Capture::open(selection(), {7, 1}, timeline()); capture->start();
        const auto late = Block_copy(sdk::captureNotices[0].block);
        const auto control = sdk::captureNotices[0].queue; dispatch_retain(control);
        dispatch_sync(control, ^{ late(0, nullptr); });
        assert(capture->queue().fault() == w::Fault::Revoked && !sdk::capture.running);
        assert(capture->close()); capture.reset();
        dispatch_sync(control, ^{ late(0, nullptr); });
        Block_release(late); dispatch_release(control);
    }
    sdk::clean();
}
NSMutableDictionary* fixture(const char* path, const m::Capture& capture, w::Precision precision) {
    NSData* data = [NSData dataWithContentsOfFile:[NSString stringWithUTF8String:path]];
    if (!data) throw std::runtime_error("Shared real-host fixture missing");
    auto* request = [NSJSONSerialization JSONObjectWithData:data options:NSJSONReadingMutableContainers error:nullptr];
    if (![request isKindOfClass:[NSMutableDictionary class]]) throw std::runtime_error("Invalid shared fixture");
    NSString* uid = [NSString stringWithUTF8String:capture.observedId().c_str()];
    request[@"inventory"][@"platform"] = @"macos";
    request[@"inventory"][@"evidence"] = @"simulated";
    request[@"inventory"][@"sources"][2][@"observedId"] = uid;
    request[@"sources"][2][@"observedId"] = uid;
    request[@"sources"][2][@"encoding"] = precision == w::Precision::Float64 ? @"f64-le" : @"f32-le";
    for (unsigned i = 0; i < 2; ++i) {
        NSString* output = i ? @"injected-output-b" : @"injected-output-a";
        NSDictionary* target = @{@"kind": @"device", @"deviceUid": output};
        request[@"config"][@"outputs"][i][@"target"] = target;
        request[@"inventory"][@"outputs"][i][@"target"] = target;
        request[@"inventory"][@"outputs"][i][@"observedId"] = output;
        request[@"outputs"][i][@"observedId"] = output;
        request[@"outputs"][i][@"channels"] = @[@"CA:1", @"CA:2"];
    }
    return request;
}
std::string serialize(NSDictionary* object) {
    NSData* bytes = [NSJSONSerialization dataWithJSONObject:object options:0 error:nullptr];
    if (!bytes) throw std::runtime_error("Fixture serialization failed");
    return std::string(static_cast<const char*>(bytes.bytes), bytes.length);
}
std::unique_ptr<w::Queue> queue(std::string id, w::Origin origin, w::Precision precision, bool output = false) {
    return std::make_unique<w::Queue>(w::Format{48000, output ? std::vector<std::string>{"CA:1", "CA:2"} : std::vector<std::string>{"FL", "FR"}, precision},
        w::Binding{7, 1}, 11, w::Provenance{origin, std::move(id)});
}
void push(w::Queue& q, uint64_t ticks, uint64_t sequence, uint64_t first, const double* pcm) {
    w::HostClock clock; const auto now = clock.nanos(ticks);
    const void* lanes[]{pcm}; const uint32_t sizes[]{32};
    assert(q.push(now, {{7, 1}, 11, sequence, first, now, now + 1000000000}, 2, lanes, sizes, 1, false));
}
void receive(uint64_t ticks, uint64_t first, void* pcm, w::Precision precision, bool malformed = false) {
    AudioTimeStamp time{}; time.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    time.mHostTime = ticks; time.mSampleTime = static_cast<double>(first);
    AudioBufferList buffers{1, {{2, static_cast<UInt32>(precision) * 4 - (malformed ? 1u : 0u), pcm}}};
    sdk::callback = true;
    assert(sdk::capture.proc(103, &time, &buffers, &time, nullptr, nullptr, sdk::capture.context) == noErr);
    sdk::callback = false;
}
void render(uint64_t ticks, uint64_t first, const double* expected, w::Precision precision, bool silence = false) {
    AudioTimeStamp time{}; time.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    time.mHostTime = ticks; time.mSampleTime = static_cast<double>(first);
    alignas(8) std::array<unsigned char, 32> pcm{}; pcm.fill(0x55);
    AudioBufferList buffers{1, {{2, static_cast<UInt32>(precision) * 4, pcm.data()}}};
    AudioUnitRenderActionFlags flags = 0;
    sdk::callback = true;
    assert(sdk::output.render.inputProc(sdk::output.render.inputProcRefCon, &flags, &time, 0, 2, &buffers) == noErr);
    sdk::callback = false;
    assert(bool(flags & kAudioUnitRenderAction_OutputIsSilence) == silence);
    for (unsigned i = 0; i < 4; ++i) {
        if (precision == w::Precision::Float64) {
            double value; std::memcpy(&value, pcm.data() + i * 8, 8); assert(value == (silence ? 0 : expected[i]));
        } else {
            float value; std::memcpy(&value, pcm.data() + i * 4, 4); assert(value == (silence ? 0 : static_cast<float>(expected[i])));
        }
    }
}
void linkedGraph(const char* path, w::Precision sourcePrecision, w::Precision sinkPrecision) {
    sdk::reset(sourcePrecision);
    auto capture = m::Capture::open(selection(), {7, 1}, timeline());
    auto privateQ = queue("owned-source-private", w::Origin::Synthetic, w::Precision::Float64);
    auto returnQ = queue("owned-source-return", w::Origin::Synthetic, w::Precision::Float64);
    auto callQ = queue("injected-output-a", w::Origin::AdaptedEngine, w::Precision::Float64, true);
    auto recordQ = queue("injected-output-b", w::Origin::AdaptedEngine, sinkPrecision, true);
    const auto ports = std::vector<m::Port>{{privateQ.get(), "owned-source-private", {"FL", "FR"}},
        {returnQ.get(), "owned-source-return", {"FL", "FR"}}, {&capture->queue(), capture->observedId(), {"FL", "FR"}}};
    const auto outputs = std::vector<m::Port>{{callQ.get(), "injected-output-a", {"FL", "FR"}},
        {recordQ.get(), "injected-output-b", {"FL", "FR"}}};
    auto* request = fixture(path, *capture, sourcePrecision);
    auto link = m::HostLink::create(serialize(request), timeline(), ports, outputs);
    auto bad = ports; bad[2].observedId = "invented-process-identity";
    rejects([&] { m::HostLink::create(serialize(request), timeline(), bad, outputs); });
    auto call = out::Worker::open("injected-output-a", *callQ);
    capture->start(); call->start();
    const double privatePcm[]{0.875, -0.875, 0.5, -0.5};
    const double returned[]{0.5, -0.5, -0.25, 0.25};
    const double precise[]{0.25 + 0x1p-45, -0.25 - 0x1p-45, 0.125 + 0x1p-46, -0.125 - 0x1p-46};
    float narrowed[4]{}; double shared[4]{}, recorded[4]{};
    for (unsigned i = 0; i < 4; ++i) {
        narrowed[i] = static_cast<float>(precise[i]);
        shared[i] = sourcePrecision == w::Precision::Float64 ? precise[i] : narrowed[i];
        recorded[i] = 0.25 * shared[i] + 0.125 * returned[i];
    }
    const auto ticks = mach_absolute_time(); w::HostClock clock; const auto now = clock.nanos(ticks);
    push(*privateQ, ticks, 0, 0, privatePcm); push(*returnQ, ticks, 0, 0, returned);
    receive(ticks, 0, sourcePrecision == w::Precision::Float64 ? static_cast<void*>(const_cast<double*>(precise)) : narrowed, sourcePrecision);
    assert(link->step(now, {7, 1, 11, 0, 0, now, now + 1000000000}, 2));
    render(ticks, 0, shared, w::Precision::Float64);
    assert(call->close()); call.reset(); // Closing one consumer preserves sibling PCM.
    sdk::output.uid = "injected-output-b"; sdk::output.pcm = sdk::format(sinkPrecision);
    auto record = out::Worker::open("injected-output-b", *recordQ); record->start();
    render(ticks, 0, recorded, sinkPrecision);
    const auto later = mach_absolute_time(); const auto laterNs = clock.nanos(later);
    push(*privateQ, later, 1, 2, privatePcm); push(*returnQ, later, 1, 2, returned);
    receive(later, 2, narrowed, w::Precision::Float32, true); // Terminal malformed source, no stale replay.
    assert(capture->queue().fault() != w::Fault::None);
    assert(link->step(laterNs, {7, 1, 11, 1, 2, laterNs, laterNs + 1000000000}, 2));
    for (unsigned i = 0; i < 4; ++i) recorded[i] = 0.125 * returned[i];
    render(later, 2, recorded, sinkPrecision);
    link->cancel(); assert(!link->step(laterNs, {7, 1, 11, 2, 4, laterNs, laterNs + 1000000000}, 2));
    render(later, 4, recorded, sinkPrecision, true);
    rejects([&] { capture->start(); });
    assert(capture->close()); assert(record->close());
    link.reset(); sdk::clean();
}
} // namespace
int main(int argc, char** argv) {
    assert(argc == 2); sdk::install();
    @autoreleasepool {
        sourceLifecycle();
        for (auto source : {w::Precision::Float32, w::Precision::Float64})
            for (auto sink : {w::Precision::Float32, w::Precision::Float64}) linkedGraph(argv[1], source, sink);
    }
    std::cout << "Actual Rust host + production tap ingress/output-only callback: scoped mixing, output DSP, exact source precision, explicit sink conversion, failure cleanup and isolation passed. Injected SDK clocks/identities only; no native audio.\n";
}

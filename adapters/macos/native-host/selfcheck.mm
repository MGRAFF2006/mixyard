// SPDX-License-Identifier: MIT
#include "capture.hpp"
#include "native_output.hpp"
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
static unsigned scrubbedQueues = 0;
namespace mixyard::macos::detail {
void queueScrubbed(const void* storage, size_t bytes) noexcept {
    assert(!sdk::callback);
    const auto* data = static_cast<const unsigned char*>(storage);
    for (size_t i = 0; i < bytes; ++i) assert(data[i] == 0);
    ++scrubbedQueues;
}
}
namespace m = mixyard::macos;
namespace w = mixyard::worker;
namespace {
template<class F> void rejects(F function) {
    bool rejected = false; try { function(); } catch (const std::exception&) { rejected = true; }
    assert(rejected);
}
m::Selection selection() { return {101, sdk::capture.pid, true, {999}}; }
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
m::OwnedQueue queue(std::string id, w::Origin origin, w::Precision precision, bool output = false) {
    return m::ownQueue(w::Format{48000, output ? std::vector<std::string>{"CA:1", "CA:2"} : std::vector<std::string>{"FL", "FR"}, precision},
        w::Binding{7, 1}, 11, w::Provenance{origin, std::move(id)});
}
void push(w::Queue& q, uint64_t ticks, uint64_t sequence, uint64_t first, const double* pcm) {
    w::HostClock clock; const auto now = clock.nanos(ticks);
    const void* lanes[]{pcm}; const uint32_t sizes[]{32};
    assert(q.push(now, {{7, 1}, 11, sequence, first, now, now + 1000000000}, 2, lanes, sizes, 1, false));
}
void receive(uint64_t ticks, uint64_t first, void* pcm, w::Precision precision, bool malformed = false, bool expired = false) {
    AudioTimeStamp time{}; time.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    time.mHostTime = ticks; time.mSampleTime = static_cast<double>(first);
    AudioBufferList buffers{1, {{2, static_cast<UInt32>(precision) * 4 - (malformed ? 1u : 0u), pcm}}};
    auto inputTime = time;
    if (expired) inputTime.mHostTime = 1; // Deliberately stale injected capture time.
    sdk::callback = true;
    assert(sdk::capture.proc(103, &time, &buffers, &inputTime, nullptr, nullptr, sdk::capture.context) == noErr);
    sdk::callback = false;
}
void render(uint64_t ticks, uint64_t first, const double* expected, w::Precision precision, bool silence = false, UInt32 frames = 2) {
    AudioTimeStamp time{}; time.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    // The real production callback rejects a schedule earlier than its current
    // monotonic clock. SDK fixtures schedule a future output slot, within the
    // admitted one-second PCM deadline; the earlier capture tick is not due time.
    mach_timebase_info_data_t basis{};
    assert(mach_timebase_info(&basis) == KERN_SUCCESS && basis.numer && basis.denom);
    const uint64_t lead = uint64_t(100000000) * basis.denom / basis.numer;
    const uint64_t current = mach_absolute_time();
    assert(lead && current <= UINT64_MAX - lead);
    time.mHostTime = std::max(ticks, current + lead);
    time.mSampleTime = static_cast<double>(first);
    alignas(8) std::array<unsigned char, 32> pcm{}; pcm.fill(0x55);
    AudioBufferList buffers{1, {{2, static_cast<UInt32>(precision) * 2 * frames, pcm.data()}}};
    AudioUnitRenderActionFlags flags = 0;
    sdk::callback = true;
    assert(sdk::output.render.inputProc(sdk::output.render.inputProcRefCon, &flags, &time, 0, frames, &buffers) == noErr);
    sdk::callback = false;
    assert(bool(flags & kAudioUnitRenderAction_OutputIsSilence) == silence);
    for (unsigned i = 0; i < 2 * frames; ++i) {
        if (precision == w::Precision::Float64) {
            double value; std::memcpy(&value, pcm.data() + i * 8, 8); assert(value == (silence ? 0 : expected[i]));
        } else {
            float value; std::memcpy(&value, pcm.data() + i * 4, 4); assert(value == (silence ? 0 : static_cast<float>(expected[i])));
        }
    }
}
m::StepResult step(m::HostLink& link, uint64_t now, mx_stamp stamp) {
    sdk::callback = true;
    const auto result = link.step(now, stamp, 2);
    sdk::callback = false;
    return result;
}
void linkedGraph(const char* path, w::Precision sourcePrecision, w::Precision sinkPrecision, bool staleSource, bool partialReader) {
    sdk::reset(sourcePrecision);
    std::vector<std::unique_ptr<m::Capture>> captures;
    captures.push_back(m::Capture::open(selection(), {7, 1}, timeline()));
    auto* capture = captures[0].get();
    std::vector<m::OwnedQueue> otherSources;
    otherSources.push_back(queue("owned-source-private", w::Origin::Synthetic, w::Precision::Float64));
    otherSources.push_back(queue("owned-source-return", w::Origin::Synthetic, w::Precision::Float64));
    auto* privateQ = otherSources[0].get(); auto* returnQ = otherSources[1].get();
    std::vector<std::unique_ptr<m::NativeOutput>> consumers;
    consumers.push_back(m::NativeOutput::prepare("injected-output-a", {48000, {"CA:1", "CA:2"}, w::Precision::Float64}, {7, 1}, timeline()));
    consumers.push_back(m::NativeOutput::prepare("injected-output-b", {48000, {"CA:1", "CA:2"}, sinkPrecision}, {7, 1}, timeline()));
    auto* call = consumers[0].get(); auto* record = consumers[1].get();
    auto* callQ = &call->queue(); auto* recordQ = &record->queue();
    const auto ports = std::vector<m::Port>{{privateQ, "owned-source-private", {"FL", "FR"}},
        {returnQ, "owned-source-return", {"FL", "FR"}}, {&capture->queue(), capture->observedId(), {"FL", "FR"}}};
    const auto outputs = std::vector<m::Port>{{callQ, "injected-output-a", {"FL", "FR"}},
        {recordQ, "injected-output-b", {"FL", "FR"}}};
    auto* request = fixture(path, *capture, sourcePrecision);
    auto link = m::HostLink::create(serialize(request), timeline(), ports, outputs);
    auto bad = ports; bad[2].observedId = "invented-process-identity";
    rejects([&] { m::HostLink::create(serialize(request), timeline(), bad, outputs); });
    call->open();
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
    assert(step(*link, now, {7, 1, 11, 0, 0, now, now + 1000000000}) == m::StepResult::Processed);
    render(ticks, 0, shared, w::Precision::Float64);
    const auto notice = sdk::outputNotices[0];
    dispatch_sync(notice.queue, ^{ notice.block(0, nullptr); });
    assert(callQ->fault() == w::Fault::Revoked && !sdk::output.running);
    assert(recordQ->fault() == w::Fault::None && capture->queue().fault() == w::Fault::None);
    assert(call->close()); // Closing one consumer preserves sibling PCM.
    sdk::output.uid = "injected-output-b"; sdk::output.pcm = sdk::format(sinkPrecision);
    record->open(); record->start();
    const auto* queued = recordQ->front(now); assert(queued);
    // Destination DSP produced nonzero PCM before the selected source fails.
    if (sinkPrecision == w::Precision::Float64) {
        double first; std::memcpy(&first, queued->pcm.data(), 8); assert(first == recorded[0]);
    } else {
        float first; std::memcpy(&first, queued->pcm.data(), 4); assert(first == static_cast<float>(recorded[0]));
    }
    if (partialReader) render(ticks, 0, recorded, sinkPrecision, false, 1);
    const auto later = mach_absolute_time(); const auto laterNs = clock.nanos(later);
    push(*privateQ, later, 1, 2, privatePcm); push(*returnQ, later, 1, 2, returned);
    receive(later, 2, sourcePrecision == w::Precision::Float64 ? static_cast<void*>(const_cast<double*>(precise)) : narrowed,
            sourcePrecision, !staleSource, staleSource);
    assert(capture->queue().fault() != w::Fault::None);
    assert(capture->queue().fault() == (staleSource ? w::Fault::Expired : w::Fault::Shape));
    assert(step(*link, laterNs, {7, 1, 11, 1, 2, laterNs, laterNs + 1000000000}) == m::StepResult::SourceLoss);
    assert(privateQ->fault() == w::Fault::Revoked && returnQ->fault() == w::Fault::Revoked);
    assert(recordQ->fault() == w::Fault::Revoked && sdk::output.running);
    const UInt32 remaining = partialReader ? 1 : 2;
    render(later, partialReader ? 1 : 0, recorded, sinkPrecision, true, remaining);
    assert(step(*link, laterNs, {7, 1, 11, 2, 4, laterNs, laterNs + 1000000000}) == m::StepResult::SourceLoss);
    rejects([&] { capture->start(); });
    // A source_loss notification / atomic queue revoke is not completion. Both
    // native stop failures retain the actual callback/cursor owners for retry.
    sdk::capture.stopFailures = 1; sdk::output.stopFailures = 1;
    const auto scrubbedBefore = scrubbedQueues;
    assert(m::finishGeneration(link, captures, consumers, otherSources) == m::FinishResult::Pending);
    assert(scrubbedQueues == scrubbedBefore);
    assert(!link && captures.size() == 1 && consumers.size() == 2 && otherSources.size() == 2);
    assert(sdk::capture.running && sdk::output.running);
    render(mach_absolute_time(), partialReader ? 1 : 0, recorded, sinkPrecision, true, remaining);
    // Successful actual close retires native contexts, then scrubs owned SPSC
    // storage off callback. No callback or engine owner remains at acknowledgement.
    assert(m::finishGeneration(link, captures, consumers, otherSources) == m::FinishResult::Complete);
    assert(scrubbedQueues == scrubbedBefore + 5);
    assert(captures.empty() && consumers.empty() && otherSources.empty());
    sdk::clean();
}
} // namespace
int main(int argc, char** argv) {
    assert(argc == 2); sdk::install();
    @autoreleasepool {
        sourceLifecycle();
        for (auto source : {w::Precision::Float32, w::Precision::Float64})
            for (auto sink : {w::Precision::Float32, w::Precision::Float64})
                for (bool stale : {false, true})
                    for (bool partial : {false, true}) linkedGraph(argv[1], source, sink, stale, partial);
    }
    std::cout << "Actual Rust host + production tap ingress/output-only callback: scoped mixing, output DSP, exact source precision, explicit sink conversion, whole-generation source_loss, queued/partial cursor discard, destination isolation and native-close completion passed. Injected SDK clocks/identities only; no native audio.\n";
}

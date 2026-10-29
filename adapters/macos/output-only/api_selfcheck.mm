// SPDX-License-Identifier: MIT
#include "output_only.hpp"
#include "api.hpp"
#include "../native_worker.hpp"
#include <Block.h>
#include <algorithm>
#include <cassert>
#include <cstdlib>
#include <iostream>
#include <new>
#include <thread>

static thread_local bool inRender = false;
void* operator new(std::size_t bytes) {
    assert(!inRender && "Render callback allocated");
    if (auto p = std::malloc(bytes ? bytes : 1)) return p;
    throw std::bad_alloc();
}
void* operator new[](std::size_t n) { return ::operator new(n); }
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t) noexcept { std::free(p); }
void* operator new(std::size_t bytes, std::align_val_t alignment) {
    assert(!inRender && "Render callback allocated aligned storage");
    void* p = nullptr;
    if (posix_memalign(&p, static_cast<size_t>(alignment), bytes ? bytes : 1)) throw std::bad_alloc();
    return p;
}
void* operator new[](std::size_t n, std::align_val_t a) { return ::operator new(n, a); }
void operator delete(void* p, std::align_val_t) noexcept { std::free(p); }
void operator delete[](void* p, std::align_val_t) noexcept { std::free(p); }
void operator delete(void* p, std::size_t, std::align_val_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t, std::align_val_t) noexcept { std::free(p); }
namespace w = mixyard::worker;
namespace out = mixyard::output;
using out::detail::Api;
namespace mixyard::output::detail { Api testApi; }
namespace {
constexpr AudioDeviceID device = 41;
constexpr AudioStreamID stream = 51;
constexpr OSStatus injectedError = -12345;
struct Notice {
    AudioObjectID object;
    AudioObjectPropertyAddress address;
    dispatch_queue_t queue;
    AudioObjectPropertyListenerBlock callback;
};
struct Fake {
    unsigned calls = 0, failAt = 0, failures = 0, starts = 0, stops = 0, inputReads = 0;
    bool live = false, initialized = false, running = false;
    UInt32 input = 1, output = 0, maxFrames = 0;
    AudioDeviceID bound = 0;
    AudioStreamBasicDescription native{}, physical{}, au{}, client{};
    std::array<SInt32, w::maxChannels> map{};
    AURenderCallbackStruct callback{};
    std::vector<Notice> notices;
    std::string uid = "chosen-output";
    UInt32 alive = 1, firstChannel = 1;
    Float64 nominal = 48000;
    UInt32 inventoryBytes = sizeof(AudioDeviceID), streamBytes = sizeof(AudioStreamID);
    bool duplicate = false, missing = false, truncate = false, wrongInputRead = false;
    bool wrongBound = false, wrongMap = false, wrongLimit = false, eventOnStart = false;
    bool corruptClient = false, corruptAfterInitialize = false, wrongPropertySize = false;
    bool suppressInputSet = false, startError = false, canonicalizeClient = false;
    unsigned stopFailures = 0;
    dispatch_semaphore_t entered = nullptr, release = nullptr;
} g;
AudioUnit unit() { return reinterpret_cast<AudioUnit>(&g); }
AudioComponent component() { return reinterpret_cast<AudioComponent>(&g); }
bool fail() {
    assert(!inRender && "Render callback called an AudioUnit/HAL API");
    ++g.calls;
    if (g.calls == g.failAt) { ++g.failures; return true; }
    return false;
}
AudioStreamBasicDescription format(w::Precision precision = w::Precision::Float64, bool planar = true) {
    AudioStreamBasicDescription f{};
    f.mSampleRate = 48000;
    f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked |
        (planar ? kAudioFormatFlagIsNonInterleaved : 0);
    f.mChannelsPerFrame = 2; f.mFramesPerPacket = 1;
    f.mBitsPerChannel = static_cast<UInt32>(precision) * 8;
    f.mBytesPerPacket = f.mBytesPerFrame = static_cast<UInt32>(precision) * (planar ? 1 : 2);
    return f;
}
void reset(w::Precision precision = w::Precision::Float64, bool planar = true) {
    assert(!g.live && !g.running && g.notices.empty());
    g = Fake{};
    g.native = g.physical = format(precision, false);
    g.au = g.client = format(precision, planar);
}
std::unique_ptr<w::Queue> queue(w::Precision precision = w::Precision::Float64,
                               std::vector<std::string> channels = {"CA:1", "CA:2"}) {
    return std::make_unique<w::Queue>(w::Format{48000, std::move(channels), precision}, w::Binding{3, 7}, 11,
                                    w::Provenance{w::Origin::AdaptedEngine, "synthetic-destination-clock"});
}
template<class T> OSStatus put(void* destination, UInt32* size, const T& value) {
    assert(*size >= sizeof(T));
    std::memcpy(destination, &value, sizeof(value)); *size = sizeof(value);
    if (g.truncate) --*size;
    return noErr;
}
AudioComponent find(AudioComponent previous, const AudioComponentDescription* d) {
    assert(!previous && d->componentType == kAudioUnitType_Output && d->componentSubType == kAudioUnitSubType_HALOutput &&
           d->componentManufacturer == kAudioUnitManufacturer_Apple && !d->componentFlags && !d->componentFlagsMask);
    return fail() ? nullptr : component();
}
OSStatus create(AudioComponent c, AudioComponentInstance* instance) {
    assert(c == component() && !g.live);
    if (fail()) return injectedError;
    *instance = unit(); g.live = true;
    return noErr;
}
OSStatus dispose(AudioComponentInstance u) {
    assert(u == unit() && g.live && !g.running);
    if (fail()) return injectedError;
    g.live = g.initialized = false; return noErr;
}
OSStatus set(AudioUnit u, AudioUnitPropertyID p, AudioUnitScope scope, AudioUnitElement bus,
             const void* value, UInt32 size) {
    assert(u == unit() && g.live && !g.initialized);
    if (fail()) return injectedError;
    if (p == kAudioOutputUnitProperty_EnableIO) {
        assert(size == sizeof(UInt32)); const auto enabled = *static_cast<const UInt32*>(value);
        if (scope == kAudioUnitScope_Input && bus == 1) {
            assert(enabled == 0 && "Library attempted to enable native capture");
            if (!g.suppressInputSet) g.input = enabled;
        } else {
            assert(scope == kAudioUnitScope_Output && bus == 0 && enabled == 1 && g.inputReads);
            g.output = enabled;
        }
    } else if (p == kAudioOutputUnitProperty_CurrentDevice) {
        assert(scope == kAudioUnitScope_Global && bus == 0 && size == sizeof(AudioDeviceID));
        assert(g.input == 0 && g.output == 1); g.bound = *static_cast<const AudioDeviceID*>(value);
        assert(g.bound == device);
    } else if (p == kAudioUnitProperty_StreamFormat) {
        assert(scope == kAudioUnitScope_Input && bus == 0 && size == sizeof(g.client));
        g.client = *static_cast<const AudioStreamBasicDescription*>(value);
        if (g.canonicalizeClient) {
            g.client.mFormatFlags ^= kAudioFormatFlagIsNonInterleaved;
            const auto lanes = (g.client.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? 1u : g.client.mChannelsPerFrame;
            g.client.mBytesPerPacket = g.client.mBytesPerFrame = lanes * g.client.mBitsPerChannel / 8;
        }
    } else if (p == kAudioOutputUnitProperty_ChannelMap) {
        assert(scope == kAudioUnitScope_Input && bus == 0 && size == 2 * sizeof(SInt32));
        std::memcpy(g.map.data(), value, size); assert(g.map[0] == 0 && g.map[1] == 1);
    } else if (p == kAudioUnitProperty_MaximumFramesPerSlice) {
        assert(scope == kAudioUnitScope_Global && bus == 0 && size == sizeof(UInt32));
        g.maxFrames = *static_cast<const UInt32*>(value);
    } else if (p == kAudioUnitProperty_SetRenderCallback) {
        assert(scope == kAudioUnitScope_Input && bus == 0 && size == sizeof(g.callback));
        g.callback = *static_cast<const AURenderCallbackStruct*>(value);
    } else assert(false && "Unexpected property write (capture/default/format mutation)");
    return noErr;
}
OSStatus get(AudioUnit u, AudioUnitPropertyID p, AudioUnitScope scope, AudioUnitElement bus,
             void* value, UInt32* size) {
    assert(u == unit() && g.live);
    if (fail()) return injectedError;
    if (p == kAudioOutputUnitProperty_EnableIO) {
        if (scope == kAudioUnitScope_Input && bus == 1) {
            ++g.inputReads;
            return put(value, size, g.wrongInputRead ? UInt32{1} : g.input);
        }
        assert(scope == kAudioUnitScope_Output && bus == 0);
        return put(value, size, g.output);
    }
    if (p == kAudioOutputUnitProperty_CurrentDevice) {
        assert(scope == kAudioUnitScope_Global && bus == 0);
        return put(value, size, g.wrongBound ? AudioDeviceID{99} : g.bound);
    }
    if (p == kAudioUnitProperty_StreamFormat) {
        assert(bus == 0 && (scope == kAudioUnitScope_Input || scope == kAudioUnitScope_Output));
        auto f = scope == kAudioUnitScope_Input ? g.client : g.au;
        if (g.corruptClient && scope == kAudioUnitScope_Input) f.mSampleRate = 44100;
        return put(value, size, f);
    }
    if (p == kAudioUnitProperty_MaximumFramesPerSlice) {
        assert(scope == kAudioUnitScope_Global && bus == 0);
        return put(value, size, g.wrongLimit ? UInt32{9999} : g.maxFrames);
    }
    assert(p == kAudioOutputUnitProperty_ChannelMap && scope == kAudioUnitScope_Input && bus == 0 && *size == 8);
    auto map = g.map;
    if (g.wrongMap) std::swap(map[0], map[1]);
    std::memcpy(value, map.data(), 8); *size = 8;
    return noErr;
}
OSStatus initialize(AudioUnit u) {
    assert(u == unit() && g.live && !g.initialized && !g.running);
    assert(g.input == 0 && g.inputReads >= 2 && g.bound == device && g.output == 1 && g.callback.inputProc);
    if (fail()) return injectedError;
    g.initialized = true;
    if (g.corruptAfterInitialize) g.input = 1;
    return noErr;
}
OSStatus uninitialize(AudioUnit u) {
    assert(u == unit() && g.initialized && !g.running);
    if (fail()) return injectedError;
    g.initialized = false; return noErr;
}
void event(size_t index = 0) {
    const auto& n = g.notices.at(index);
    const auto callback = n.callback;
    const auto address = n.address;
    dispatch_async(n.queue, ^{ callback(1, &address); });
}
OSStatus start(AudioUnit u) {
    assert(u == unit() && g.live && g.initialized && !g.running && g.input == 0 && g.inputReads >= 4);
    ++g.starts; g.running = true; // A failing start may have partial effects.
    if (g.eventOnStart) event(5); // Input topology changes WHILE start owns control.
    if (g.entered) {
        dispatch_semaphore_signal(g.entered);
        dispatch_semaphore_wait(g.release, DISPATCH_TIME_FOREVER);
    }
    return (fail() || g.startError) ? injectedError : noErr;
}
OSStatus stop(AudioUnit u) {
    assert(u == unit() && g.live); ++g.stops;
    if (fail()) return injectedError;
    if (g.stopFailures) { --g.stopFailures; return injectedError; }
    g.running = false; return noErr;
}
OSStatus size(AudioObjectID object, const AudioObjectPropertyAddress* a, UInt32 qualifierSize,
              const void* qualifier, UInt32* bytes) {
    assert(!qualifierSize && !qualifier && a->mElement == kAudioObjectPropertyElementMain);
    if (fail()) return injectedError;
    if (object == kAudioObjectSystemObject) {
        assert(a->mSelector == kAudioHardwarePropertyDevices && a->mScope == kAudioObjectPropertyScopeGlobal);
        *bytes = g.duplicate ? 2 * sizeof(AudioDeviceID) : g.inventoryBytes;
    } else {
        assert(object == device && a->mSelector == kAudioDevicePropertyStreams && a->mScope == kAudioObjectPropertyScopeOutput);
        *bytes = g.streamBytes;
    }
    return noErr;
}
OSStatus data(AudioObjectID object, const AudioObjectPropertyAddress* a, UInt32 qualifierSize,
              const void* qualifier, UInt32* bytes, void* value) {
    assert(!qualifierSize && !qualifier && a->mElement == kAudioObjectPropertyElementMain);
    if (fail()) return injectedError;
    if (object == kAudioObjectSystemObject) {
        assert(a->mSelector == kAudioHardwarePropertyDevices && a->mScope == kAudioObjectPropertyScopeGlobal);
        if (g.duplicate) {
            const AudioDeviceID ids[]{device, 42}; assert(*bytes == sizeof(ids));
            std::memcpy(value, ids, sizeof(ids)); return noErr;
        }
        return put(value, bytes, device);
    }
    if (object == device || object == 42) {
        if (a->mSelector == kAudioDevicePropertyDeviceUID) {
            assert(a->mScope == kAudioObjectPropertyScopeGlobal);
            const auto text = CFStringCreateWithCString(nullptr, g.missing ? "other" : g.uid.c_str(), kCFStringEncodingUTF8);
            return put(value, bytes, text);
        }
        if (a->mSelector == kAudioDevicePropertyDeviceIsAlive) return put(value, bytes, g.alive);
        if (a->mSelector == kAudioDevicePropertyNominalSampleRate) {
            const auto status = put(value, bytes, g.nominal);
            if (g.wrongPropertySize) --*bytes;
            return status;
        }
        assert(a->mSelector == kAudioDevicePropertyStreams && a->mScope == kAudioObjectPropertyScopeOutput);
        return put(value, bytes, stream);
    }
    assert(object == stream && a->mScope == kAudioObjectPropertyScopeGlobal);
    if (a->mSelector == kAudioStreamPropertyVirtualFormat) return put(value, bytes, g.native);
    if (a->mSelector == kAudioStreamPropertyPhysicalFormat) return put(value, bytes, g.physical);
    assert(a->mSelector == kAudioStreamPropertyStartingChannel);
    return put(value, bytes, g.firstChannel);
}
OSStatus listen(AudioObjectID object, const AudioObjectPropertyAddress* a, dispatch_queue_t q,
                AudioObjectPropertyListenerBlock callback) {
    if (fail()) return injectedError;
    assert(q && callback && g.notices.size() < 9);
    dispatch_retain(q);
    g.notices.push_back({object, *a, q, Block_copy(callback)});
    return noErr;
}
OSStatus unlisten(AudioObjectID object, const AudioObjectPropertyAddress* a, dispatch_queue_t q,
                  AudioObjectPropertyListenerBlock callback) {
    if (fail()) return injectedError;
    assert(!g.notices.empty());
    const auto& n = g.notices.back();
    assert(n.object == object && n.address.mSelector == a->mSelector && n.address.mScope == a->mScope &&
           n.queue == q && n.callback == callback);
    Block_release(n.callback); dispatch_release(n.queue); g.notices.pop_back();
    return noErr;
}
void install() {
    out::detail::testApi = Api{find, create, dispose, set, get, initialize, uninitialize, start, stop,
                               size, data, listen, unlisten};
}
template<class F> void rejects(F f) {
    bool rejected = false;
    try { f(); } catch (const std::exception&) { rejected = true; }
    assert(rejected);
}
void clean() { assert(!g.live && !g.running && g.notices.empty()); }
void lifecycleFailures() {
    reset();
    unsigned total = 0;
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        assert(!g.running && g.input == 0 && g.notices.size() == 9);
        worker->start(); assert(g.running && g.starts == 1);
        assert(worker->revoke() && !g.running && q->fault() == w::Fault::Revoked);
        rejects([&] { worker->start(); }); assert(g.starts == 1);
        assert(worker->close()); total = g.calls;
        assert(worker->close() && g.calls == total);
    }
    clean();
    // A single injected failure at EVERY public API call in open/start/revoke/close.
    // Failure is one-shot, so rollback/retry must reclaim every owned resource.
    for (unsigned at = 1; at <= total; ++at) {
        reset(); g.failAt = at;
        auto q = queue();
        try {
            auto worker = out::Worker::open("chosen-output", *q);
            try { worker->start(); } catch (const std::exception&) {}
            worker->revoke();
            if (!worker->close()) assert(worker->close());
        } catch (const std::exception&) {}
        assert(g.failures == 1 && q->fault() != w::Fault::None);
        clean();
    }
    reset();
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        g.startError = true; g.stopFailures = 2;
        rejects([&] { worker->start(); });
        assert(g.running && q->fault() == w::Fault::Revoked && g.stops == 1);
        assert(!worker->revoke() && g.running && g.stops == 2);
        rejects([&] { worker->start(); }); assert(g.starts == 1);
        assert(worker->close() && !g.running && g.stops == 3);
    }
    clean();
    std::cout << "Injected failure and cleanup at all " << total << " public API operations\n";
}
void admission() {
    for (unsigned mode = 0; mode != 21; ++mode) {
        reset(); auto q = queue();
        switch (mode) {
        case 0: g.inventoryBytes = 0; break;
        case 1: g.inventoryBytes = 3; break;
        case 2: g.inventoryBytes = 1025 * sizeof(AudioDeviceID); break;
        case 3: g.duplicate = true; break;
        case 4: g.missing = true; break;
        case 5: g.alive = 0; break;
        case 6: g.nominal = 44100; break;
        case 7: g.streamBytes = 2 * sizeof(AudioStreamID); break;
        case 8: g.physical.mBitsPerChannel = 32; break;
        case 9: g.native.mSampleRate = 44100; break;
        case 10: g.au.mFormatFlags = kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked; break;
        case 11: g.firstChannel = 2; break;
        case 12: g.wrongInputRead = true; break;
        case 13: g.suppressInputSet = true; break;
        case 14: g.wrongBound = true; break;
        case 15: g.wrongMap = true; break;
        case 16: g.wrongLimit = true; break;
        case 17: g.corruptClient = true; break;
        case 18: g.corruptAfterInitialize = true; break;
        case 19: q = queue(w::Precision::Float64, {"CA:2", "CA:1"}); break;
        case 20: g.wrongPropertySize = true; break;
        }
        rejects([&] { out::Worker::open("chosen-output", *q); });
        assert(g.starts == 0); clean();
    }
    reset(); auto q = queue();
    rejects([&] { out::Worker::open("", *q); }); assert(g.calls == 0);
    rejects([&] { out::Worker::open(std::string("x\0y", 3), *q); }); assert(g.calls == 0);
    q->revoke(); rejects([&] { out::Worker::open("chosen-output", *q); }); assert(g.calls == 0);
    w::Queue synthetic({48000, {"CA:1", "CA:2"}, w::Precision::Float64}, {3, 7}, 11,
                       {w::Origin::Synthetic, "unadapted"});
    rejects([&] { out::Worker::open("chosen-output", synthetic); }); assert(g.calls == 0);
    clean();
}
void topologyAndRaces() {
    reset();
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        g.client = format(w::Precision::Float64, false); // Frozen client was planar.
        rejects([&] { worker->start(); });
        assert(g.starts == 0 && !g.running);
        assert(worker->close());
    }
    clean();
    for (size_t i = 0; i < 9; ++i) {
        reset(); auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        event(i); // Queued BEFORE start; serialized event must prevent native start.
        rejects([&] { worker->start(); }); assert(g.starts == 0 && !g.running);
        assert(worker->close()); clean();
    }
    reset();
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        g.eventOnStart = true; worker->start();
        // Revoke is a barrier for the earlier topology block queued inside start.
        assert(worker->revoke()); assert(g.starts == 1 && g.stops == 1 && !g.running && g.input == 0);
        rejects([&] { worker->start(); }); assert(g.starts == 1);
        assert(worker->close());
    }
    clean(); reset();
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        g.entered = dispatch_semaphore_create(0); g.release = dispatch_semaphore_create(0);
        std::thread starter([&] { worker->start(); });
        assert(dispatch_semaphore_wait(g.entered, dispatch_time(DISPATCH_TIME_NOW, 2000000000)) == 0);
        // Start is deliberately blocked INSIDE the native API. Concurrent revoke
        // must wait for its ownership, then stop. No restart may follow revoke.
        const auto revoking = dispatch_semaphore_create(0);
        std::thread revoker([&] { dispatch_semaphore_signal(revoking); assert(worker->revoke()); });
        assert(dispatch_semaphore_wait(revoking, dispatch_time(DISPATCH_TIME_NOW, 2000000000)) == 0);
        dispatch_semaphore_signal(g.release);
        starter.join(); revoker.join(); dispatch_release(revoking);
        assert(!g.running && g.starts == 1 && g.stops == 1 && g.input == 0);
        rejects([&] { worker->start(); }); assert(g.starts == 1);
        dispatch_release(g.entered); dispatch_release(g.release); g.entered = g.release = nullptr;
        assert(worker->close());
    }
    clean(); reset();
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        g.entered = dispatch_semaphore_create(0); g.release = dispatch_semaphore_create(0);
        std::thread starter([&] { worker->start(); });
        assert(dispatch_semaphore_wait(g.entered, dispatch_time(DISPATCH_TIME_NOW, 2000000000)) == 0);
        const auto closing = dispatch_semaphore_create(0);
        std::thread closer([&] { dispatch_semaphore_signal(closing); assert(worker->close()); });
        assert(dispatch_semaphore_wait(closing, dispatch_time(DISPATCH_TIME_NOW, 2000000000)) == 0);
        dispatch_semaphore_signal(g.release);
        starter.join(); closer.join(); dispatch_release(closing);
        assert(!g.live && !g.running && g.starts == 1 && g.stops == 1);
        rejects([&] { worker->start(); }); assert(g.starts == 1);
        dispatch_release(g.entered); dispatch_release(g.release); g.entered = g.release = nullptr;
    }
    clean(); reset();
    {
        auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        // Model an event retained by HAL until AFTER successful listener removal.
        const auto late = Block_copy(g.notices[0].callback);
        const auto lateAddress = g.notices[0].address;
        const auto control = g.notices[0].queue; dispatch_retain(control);
        assert(worker->close()); worker.reset();
        dispatch_sync(control, ^{ late(1, &lateAddress); });
        Block_release(late); dispatch_release(control);
    }
    clean();
}
struct Buffers {
    UInt32 count;
    AudioBuffer lanes[2];
};
uint64_t futureTick() { return mach_absolute_time() + 1000000000; }
void push(w::Queue& q, uint64_t sequence, uint64_t first, uint64_t ticks, uint32_t frames, const void* pcm) {
    w::HostClock clock;
    const auto captured = clock.now();
    const void* buffers[]{pcm}; const uint32_t sizes[]{frames * q.format().frameBytes()};
    assert(q.push(captured, {q.binding(), q.clockId(), sequence, first, captured, clock.nanos(ticks) + 1000000000},
                  frames, buffers, sizes, 1, false));
}
OSStatus render(UInt32 frames, uint64_t first, uint64_t ticks, Buffers& buffers,
                AudioUnitRenderActionFlags& flags, UInt32 bus = 0) {
    AudioTimeStamp time{}; time.mSampleTime = static_cast<double>(first); time.mHostTime = ticks;
    time.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    inRender = true;
    const auto status = g.callback.inputProc(g.callback.inputProcRefCon, &flags, &time, bus, frames,
                                             reinterpret_cast<AudioBufferList*>(&buffers));
    inRender = false;
    return status;
}
void renderPcm() {
    reset();
    {
        auto first = queue(); auto sibling = queue();
        auto worker = out::Worker::open("chosen-output", *first);
        const auto ticks = futureTick();
        const double pcm[]{0.125, -0.125};
        push(*sibling, 0, 3, ticks, 1, pcm);
        assert(worker->revoke() && first->fault() == w::Fault::Revoked);
        w::Renderer renderer(*sibling); double actual[2]{}; w::HostClock clock;
        inRender = true;
        assert(renderer.render(clock.nanos(ticks), 3, 1, actual, sizeof(actual)));
        inRender = false;
        assert(sibling->fault() == w::Fault::None && std::memcmp(actual, pcm, sizeof(pcm)) == 0);
        assert(worker->close());
    }
    clean();
    for (auto precision : {w::Precision::Float32, w::Precision::Float64})
      for (bool requestedPlanar : {false, true}) for (bool canonicalized : {false, true}) {
        reset(precision, requestedPlanar); g.canonicalizeClient = canonicalized;
        const bool planar = requestedPlanar != canonicalized;
        auto q = queue(precision); auto worker = out::Worker::open("chosen-output", *q);
        worker->start();
        const double doubles[]{1.0 + 0x1p-40, -1.0 - 0x1p-40, -0.0, 0.0, 0x1p-1074, -0x1p-1074};
        const float floats[]{0.25f, -0.25f, -0.0f, 0.0f, 0x1p-149f, -0x1p-149f};
        const auto width = static_cast<UInt32>(precision);
        const auto* pcm = precision == w::Precision::Float64 ? static_cast<const void*>(doubles) : static_cast<const void*>(floats);
        const auto ticks = futureTick();
        push(*q, 0, 123, ticks, 2, pcm);
        push(*q, 1, 125, ticks, 1, static_cast<const unsigned char*>(pcm) + 4 * width);
        std::array<unsigned char, 64> a{}, b{};
        Buffers buffers{planar ? 2u : 1u, {{planar ? 1u : 2u, 3 * width * (planar ? 1u : 2u), a.data()},
                                         {1, 3 * width, b.data()}}};
        AudioUnitRenderActionFlags flags = 0;
        assert(render(3, 123, ticks, buffers, flags) == noErr && !(flags & kAudioUnitRenderAction_OutputIsSilence));
        if (planar) {
            for (unsigned i = 0; i < 3; ++i) {
                assert(std::memcmp(a.data() + i * width, static_cast<const unsigned char*>(pcm) + 2 * i * width, width) == 0);
                assert(std::memcmp(b.data() + i * width, static_cast<const unsigned char*>(pcm) + (2 * i + 1) * width, width) == 0);
            }
        } else assert(std::memcmp(a.data(), pcm, 6 * width) == 0);
        a.fill(0x55); b.fill(0x55);
        assert(render(3, 126, ticks, buffers, flags) == noErr && (flags & kAudioUnitRenderAction_OutputIsSilence));
        for (unsigned i = 0; i < buffers.lanes[0].mDataByteSize; ++i) assert(a[i] == 0);
        if (planar) for (unsigned i = 0; i < 3 * width; ++i) assert(b[i] == 0);
        assert(q->fault() == w::Fault::Underflow);
        assert(worker->close()); clean();
    }
    for (unsigned mode = 0; mode != 7; ++mode) {
        reset(w::Precision::Float64, false); auto q = queue(); auto worker = out::Worker::open("chosen-output", *q);
        worker->start();
        const double pcm[]{0.25, -0.25, 0.5, -0.5};
        const auto ticks = futureTick(); push(*q, 0, 10, ticks, 1, pcm);
        std::array<unsigned char, 64> data{}; data.fill(0x5a);
        Buffers buffers{1, {{2, 32, data.data()}, {0, 0, nullptr}}};
        AudioUnitRenderActionFlags flags = 0;
        switch (mode) {
        case 0: break; // Valid first block, missing suffix: clear copied prefix too.
        case 1: buffers.lanes[0].mDataByteSize = 31; break;
        case 2: buffers.lanes[0].mNumberChannels = 1; break;
        case 3: q->revoke(); break;
        case 4: break; // Destination timeline mismatch.
        case 5: break; // Expired scheduled timestamp.
        case 6: break; // Wrong output bus.
        }
        render(2, mode == 4 ? 11 : 10, mode == 5 ? 0 : ticks, buffers, flags, mode == 6 ? 1 : 0);
        assert(q->fault() != w::Fault::None && (flags & kAudioUnitRenderAction_OutputIsSilence));
        for (unsigned i = 0; i < buffers.lanes[0].mDataByteSize; ++i) assert(data[i] == 0);
        assert(worker->close()); clean();
    }
}
}
int main() {
    install();
    lifecycleFailures(); admission(); topologyAndRaces(); renderPcm();
    std::cout << "Output-only actual callback/API-double lifecycle, privacy, exact PCM and topology/start races passed. No native audio.\n";
}

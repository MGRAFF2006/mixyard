// SPDX-License-Identifier: MIT
#pragma once
// Strict public SDK boundary doubles. No native audio API may be called here.
#include "capture_api.hpp"
#include "../output-only/api.hpp"
#include <Block.h>
#include <algorithm>
#include <cassert>
#include <vector>
#include <unistd.h>

namespace mixyard::macos::detail { CaptureApi captureTestApi; }
namespace mixyard::output::detail { Api testApi; }
namespace sdk {
namespace w = mixyard::worker;
inline thread_local bool callback = false;
inline constexpr OSStatus failure = -12345;
inline unsigned calls = 0, failAt = 0, hits = 0;
inline bool fail() {
    assert(!callback && "Audio callback performed native API I/O");
    if (++calls == failAt) { ++hits; return true; } return false;
}
struct Notice {
    AudioObjectID object; AudioObjectPropertyAddress address;
    dispatch_queue_t queue; AudioObjectPropertyListenerBlock block;
};
inline std::vector<Notice> captureNotices, outputNotices;
inline OSStatus add(std::vector<Notice>& list, AudioObjectID o, const AudioObjectPropertyAddress* a,
                    dispatch_queue_t q, AudioObjectPropertyListenerBlock b) {
    if (fail()) return failure;
    list.push_back({o, *a, q, Block_copy(b)}); return noErr;
}
inline OSStatus remove(std::vector<Notice>& list, AudioObjectID o, const AudioObjectPropertyAddress* a,
                       dispatch_queue_t q, AudioObjectPropertyListenerBlock b) {
    if (fail()) return failure;
    const auto it = std::find_if(list.begin(), list.end(), [&](const Notice& n) {
        return n.object == o && n.address.mSelector == a->mSelector && n.address.mScope == a->mScope && n.queue == q && n.block == b;
    });
    assert(it != list.end()); Block_release(it->block); list.erase(it); return noErr;
}
template<class T> inline OSStatus put(void* p, UInt32* bytes, T value) {
    assert(*bytes >= sizeof(T)); std::memcpy(p, &value, sizeof(value)); *bytes = sizeof(value); return noErr;
}
inline AudioStreamBasicDescription format(w::Precision precision) {
    AudioStreamBasicDescription f{};
    f.mSampleRate = 48000; f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    f.mFramesPerPacket = 1; f.mChannelsPerFrame = 2;
    f.mBitsPerChannel = static_cast<UInt32>(precision) * 8;
    f.mBytesPerPacket = f.mBytesPerFrame = static_cast<UInt32>(precision) * 2; return f;
}
inline struct CaptureState {
    bool tap = false, aggregate = false, io = false, running = false;
    unsigned starts = 0, stops = 0;
    pid_t pid = getpid() + 1000;
    AudioStreamBasicDescription pcm = format(w::Precision::Float64);
    AudioDeviceIOProc proc = nullptr; void* context = nullptr;
    unsigned stopFailures = 0;
} capture;
inline AudioDeviceIOProcID io() { return reinterpret_cast<AudioDeviceIOProcID>(&capture); }
inline OSStatus tap(CATapDescription* d, AudioObjectID* id) {
    assert(d.isPrivate && d.muteBehavior == CATapUnmuted && [d.processes isEqual:@[@101]]);
    assert(!capture.tap); if (fail()) return failure; capture.tap = true; *id = 102; return noErr;
}
inline OSStatus untap(AudioObjectID id) {
    assert(id == 102 && capture.tap && !capture.aggregate && !capture.io);
    if (fail()) return failure; capture.tap = false; return noErr;
}
inline OSStatus aggregate(CFDictionaryRef spec, AudioDeviceID* id) {
    auto* d = (__bridge NSDictionary*)spec;
    assert([d[@(kAudioAggregateDeviceIsPrivateKey)] boolValue]);
    assert([d[@(kAudioAggregateDeviceTapListKey)][0][@(kAudioSubTapUIDKey)] isEqual:@"injected-tap-uid"]);
    assert(![d[@(kAudioAggregateDeviceTapListKey)][0][@(kAudioSubTapDriftCompensationKey)] boolValue]);
    assert(!d[@(kAudioAggregateDeviceSubDeviceListKey)]);
    assert(capture.tap && !capture.aggregate); if (fail()) return failure;
    capture.aggregate = true; *id = 103; return noErr;
}
inline OSStatus unaggregate(AudioDeviceID id) {
    assert(id == 103 && capture.aggregate && !capture.io);
    if (fail()) return failure; capture.aggregate = false; return noErr;
}
inline OSStatus create(AudioDeviceID id, AudioDeviceIOProc p, void* context, AudioDeviceIOProcID* result) {
    assert(id == 103 && capture.aggregate && !capture.io);
    if (fail()) return failure;
    capture.proc = p; capture.context = context; capture.io = true; *result = io(); return noErr;
}
inline OSStatus destroy(AudioDeviceID id, AudioDeviceIOProcID proc) {
    assert(id == 103 && proc == io() && capture.io && !capture.running);
    if (fail()) return failure; capture.io = false; return noErr;
}
inline OSStatus start(AudioDeviceID id, AudioDeviceIOProcID proc) {
    assert(id == 103 && proc == io() && capture.io && !capture.running);
    ++capture.starts; capture.running = true; return fail() ? failure : noErr;
}
inline OSStatus stop(AudioDeviceID id, AudioDeviceIOProcID proc) {
    assert(id == 103 && proc == io() && capture.io);
    if (fail() || capture.stopFailures) { if (capture.stopFailures) --capture.stopFailures; return failure; }
    ++capture.stops; capture.running = false; return noErr;
}
inline OSStatus captureSize(AudioObjectID o, const AudioObjectPropertyAddress* a, UInt32, const void*, UInt32* size) {
    assert(o == kAudioObjectSystemObject && a->mSelector == kAudioHardwarePropertyProcessObjectList);
    if (fail()) return failure; *size = sizeof(AudioObjectID); return noErr;
}
inline OSStatus captureData(AudioObjectID o, const AudioObjectPropertyAddress* a, UInt32, const void*, UInt32* size, void* p) {
    if (fail()) return failure;
    if (o == kAudioObjectSystemObject && a->mSelector == kAudioHardwarePropertyProcessObjectList) return put(p, size, AudioObjectID{101});
    if (o == 101 && a->mSelector == kAudioProcessPropertyPID) return put(p, size, capture.pid);
    if (o == 102 && a->mSelector == kAudioTapPropertyUID) return put(p, size, CFStringCreateCopy(nullptr, CFSTR("injected-tap-uid")));
    if (o == 102 && a->mSelector == kAudioTapPropertyFormat) return put(p, size, capture.pcm);
    if (o == 103 && a->mSelector == kAudioDevicePropertyDeviceIsAlive) return put(p, size, UInt32{1});
    assert(false && "Unexpected source native query"); return failure;
}
inline struct OutputState {
    bool live = false, initialized = false, running = false;
    UInt32 input = 1, output = 0, limit = 0; AudioDeviceID device = 0;
    AudioStreamBasicDescription pcm = format(w::Precision::Float64), client{};
    AURenderCallbackStruct render{};
    std::string uid = "injected-output-a";
    unsigned inputReads = 0, starts = 0, stops = 0, stopFailures = 0;
} output;
inline AudioUnit unit() { return reinterpret_cast<AudioUnit>(&output); }
inline AudioComponent component() { return reinterpret_cast<AudioComponent>(&output); }
inline AudioComponent find(AudioComponent c, const AudioComponentDescription* d) {
    assert(!c && d->componentType == kAudioUnitType_Output && d->componentSubType == kAudioUnitSubType_HALOutput);
    return fail() ? nullptr : component();
}
inline OSStatus newUnit(AudioComponent c, AudioComponentInstance* result) {
    assert(c == component() && !output.live); if (fail()) return failure;
    output.live = true; output.input = 1; *result = unit(); return noErr;
}
inline OSStatus dispose(AudioComponentInstance u) {
    assert(u == unit() && !output.running && output.live);
    if (fail()) return failure; output.live = false; return noErr;
}
inline OSStatus set(AudioUnit u, AudioUnitPropertyID p, AudioUnitScope s, AudioUnitElement b, const void* data, UInt32 size) {
    assert(u == unit() && output.live && !output.initialized); if (fail()) return failure;
    if (p == kAudioOutputUnitProperty_EnableIO) {
        assert(size == sizeof(UInt32)); const auto value = *static_cast<const UInt32*>(data);
        if (s == kAudioUnitScope_Input && b == 1) { assert(value == 0); output.input = 0; }
        else { assert(s == kAudioUnitScope_Output && b == 0 && value == 1 && output.inputReads); output.output = 1; }
    } else if (p == kAudioOutputUnitProperty_CurrentDevice) {
        assert(s == kAudioUnitScope_Global && b == 0 && !output.input); output.device = *static_cast<const AudioDeviceID*>(data); assert(output.device == 201);
    } else if (p == kAudioUnitProperty_StreamFormat) {
        assert(s == kAudioUnitScope_Input && b == 0 && size == sizeof(output.client)); output.client = *static_cast<const AudioStreamBasicDescription*>(data);
    } else if (p == kAudioOutputUnitProperty_ChannelMap) {
        assert(s == kAudioUnitScope_Input && b == 0 && size == 8); const auto* map = static_cast<const SInt32*>(data); assert(map[0] == 0 && map[1] == 1);
    } else if (p == kAudioUnitProperty_MaximumFramesPerSlice) {
        assert(s == kAudioUnitScope_Global && b == 0); output.limit = *static_cast<const UInt32*>(data);
    } else if (p == kAudioUnitProperty_SetRenderCallback) {
        assert(s == kAudioUnitScope_Input && b == 0); output.render = *static_cast<const AURenderCallbackStruct*>(data);
    } else assert(false && "Unexpected capture/default/format mutation");
    return noErr;
}
inline OSStatus get(AudioUnit u, AudioUnitPropertyID p, AudioUnitScope s, AudioUnitElement b, void* data, UInt32* size) {
    assert(u == unit() && output.live); if (fail()) return failure;
    if (p == kAudioOutputUnitProperty_EnableIO) {
        if (s == kAudioUnitScope_Input && b == 1) { ++output.inputReads; return put(data, size, output.input); }
        assert(s == kAudioUnitScope_Output && b == 0); return put(data, size, output.output);
    }
    if (p == kAudioOutputUnitProperty_CurrentDevice) return put(data, size, output.device);
    if (p == kAudioUnitProperty_StreamFormat) return put(data, size, s == kAudioUnitScope_Input ? output.client : output.pcm);
    if (p == kAudioUnitProperty_MaximumFramesPerSlice) return put(data, size, output.limit);
    assert(p == kAudioOutputUnitProperty_ChannelMap && *size == 8);
    const SInt32 map[]{0, 1}; std::memcpy(data, map, 8); *size = 8; return noErr;
}
inline OSStatus initialize(AudioUnit u) { assert(u == unit() && !output.input); if (fail()) return failure; output.initialized = true; return noErr; }
inline OSStatus uninitialize(AudioUnit u) { assert(u == unit() && !output.running); if (fail()) return failure; output.initialized = false; return noErr; }
inline OSStatus startUnit(AudioUnit u) { assert(u == unit() && !output.input && output.initialized); ++output.starts; output.running = true; return fail() ? failure : noErr; }
inline OSStatus stopUnit(AudioUnit u) {
    assert(u == unit()); if (fail() || output.stopFailures) { if (output.stopFailures) --output.stopFailures; return failure; }
    ++output.stops; output.running = false; return noErr;
}
inline OSStatus outputSize(AudioObjectID o, const AudioObjectPropertyAddress* a, UInt32, const void*, UInt32* size) {
    if (fail()) return failure;
    assert((o == kAudioObjectSystemObject && a->mSelector == kAudioHardwarePropertyDevices) ||
           (o == 201 && a->mSelector == kAudioDevicePropertyStreams && a->mScope == kAudioObjectPropertyScopeOutput));
    *size = sizeof(AudioObjectID); return noErr;
}
inline OSStatus outputData(AudioObjectID o, const AudioObjectPropertyAddress* a, UInt32, const void*, UInt32* size, void* p) {
    if (fail()) return failure;
    const auto s = a->mSelector;
    if (o == kAudioObjectSystemObject && s == kAudioHardwarePropertyDevices) return put(p, size, AudioDeviceID{201});
    if (o == 201) {
        if (s == kAudioDevicePropertyDeviceUID) return put(p, size, CFStringCreateWithCString(nullptr, output.uid.c_str(), kCFStringEncodingUTF8));
        if (s == kAudioDevicePropertyDeviceIsAlive) return put(p, size, UInt32{1});
        if (s == kAudioDevicePropertyNominalSampleRate) return put(p, size, Float64{48000});
        if (s == kAudioDevicePropertyStreams) { assert(a->mScope == kAudioObjectPropertyScopeOutput); return put(p, size, AudioStreamID{202}); }
    }
    if (o == 202) {
        if (s == kAudioStreamPropertyVirtualFormat || s == kAudioStreamPropertyPhysicalFormat) return put(p, size, output.pcm);
        if (s == kAudioStreamPropertyStartingChannel) return put(p, size, UInt32{1});
    }
    assert(false && "Unexpected output native query"); return failure;
}
inline void install() {
    auto& c = mixyard::macos::detail::captureTestApi;
    c.tap = tap; c.untap = untap; c.aggregate = aggregate; c.unaggregate = unaggregate;
    c.create = create; c.destroy = destroy; c.start = start; c.stop = stop;
    c.size = captureSize; c.data = captureData;
    c.listen = [](AudioObjectID o, const AudioObjectPropertyAddress* a, dispatch_queue_t q, AudioObjectPropertyListenerBlock b) { return add(captureNotices, o, a, q, b); };
    c.unlisten = [](AudioObjectID o, const AudioObjectPropertyAddress* a, dispatch_queue_t q, AudioObjectPropertyListenerBlock b) { return remove(captureNotices, o, a, q, b); };
    auto& out = mixyard::output::detail::testApi;
    out.find = find; out.create = newUnit; out.dispose = dispose; out.set = set; out.get = get;
    out.initialize = initialize; out.uninitialize = uninitialize; out.start = startUnit; out.stop = stopUnit;
    out.size = outputSize; out.data = outputData;
    out.listen = [](AudioObjectID o, const AudioObjectPropertyAddress* a, dispatch_queue_t q, AudioObjectPropertyListenerBlock b) { return add(outputNotices, o, a, q, b); };
    out.unlisten = [](AudioObjectID o, const AudioObjectPropertyAddress* a, dispatch_queue_t q, AudioObjectPropertyListenerBlock b) { return remove(outputNotices, o, a, q, b); };
}
inline void clean() {
    assert(!capture.tap && !capture.aggregate && !capture.io && !capture.running && captureNotices.empty());
    assert(!output.live && !output.running && outputNotices.empty());
}
inline void reset(w::Precision precision = w::Precision::Float64) {
    clean(); capture = CaptureState{}; output = OutputState{};
    capture.pcm = format(precision); calls = failAt = hits = 0;
}
} // namespace sdk

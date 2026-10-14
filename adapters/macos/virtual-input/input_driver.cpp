// SPDX-License-Identifier: MIT
// Original Mixyard implementation. SDK ABI, not derived Apple sample implementation.
#include "fixture_api.h"
#include <CoreFoundation/CFPlugInCOM.h>
#include <mach/mach_time.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstring>
#include <limits>
#include <mutex>
#include <cstdio>

namespace {
constexpr unsigned endpointLimit = 2, clientLimit = 16, maxFrames = 4096;
#ifdef MIXYARD_INPUT_TESTING
constexpr unsigned ringFrames = 16384;
#endif
constexpr UInt32 period = 512;
constexpr const char *initialUUID = "c963dc90-9a0d-4c6c-8934-d8d5af579260";
constexpr const char *uidPrefix = "org.mixyard.input.";
constexpr uint64_t maxFrame = (uint64_t{1} << 53) - 1;
static_assert(sizeof(float) == 4 && std::numeric_limits<float>::is_iec559);
static_assert(std::atomic<uint64_t>::is_always_lock_free);
static_assert(std::atomic<uint32_t>::is_always_lock_free);
static_assert(std::atomic<bool>::is_always_lock_free);
static_assert(std::atomic<double>::is_always_lock_free);
#if __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "This artifact requires native little-endian Float32."
#endif

// All frame fields are atomic, not a seqlock over racy ordinary sample storage.
// Sequential consistency gives a bounded reader two coherent stamp observations.
#ifdef MIXYARD_INPUT_TESTING
struct Frame {
    std::atomic<uint64_t> stamp{0}, position{0}, generation{0}, seed{0}, revision{0};
    std::atomic<uint32_t> left{0}, right{0};
};
#endif
struct Reader {
    std::atomic<bool> present{false}, running{false}, observed{false};
    std::atomic<uint32_t> id{0};
    std::atomic<uint64_t> evidenceStamp{0}, first{0}, next{0}, revision{0}, seed{0}, generation{0};
    std::atomic<uint64_t> delivered{0}, zeros{0}, cursor{0}, readSeed{0};
    std::atomic<uint32_t> reason{4};
    // Non-RT only, copied/retained because AddDeviceClient's argument is ephemeral.
    pid_t process = 0;
    CFStringRef bundle = nullptr;
};
struct Endpoint {
    std::atomic<bool> allocated{false}, alive{false};
    CFStringRef uid = nullptr, name = nullptr; // registryMutex only
    std::atomic<uint32_t> running{0};
    std::atomic<uint64_t> anchor{0}, clockSeed{0}, producer{0}, clockVersion{0};
    uint64_t lastGeneration = 0, packetEnd = 0, lastRevision = 0, pending = 0;
    bool hasPacket = false;
    unsigned readerCount = 0;
    std::array<Reader, clientLimit> readers{};
#ifdef MIXYARD_INPUT_TESTING
    std::array<Frame, ringFrames> frames{};
#endif
};
// ponytail: two lifetime endpoints, 16 lifetime readers each. No object reuse/free
// means late callbacks cannot access recycled memory. A production registry is blocked.
std::array<Endpoint, endpointLimit> endpoints{};
std::mutex registryMutex; // properties, lifecycle, fixture controls only; never RT/clock
AudioServerPlugInHostRef host = nullptr;
std::atomic<double> ticksPerFrame{0};
std::atomic<uint32_t> references{0};
#ifdef MIXYARD_INPUT_TESTING
uint64_t nextAction = 1;
#endif
bool initialized = false;
AudioServerPlugInDriverInterface interfaceTable;
AudioServerPlugInDriverInterface *interfacePointer = &interfaceTable;
AudioServerPlugInDriverRef driverRef = &interfacePointer;

bool valid(AudioServerPlugInDriverRef d) { return d == driverRef; }
AudioObjectID deviceID(unsigned i) { return 2 + 2 * i; }
Endpoint *device(AudioObjectID id) {
    if (id < 2 || id > 4 || (id & 1)) return nullptr;
    auto &e = endpoints[(id - 2) / 2];
    return e.allocated.load() ? &e : nullptr;
}
Endpoint *object(AudioObjectID id, bool &stream) {
    stream = id >= 3 && id <= 5 && (id & 1);
    return device(stream ? id - 1 : id);
}
Reader *reader(Endpoint &e, UInt32 id) {
    for (auto &r : e.readers)
        if (r.present.load() && r.id.load() == id) return &r;
    return nullptr;
}
OSStatus notify(AudioObjectID id, AudioObjectPropertySelector selector) {
    const AudioObjectPropertyAddress a{selector, kAudioObjectPropertyScopeGlobal, 0};
    return host->PropertiesChanged(host, id, 1, &a);
}
OSStatus notifyRegistry() {
    const AudioObjectPropertyAddress a[] = {
        {kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, 0},
        {kAudioObjectPropertyOwnedObjects, kAudioObjectPropertyScopeGlobal, 0}};
    return host->PropertiesChanged(host, kAudioObjectPlugInObject, 2, a);
}
// Post-change notifications cannot undo a committed lifecycle transition. Keep
// failures for bounded non-RT reconciliation, including partial dual notices.
struct Notice { AudioObjectID id; AudioObjectPropertySelector selector; bool pending = false; };
std::array<Notice, 9> notices{{
    {kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList},
    {2, kAudioDevicePropertyDeviceIsRunning}, {4, kAudioDevicePropertyDeviceIsRunning},
    {2, kAudioDevicePropertyDeviceIsAlive}, {4, kAudioDevicePropertyDeviceIsAlive},
    {2, kAudioObjectPropertyName}, {3, kAudioObjectPropertyName},
    {4, kAudioObjectPropertyName}, {5, kAudioObjectPropertyName}}};
bool flushingNotices = false; // registryMutex only; host callbacks may reenter
OSStatus lastNoticeError = noErr;
void queueNotice(AudioObjectID id, AudioObjectPropertySelector selector) {
    // Caller holds registryMutex; only the fixed entries above can be queued.
    for (auto &n : notices) if (n.id == id && n.selector == selector) n.pending = true;
}
bool flushNotices() {
    std::array<bool, 9> selected{};
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        if (flushingNotices) return false;
        flushingNotices = true;
        for (unsigned i = 0; i < notices.size(); ++i) {
            selected[i] = notices[i].pending;
            notices[i].pending = false;
        }
    }
    // One snapshot only: never spin on a failing/reentrant host. Clear before
    // calling out so a newer change to the same property cannot be lost.
    for (unsigned i = 0; i < notices.size(); ++i) if (selected[i]) {
        const auto &n = notices[i];
        const OSStatus status = n.id == kAudioObjectPlugInObject ? notifyRegistry() : notify(n.id, n.selector);
        if (status) {
            std::lock_guard<std::mutex> guard(registryMutex);
            notices[i].pending = true;
            lastNoticeError = status;
        }
    }
    std::lock_guard<std::mutex> guard(registryMutex);
    flushingNotices = false;
    for (const auto &n : notices) if (n.pending) return false;
    return true;
}
bool createEndpoint(Endpoint &e, const char *uuid, const char *name) {
    char uid[80]{};
    std::snprintf(uid, sizeof(uid), "%s%s", uidPrefix, uuid);
    CFStringRef u = CFStringCreateWithCString(nullptr, uid, kCFStringEncodingUTF8);
    CFStringRef n = CFStringCreateWithCString(nullptr, name, kCFStringEncodingUTF8);
    if (!u || !n) { if (u) CFRelease(u); if (n) CFRelease(n); return false; }
    e.uid = u; e.name = n;
    e.allocated.store(true);
    e.alive.store(true);
    return true;
}

ULONG addRef(void *d) {
    if (d != driverRef) return 0;
    uint32_t old = references.load();
    while (old != UINT32_MAX && !references.compare_exchange_weak(old, old + 1)) {}
    return old == UINT32_MAX ? old : old + 1;
}
ULONG release(void *d) {
    if (d != driverRef) return 0;
    uint32_t old = references.load();
    while (old && !references.compare_exchange_weak(old, old - 1)) {}
    // Singleton interface and tombstones stay valid for the plug-in image lifetime.
    return old ? old - 1 : 0;
}
HRESULT query(void *d, REFIID uuid, LPVOID *out) {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (d != driverRef) return E_NOINTERFACE;
    CFUUIDRef requested = CFUUIDCreateFromUUIDBytes(nullptr, uuid);
    if (!requested) return E_NOINTERFACE;
    const bool match = CFEqual(requested, IUnknownUUID) ||
                       CFEqual(requested, kAudioServerPlugInDriverInterfaceUUID);
    CFRelease(requested);
    if (!match) return E_NOINTERFACE;
    addRef(d); *out = driverRef; return S_OK;
}
OSStatus initialize(AudioServerPlugInDriverRef d, AudioServerPlugInHostRef h) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!h || !h->PropertiesChanged || !h->RequestDeviceConfigurationChange)
        return kAudioHardwareIllegalOperationError;
    std::lock_guard<std::mutex> guard(registryMutex);
    if (initialized) return h == host ? noErr : kAudioHardwareIllegalOperationError;
    mach_timebase_info_data_t tb{};
    if (mach_timebase_info(&tb) != KERN_SUCCESS || !tb.numer || !tb.denom)
        return kAudioHardwareUnspecifiedError;
    ticksPerFrame.store(1.0e9 * tb.denom / (48000.0 * tb.numer));
    if (!createEndpoint(endpoints[0], initialUUID, "Mixyard Input A"))
        return kAudioHardwareUnspecifiedError;
    host = h; initialized = true;
    return noErr;
}
OSStatus createDevice(AudioServerPlugInDriverRef d, CFDictionaryRef,
                      const AudioServerPlugInClientInfo *, AudioObjectID *out) {
    if (!out) return kAudioHardwareIllegalOperationError;
    *out = kAudioObjectUnknown;
    return valid(d) ? kAudioHardwareUnsupportedOperationError : kAudioHardwareBadObjectError;
}
OSStatus destroyDevice(AudioServerPlugInDriverRef d, AudioObjectID) {
    return valid(d) ? kAudioHardwareUnsupportedOperationError : kAudioHardwareBadObjectError;
}
OSStatus addClient(AudioServerPlugInDriverRef d, AudioObjectID id,
                   const AudioServerPlugInClientInfo *info) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!info || !info->mIsNativeEndian) return kAudioHardwareIllegalOperationError;
    std::lock_guard<std::mutex> guard(registryMutex);
    auto *e = device(id);
    if (!e || !e->alive.load()) return kAudioHardwareBadObjectError;
    if (reader(*e, info->mClientID)) return kAudioHardwareIllegalOperationError;
    if (e->readerCount == clientLimit) return kAudioHardwareUnspecifiedError;
    Reader &r = e->readers[e->readerCount++];
    r.id.store(info->mClientID); r.process = info->mProcessID;
    r.bundle = info->mBundleID;
    if (r.bundle) CFRetain(r.bundle);
    r.present.store(true);
    return noErr;
}
OSStatus removeClient(AudioServerPlugInDriverRef d, AudioObjectID id,
                      const AudioServerPlugInClientInfo *info) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!info) return kAudioHardwareIllegalOperationError;
    bool changed = false;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (!e) return kAudioHardwareBadObjectError;
        auto *r = reader(*e, info->mClientID);
        if (!r) return kAudioHardwareIllegalOperationError;
        r->present.store(false);
        if (r->running.exchange(false)) {
            changed = e->running.fetch_sub(1) == 1;
            if (changed) e->producer.store(0);
        }
        if (r->bundle) { CFRelease(r->bundle); r->bundle = nullptr; }
        if (changed) queueNotice(id, kAudioDevicePropertyDeviceIsRunning);
    }
    (void)flushNotices(); // removal committed; failures remain queued, never returned as removal failure
    return noErr;
}
OSStatus finishChange(AudioServerPlugInDriverRef d, AudioObjectID id, UInt64 action,
                      void *info, bool perform) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (!e) return kAudioHardwareBadObjectError;
        if (info || !action || e->pending != action) return kAudioHardwareIllegalOperationError;
        e->pending = 0;
        if (perform) {
            e->alive.store(false);
            queueNotice(id, kAudioDevicePropertyDeviceIsAlive);
            queueNotice(kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList);
        }
        // Abort retains the endpoint, but admission revoked at request is never restored.
    }
    if (!perform) return noErr;
    (void)flushNotices(); // retirement committed; reconcile notices without repeating its action token
    return noErr;
}
OSStatus performChange(AudioServerPlugInDriverRef d, AudioObjectID id, UInt64 a, void *p) {
    return finishChange(d, id, a, p, true);
}
OSStatus abortChange(AudioServerPlugInDriverRef d, AudioObjectID id, UInt64 a, void *p) {
    return finishChange(d, id, a, p, false);
}

AudioStreamBasicDescription format() {
    AudioStreamBasicDescription f{};
    f.mSampleRate = 48000;
    f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagsNativeEndian;
    f.mBytesPerPacket = 8; f.mFramesPerPacket = 1; f.mBytesPerFrame = 8;
    f.mChannelsPerFrame = 2; f.mBitsPerChannel = 32;
    return f;
}
// Returns a byte size or -1. Scope/element restrictions are shared by all property calls.
int propertySize(AudioObjectID id, const AudioObjectPropertyAddress &a) {
    if (a.mElement != 0) return -1;
    const auto s = a.mSelector;
    const bool global = a.mScope == kAudioObjectPropertyScopeGlobal;
    const bool input = a.mScope == kAudioObjectPropertyScopeInput;
    const bool output = a.mScope == kAudioObjectPropertyScopeOutput;
    if (id == kAudioObjectPlugInObject) {
        if (!initialized || !global) return -1;
        switch (s) {
            case kAudioObjectPropertyBaseClass: case kAudioObjectPropertyClass:
            case kAudioObjectPropertyOwner: case kAudioPlugInPropertyTranslateUIDToDevice:
                return sizeof(UInt32);
            case kAudioObjectPropertyName: case kAudioObjectPropertyManufacturer:
            case kAudioPlugInPropertyResourceBundle: return sizeof(CFStringRef);
            case kAudioPlugInPropertyBoxList: case kAudioPlugInPropertyClockDeviceList:
            case kAudioObjectPropertyCustomPropertyInfoList: return 0;
            case kAudioPlugInPropertyDeviceList: case kAudioObjectPropertyOwnedObjects: {
                int n = 0;
                for (auto &e : endpoints) if (e.alive.load()) ++n;
                return n * sizeof(AudioObjectID);
            }
            default: return -1;
        }
    }
    bool stream = false;
    auto *e = object(id, stream);
    if (!e || !e->alive.load() || (!global && !input && !output)) return -1;
    if (stream && !global) return -1;
    if (s == kAudioObjectPropertyOwnedObjects) return stream || output ? 0 : sizeof(AudioObjectID);
    if (s == kAudioObjectPropertyControlList || s == kAudioObjectPropertyCustomPropertyInfoList) return 0;
    if (stream) {
        if (!global) return -1;
        switch (s) {
            case kAudioObjectPropertyName: case kAudioObjectPropertyManufacturer: return sizeof(CFStringRef);
            case kAudioObjectPropertyBaseClass: case kAudioObjectPropertyClass:
            case kAudioObjectPropertyOwner: case kAudioStreamPropertyIsActive:
            case kAudioStreamPropertyDirection: case kAudioStreamPropertyTerminalType:
            case kAudioStreamPropertyStartingChannel: case kAudioStreamPropertyLatency: return sizeof(UInt32);
            case kAudioStreamPropertyVirtualFormat: case kAudioStreamPropertyPhysicalFormat:
                return sizeof(AudioStreamBasicDescription);
            case kAudioStreamPropertyAvailableVirtualFormats: case kAudioStreamPropertyAvailablePhysicalFormats:
                return sizeof(AudioStreamRangedDescription);
            default: return -1;
        }
    }
    if (s == kAudioDevicePropertyStreams) return output ? 0 : sizeof(AudioObjectID);
    if (s == kAudioDevicePropertyDeviceCanBeDefaultDevice || s == kAudioDevicePropertyDeviceCanBeDefaultSystemDevice ||
        s == kAudioDevicePropertyLatency || s == kAudioDevicePropertySafetyOffset)
        return input || output ? sizeof(UInt32) : -1;
    if (s == kAudioDevicePropertyPreferredChannelsForStereo) return input ? 2 * sizeof(UInt32) : -1;
    if (s == kAudioDevicePropertyPreferredChannelLayout)
        return input ? offsetof(AudioChannelLayout, mChannelDescriptions) : -1;
    if (!global) return -1;
    switch (s) {
        case kAudioObjectPropertyBaseClass: case kAudioObjectPropertyClass: case kAudioObjectPropertyOwner:
        case kAudioDevicePropertyTransportType: case kAudioDevicePropertyClockDomain:
        case kAudioDevicePropertyDeviceIsAlive: case kAudioDevicePropertyDeviceIsRunning:
        case kAudioDevicePropertyIsHidden: case kAudioDevicePropertyZeroTimeStampPeriod:
        case kAudioDevicePropertyClockAlgorithm: case kAudioDevicePropertyClockIsStable: return sizeof(UInt32);
        case kAudioObjectPropertyName: case kAudioObjectPropertyManufacturer:
        case kAudioDevicePropertyDeviceUID: case kAudioDevicePropertyModelUID: return sizeof(CFStringRef);
        case kAudioDevicePropertyRelatedDevices: return sizeof(AudioObjectID);
        case kAudioDevicePropertyNominalSampleRate: return sizeof(Float64);
        case kAudioDevicePropertyAvailableNominalSampleRates: return sizeof(AudioValueRange);
        default: return -1;
    }
}
bool knownObject(AudioObjectID id) {
    if (id == kAudioObjectPlugInObject) return initialized;
    bool stream = false;
    auto *e = object(id, stream);
    return e && e->alive.load();
}
OSStatus propertyError(AudioObjectID id) {
    return knownObject(id) ? kAudioHardwareUnknownPropertyError : kAudioHardwareBadObjectError;
}
Boolean hasProperty(AudioServerPlugInDriverRef d, AudioObjectID id, pid_t,
                    const AudioObjectPropertyAddress *a) {
    if (!valid(d) || !a) return false;
    std::lock_guard<std::mutex> guard(registryMutex);
    return propertySize(id, *a) >= 0;
}
OSStatus settable(AudioServerPlugInDriverRef d, AudioObjectID id, pid_t,
                  const AudioObjectPropertyAddress *a, Boolean *out) {
    if (!out) return kAudioHardwareIllegalOperationError;
    *out = false;
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!a) return kAudioHardwareIllegalOperationError;
    std::lock_guard<std::mutex> guard(registryMutex);
    return propertySize(id, *a) < 0 ? propertyError(id) : noErr;
}
// Optional OwnedObjects qualifier is an array of class IDs. Other than UID translation,
// all other properties in this artifact reject qualifiers rather than ignoring bad lengths.
bool listProperty(AudioObjectPropertySelector s) {
    return s == kAudioObjectPropertyOwnedObjects || s == kAudioPlugInPropertyDeviceList ||
           s == kAudioDevicePropertyStreams || s == kAudioDevicePropertyRelatedDevices ||
           s == kAudioObjectPropertyControlList || s == kAudioPlugInPropertyBoxList ||
           s == kAudioPlugInPropertyClockDeviceList || s == kAudioObjectPropertyCustomPropertyInfoList;
}
OSStatus qualifier(const AudioObjectPropertyAddress &a, UInt32 bytes, const void *p) {
    if (a.mSelector == kAudioPlugInPropertyTranslateUIDToDevice) {
        if (bytes != sizeof(CFStringRef) || !p) return kAudioHardwareBadPropertySizeError;
        CFStringRef uid = nullptr; std::memcpy(&uid, p, sizeof(uid));
        return uid && CFGetTypeID(uid) == CFStringGetTypeID() ? noErr : kAudioHardwareIllegalOperationError;
    }
    if (a.mSelector == kAudioObjectPropertyOwnedObjects) {
        return (bytes % sizeof(AudioClassID) || (bytes && !p)) ? kAudioHardwareBadPropertySizeError : noErr;
    }
    return bytes ? kAudioHardwareBadPropertySizeError : noErr;
}
bool classMatches(UInt32 bytes, const void *p, AudioClassID cls) {
    if (!bytes) return true;
    for (UInt32 i = 0; i < bytes; i += sizeof(AudioClassID)) {
        AudioClassID c; std::memcpy(&c, static_cast<const char *>(p) + i, sizeof(c));
        if (c == cls || c == kAudioObjectClassID) return true;
    }
    return false;
}
int qualifiedSize(AudioObjectID id, const AudioObjectPropertyAddress &a, UInt32 bytes, const void *p) {
    int size = propertySize(id, a);
    if (size >= 0 && a.mSelector == kAudioObjectPropertyOwnedObjects &&
        !classMatches(bytes, p, id == kAudioObjectPlugInObject ? kAudioDeviceClassID : kAudioStreamClassID))
        size = 0;
    return size;
}
OSStatus getSize(AudioServerPlugInDriverRef d, AudioObjectID id, pid_t,
                 const AudioObjectPropertyAddress *a, UInt32 qb, const void *q, UInt32 *out) {
    if (!out) return kAudioHardwareIllegalOperationError;
    *out = 0;
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!a) return kAudioHardwareIllegalOperationError;
    std::lock_guard<std::mutex> guard(registryMutex);
    if (propertySize(id, *a) < 0) return propertyError(id);
    const OSStatus status = qualifier(*a, qb, q);
    if (status) return status;
    *out = static_cast<UInt32>(qualifiedSize(id, *a, qb, q));
    return noErr;
}
OSStatus getData(AudioServerPlugInDriverRef d, AudioObjectID id, pid_t,
                 const AudioObjectPropertyAddress *a, UInt32 qb, const void *q,
                 UInt32 capacity, UInt32 *outBytes, void *out) {
    if (!outBytes) return kAudioHardwareIllegalOperationError;
    *outBytes = 0;
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!a || (capacity && !out)) return kAudioHardwareIllegalOperationError;
    std::lock_guard<std::mutex> guard(registryMutex);
    if (propertySize(id, *a) < 0) return propertyError(id);
    const OSStatus status = qualifier(*a, qb, q);
    if (status) return status;
    const UInt32 needed = static_cast<UInt32>(qualifiedSize(id, *a, qb, q));
    if (capacity < needed && !listProperty(a->mSelector)) return kAudioHardwareBadPropertySizeError;
    if (needed && !out && !listProperty(a->mSelector)) return kAudioHardwareIllegalOperationError;
    if (!needed) return noErr;
    const auto s = a->mSelector;
    bool isStream = false;
    Endpoint *e = id == kAudioObjectPlugInObject ? nullptr : object(id, isStream);
    UInt32 u = 0;
    CFStringRef string = nullptr;
    if (listProperty(s)) {
        AudioObjectID ids[endpointLimit]{};
        unsigned n = 0;
        if (id == kAudioObjectPlugInObject) {
            for (unsigned i = 0; i < endpointLimit; ++i)
                if (endpoints[i].alive.load()) ids[n++] = deviceID(i);
        } else if (s == kAudioDevicePropertyRelatedDevices) ids[n++] = id;
        else if (!isStream && a->mScope != kAudioObjectPropertyScopeOutput) ids[n++] = id + 1;
        const UInt32 count = std::min(static_cast<UInt32>(n), capacity / static_cast<UInt32>(sizeof(AudioObjectID)));
        if (count) std::memcpy(out, ids, count * sizeof(AudioObjectID));
        *outBytes = count * sizeof(AudioObjectID);
        return noErr;
    }
    switch (s) {
        case kAudioObjectPropertyBaseClass: u = kAudioObjectClassID; break;
        case kAudioObjectPropertyClass:
            u = id == kAudioObjectPlugInObject ? kAudioPlugInClassID : isStream ? kAudioStreamClassID : kAudioDeviceClassID;
            break;
        case kAudioObjectPropertyOwner:
            u = id == kAudioObjectPlugInObject ? kAudioObjectUnknown : isStream ? id - 1 : kAudioObjectPlugInObject;
            break;
        case kAudioObjectPropertyName: string = e ? e->name : CFSTR("Mixyard Input Proof"); break;
        case kAudioObjectPropertyManufacturer: string = CFSTR("Mixyard"); break;
        case kAudioPlugInPropertyResourceBundle: string = CFSTR(""); break;
        case kAudioDevicePropertyDeviceUID: string = e->uid; break;
        case kAudioDevicePropertyModelUID: string = CFSTR("org.mixyard.input.fixed48k.proof"); break;
        case kAudioPlugInPropertyTranslateUIDToDevice: {
            CFStringRef uid; std::memcpy(&uid, q, sizeof(uid));
            for (unsigned i = 0; i < endpointLimit; ++i)
                if (endpoints[i].alive.load() && CFEqual(uid, endpoints[i].uid)) u = deviceID(i);
            break;
        }
        case kAudioDevicePropertyNominalSampleRate: {
            const Float64 rate = 48000; std::memcpy(out, &rate, sizeof(rate)); *outBytes = sizeof(rate); return noErr;
        }
        case kAudioDevicePropertyAvailableNominalSampleRates: {
            const AudioValueRange range{48000, 48000};
            std::memcpy(out, &range, sizeof(range)); *outBytes = sizeof(range); return noErr;
        }
        case kAudioDevicePropertyTransportType: u = kAudioDeviceTransportTypeVirtual; break;
        case kAudioDevicePropertyClockDomain: u = 0; break; // system host clock, no hardware synchronization claim
        case kAudioDevicePropertyDeviceIsAlive: u = e->alive.load(); break;
        case kAudioDevicePropertyDeviceIsRunning: u = e->running.load() != 0; break;
        case kAudioDevicePropertyDeviceCanBeDefaultDevice:
        case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
        case kAudioDevicePropertyLatency: case kAudioDevicePropertySafetyOffset:
        case kAudioDevicePropertyIsHidden: u = 0; break; // stream/device latency share the SDK selector
        case kAudioDevicePropertyZeroTimeStampPeriod: u = period; break;
        case kAudioDevicePropertyClockAlgorithm: u = kAudioDeviceClockAlgorithmRaw; break;
        case kAudioDevicePropertyClockIsStable: u = 1; break;
        case kAudioDevicePropertyPreferredChannelsForStereo: {
            const UInt32 channels[2]{1, 2}; std::memcpy(out, channels, sizeof(channels));
            *outBytes = sizeof(channels); return noErr;
        }
        case kAudioDevicePropertyPreferredChannelLayout: {
            AudioChannelLayout layout{}; layout.mChannelLayoutTag = kAudioChannelLayoutTag_Stereo;
            std::memcpy(out, &layout, needed); *outBytes = needed; return noErr;
        }
        case kAudioStreamPropertyIsActive: case kAudioStreamPropertyDirection:
        case kAudioStreamPropertyStartingChannel: u = 1; break;
        case kAudioStreamPropertyTerminalType: u = kAudioStreamTerminalTypeMicrophone; break;
        case kAudioStreamPropertyVirtualFormat: case kAudioStreamPropertyPhysicalFormat: {
            const auto f = format(); std::memcpy(out, &f, sizeof(f)); *outBytes = sizeof(f); return noErr;
        }
        case kAudioStreamPropertyAvailableVirtualFormats: case kAudioStreamPropertyAvailablePhysicalFormats: {
            const AudioStreamRangedDescription f{format(), {48000, 48000}};
            std::memcpy(out, &f, sizeof(f)); *outBytes = sizeof(f); return noErr;
        }
        default: return kAudioHardwareUnknownPropertyError;
    }
    if (string) {
        CFRetain(string); std::memcpy(out, &string, sizeof(string)); *outBytes = sizeof(string);
    } else { std::memcpy(out, &u, sizeof(u)); *outBytes = sizeof(u); }
    return noErr;
}
OSStatus setData(AudioServerPlugInDriverRef d, AudioObjectID id, pid_t,
                 const AudioObjectPropertyAddress *a, UInt32 qb, const void *q, UInt32 bytes, const void *data) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    if (!a || (bytes && !data)) return kAudioHardwareIllegalOperationError;
    std::lock_guard<std::mutex> guard(registryMutex);
    if (propertySize(id, *a) < 0) return propertyError(id);
    const OSStatus status = qualifier(*a, qb, q);
    return status ? status : kAudioHardwareIllegalOperationError;
}

OSStatus startIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    bool changed = false;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (!e || !e->alive.load()) return kAudioHardwareBadObjectError;
        auto *r = reader(*e, client);
        if (!r || r->running.load() || e->pending) return kAudioHardwareIllegalOperationError;
        if (!e->running.load()) {
            if (e->clockSeed.load() == UINT64_MAX) return kAudioHardwareUnspecifiedError;
            if (e->clockVersion.load() > UINT64_MAX - 2) return kAudioHardwareUnspecifiedError;
            e->clockVersion.fetch_add(1);
            e->producer.store(0); e->hasPacket = false;
            e->anchor.store(mach_absolute_time());
            e->clockSeed.fetch_add(1);
            e->clockVersion.fetch_add(1);
            changed = true;
        }
        r->observed.store(false); r->running.store(true); e->running.fetch_add(1);
        if (changed) queueNotice(id, kAudioDevicePropertyDeviceIsRunning);
    }
    (void)flushNotices(); // IO started; failed post-change notices remain queued
    return noErr;
}
OSStatus stopIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    bool changed = false;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (!e) return kAudioHardwareBadObjectError;
        auto *r = reader(*e, client);
        if (!r || !r->running.exchange(false)) return kAudioHardwareIllegalOperationError;
        changed = e->running.fetch_sub(1) == 1;
        if (changed) e->producer.store(0);
        if (changed) queueNotice(id, kAudioDevicePropertyDeviceIsRunning);
    }
    (void)flushNotices(); // IO stopped; failed post-change notices remain queued
    return noErr;
}
OSStatus zeroTime(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client,
                  Float64 *sample, UInt64 *time, UInt64 *seed) {
    if (sample) *sample = 0;
    if (time) *time = 0;
    if (seed) *seed = 0;
    if (!sample || !time || !seed) return kAudioHardwareIllegalOperationError;
    if (!valid(d)) return kAudioHardwareBadObjectError;
    auto *e = device(id);
    if (!e || !e->alive.load()) return kAudioHardwareBadObjectError;
    if (!reader(*e, client) || !e->running.load()) return kAudioHardwareIllegalOperationError;
    const uint64_t version = e->clockVersion.load();
    if (version & 1) return kAudioHardwareUnspecifiedError;
    const uint64_t s = e->clockSeed.load(), anchor = e->anchor.load(), now = mach_absolute_time();
    const double tpf = ticksPerFrame.load();
    if (!s || !(tpf > 0) || now < anchor) return kAudioHardwareUnspecifiedError;
    const double elapsed = static_cast<double>(now - anchor) / tpf;
    if (elapsed > static_cast<double>(maxFrame)) return kAudioHardwareUnspecifiedError;
    const uint64_t frame = (static_cast<uint64_t>(elapsed) / period) * period;
    const uint64_t ticks = static_cast<uint64_t>(frame * tpf);
    if (ticks > UINT64_MAX - anchor || version != e->clockVersion.load() || !e->running.load())
        return kAudioHardwareUnspecifiedError;
    *sample = static_cast<double>(frame); *time = anchor + ticks; *seed = s;
    return noErr;
}
OSStatus willIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client, UInt32 op,
                Boolean *will, Boolean *inPlace) {
    if (will) *will = false;
    if (inPlace) *inPlace = true;
    if (!will || !inPlace) return kAudioHardwareIllegalOperationError;
    if (!valid(d)) return kAudioHardwareBadObjectError;
    auto *e = device(id);
    if (!e || !e->alive.load()) return kAudioHardwareBadObjectError;
    if (!reader(*e, client)) return kAudioHardwareIllegalOperationError;
    *will = op == kAudioServerPlugInIOOperationReadInput;
    return noErr;
}
OSStatus phaseIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client,
                 UInt32 op, UInt32 frames, const AudioServerPlugInIOCycleInfo *cycle) {
    if (!valid(d)) return kAudioHardwareBadObjectError;
    auto *e = device(id);
    if (!e) return kAudioHardwareBadObjectError;
    if (!reader(*e, client) || !cycle || frames > maxFrames) return kAudioHardwareIllegalOperationError;
    // Thread and cycle phases surround the ReadInput operation on real HAL hosts.
    return op == kAudioServerPlugInIOOperationReadInput || op == kAudioServerPlugInIOOperationThread ||
           op == kAudioServerPlugInIOOperationCycle ? noErr : kAudioHardwareUnsupportedOperationError;
}
void evidence(Reader &r, uint64_t first, uint64_t next, uint64_t seed, uint64_t generation,
              uint64_t revision, uint64_t delivered, uint64_t zeros, uint32_t reason) {
    r.evidenceStamp.fetch_add(1);
    r.first.store(first); r.next.store(next); r.seed.store(seed); r.generation.store(generation);
    r.revision.store(revision); r.delivered.store(delivered); r.zeros.store(zeros); r.reason.store(reason);
    r.evidenceStamp.fetch_add(1);
}
OSStatus doIO(AudioServerPlugInDriverRef d, AudioObjectID id, AudioObjectID stream, UInt32 client,
              UInt32 op, UInt32 count, const AudioServerPlugInIOCycleInfo *cycle, void *main, void *) {
    // Core Audio supplies the buffer's length. Never infer capacity from an untrusted packet.
    if (main && count <= maxFrames) std::memset(main, 0, static_cast<size_t>(count) * 8);
    if (!valid(d)) return kAudioHardwareBadObjectError;
    auto *e = device(id);
    if (!e || stream != id + 1) return kAudioHardwareBadObjectError;
    if (op != kAudioServerPlugInIOOperationReadInput) return kAudioHardwareUnsupportedOperationError;
    if (!main || !cycle || !count || count > maxFrames) return kAudioHardwareIllegalOperationError;
    auto *r = reader(*e, client);
    if (!r) return kAudioHardwareIllegalOperationError;
    const uint64_t seed = e->clockSeed.load(), generation = e->producer.load();
    const double sample = cycle->mInputTime.mSampleTime;
    if (!(cycle->mInputTime.mFlags & kAudioTimeStampSampleTimeValid) || !std::isfinite(sample) ||
        sample < 0 || sample > static_cast<double>(maxFrame - count) || std::floor(sample) != sample) {
        evidence(*r, 0, r->cursor.load(), seed, generation, 0, 0, count, 5);
        return kAudioHardwareIllegalOperationError;
    }
    const uint64_t first = static_cast<uint64_t>(sample);
    const uint64_t next = first + count;
    if (r->observed.load() && r->readSeed.load() == seed && first < r->cursor.load()) {
        evidence(*r, first, r->cursor.load(), seed, generation, 0, 0, count, 5);
        return kAudioHardwareIllegalOperationError;
    }
    r->readSeed.store(seed); r->cursor.store(next); r->observed.store(true);
    uint64_t delivered = 0, revision = 0;
    uint32_t reason = 0;
    bool uniform = true;
    if (!e->alive.load()) reason = 3;
    else if (!r->running.load()) reason = 4;
    else if (!generation) reason = 1;
    else {
#ifdef MIXYARD_INPUT_TESTING
        auto *out = static_cast<unsigned char *>(main);
        for (UInt32 n = 0; n < count; ++n) {
            Frame &f = e->frames[(first + n) % ringFrames];
            const uint64_t stamp = f.stamp.load();
            if (!stamp || (stamp & 1)) { reason = 2; continue; }
            const uint64_t pos = f.position.load(), gen = f.generation.load(), epoch = f.seed.load();
            const uint64_t rev = f.revision.load();
            const uint32_t left = f.left.load(), right = f.right.load();
            if (stamp != f.stamp.load() || pos != first + n || gen != generation || epoch != seed) {
                reason = 2; continue;
            }
            std::memcpy(out + n * 8, &left, 4); std::memcpy(out + n * 8 + 4, &right, 4);
            if (!delivered) revision = rev;
            else if (revision != rev) uniform = false;
            ++delivered;
        }
#else
        reason = 1;
#endif
    }
    // A revoke/retire/stop/epoch transition racing the copy must fail closed.
    if (!e->alive.load() || !r->running.load() || generation != e->producer.load() || seed != e->clockSeed.load()) {
        std::memset(main, 0, static_cast<size_t>(count) * 8); delivered = 0; revision = 0;
        reason = !e->alive.load() ? 3 : !r->running.load() ? 4 : 6;
    }
    if (delivered == count && !uniform) reason = 6;
    evidence(*r, first, next, seed, generation, delivered == count && uniform ? revision : 0,
             delivered, count - delivered, reason);
    return noErr;
}
// Compile against Apple's real AudioServerPlugIn.h: the initializer checks every ABI signature.
struct SetInterface {
    SetInterface() {
        interfaceTable = {nullptr, query, addRef, release, initialize, createDevice, destroyDevice,
                          addClient, removeClient, performChange, abortChange, hasProperty, settable,
                          getSize, getData, setData, startIO, stopIO, zeroTime, willIO, phaseIO, doIO, phaseIO};
    }
    ~SetInterface() {
        // Image unload is non-RT; the host must have quiesced callbacks before unloading code.
        for (auto &e : endpoints) {
            for (auto &r : e.readers) if (r.bundle) CFRelease(r.bundle);
            if (e.name) CFRelease(e.name);
            if (e.uid) CFRelease(e.uid);
        }
    }
} setInterface;
} // namespace

extern "C" __attribute__((visibility("default")))
void *MixyardInput_Create(CFAllocatorRef, CFUUIDRef type) {
    if (!type || !CFEqual(type, kAudioServerPlugInTypeUUID)) return nullptr;
    addRef(driverRef);
    return driverRef;
}

#ifdef MIXYARD_INPUT_TESTING
namespace {
bool validName(const char *name) {
    if (!name) return false;
    const size_t n = strnlen(name, 129);
    if (!n || n > 128) return false;
    for (size_t i = 0; i < n; ++i)
        if (static_cast<unsigned char>(name[i]) < 32 || name[i] == 127) return false;
    return true; // CFStringCreateWithCString below validates UTF-8.
}
bool validUUID(const char *uuid) {
    if (!uuid || strnlen(uuid, 37) != 36) return false;
    bool nonzero = false;
    for (unsigned i = 0; i < 36; ++i) {
        if (i == 8 || i == 13 || i == 18 || i == 23) { if (uuid[i] != '-') return false; }
        else {
            if (!((uuid[i] >= '0' && uuid[i] <= '9') || (uuid[i] >= 'a' && uuid[i] <= 'f'))) return false;
            nonzero = nonzero || uuid[i] != '0';
        }
    }
    return nonzero;
}
}
extern "C" int MixyardInputTest_PublishEndpoint(const char *uuid, const char *name, AudioObjectID *out) {
    if (!out) return MY_INPUT_INVALID;
    *out = kAudioObjectUnknown;
    if (!validUUID(uuid) || !validName(name)) return MY_INPUT_INVALID;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        if (!initialized) return MY_INPUT_BUSY;
        char text[80]{}; std::snprintf(text, sizeof(text), "%s%s", uidPrefix, uuid);
        CFStringRef uid = CFStringCreateWithCString(nullptr, text, kCFStringEncodingUTF8);
        if (!uid) return MY_INPUT_INVALID;
        bool duplicate = false;
        for (auto &e : endpoints) if (e.allocated.load() && CFEqual(e.uid, uid)) duplicate = true;
        CFRelease(uid);
        if (duplicate) return MY_INPUT_INVALID;
        unsigned i = 0;
        while (i < endpointLimit && endpoints[i].allocated.load()) ++i;
        if (i == endpointLimit) return MY_INPUT_CAPACITY;
        if (!createEndpoint(endpoints[i], uuid, name)) return MY_INPUT_INVALID;
        *out = deviceID(i);
        queueNotice(kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList);
    }
    return flushNotices() ? MY_INPUT_OK : MY_INPUT_COMMITTED_NOTIFY_PENDING;
}
extern "C" int MixyardInputTest_Rename(AudioObjectID id, const char *name) {
    if (!validName(name)) return MY_INPUT_INVALID;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (!e || !e->alive.load()) return MY_INPUT_NOT_FOUND;
        if (e->pending) return MY_INPUT_BUSY;
        CFStringRef n = CFStringCreateWithCString(nullptr, name, kCFStringEncodingUTF8);
        if (!n) return MY_INPUT_INVALID;
        CFRelease(e->name); e->name = n;
        queueNotice(id, kAudioObjectPropertyName);
        queueNotice(id + 1, kAudioObjectPropertyName);
    }
    return flushNotices() ? MY_INPUT_OK : MY_INPUT_COMMITTED_NOTIFY_PENDING;
}
extern "C" int MixyardInputTest_ReconcileNotifications(uint32_t *pending, OSStatus *lastError) {
    if (!pending || !lastError) return MY_INPUT_INVALID;
    const bool complete = flushNotices();
    std::lock_guard<std::mutex> guard(registryMutex);
    *pending = 0;
    for (const auto &n : notices) if (n.pending) ++*pending;
    *lastError = lastNoticeError;
    return complete && !*pending ? MY_INPUT_OK : MY_INPUT_HOST_ERROR;
}
extern "C" int MixyardInputTest_Retire(AudioObjectID id, uint64_t *out) {
    if (!out) return MY_INPUT_INVALID;
    *out = 0;
    uint64_t action = 0;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (!e || !e->alive.load()) return MY_INPUT_NOT_FOUND;
        if (e->pending) return MY_INPUT_BUSY;
        if (nextAction == UINT64_MAX) return MY_INPUT_CAPACITY;
        action = nextAction++; e->pending = action; e->producer.store(0);
    }
    *out = action;
    // Host may synchronously perform/abort: never call out with registryMutex held.
    const OSStatus status = host->RequestDeviceConfigurationChange(host, id, action, nullptr);
    if (!status) return MY_INPUT_OK;
    {
        std::lock_guard<std::mutex> guard(registryMutex);
        auto *e = device(id);
        if (e->pending == action) e->pending = 0;
    }
    return MY_INPUT_HOST_ERROR;
}
extern "C" int MixyardInputTest_Admit(AudioObjectID id, uint64_t generation) {
    if (!generation) return MY_INPUT_INVALID;
    std::lock_guard<std::mutex> guard(registryMutex);
    auto *e = device(id);
    if (!e || !e->alive.load()) return MY_INPUT_NOT_FOUND;
    if (e->pending || !e->running.load() || e->producer.load()) return MY_INPUT_BUSY;
    if (generation <= e->lastGeneration) return MY_INPUT_STALE;
    e->lastGeneration = generation; e->hasPacket = false; e->lastRevision = 0;
    e->producer.store(generation);
    return MY_INPUT_OK;
}
extern "C" int MixyardInputTest_Disconnect(AudioObjectID id, uint64_t generation) {
    std::lock_guard<std::mutex> guard(registryMutex);
    auto *e = device(id);
    if (!e || !e->alive.load()) return MY_INPUT_NOT_FOUND;
    if (!generation || generation != e->producer.load()) return MY_INPUT_STALE;
    e->producer.store(0);
    return MY_INPUT_OK;
}
extern "C" int MixyardInputTest_Packet(AudioObjectID id, uint64_t generation, uint64_t seed,
                                      uint64_t revision, uint64_t first, uint32_t count,
                                      const float *pcm, size_t bytes) {
    if (!pcm || !generation || !seed || !revision || !count || count > maxFrames ||
        bytes != static_cast<size_t>(count) * 8 || first > maxFrame - count) return MY_INPUT_INVALID;
    for (uint32_t i = 0; i < count * 2; ++i) if (!std::isfinite(pcm[i])) return MY_INPUT_INVALID;
    std::lock_guard<std::mutex> guard(registryMutex);
    auto *e = device(id);
    if (!e || !e->alive.load()) return MY_INPUT_NOT_FOUND;
    if (e->pending || generation != e->producer.load() || seed != e->clockSeed.load() ||
        revision < e->lastRevision || (e->hasPacket && first != e->packetEnd)) return MY_INPUT_STALE;
    for (uint32_t n = 0; n < count; ++n)
        if (e->frames[(first + n) % ringFrames].stamp.load() > UINT64_MAX - 2) return MY_INPUT_CAPACITY;
    for (uint32_t n = 0; n < count; ++n) {
        auto &f = e->frames[(first + n) % ringFrames];
        const uint64_t stamp = f.stamp.load();
        f.stamp.store(stamp + 1);
        uint32_t left, right;
        std::memcpy(&left, pcm + n * 2, 4); std::memcpy(&right, pcm + n * 2 + 1, 4);
        f.position.store(first + n); f.generation.store(generation); f.seed.store(seed); f.revision.store(revision);
        f.left.store(left); f.right.store(right);
        f.stamp.store(stamp + 2);
    }
    e->hasPacket = true; e->packetEnd = first + count; e->lastRevision = revision;
    return MY_INPUT_OK;
}
extern "C" int MixyardInputTest_Evidence(AudioObjectID id, uint32_t client, MixyardInputTestEvidence *out) {
    if (!out) return MY_INPUT_INVALID;
    *out = {};
    std::lock_guard<std::mutex> guard(registryMutex);
    auto *e = device(id);
    if (!e) return MY_INPUT_NOT_FOUND;
    auto *r = reader(*e, client);
    if (!r) return MY_INPUT_NOT_FOUND;
    const uint64_t stamp = r->evidenceStamp.load();
    if (!stamp || (stamp & 1)) return MY_INPUT_BUSY;
    MixyardInputTestEvidence v{};
    v.clockSeed = r->seed.load(); v.generation = r->generation.load();
    v.firstFrame = r->first.load(); v.nextFrame = r->next.load(); v.effectiveRevision = r->revision.load();
    v.deliveredFrames = r->delivered.load(); v.zeroFrames = r->zeros.load(); v.reason = r->reason.load();
    if (stamp != r->evidenceStamp.load()) return MY_INPUT_BUSY;
    *out = v;
    return MY_INPUT_OK;
}
#endif

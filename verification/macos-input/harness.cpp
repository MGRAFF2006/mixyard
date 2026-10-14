// SPDX-License-Identifier: MIT
// Real SDK ABI + source-in-process fixture; this never loads a driver into HAL.
#include "../../adapters/macos/virtual-input/fixture_api.h"
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <limits>
#include <vector>

static unsigned notifications = 0;
static AudioObjectID requestedDevice = 0;
static UInt64 requestedAction = 0;
static bool rejectRequest = false;
struct NoticeCall { AudioObjectID object; AudioObjectPropertySelector selector; OSStatus status; };
static std::vector<NoticeCall> noticeCalls;
static AudioObjectID rejectNoticeObject = kAudioObjectUnknown;
static AudioObjectPropertySelector rejectNoticeSelector = 0;
static bool rejectAllNotices = false;
static AudioObjectID reentrantRenameDevice = kAudioObjectUnknown;
static OSStatus changed(AudioServerPlugInHostRef, AudioObjectID object, UInt32 count,
                        const AudioObjectPropertyAddress* addresses) {
    assert(count && addresses); notifications += count;
    if (object == kAudioObjectPlugInObject) {
        assert(count == 2 && addresses[0].mSelector == kAudioPlugInPropertyDeviceList);
        assert(addresses[1].mSelector == kAudioObjectPropertyOwnedObjects);
    } else assert(count == 1);
    const OSStatus status = rejectAllNotices ||
        (object == rejectNoticeObject && addresses[0].mSelector == rejectNoticeSelector)
        ? kAudioHardwareUnspecifiedError : noErr;
    noticeCalls.push_back({object, addresses[0].mSelector, status});
    if (object == reentrantRenameDevice && addresses[0].mSelector == kAudioObjectPropertyName) {
        reentrantRenameDevice = kAudioObjectUnknown;
        assert(MixyardInputTest_Rename(object, "Reentrant B") == MY_INPUT_COMMITTED_NOTIFY_PENDING);
    }
    return status;
}
static OSStatus copyStorage(AudioServerPlugInHostRef, CFStringRef, CFPropertyListRef* out) {
    if (out) *out = nullptr;
    return kAudioHardwareUnknownPropertyError;
}
static OSStatus writeStorage(AudioServerPlugInHostRef, CFStringRef, CFPropertyListRef) { return noErr; }
static OSStatus deleteStorage(AudioServerPlugInHostRef, CFStringRef) { return noErr; }
static OSStatus request(AudioServerPlugInHostRef, AudioObjectID device, UInt64 action, void*) {
    requestedDevice = device; requestedAction = action;
    return rejectRequest ? kAudioHardwareIllegalOperationError : noErr;
}
static AudioServerPlugInHostInterface host{changed, copyStorage, writeStorage, deleteStorage, request};

static AudioObjectPropertyAddress address(AudioObjectPropertySelector selector,
        AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    return {selector, scope, kAudioObjectPropertyElementMain};
}
static std::vector<AudioObjectID> list(AudioServerPlugInDriverRef d, AudioObjectID object,
        AudioObjectPropertySelector selector, AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    auto p = address(selector, scope); UInt32 size = 0;
    assert((*d)->GetPropertyDataSize(d, object, 0, &p, 0, nullptr, &size) == noErr);
    assert(size % sizeof(AudioObjectID) == 0);
    std::vector<AudioObjectID> result(size / sizeof(AudioObjectID));
    if (size) assert((*d)->GetPropertyData(d, object, 0, &p, 0, nullptr, size, &size, result.data()) == noErr);
    return result;
}
template<class T> static T property(AudioServerPlugInDriverRef d, AudioObjectID object,
        AudioObjectPropertySelector selector, AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    auto p = address(selector, scope); UInt32 size = sizeof(T); T value{};
    assert((*d)->GetPropertyData(d, object, 0, &p, 0, nullptr, sizeof(T), &size, &value) == noErr);
    assert(size == sizeof(T)); return value;
}
static AudioObjectID translate(AudioServerPlugInDriverRef d, const char* uid) {
    CFStringRef string = CFStringCreateWithCString(nullptr, uid, kCFStringEncodingUTF8);
    auto p = address(kAudioPlugInPropertyTranslateUIDToDevice);
    AudioObjectID id = 999; UInt32 size = sizeof(id);
    assert((*d)->GetPropertyData(d, kAudioObjectPlugInObject, 0, &p, sizeof(string), &string,
        size, &size, &id) == noErr); CFRelease(string); return id;
}
static AudioObjectID inspect(AudioServerPlugInDriverRef d, AudioObjectID device) {
    assert(property<AudioClassID>(d, device, kAudioObjectPropertyClass) == kAudioDeviceClassID);
    assert(property<UInt32>(d, device, kAudioDevicePropertyDeviceIsAlive) == 1);
    assert(property<Float64>(d, device, kAudioDevicePropertyNominalSampleRate) == 48000);
    assert(list(d, device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput).empty());
    const auto streams = list(d, device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
    assert(streams.size() == 1);
    assert(property<UInt32>(d, streams[0], kAudioStreamPropertyDirection) == 1);
    assert(property<UInt32>(d, streams[0], kAudioStreamPropertyStartingChannel) == 1);
    for (auto selector : {kAudioStreamPropertyPhysicalFormat, kAudioStreamPropertyVirtualFormat}) {
        const auto f = property<AudioStreamBasicDescription>(d, streams[0], selector);
        assert(f.mSampleRate == 48000 && f.mFormatID == kAudioFormatLinearPCM);
        assert(f.mFormatFlags == (kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian | kAudioFormatFlagIsPacked));
        assert(f.mChannelsPerFrame == 2 && f.mBitsPerChannel == 32 && f.mBytesPerFrame == 8);
        assert(f.mFramesPerPacket == 1 && f.mBytesPerPacket == 8);
    }
    auto p = address(kAudioDevicePropertyNominalSampleRate);
    Boolean writable = true;
    assert((*d)->IsPropertySettable(d, device, 0, &p, &writable) == noErr && !writable);
    Float64 rate = 44100; UInt32 size = 0;
    assert((*d)->SetPropertyData(d, device, 0, &p, 0, nullptr, sizeof(rate), &rate) == kAudioHardwareIllegalOperationError);
    assert((*d)->GetPropertyData(d, device, 0, &p, 0, nullptr, 1, &size, &rate) == kAudioHardwareBadPropertySizeError);
    p = address(0x7f7f7f7f);
    assert(!(*d)->HasProperty(d, device, 0, &p));
    assert((*d)->GetPropertyDataSize(d, device, 0, &p, 0, nullptr, &size) == kAudioHardwareUnknownPropertyError);
    assert((*d)->GetPropertyDataSize(d, 9999, 0, &p, 0, nullptr, &size) == kAudioHardwareBadObjectError);
    p = address(kAudioStreamPropertyPhysicalFormat, kAudioObjectPropertyScopeOutput);
    assert(!(*d)->HasProperty(d, streams[0], 0, &p));
    return streams[0];
}
static AudioServerPlugInClientInfo client(UInt32 id) { return {id, 0, true, CFSTR("org.mixyard.abi-fixture")}; }
static void start(AudioServerPlugInDriverRef d, AudioObjectID device, UInt32 id) {
    auto info = client(id);
    assert((*d)->AddDeviceClient(d, device, &info) == noErr);
    assert((*d)->StartIO(d, device, id) == noErr);
    Boolean will = false, inplace = false;
    assert((*d)->WillDoIOOperation(d, device, id, kAudioServerPlugInIOOperationReadInput, &will, &inplace) == noErr);
    assert(will && inplace);
    assert((*d)->WillDoIOOperation(d, device, id, kAudioServerPlugInIOOperationWriteMix, &will, &inplace) == noErr);
    assert(!will);
}
static void stop(AudioServerPlugInDriverRef d, AudioObjectID device, UInt32 id) {
    auto info = client(id);
    assert((*d)->StopIO(d, device, id) == noErr);
    assert((*d)->RemoveDeviceClient(d, device, &info) == noErr);
}
static UInt64 seed(AudioServerPlugInDriverRef d, AudioObjectID device, UInt32 id) {
    Float64 sample = -1; UInt64 hostTime = 0, epoch = 0;
    assert((*d)->GetZeroTimeStamp(d, device, id, &sample, &hostTime, &epoch) == noErr);
    assert(sample >= 0 && hostTime && epoch); return epoch;
}
static std::vector<float> read(AudioServerPlugInDriverRef d, AudioObjectID device, AudioObjectID stream,
        UInt32 id, uint64_t frame, UInt32 count) {
    AudioServerPlugInIOCycleInfo cycle{};
    cycle.mInputTime.mSampleTime = static_cast<double>(frame);
    cycle.mInputTime.mFlags = kAudioTimeStampSampleTimeValid;
    std::vector<float> pcm(count * 2, 0.875f); // forbidden sentinel: every sample must be overwritten
    assert((*d)->BeginIOOperation(d, device, id, kAudioServerPlugInIOOperationReadInput, count, &cycle) == noErr);
    assert((*d)->DoIOOperation(d, device, stream, id, kAudioServerPlugInIOOperationReadInput,
        count, &cycle, pcm.data(), nullptr) == noErr);
    assert((*d)->EndIOOperation(d, device, id, kAudioServerPlugInIOOperationReadInput, count, &cycle) == noErr);
    return pcm;
}
static void zeros(const std::vector<float>& pcm) { for (float sample : pcm) assert(sample == 0); }
static std::vector<float> fixture(unsigned frames, bool b = false) {
    std::vector<float> pcm(frames * 2);
    for (unsigned i = 0; i < frames; ++i) {
        pcm[i * 2] = (b ? -0.25f : 0.25f) + static_cast<float>(i % 16) / 512;
        pcm[i * 2 + 1] = (b ? 0.5f : -0.5f) - static_cast<float>(i % 16) / 1024;
    }
    return pcm;
}
static void reconcile(uint32_t expectedPending) {
    uint32_t pending = UINT32_MAX; OSStatus lastError = noErr;
    assert(MixyardInputTest_ReconcileNotifications(&pending, &lastError) ==
        (expectedPending ? MY_INPUT_HOST_ERROR : MY_INPUT_OK));
    assert(pending == expectedPending && lastError == kAudioHardwareUnspecifiedError);
}
static void named(AudioServerPlugInDriverRef d, AudioObjectID id, CFStringRef expected) {
    auto name = property<CFStringRef>(d, id, kAudioObjectPropertyName);
    assert(CFEqual(name, expected)); CFRelease(name);
}
static void lifecycleNotificationErrors(AudioServerPlugInDriverRef d, AudioObjectID a, AudioObjectID stream) {
    auto first = client(90), second = client(91), removed = client(92);
    assert((*d)->AddDeviceClient(d, a, &first) == noErr);
    rejectNoticeObject = a; rejectNoticeSelector = kAudioDevicePropertyDeviceIsRunning;
    const auto before = noticeCalls.size();
    assert((*d)->StartIO(d, a, first.mClientID) == noErr); // committed despite failed notice
    assert(noticeCalls.size() == before + 1 && noticeCalls.back().status != noErr);
    assert(property<UInt32>(d, a, kAudioDevicePropertyDeviceIsRunning) == 1);
    const auto firstSeed = seed(d, a, first.mClientID);
    zeros(read(d, a, stream, first.mClientID, 0, 8));
    assert((*d)->StartIO(d, a, first.mClientID) == kAudioHardwareIllegalOperationError);
    reconcile(1); // failing retry is bounded and leaves state/epoch intact
    assert(noticeCalls.size() == before + 2 && seed(d, a, first.mClientID) == firstSeed);
    assert((*d)->AddDeviceClient(d, a, &second) == noErr);
    assert((*d)->StartIO(d, a, second.mClientID) == noErr); // retries pending notice once
    assert(noticeCalls.size() == before + 3 && noticeCalls.back().status != noErr);
    assert(seed(d, a, second.mClientID) == firstSeed);
    rejectNoticeObject = kAudioObjectUnknown; reconcile(0);
    assert((*d)->StopIO(d, a, first.mClientID) == noErr);
    assert(property<UInt32>(d, a, kAudioDevicePropertyDeviceIsRunning) == 1);
    rejectNoticeObject = a;
    assert((*d)->StopIO(d, a, second.mClientID) == noErr);
    assert(property<UInt32>(d, a, kAudioDevicePropertyDeviceIsRunning) == 0);
    assert((*d)->StopIO(d, a, second.mClientID) == kAudioHardwareIllegalOperationError);
    reconcile(1); rejectNoticeObject = kAudioObjectUnknown; reconcile(0);
    assert((*d)->RemoveDeviceClient(d, a, &first) == noErr);
    assert((*d)->RemoveDeviceClient(d, a, &second) == noErr);
    assert((*d)->AddDeviceClient(d, a, &removed) == noErr);
    assert((*d)->StartIO(d, a, removed.mClientID) == noErr);
    const auto secondSeed = seed(d, a, removed.mClientID); assert(secondSeed != firstSeed);
    rejectNoticeObject = a;
    assert((*d)->RemoveDeviceClient(d, a, &removed) == noErr); // also commits a last-client stop
    assert(property<UInt32>(d, a, kAudioDevicePropertyDeviceIsRunning) == 0);
    assert((*d)->RemoveDeviceClient(d, a, &removed) == kAudioHardwareIllegalOperationError);
    reconcile(1); rejectNoticeObject = kAudioObjectUnknown; reconcile(0);
    start(d, a, 93); assert(seed(d, a, 93) != secondSeed); stop(d, a, 93);
    const auto after = noticeCalls.size(); reconcile(0); assert(noticeCalls.size() == after);
}
static AudioServerPlugInDriverRef open(void* (*create)(CFAllocatorRef, CFUUIDRef)) {
    assert(!create(nullptr, kAudioServerPlugInDriverInterfaceUUID));
    auto d = static_cast<AudioServerPlugInDriverRef>(create(nullptr, kAudioServerPlugInTypeUUID));
    assert(d && *d && (*d)->_reserved == nullptr);
    void* queried = nullptr;
    assert((*d)->QueryInterface(d, CFUUIDGetUUIDBytes(kAudioServerPlugInDriverInterfaceUUID), &queried) == S_OK);
    assert(queried == d);
    assert((*d)->Release(d) >= 1);
    const auto refs = (*d)->AddRef(d); assert(refs >= 2); assert((*d)->Release(d) == refs - 1);
    assert((*d)->Initialize(d, &host) == noErr);
    auto p = address(kAudioPlugInPropertyTranslateUIDToDevice); UInt32 size = 99;
    assert((*d)->GetPropertyDataSize(d, kAudioObjectPlugInObject, 0, &p, 1, nullptr, &size) == kAudioHardwareBadPropertySizeError);
    assert(size == 0);
    return d;
}

int main(int argc, char** argv) {
    assert(argc == 2);
    // Load the exact signed release artifact into this ordinary process. It never enters HAL.
    void* bundle = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!bundle) { std::fprintf(stderr, "%s\n", dlerror()); return 1; }
    assert(!dlsym(bundle, "MixyardInputTest_Admit"));
    auto create = reinterpret_cast<void* (*)(CFAllocatorRef, CFUUIDRef)>(dlsym(bundle, "MixyardInput_Create"));
    assert(create);
    auto release = open(create);
    auto devices = list(release, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList);
    assert(devices.size() == 1);
    auto stream = inspect(release, devices[0]); start(release, devices[0], 1);
    zeros(read(release, devices[0], stream, 1, 0, 16));
    stop(release, devices[0], 1); (*release)->Release(release);
    assert(dlclose(bundle) == 0);

    auto d = open(MixyardInput_Create);
    devices = list(d, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList);
    assert(devices.size() == 1); const auto a = devices[0];
    const auto as = inspect(d, a);
    assert(translate(d, MY_INPUT_UID_PREFIX MY_INPUT_INITIAL_UUID) == a);
    assert(translate(d, "org.mixyard.input.missing") == kAudioObjectUnknown);
    assert(list(d, kAudioObjectPlugInObject, kAudioObjectPropertyOwnedObjects) == devices);
    AudioObjectID out = 0; auto info = client(1);
    CFDictionaryRef description = CFDictionaryCreate(nullptr, nullptr, nullptr, 0,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    assert(description);
    assert((*d)->CreateDevice(d, description, &info, &out) == kAudioHardwareUnsupportedOperationError);
    assert(out == kAudioObjectUnknown); CFRelease(description);
    assert((*d)->DestroyDevice(d, a) == kAudioHardwareUnsupportedOperationError);
    lifecycleNotificationErrors(d, a, as);
    start(d, a, 1); start(d, a, 2);
    const auto aseed = seed(d, a, 1); assert(seed(d, a, 2) == aseed);
    zeros(read(d, a, as, 1, 0, 8)); zeros(read(d, a, as, 2, 0, 8));
    assert(MixyardInputTest_Admit(a, 1) == MY_INPUT_OK);
    auto apcm = fixture(16);
    assert(MixyardInputTest_Packet(a, 1, aseed, 7, 8, 16, apcm.data(), apcm.size() * sizeof(float)) == MY_INPUT_OK);
    assert(read(d, a, as, 1, 8, 16) == apcm);
    assert(read(d, a, as, 2, 8, 16) == apcm); // reader 1 did not drain reader 2
    MixyardInputTestEvidence evidence{};
    assert(MixyardInputTest_Evidence(a, 2, &evidence) == MY_INPUT_OK);
    assert(evidence.firstFrame == 8 && evidence.nextFrame == 24 && evidence.effectiveRevision == 7);
    assert(evidence.generation == 1 && evidence.clockSeed == aseed && evidence.deliveredFrames >= 16);
    // Invalid times and read replays must fail with initialized zeros and preserve cursor.
    AudioServerPlugInIOCycleInfo badCycle{};
    std::vector<float> bad(16, 0.875f);
    assert((*d)->DoIOOperation(d, a, as, 1, kAudioServerPlugInIOOperationReadInput,
        8, &badCycle, bad.data(), nullptr) != noErr); zeros(bad);
    badCycle.mInputTime.mFlags = kAudioTimeStampSampleTimeValid;
    badCycle.mInputTime.mSampleTime = 8.5;
    assert((*d)->DoIOOperation(d, a, as, 1, kAudioServerPlugInIOOperationReadInput,
        8, &badCycle, bad.data(), nullptr) != noErr); zeros(bad);
    badCycle.mInputTime.mSampleTime = 8;
    assert((*d)->DoIOOperation(d, a, as, 1, kAudioServerPlugInIOOperationReadInput,
        8, &badCycle, bad.data(), nullptr) != noErr); zeros(bad);
    zeros(read(d, a, as, 1, 24, 8)); zeros(read(d, a, as, 2, 24, 8));
    assert(MixyardInputTest_Packet(a, 0, aseed, 7, 24, 16, apcm.data(), 128) != MY_INPUT_OK);
    assert(MixyardInputTest_Packet(a, 1, aseed + 1, 7, 24, 16, apcm.data(), 128) != MY_INPUT_OK);
    assert(MixyardInputTest_Packet(a, 1, aseed, 6, 24, 16, apcm.data(), 128) != MY_INPUT_OK);
    assert(MixyardInputTest_Packet(a, 1, aseed, 8, 24, 16, apcm.data(), 127) != MY_INPUT_OK);
    apcm[0] = std::numeric_limits<float>::infinity();
    assert(MixyardInputTest_Packet(a, 1, aseed, 8, 24, 16, apcm.data(), 128) != MY_INPUT_OK);
    zeros(read(d, a, as, 1, 32, 8));

    const unsigned before = notifications;
    const char* buuid = "b670ff62-f0de-490a-a0f6-9c748bc75343";
    AudioObjectID b = 0;
    rejectNoticeObject = kAudioObjectPlugInObject; rejectNoticeSelector = kAudioPlugInPropertyDeviceList;
    assert(MixyardInputTest_PublishEndpoint(buuid, "Independent B", &b) == MY_INPUT_COMMITTED_NOTIFY_PENDING);
    assert(b != kAudioObjectUnknown && b != a && notifications == before + 2);
    assert(MixyardInputTest_PublishEndpoint(buuid, "Duplicate", &out) != MY_INPUT_OK);
    assert(out == kAudioObjectUnknown);
    assert(MixyardInputTest_PublishEndpoint("fa1bf82a-087b-428b-b274-c42b49d147fa", "C", &out) == MY_INPUT_CAPACITY);
    assert(out == kAudioObjectUnknown);
    devices = {a, b};
    assert(list(d, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList) == devices);
    assert(list(d, kAudioObjectPlugInObject, kAudioObjectPropertyOwnedObjects) == devices);
    assert(translate(d, "org.mixyard.input.b670ff62-f0de-490a-a0f6-9c748bc75343") == b);
    named(d, b, CFSTR("Independent B")); named(d, b + 1, CFSTR("Independent B"));
    reconcile(1); rejectNoticeObject = kAudioObjectUnknown; reconcile(0);
    assert(list(d, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList) == devices);
    const auto bs = inspect(d, b); start(d, b, 3);
    const auto bseed = seed(d, b, 3);
    zeros(read(d, b, bs, 3, 0, 8));
    assert(MixyardInputTest_Admit(b, 1) == MY_INPUT_OK);
    auto bpcm = fixture(16, true);
    assert(MixyardInputTest_Packet(b, 1, bseed, 9, 8, 16, bpcm.data(), 128) == MY_INPUT_OK);
    assert(read(d, b, bs, 3, 8, 16) == bpcm);
    zeros(read(d, a, as, 2, 32, 8));
    // Compute once, fan out the exact same mix; independent clocks/consumers remain unproved.
    auto shared = fixture(16);
    for (auto& sample : shared) sample *= 0.5f;
    assert(MixyardInputTest_Disconnect(a, 1) == MY_INPUT_OK);
    assert(MixyardInputTest_Admit(a, 2) == MY_INPUT_OK);
    assert(MixyardInputTest_Packet(a, 2, aseed, 10, 40, 16, shared.data(), 128) == MY_INPUT_OK);
    assert(MixyardInputTest_Packet(b, 1, bseed, 10, 24, 16, shared.data(), 128) == MY_INPUT_OK);
    assert(read(d, a, as, 1, 40, 16) == shared);
    assert(read(d, b, bs, 3, 24, 16) == shared);
    // Exercise each half of the dual device/stream post-change notification.
    rejectNoticeSelector = kAudioObjectPropertyName;
    for (auto failedObject : {b, bs}) {
        rejectNoticeObject = failedObject;
        const auto count = noticeCalls.size();
        const char* newName = failedObject == b ? "Device pending" : "Stream pending";
        CFStringRef expectedName = failedObject == b ? CFSTR("Device pending") : CFSTR("Stream pending");
        assert(MixyardInputTest_Rename(b, newName) == MY_INPUT_COMMITTED_NOTIFY_PENDING);
        assert(noticeCalls.size() == count + 2);
        assert(noticeCalls[count].object == b && noticeCalls[count + 1].object == bs);
        assert((noticeCalls[count].status != noErr) == (failedObject == b));
        assert((noticeCalls[count + 1].status != noErr) == (failedObject == bs));
        named(d, b, expectedName); named(d, bs, expectedName);
        reconcile(1); assert(noticeCalls.size() == count + 3 && noticeCalls.back().object == failedObject);
        rejectNoticeObject = kAudioObjectUnknown; reconcile(0);
        assert(noticeCalls.size() == count + 4 && noticeCalls.back().object == failedObject);
    }
    rejectAllNotices = true;
    assert(MixyardInputTest_Rename(b, "Both pending") == MY_INPUT_COMMITTED_NOTIFY_PENDING);
    named(d, b, CFSTR("Both pending")); named(d, bs, CFSTR("Both pending"));
    reconcile(2); rejectAllNotices = false; reconcile(0);
    const auto afterRename = noticeCalls.size(); reconcile(0); assert(noticeCalls.size() == afterRename);
    // A host callback may synchronously change the same names while we notify.
    // Its newer notices survive the current pass, which cannot recurse or spin.
    reentrantRenameDevice = b;
    assert(MixyardInputTest_Rename(b, "Before reentry") == MY_INPUT_COMMITTED_NOTIFY_PENDING);
    assert(noticeCalls.size() == afterRename + 2);
    named(d, b, CFSTR("Reentrant B")); named(d, bs, CFSTR("Reentrant B"));
    reconcile(0); assert(noticeCalls.size() == afterRename + 4);
    assert(MixyardInputTest_Rename(b, "Renamed B") == MY_INPUT_OK);
    assert(translate(d, "org.mixyard.input.b670ff62-f0de-490a-a0f6-9c748bc75343") == b);
    assert(list(d, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList) == devices);
    assert(list(d, kAudioObjectPlugInObject, kAudioObjectPropertyOwnedObjects) == devices);
    assert(MixyardInputTest_PublishEndpoint("fa1bf82a-087b-428b-b274-c42b49d147fa", "C", &out) == MY_INPUT_CAPACITY);

    uint64_t action = 0;
    assert(MixyardInputTest_Retire(b, &action) == MY_INPUT_OK);
    assert(requestedDevice == b && requestedAction == action && action);
    zeros(read(d, b, bs, 3, 40, 8));
    assert((*d)->AbortDeviceConfigurationChange(d, b, action, nullptr) == noErr);
    assert(list(d, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList).size() == 2);
    assert(MixyardInputTest_Admit(b, 2) == MY_INPUT_OK);
    assert(MixyardInputTest_Packet(b, 1, bseed, 11, 48, 16, shared.data(), 128) != MY_INPUT_OK);
    assert(MixyardInputTest_Packet(b, 2, bseed, 11, 48, 16, shared.data(), 128) == MY_INPUT_OK);
    assert(read(d, b, bs, 3, 48, 16) == shared);
    rejectRequest = true;
    assert(MixyardInputTest_Retire(b, &action) == MY_INPUT_HOST_ERROR);
    rejectRequest = false;
    zeros(read(d, b, bs, 3, 64, 8));
    assert(MixyardInputTest_Retire(b, &action) == MY_INPUT_OK);
    rejectAllNotices = true;
    assert((*d)->PerformDeviceConfigurationChange(d, b, action, nullptr) == noErr);
    reconcile(2); // retirement committed; alive and registry notifications both failed
    assert((*d)->PerformDeviceConfigurationChange(d, b, action, nullptr) == kAudioHardwareIllegalOperationError);
    assert(list(d, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList) == std::vector<AudioObjectID>{a});
    assert(list(d, kAudioObjectPlugInObject, kAudioObjectPropertyOwnedObjects) == std::vector<AudioObjectID>{a});
    assert(translate(d, "org.mixyard.input.b670ff62-f0de-490a-a0f6-9c748bc75343") == kAudioObjectUnknown);
    rejectAllNotices = false; reconcile(0);
    assert(MixyardInputTest_PublishEndpoint("fa1bf82a-087b-428b-b274-c42b49d147fa", "C", &out) == MY_INPUT_CAPACITY);
    zeros(read(d, b, bs, 3, 72, 8)); // static tombstone; late callback cannot replay
    stop(d, b, 3);
    // Silence tail exceeds retained ring without exceeding one callback's bounded size.
    for (unsigned i = 0; i < 5; ++i) zeros(read(d, a, as, 1, 56 + i * 4096, 4096));
    stop(d, a, 1); stop(d, a, 2);
    start(d, a, 4); const auto newSeed = seed(d, a, 4); assert(newSeed != aseed);
    assert(MixyardInputTest_Admit(a, 2) != MY_INPUT_OK);
    zeros(read(d, a, as, 4, 0, 8));
    assert(MixyardInputTest_Admit(a, 3) == MY_INPUT_OK);
    assert(MixyardInputTest_Packet(a, 3, aseed, 12, 8, 16, shared.data(), 128) != MY_INPUT_OK);
    assert(MixyardInputTest_Packet(a, 3, newSeed, 12, 8, 16, shared.data(), 128) == MY_INPUT_OK);
    assert(read(d, a, as, 4, 8, 16) == shared);
    // Overwrite older ring positions; a lagging reader gets zeros, another gets latest PCM.
    start(d, a, 5);
    auto bulk = fixture(4096);
    for (unsigned i = 0; i < 5; ++i)
        assert(MixyardInputTest_Packet(a, 3, newSeed, 13, 24 + i * 4096, 4096, bulk.data(), bulk.size() * 4) == MY_INPUT_OK);
    zeros(read(d, a, as, 4, 24, 4096));
    assert(read(d, a, as, 5, 24 + 4 * 4096, 4096) == bulk);
    stop(d, a, 5);
    stop(d, a, 4); (*d)->Release(d);
    std::printf("{\"schema_version\":1,\"status\":\"pass\",\"evidence_kind\":\"in_process_real_Apple_ABI\","
        "\"native_HAL_PCM\":\"BLOCKED\",\"producer_authentication\":\"BLOCKED\","
        "\"production_apply\":false,\"property_notifications\":%u,"
        "\"checks\":[\"signed_release_ABI_silence\",\"COM_lifetime\",\"property_errors_stereo\","
        "\"A_B_first_tail_exact_samples\",\"half_gain_fanout_reader_isolation\","
        "\"stale_length_nonfinite_rejection\",\"effective_revision\","
        "\"rename_retire_abort_remove_reopen_epoch\",\"committed_lifecycle_notification_errors\","
        "\"publication_rename_partial_notification_reconciliation\"]}\n", notifications);
}

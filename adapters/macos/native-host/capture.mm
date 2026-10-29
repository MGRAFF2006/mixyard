// SPDX-License-Identifier: MIT
#include "capture.hpp"
#include "capture_api.hpp"
#include "owned_queue.hpp"
#include <Block.h>
#include <algorithm>
#include <unistd.h>

namespace mixyard::macos {
namespace w = worker;
namespace {
void checked(OSStatus status, const char* operation) {
    if (status != noErr) throw std::runtime_error(std::string(operation) + ": OSStatus " + std::to_string(status));
}
}
struct Capture::Impl {
    const Selection selection;
    const w::Binding binding;
    const Timeline timeline;
    const detail::CaptureApi api;
    AudioObjectID tap = kAudioObjectUnknown;
    AudioDeviceID aggregate = kAudioObjectUnknown;
    AudioDeviceIOProcID io = nullptr;
    bool startAttempted = false, closed = false;
    std::string uid;
    OwnedQueue queue;
    std::unique_ptr<w::CaptureIngress> ingress;
    dispatch_queue_t control = nullptr;
    struct State { Impl* owner; };
    std::shared_ptr<State> state;
    AudioObjectPropertyListenerBlock listener = nullptr;
    struct Notice { AudioObjectID object; AudioObjectPropertyAddress address; };
    std::array<Notice, 5> notices{};
    size_t count = 0;
    Impl(Selection s, w::Binding b, Timeline t) : selection(std::move(s)), binding(b), timeline(t),
#ifdef MIXYARD_NATIVE_HOST_TEST
        api(detail::captureTestApi)
#else
        api()
#endif
    {
        state = std::make_shared<State>(State{this});
        control = dispatch_queue_create("org.mixyard.host.capture", DISPATCH_QUEUE_SERIAL);
        if (!control) throw std::runtime_error("Capture control queue unavailable");
        const auto shared = state;
        listener = Block_copy(^(UInt32, const AudioObjectPropertyAddress*) {
            if (shared->owner) shared->owner->revokeOwned();
        });
    }
    ~Impl() {
        if (!close()) std::terminate();
        Block_release(listener); dispatch_release(control);
    }
    template<class F> void sync(F function) {
        std::exception_ptr error; auto* target = &error;
        dispatch_sync(control, ^{ try { function(); } catch (...) { *target = std::current_exception(); } });
        if (error) std::rethrow_exception(error);
    }
    template<class T> T property(AudioObjectID object, AudioObjectPropertySelector selector) {
        const AudioObjectPropertyAddress a{selector, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        T value{}; UInt32 bytes = sizeof(value);
        checked(api.data(object, &a, 0, nullptr, &bytes, &value), "read capture property");
        if (bytes != sizeof(value)) throw std::runtime_error("Capture property changed size");
        return value;
    }
    void verifyProcess() {
        const AudioObjectPropertyAddress a{kAudioHardwarePropertyProcessObjectList,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        UInt32 size = 0;
        checked(api.size(kAudioObjectSystemObject, &a, 0, nullptr, &size), "size observed process list");
        if (!size || size % sizeof(AudioObjectID) || size > 4096 * sizeof(AudioObjectID))
            throw std::runtime_error("Unbounded/empty process inventory");
        std::vector<AudioObjectID> ids(size / sizeof(AudioObjectID)); const auto capacity = size;
        checked(api.data(kAudioObjectSystemObject, &a, 0, nullptr, &size, ids.data()), "read process inventory");
        if (size != capacity || std::count(ids.begin(), ids.end(), selection.process) != 1 ||
            property<pid_t>(selection.process, kAudioProcessPropertyPID) != selection.observedPid)
            throw std::runtime_error("Observed process lost or PID changed");
    }
    std::string tapUid() {
        CFStringRef value = property<CFStringRef>(tap, kAudioTapPropertyUID);
        if (!value) throw std::runtime_error("Tap UID unavailable");
        std::array<char, 4096> text{};
        const bool ok = CFStringGetCString(value, text.data(), text.size(), kCFStringEncodingUTF8);
        CFRelease(value);
        if (!ok || !text[0]) throw std::runtime_error("Invalid observed tap UID");
        return text.data();
    }
    void listen(AudioObjectID object, AudioObjectPropertySelector selector) {
        const AudioObjectPropertyAddress a{selector, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        checked(api.listen(object, &a, control, listener), "watch scoped capture");
        notices[count++] = {object, a};
    }
    void verify() {
        verifyProcess();
        if (tapUid() != uid || !w::exactFloat(property<AudioStreamBasicDescription>(tap, kAudioTapPropertyFormat), queue->format()))
            throw std::runtime_error("Observed tap identity/format changed");
        if (aggregate && property<UInt32>(aggregate, kAudioDevicePropertyDeviceIsAlive) != 1)
            throw std::runtime_error("Owned capture aggregate lost");
    }
    static OSStatus receive(AudioObjectID, const AudioTimeStamp* now, const AudioBufferList* input,
        const AudioTimeStamp* inputTime, AudioBufferList*, const AudioTimeStamp*, void* context) noexcept {
        auto& self = *static_cast<Impl*>(context);
        self.ingress->receive(now, input, inputTime);
        return noErr;
    }
    void openOwned() {
        if (!selection.consentToStereoTap || !selection.process || selection.observedPid <= 0 ||
            selection.observedPid == getpid() ||
            std::find(selection.excludedProcesses.begin(), selection.excludedProcesses.end(), selection.process) != selection.excludedProcesses.end())
            throw std::invalid_argument("Explicit consent for observed non-producer process required");
        verifyProcess();
        // Explicit stereo source operation, never a claim of raw multichannel
        // process PCM. Preserve the actual returned tap format without narrowing.
        CATapDescription* description = [[CATapDescription alloc] initStereoMixdownOfProcesses:@[@(selection.process)]];
        description.name = @"Mixyard scoped native-host source";
        [description setPrivate:YES]; description.muteBehavior = CATapUnmuted;
        const auto status = api.tap(description, &tap);
        [description release];
        checked(status, "create consented process tap (macOS permission still required)");
        if (!tap) throw std::runtime_error("Tap creation returned no owned object");
        const auto actual = property<AudioStreamBasicDescription>(tap, kAudioTapPropertyFormat);
        if (!std::isfinite(actual.mSampleRate) || actual.mSampleRate < 1 || actual.mSampleRate > UINT32_MAX ||
            std::floor(actual.mSampleRate) != actual.mSampleRate || actual.mChannelsPerFrame != 2 ||
            (actual.mBitsPerChannel != 32 && actual.mBitsPerChannel != 64))
            throw std::invalid_argument("Unsupported exact stereo tap format");
        uid = tapUid();
        queue = ownQueue(w::Format{static_cast<uint32_t>(actual.mSampleRate), {"FL", "FR"},
            actual.mBitsPerChannel == 64 ? w::Precision::Float64 : w::Precision::Float32}, binding, timeline.id(),
            w::Provenance{w::Origin::ProcessTap, uid});
        ingress = std::make_unique<w::CaptureIngress>(*queue, actual, 1000000000);
        NSString* tapName = [NSString stringWithUTF8String:uid.c_str()];
        NSDictionary* specification = @{
            @(kAudioAggregateDeviceNameKey): @"Mixyard private native-host capture",
            @(kAudioAggregateDeviceUIDKey): [@"org.mixyard.native-host." stringByAppendingString:NSUUID.UUID.UUIDString],
            @(kAudioAggregateDeviceIsPrivateKey): @YES,
            @(kAudioAggregateDeviceTapAutoStartKey): @YES,
            @(kAudioAggregateDeviceTapListKey): @[@{@(kAudioSubTapUIDKey): tapName, @(kAudioSubTapDriftCompensationKey): @NO}]
        };
        checked(api.aggregate((__bridge CFDictionaryRef)specification, &aggregate), "create owned private tap-only aggregate");
        if (!aggregate) throw std::runtime_error("Aggregate creation returned no owned object");
        listen(selection.process, kAudioProcessPropertyPID);
        listen(kAudioObjectSystemObject, kAudioHardwarePropertyProcessObjectList);
        listen(tap, kAudioTapPropertyUID); listen(tap, kAudioTapPropertyFormat);
        listen(aggregate, kAudioDevicePropertyDeviceIsAlive);
        verify();
        checked(api.create(aggregate, receive, this, &io), "create owned capture callback");
        if (!io) throw std::runtime_error("Capture callback missing");
        if (queue->fault() != w::Fault::None) throw std::runtime_error("Capture revoked during admission");
    }
    void startOwned() {
        if (closed || startAttempted || !io || queue->fault() != w::Fault::None)
            throw std::runtime_error("Capture is closed, started or revoked");
        try {
            verify(); startAttempted = true;
            checked(api.start(aggregate, io), "start consented capture"); verify();
            if (queue->fault() != w::Fault::None) throw std::runtime_error("Capture revoked during start");
        } catch (...) { revokeOwned(); throw; }
    }
    bool revokeOwned() noexcept {
        if (queue) queue->revoke();
        if (startAttempted) {
            if (api.stop(aggregate, io) != noErr) return false;
            startAttempted = false;
        }
        return true;
    }
    bool closeOwned() noexcept {
        if (closed) return true;
        if (!revokeOwned()) return false;
        if (io) { if (api.destroy(aggregate, io) != noErr) return false; io = nullptr; }
        while (count) {
            const auto& n = notices[count - 1];
            if (api.unlisten(n.object, &n.address, control, listener) != noErr) return false;
            --count;
        }
        if (aggregate) { if (api.unaggregate(aggregate) != noErr) return false; aggregate = 0; }
        if (tap) { if (api.untap(tap) != noErr) return false; tap = 0; }
        state->owner = nullptr; closed = true;
        return true;
    }
    bool close() noexcept {
        __block bool ok = false; dispatch_sync(control, ^{ ok = closeOwned(); }); return ok;
    }
};
Capture::Capture(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}
Capture::~Capture() = default;
std::unique_ptr<Capture> Capture::open(Selection selection, w::Binding binding, Timeline timeline) {
    auto impl = std::make_unique<Impl>(std::move(selection), binding, timeline);
    impl->sync([&] { impl->openOwned(); });
    return std::unique_ptr<Capture>(new Capture(std::move(impl)));
}
w::Queue& Capture::queue() noexcept { return *impl_->queue; }
const std::string& Capture::observedId() const noexcept { return impl_->uid; }
void Capture::start() { impl_->sync([&] { impl_->startOwned(); }); }
bool Capture::revoke() noexcept {
    bool ok = false; impl_->sync([&] { ok = impl_->revokeOwned(); }); return ok;
}
bool Capture::close() noexcept { return impl_->close(); }
} // namespace mixyard::macos

// SPDX-License-Identifier: MIT
#include "output_only.hpp"
#include "api.hpp"
#include "../native_worker.hpp"
#include <Block.h>
#include <algorithm>
#include <exception>

namespace mixyard::output {
using namespace worker;
using detail::Api;
namespace {
void checked(OSStatus status, const char* operation) {
    if (status != noErr)
        throw std::runtime_error(std::string(operation) + ": OSStatus " + std::to_string(status));
}
}
struct Worker::Impl {
    Queue& queue;
    Renderer renderer;
    HostClock clock;
    const std::string uid;
    const Api api;
    AudioUnit unit = nullptr;
    AudioDeviceID device = kAudioObjectUnknown;
    AudioStreamID stream = kAudioObjectUnknown;
    bool initialized = false, startAttempted = false, closed = false;
    dispatch_queue_t control = nullptr;
    // A listener block may already be queued when removal completes. Its state
    // outlives this object; all owner reads/writes occur on the control queue.
    struct ListenerState { Impl* owner; };
    std::shared_ptr<ListenerState> listenerState;
    AudioObjectPropertyListenerBlock listener = nullptr;
    struct Notice { AudioObjectID object; AudioObjectPropertyAddress address; };
    std::array<Notice, 9> notices{};
    size_t noticeCount = 0;
    alignas(8) std::array<unsigned char, maxBytes> scratch{};

    Impl(std::string selected, Queue& q) : queue(q), renderer(q), uid(std::move(selected)),
#ifdef MIXYARD_OUTPUT_API_TEST
        api(detail::testApi)
#else
        api()
#endif
    {
        listenerState = std::make_shared<ListenerState>(ListenerState{this});
        control = dispatch_queue_create("org.mixyard.output-only.control", DISPATCH_QUEUE_SERIAL);
        if (!control) throw std::runtime_error("Output control queue unavailable");
        const auto state = listenerState;
        listener = Block_copy(^(UInt32, const AudioObjectPropertyAddress*) {
            if (state->owner) state->owner->revokeOwned();
        });
    }
    ~Impl() {
        if (!close()) std::terminate();
        Block_release(listener);
        dispatch_release(control);
    }
    template<class F> void sync(F function) {
        std::exception_ptr error;
        auto* result = &error;
        dispatch_sync(control, ^{
            try { function(); } catch (...) { *result = std::current_exception(); }
        });
        if (error) std::rethrow_exception(error);
    }
    template<class T> T property(AudioObjectID object, AudioObjectPropertySelector selector,
                                AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
        const AudioObjectPropertyAddress address{selector, scope, kAudioObjectPropertyElementMain};
        T value{}; UInt32 bytes = sizeof(value);
        checked(api.data(object, &address, 0, nullptr, &bytes, &value), "read selected output property");
        if (bytes != sizeof(value)) throw std::runtime_error("Selected output property size changed");
        return value;
    }
    std::string deviceUid(AudioDeviceID id) {
        const auto text = property<CFStringRef>(id, kAudioDevicePropertyDeviceUID);
        if (!text) throw std::runtime_error("Output UID unavailable");
        std::array<char, 4096> buffer{};
        const bool ok = CFStringGetCString(text, buffer.data(), buffer.size(), kCFStringEncodingUTF8);
        CFRelease(text);
        if (!ok || !buffer[0]) throw std::runtime_error("Invalid output UID");
        return buffer.data();
    }
    template<class T> T get(AudioUnitPropertyID property, AudioUnitScope scope, AudioUnitElement bus) {
        T value{}; UInt32 size = sizeof(value);
        checked(api.get(unit, property, scope, bus, &value, &size), "read output AudioUnit property");
        if (size != sizeof(value)) throw std::runtime_error("AudioUnit property size changed");
        return value;
    }
    template<class T> void set(AudioUnitPropertyID property, AudioUnitScope scope,
                               AudioUnitElement bus, const T& value) {
        checked(api.set(unit, property, scope, bus, &value, sizeof(value)), "configure private output AudioUnit");
    }
    UInt32 limit() const noexcept {
        return std::min(maxFrames, maxBytes / queue.format().frameBytes());
    }
    void verifyDevice() {
        if (deviceUid(device) != uid || property<UInt32>(device, kAudioDevicePropertyDeviceIsAlive) != 1 ||
            property<Float64>(device, kAudioDevicePropertyNominalSampleRate) != queue.format().sampleRateHz)
            throw std::runtime_error("Selected output UID/alive/rate changed");
        const AudioObjectPropertyAddress address{kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeOutput, kAudioObjectPropertyElementMain};
        UInt32 size = 0;
        checked(api.size(device, &address, 0, nullptr, &size), "size selected output streams");
        if (size != sizeof(AudioStreamID)) throw std::runtime_error("Exactly one output stream required");
        AudioStreamID actual = kAudioObjectUnknown;
        checked(api.data(device, &address, 0, nullptr, &size, &actual), "read selected output stream");
        if (size != sizeof(actual) || actual == kAudioObjectUnknown || (stream && actual != stream))
            throw std::runtime_error("Selected output stream changed");
        stream = actual;
        if (!exactFloat(property<AudioStreamBasicDescription>(stream, kAudioStreamPropertyVirtualFormat), queue.format()) ||
            !exactFloat(property<AudioStreamBasicDescription>(stream, kAudioStreamPropertyPhysicalFormat), queue.format()))
            throw std::runtime_error("Explicit destination conversion required: physical/virtual format differs");
        if (property<UInt32>(stream, kAudioStreamPropertyStartingChannel) != 1)
            throw std::runtime_error("Single stream must start at native channel 1");
        for (size_t i = 0; i < queue.format().channels.size(); ++i)
            if (queue.format().channels[i] != "CA:" + std::to_string(i + 1))
                throw std::runtime_error("Explicit ordered CA:1..N destination channels required");
    }
    void verifyUnit() {
        if (get<UInt32>(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1) != 0 ||
            get<UInt32>(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, 0) != 1 ||
            get<AudioDeviceID>(kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0) != device)
            throw std::runtime_error("AudioUnit input-disable/output/device proof failed");
        for (const auto scope : {kAudioUnitScope_Input, kAudioUnitScope_Output}) {
            const auto actual = get<AudioStreamBasicDescription>(kAudioUnitProperty_StreamFormat, scope, 0);
            if (!exactFloat(actual, queue.format()))
                throw std::runtime_error("AudioUnit conversion would change destination PCM");
            if (scope == kAudioUnitScope_Input &&
                bool(actual.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != clientPlanar)
                throw std::runtime_error("AudioUnit client storage layout changed after admission");
        }
        if (get<UInt32>(kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0) != limit())
            throw std::runtime_error("AudioUnit callback bound changed");
        std::array<SInt32, maxChannels> map{};
        UInt32 size = static_cast<UInt32>(queue.format().channels.size() * sizeof(SInt32));
        const auto expected = size;
        checked(api.get(unit, kAudioOutputUnitProperty_ChannelMap, kAudioUnitScope_Input, 0, map.data(), &size),
                "read output identity channel map");
        if (size != expected) throw std::runtime_error("AudioUnit channel map size changed");
        for (size_t i = 0; i < queue.format().channels.size(); ++i)
            if (map[i] != static_cast<SInt32>(i)) throw std::runtime_error("AudioUnit channel mapping changed");
    }
    void listen(AudioObjectID object, AudioObjectPropertySelector selector,
                AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
        const AudioObjectPropertyAddress address{selector, scope, kAudioObjectPropertyElementMain};
        checked(api.listen(object, &address, control, listener), "watch output topology");
        notices[noticeCount++] = {object, address};
    }
    void openOwned() {
        const AudioObjectPropertyAddress inventory{kAudioHardwarePropertyDevices,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        UInt32 bytes = 0;
        checked(api.size(kAudioObjectSystemObject, &inventory, 0, nullptr, &bytes), "size output inventory");
        if (!bytes || bytes % sizeof(AudioDeviceID) || bytes > 1024 * sizeof(AudioDeviceID))
            throw std::runtime_error("Unsupported output inventory size");
        std::vector<AudioDeviceID> devices(bytes / sizeof(AudioDeviceID));
        const auto capacity = bytes;
        checked(api.data(kAudioObjectSystemObject, &inventory, 0, nullptr, &bytes, devices.data()), "read output inventory");
        if (bytes != capacity) throw std::runtime_error("Output inventory changed during admission");
        for (auto id : devices) if (deviceUid(id) == uid) {
            if (device) throw std::runtime_error("Ambiguous output UID");
            device = id;
        }
        if (!device) throw std::runtime_error("Selected output UID absent");
        verifyDevice();
        listen(kAudioObjectSystemObject, kAudioHardwarePropertyDevices);
        listen(device, kAudioDevicePropertyDeviceUID);
        listen(device, kAudioDevicePropertyDeviceIsAlive);
        listen(device, kAudioDevicePropertyNominalSampleRate);
        listen(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
        listen(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
        listen(stream, kAudioStreamPropertyVirtualFormat);
        listen(stream, kAudioStreamPropertyPhysicalFormat);
        listen(stream, kAudioStreamPropertyStartingChannel);
        const AudioComponentDescription description{kAudioUnitType_Output, kAudioUnitSubType_HALOutput,
            kAudioUnitManufacturer_Apple, 0, 0};
        const auto component = api.find(nullptr, &description);
        if (!component) throw std::runtime_error("Apple HAL output AudioUnit unavailable");
        checked(api.create(component, &unit), "create private HAL output AudioUnit");
        if (!unit) throw std::runtime_error("HAL output AudioUnit missing");
        set(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1, UInt32{0});
        if (get<UInt32>(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1) != 0)
            throw std::runtime_error("Native input I/O remains enabled");
        set(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, 0, UInt32{1});
        set(kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, device);
        // AUHAL flattens a single stream. Only storage transposition is allowed.
        auto client = get<AudioStreamBasicDescription>(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0);
        if (!exactFloat(client, queue.format())) throw std::runtime_error("AUHAL device format differs");
        set(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, client);
        // The AudioUnit may canonicalize storage layout without changing PCM.
        // Freeze its actual client readback before registering the callback,
        // then require that same layout at all later admission/start checks.
        client = get<AudioStreamBasicDescription>(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0);
        if (!exactFloat(client, queue.format())) throw std::runtime_error("AUHAL client format differs");
        clientPlanar = client.mFormatFlags & kAudioFormatFlagIsNonInterleaved;
        std::array<SInt32, maxChannels> map{};
        for (size_t i = 0; i < queue.format().channels.size(); ++i) map[i] = static_cast<SInt32>(i);
        checked(api.set(unit, kAudioOutputUnitProperty_ChannelMap, kAudioUnitScope_Input, 0, map.data(),
                        static_cast<UInt32>(queue.format().channels.size() * sizeof(SInt32))), "set identity output channel map");
        set(kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, limit());
        const AURenderCallbackStruct callback{render, this};
        set(kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, callback);
        verifyDevice(); verifyUnit(); // Input proof before initialize, including after binding.
        checked(api.initialize(unit), "initialize output-only AudioUnit");
        initialized = true;
        verifyDevice(); verifyUnit();
        if (queue.fault() != Fault::None) throw std::runtime_error("Output revoked during admission");
    }
    bool revokeOwned() noexcept {
        queue.revoke();
        // A failed start can have partial effects. Stop even after start failure.
        if (startAttempted) {
            if (api.stop(unit) != noErr) return false;
            startAttempted = false;
        }
        return true;
    }
    void startOwned() {
        if (closed || !initialized || startAttempted || queue.fault() != Fault::None)
            throw std::runtime_error("Output is closed, started or revoked");
        try {
            verifyDevice(); verifyUnit(); // No disable, format or device mutation after admission.
            if (queue.fault() != Fault::None) throw std::runtime_error("Output queue revoked before start");
            startAttempted = true;
            checked(api.start(unit), "start output-only AudioUnit");
            verifyDevice(); verifyUnit();
            if (queue.fault() != Fault::None) throw std::runtime_error("Output failed during start");
        } catch (...) {
            revokeOwned(); // Retain stop ownership after a failed stop for close/revoke retry.
            throw;
        }
    }
    bool closeOwned() noexcept {
        if (closed) return true;
        if (!revokeOwned()) return false;
        if (initialized) {
            if (api.uninitialize(unit) != noErr) return false;
            initialized = false;
        }
        if (unit) {
            if (api.dispose(unit) != noErr) return false;
            unit = nullptr;
        }
        while (noticeCount) {
            const auto& notice = notices[noticeCount - 1];
            if (api.unlisten(notice.object, &notice.address, control, listener) != noErr) return false;
            --noticeCount;
        }
        listenerState->owner = nullptr;
        closed = true;
        return true;
    }
    bool close() noexcept {
        __block bool ok = false;
        dispatch_sync(control, ^{ ok = closeOwned(); });
        return ok;
    }
    // The output render pull callback has no device-input argument. Native input
    // bus 1 is disabled on the private unit and never enabled by this library.
    static OSStatus render(void* context, AudioUnitRenderActionFlags* flags, const AudioTimeStamp* time,
                           UInt32 bus, UInt32 frames, AudioBufferList* output) noexcept {
        auto& self = *static_cast<Impl*>(context);
        auto& queue = self.queue;
        // Malformed/unbounded HAL buffers cannot be traversed safely. Return an
        // error and mark silence; only admitted buffers can receive successful PCM.
        if (flags) *flags |= kAudioUnitRenderAction_OutputIsSilence;
        if (!output || output->mNumberBuffers > maxChannels) {
            queue.fail(Fault::Shape); return kAudio_ParamError;
        }
        for (UInt32 i = 0; i < output->mNumberBuffers; ++i)
            if (output->mBuffers[i].mData && output->mBuffers[i].mDataByteSize <= maxBytes)
                std::memset(output->mBuffers[i].mData, 0, output->mBuffers[i].mDataByteSize);
        const auto channels = static_cast<UInt32>(queue.format().channels.size());
        // Client format is frozen after open; query-free callback uses the stored
        // AU device storage layout, captured before initialize.
        const bool planar = self.clientPlanar;
        const auto laneBytes = uint64_t(frames) * (planar ? queue.format().sampleBytes() : queue.format().frameBytes());
        if (bus != 0 || !frames || frames > self.limit() ||
            output->mNumberBuffers != (planar ? channels : 1u)) {
            queue.fail(Fault::Shape); return kAudio_ParamError;
        }
        for (UInt32 i = 0; i < output->mNumberBuffers; ++i)
            if (!output->mBuffers[i].mData || output->mBuffers[i].mDataByteSize != laneBytes ||
                output->mBuffers[i].mNumberChannels != (planar ? 1u : channels)) {
                queue.fail(Fault::Shape); return kAudio_ParamError;
            }
        if (queue.fault() != Fault::None) return noErr;
        uint64_t first = 0;
        if (!samplePosition(time, first) || !(time->mFlags & kAudioTimeStampHostTimeValid)) {
            queue.fail(Fault::Clock); return noErr;
        }
        const auto scheduled = self.clock.nanos(time->mHostTime);
        if (scheduled < self.clock.now()) { queue.fail(Fault::Expired); return noErr; }
        const auto total = frames * queue.format().frameBytes();
        if (!self.renderer.render(scheduled, first, frames, self.scratch.data(), total)) return noErr;
        const auto width = queue.format().sampleBytes();
        if (planar) {
            for (UInt32 frame = 0; frame < frames; ++frame)
                for (UInt32 channel = 0; channel < channels; ++channel)
                    std::memcpy(static_cast<unsigned char*>(output->mBuffers[channel].mData) + frame * width,
                                self.scratch.data() + (frame * channels + channel) * width, width);
        } else std::memcpy(output->mBuffers[0].mData, self.scratch.data(), total);
        if (queue.fault() != Fault::None) {
            for (UInt32 i = 0; i < output->mNumberBuffers; ++i)
                std::memset(output->mBuffers[i].mData, 0, output->mBuffers[i].mDataByteSize);
        } else if (flags) *flags &= ~kAudioUnitRenderAction_OutputIsSilence;
        return noErr;
    }
    bool clientPlanar = false;
};
Worker::Worker(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}
Worker::~Worker() = default;
std::unique_ptr<Worker> Worker::open(std::string uid, Queue& queue) {
    if (uid.empty() || uid.size() > 4095 || uid.find('\0') != std::string::npos ||
        queue.provenance().origin != Origin::AdaptedEngine || queue.fault() != Fault::None)
        throw std::invalid_argument("Explicit output UID and fresh adapted destination queue required");
    auto impl = std::make_unique<Impl>(std::move(uid), queue);
    impl->sync([&] { impl->openOwned(); });
    return std::unique_ptr<Worker>(new Worker(std::move(impl)));
}
void Worker::start() { impl_->sync([&] { impl_->startOwned(); }); }
bool Worker::revoke() noexcept {
    __block bool ok = false;
    dispatch_sync(impl_->control, ^{ ok = impl_->revokeOwned(); });
    return ok;
}
bool Worker::close() noexcept { return impl_->close(); }
} // namespace mixyard::output

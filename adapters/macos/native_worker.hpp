// SPDX-License-Identifier: MIT
#pragma once
#include "worker_bridge.hpp"
#include <CoreAudio/CoreAudio.h>
#include <mach/mach_time.h>
#include <exception>
#include <memory>

namespace mixyard::worker {
// Immutable conversion factors are queried off callbacks. Arithmetic thereafter
// uses the same host monotonic domain as the Core Audio host timestamps.
class HostClock {
    mach_timebase_info_data_t scale_{};
public:
    HostClock() {
        if (mach_timebase_info(&scale_) != KERN_SUCCESS || !scale_.numer || !scale_.denom)
            throw std::runtime_error("Unavailable Core Audio monotonic timebase");
    }
    uint64_t nanos(uint64_t ticks) const noexcept {
        const __uint128_t value = __uint128_t(ticks) * scale_.numer / scale_.denom;
        return value > UINT64_MAX ? UINT64_MAX : static_cast<uint64_t>(value);
    }
    uint64_t now() const noexcept { return nanos(mach_absolute_time()); }
};

inline bool exactFloat(const AudioStreamBasicDescription& f, const Format& expected) noexcept {
    const auto allowed = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
    const auto sampleBytes = expected.sampleBytes();
    const auto frameBytes = (f.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? sampleBytes : expected.frameBytes();
    return f.mFormatID == kAudioFormatLinearPCM &&
        (f.mFormatFlags & (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked)) ==
            (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked) && !(f.mFormatFlags & ~allowed) &&
        f.mSampleRate == expected.sampleRateHz && f.mChannelsPerFrame == expected.channels.size() &&
        f.mBitsPerChannel == sampleBytes * 8 && f.mFramesPerPacket == 1 &&
        f.mBytesPerFrame == frameBytes && f.mBytesPerPacket == frameBytes;
}

inline bool samplePosition(const AudioTimeStamp* time, uint64_t& frame) noexcept {
    // Above 2^53 a double cannot distinguish every adjacent frame.
    if (!time || !(time->mFlags & kAudioTimeStampSampleTimeValid) ||
        !std::isfinite(time->mSampleTime) || time->mSampleTime < 0 ||
        time->mSampleTime > 9007199254736895.0 || std::floor(time->mSampleTime) != time->mSampleTime)
        return false;
    frame = static_cast<uint64_t>(time->mSampleTime);
    return true;
}

// Converts only buffer storage layout. Numeric precision/rate/order are fixed.
// The queue's clockId remains a SOURCE clock here; a host must explicitly adapt
// it before producing an AdaptedEngine queue for a destination clock.
class CaptureIngress {
    Queue& queue_;
    const bool planar_;
    const uint64_t freshnessNs_;
    HostClock clock_;
    uint64_t sequence_ = 0;
public:
    CaptureIngress(Queue& queue, const AudioStreamBasicDescription& actual, uint64_t freshnessNs)
        : queue_(queue), planar_(actual.mFormatFlags & kAudioFormatFlagIsNonInterleaved), freshnessNs_(freshnessNs) {
        if (!exactFloat(actual, queue.format()) || !freshnessNs || freshnessNs > 2000000000)
            throw std::invalid_argument("Capture requires exact Float32/64 ASBD and a <=2s freshness bound");
    }
    bool receive(const AudioTimeStamp* now, const AudioBufferList* input,
                 const AudioTimeStamp* inputTime) noexcept {
        if (queue_.fault() != Fault::None) return false;
        uint64_t first = 0;
        if (!samplePosition(inputTime, first) || !now || !(now->mFlags & kAudioTimeStampHostTimeValid) ||
            !(inputTime->mFlags & kAudioTimeStampHostTimeValid)) return queue_.fail(Fault::Clock);
        const auto channels = static_cast<uint32_t>(queue_.format().channels.size());
        if (!input || input->mNumberBuffers != (planar_ ? channels : 1u)) return queue_.fail(Fault::Shape);
        std::array<const void*, maxChannels> pointers{};
        std::array<uint32_t, maxChannels> sizes{};
        const auto frameBytes = planar_ ? queue_.format().sampleBytes() : queue_.format().frameBytes();
        const auto bytes = input->mBuffers[0].mDataByteSize;
        if (bytes % frameBytes) return queue_.fail(Fault::Shape);
        const auto frames = bytes / frameBytes;
        if (!frames) return true; // A zero-frame native cycle publishes no sequence.
        for (uint32_t i = 0; i < input->mNumberBuffers; ++i) {
            if (input->mBuffers[i].mNumberChannels != (planar_ ? 1u : channels)) return queue_.fail(Fault::Shape);
            pointers[i] = input->mBuffers[i].mData; sizes[i] = input->mBuffers[i].mDataByteSize;
        }
        const auto captured = clock_.nanos(inputTime->mHostTime);
        if (captured > UINT64_MAX - freshnessNs_) return queue_.fail(Fault::Clock);
        const Stamp stamp{queue_.binding(), queue_.clockId(), sequence_, first, captured, captured + freshnessNs_};
        if (!queue_.push(clock_.nanos(now->mHostTime), stamp, frames, pointers.data(), sizes.data(),
                         input->mNumberBuffers, planar_)) return false;
        ++sequence_;
        return true;
    }
};

// Explicit-UID, read-only format admission followed by owned IOProc registration.
// It never selects defaults, changes a device format or instantiates a HAL input.
// A caller must schedule/adapt into this destination's actual sample timeline,
// prefill the queue, then explicitly start. open() alone does not start playback.
class OutputWorker {
    Queue& queue_;
    Renderer renderer_;
    HostClock clock_;
    const std::string uid_;
    AudioDeviceID device_ = kAudioObjectUnknown;
    AudioStreamID stream_ = kAudioObjectUnknown;
    AudioDeviceIOProcID io_ = nullptr;
    bool started_ = false, planar_ = false;
    struct Notice { AudioObjectID object; AudioObjectPropertyAddress address; };
    std::array<Notice, 7> notices_{};
    size_t noticeCount_ = 0;
    alignas(8) std::array<unsigned char, maxBytes> scratch_{};

    static void checked(OSStatus status, const char* operation) {
        if (status != noErr) throw std::runtime_error(std::string(operation) + ": OSStatus " + std::to_string(status));
    }
    template<class T> static T property(AudioObjectID object, AudioObjectPropertySelector selector,
                                       AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
        AudioObjectPropertyAddress address{selector, scope, kAudioObjectPropertyElementMain};
        T value{}; UInt32 bytes = sizeof(value);
        checked(AudioObjectGetPropertyData(object, &address, 0, nullptr, &bytes, &value), "read worker property");
        if (bytes != sizeof(value)) throw std::runtime_error("Unexpected worker property size");
        return value;
    }
    static std::string uid(AudioDeviceID device) {
        const auto text = property<CFStringRef>(device, kAudioDevicePropertyDeviceUID);
        if (!text) throw std::runtime_error("Unavailable output UID");
        std::array<char, 4096> buffer{};
        const bool ok = CFStringGetCString(text, buffer.data(), buffer.size(), kCFStringEncodingUTF8);
        CFRelease(text);
        if (!ok || !buffer[0]) throw std::runtime_error("Unsupported output UID");
        return buffer.data();
    }
    void validate() {
        if (uid(device_) != uid_ || property<UInt32>(device_, kAudioDevicePropertyDeviceIsAlive) != 1)
            throw std::runtime_error("Selected output identity lost");
        // A device IOProc can receive both directions. Until per-IOProc input
        // suppression is qualified, never start one on a capture-capable device.
        const AudioObjectPropertyAddress inputs{kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain};
        UInt32 inputBytes = 0;
        checked(AudioObjectGetPropertyDataSize(device_, &inputs, 0, nullptr, &inputBytes), "size input streams");
        if (inputBytes != 0)
            throw std::runtime_error("Duplex output unsupported: input stream suppression is unqualified");
        AudioObjectPropertyAddress address{kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput,
                                           kAudioObjectPropertyElementMain};
        UInt32 bytes = 0;
        checked(AudioObjectGetPropertyDataSize(device_, &address, 0, nullptr, &bytes), "size output streams");
        if (bytes != sizeof(AudioStreamID)) throw std::runtime_error("Output worker admits exactly one native stream");
        AudioStreamID stream = kAudioObjectUnknown;
        checked(AudioObjectGetPropertyData(device_, &address, 0, nullptr, &bytes, &stream), "read output stream");
        if (bytes != sizeof(stream) || stream == kAudioObjectUnknown ||
            (stream_ != kAudioObjectUnknown && stream != stream_)) throw std::runtime_error("Selected output stream changed");
        stream_ = stream;
        const auto actual = property<AudioStreamBasicDescription>(stream_, kAudioStreamPropertyVirtualFormat);
        if (!exactFloat(actual, queue_.format())) throw std::runtime_error("Output requires explicit destination format conversion");
        planar_ = actual.mFormatFlags & kAudioFormatFlagIsNonInterleaved;
        const auto first = property<UInt32>(stream_, kAudioStreamPropertyStartingChannel);
        if (!first || uint64_t(first) + queue_.format().channels.size() > UINT32_MAX)
            throw std::runtime_error("Invalid native channel positions");
        // Positional identities preserve order without claiming a speaker layout
        // from ASBD's channel count or from a merely preferred device layout.
        for (size_t i = 0; i < queue_.format().channels.size(); ++i)
            if (queue_.format().channels[i] != "CA:" + std::to_string(first + i))
                throw std::runtime_error("Explicit ordered Core Audio channel mapping required");
    }
    static OSStatus changed(AudioObjectID, UInt32, const AudioObjectPropertyAddress*, void* context) noexcept {
        static_cast<OutputWorker*>(context)->queue_.revoke();
        return noErr;
    }
    void listen(AudioObjectID object, AudioObjectPropertySelector selector, AudioObjectPropertyScope scope) {
        const AudioObjectPropertyAddress address{selector, scope, kAudioObjectPropertyElementMain};
        checked(AudioObjectAddPropertyListener(object, &address, changed, this), "watch selected output");
        notices_[noticeCount_++] = {object, address};
    }
    static OSStatus render(AudioObjectID, const AudioTimeStamp* now, const AudioBufferList*,
                           const AudioTimeStamp*, AudioBufferList* output,
                           const AudioTimeStamp* outputTime, void* context) noexcept {
        auto& self = *static_cast<OutputWorker*>(context);
        auto& queue = self.queue_;
        // HAL supplies pre-zeroed output. Clear bounded buffers explicitly too.
        if (output && output->mNumberBuffers <= maxChannels)
            for (UInt32 i = 0; i < output->mNumberBuffers; ++i)
                if (output->mBuffers[i].mData && output->mBuffers[i].mDataByteSize <= maxBytes)
                    std::memset(output->mBuffers[i].mData, 0, output->mBuffers[i].mDataByteSize);
        if (queue.fault() != Fault::None) return noErr;
        const auto channels = static_cast<uint32_t>(queue.format().channels.size());
        uint64_t first = 0;
        if (!samplePosition(outputTime, first) || !now || !(now->mFlags & kAudioTimeStampHostTimeValid) ||
            !(outputTime->mFlags & kAudioTimeStampHostTimeValid) || outputTime->mHostTime < now->mHostTime) {
            queue.fail(Fault::Clock); return noErr;
        }
        if (!output || output->mNumberBuffers != (self.planar_ ? channels : 1u)) {
            queue.fail(Fault::Shape); return noErr;
        }
        const auto laneWidth = self.planar_ ? queue.format().sampleBytes() : queue.format().frameBytes();
        const auto bytes = output->mBuffers[0].mDataByteSize;
        if (bytes % laneWidth) { queue.fail(Fault::Shape); return noErr; }
        const auto frames = bytes / laneWidth;
        for (UInt32 i = 0; i < output->mNumberBuffers; ++i)
            if (!output->mBuffers[i].mData || output->mBuffers[i].mDataByteSize != bytes ||
                output->mBuffers[i].mNumberChannels != (self.planar_ ? 1u : channels)) {
                queue.fail(Fault::Shape); return noErr;
            }
        const uint64_t total = uint64_t(frames) * queue.format().frameBytes();
        if (total > maxBytes) { queue.fail(Fault::Shape); return noErr; }
        // Validate freshness at the scheduled output time, not merely at the
        // earlier callback invocation. This still is not measured audible PCM.
        if (!self.renderer_.render(self.clock_.nanos(outputTime->mHostTime), first, frames,
                                   self.scratch_.data(), static_cast<uint32_t>(total))) return noErr;
        const auto width = queue.format().sampleBytes();
        if (self.planar_) {
            for (uint32_t frame = 0; frame < frames; ++frame)
                for (uint32_t channel = 0; channel < channels; ++channel)
                    std::memcpy(static_cast<unsigned char*>(output->mBuffers[channel].mData) + frame * width,
                                self.scratch_.data() + (frame * channels + channel) * width, width);
        } else std::memcpy(output->mBuffers[0].mData, self.scratch_.data(), static_cast<size_t>(total));
        if (queue.fault() != Fault::None)
            for (UInt32 i = 0; i < output->mNumberBuffers; ++i)
                std::memset(output->mBuffers[i].mData, 0, output->mBuffers[i].mDataByteSize);
        return noErr;
    }
    OutputWorker(std::string deviceUid, Queue& queue) : queue_(queue), renderer_(queue), uid_(std::move(deviceUid)) {}
public:
    static std::unique_ptr<OutputWorker> open(std::string deviceUid, Queue& queue) {
        if (deviceUid.empty() || deviceUid.size() > 4095 || deviceUid.find('\0') != std::string::npos ||
            queue.provenance().origin != Origin::AdaptedEngine || queue.fault() != Fault::None)
            throw std::invalid_argument("Explicit output UID and freshly adapted engine queue required");
        auto worker = std::unique_ptr<OutputWorker>(new OutputWorker(std::move(deviceUid), queue));
        AudioObjectPropertyAddress address{kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal,
                                           kAudioObjectPropertyElementMain};
        UInt32 bytes = 0;
        checked(AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, nullptr, &bytes), "size output inventory");
        if (!bytes || bytes % sizeof(AudioDeviceID) || bytes > 1024 * sizeof(AudioDeviceID))
            throw std::runtime_error("Unsupported output inventory size");
        std::vector<AudioDeviceID> devices(bytes / sizeof(AudioDeviceID));
        const auto capacity = bytes;
        checked(AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, nullptr, &bytes, devices.data()), "read output inventory");
        if (bytes != capacity) throw std::runtime_error("Output inventory changed during admission");
        for (const auto device : devices) if (uid(device) == worker->uid_) {
            if (worker->device_ != kAudioObjectUnknown) throw std::runtime_error("Ambiguous output UID");
            worker->device_ = device;
        }
        if (worker->device_ == kAudioObjectUnknown) throw std::runtime_error("Selected output UID absent");
        worker->validate();
        worker->listen(worker->device_, kAudioDevicePropertyDeviceUID, kAudioObjectPropertyScopeGlobal);
        worker->listen(worker->device_, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal);
        worker->listen(worker->device_, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
        worker->listen(worker->device_, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
        worker->listen(worker->device_, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
        worker->listen(worker->stream_, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal);
        worker->listen(worker->stream_, kAudioStreamPropertyStartingChannel, kAudioObjectPropertyScopeGlobal);
        checked(AudioDeviceCreateIOProcID(worker->device_, render, worker.get(), &worker->io_), "create owned output IOProc");
        worker->validate(); // Close the property-read/listener-registration window.
        if (queue.fault() != Fault::None) throw std::runtime_error("Output changed during admission");
        return worker;
    }
    OutputWorker(const OutputWorker&) = delete;
    OutputWorker& operator=(const OutputWorker&) = delete;
    ~OutputWorker() { if (!close()) std::terminate(); }
    // startTime is caller-scheduled in this output's actual hardware timeline.
    // HAL may adjust it; a mismatching queued timeline fails silent in render.
    void startAt(AudioTimeStamp& startTime) {
        uint64_t frame = 0;
        if (started_ || !samplePosition(&startTime, frame) || queue_.fault() != Fault::None)
            throw std::invalid_argument("Fresh output and explicit scheduled sample position required");
        validate();
        checked(AudioDeviceStartAtTime(device_, io_, &startTime, 0), "start explicit owned output");
        started_ = true;
    }
    // Control thread only. False requires retaining all callback storage (or
    // process exit); never free a possibly live IOProc/listener context.
    bool close() noexcept {
        queue_.revoke();
        if (started_) {
            if (AudioDeviceStop(device_, io_) != noErr) return false;
            started_ = false;
        }
        if (io_) {
            if (AudioDeviceDestroyIOProcID(device_, io_) != noErr) return false;
            io_ = nullptr;
        }
        while (noticeCount_) {
            const auto& notice = notices_[noticeCount_ - 1];
            if (AudioObjectRemovePropertyListener(notice.object, &notice.address, changed, this) != noErr) return false;
            --noticeCount_;
        }
        return true;
    }
};
} // namespace mixyard::worker

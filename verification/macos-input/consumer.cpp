// SPDX-License-Identifier: MIT
// Explicit-UID HAL recorder for a separately authorized native experiment.
#include <CoreAudio/CoreAudio.h>
#include <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

struct Capture {
    struct Span { uint64_t offset, frames; AudioTimeStamp time; };
    std::vector<float> pcm;
    std::vector<Span> spans;
    uint64_t spanCount = 0;
    uint64_t target = 0;
    std::atomic<uint64_t> count{0};
    std::atomic<bool> done{false};
    std::atomic<bool> invalid{false};
    AudioTimeStamp first{}, last{};
};

static OSStatus record(AudioDeviceID, const AudioTimeStamp*, const AudioBufferList* input,
                       const AudioTimeStamp* inputTime, AudioBufferList*,
                       const AudioTimeStamp*, void* context) {
    auto& c = *static_cast<Capture*>(context);
    if (c.done.load(std::memory_order_acquire)) return noErr;
    if (!input || !inputTime || input->mNumberBuffers != 1 ||
        input->mBuffers[0].mNumberChannels != 2 || !input->mBuffers[0].mData ||
        input->mBuffers[0].mDataByteSize == 0 || input->mBuffers[0].mDataByteSize % 8) {
        c.invalid.store(true, std::memory_order_relaxed);
        c.done.store(true, std::memory_order_release);
        return noErr;
    }
    const auto& buffer = input->mBuffers[0];
    const uint64_t frames = buffer.mDataByteSize / 8;
    const uint64_t offset = c.count.load(std::memory_order_relaxed);
    // Preserve the whole last callback, including all first/tail samples.
    if (frames > 8192 || frames > c.pcm.size() / 2 - offset ||
        !(inputTime->mFlags & kAudioTimeStampSampleTimeValid) ||
        !(inputTime->mFlags & kAudioTimeStampHostTimeValid) ||
        !std::isfinite(inputTime->mSampleTime)) {
        c.invalid.store(true, std::memory_order_relaxed);
        c.done.store(true, std::memory_order_release);
        return noErr;
    }
    const auto* samples = static_cast<const float*>(buffer.mData);
    if (c.spanCount) {
        const auto& previous = c.spans[c.spanCount - 1];
        if (inputTime->mSampleTime != previous.time.mSampleTime + static_cast<double>(previous.frames) ||
            inputTime->mHostTime <= previous.time.mHostTime)
            c.invalid.store(true, std::memory_order_relaxed);
    }
    c.spans[c.spanCount++] = {offset, frames, *inputTime};
    for (uint64_t i = 0; i < frames * 2; ++i) {
        if (!std::isfinite(samples[i])) c.invalid.store(true, std::memory_order_relaxed);
        c.pcm[offset * 2 + i] = samples[i];
    }
    if (!offset) c.first = *inputTime;
    c.last = *inputTime;
    c.count.store(offset + frames, std::memory_order_release);
    if (offset + frames >= c.target) c.done.store(true, std::memory_order_release);
    return noErr;
}

static bool status(OSStatus result, const char* operation) {
    if (!result) return true;
    std::fprintf(stderr, "%s failed: OSStatus %d\n", operation, static_cast<int>(result));
    return false;
}

static void u32(FILE* file, uint32_t value) {
    const unsigned char bytes[] = {static_cast<unsigned char>(value),
        static_cast<unsigned char>(value >> 8), static_cast<unsigned char>(value >> 16),
        static_cast<unsigned char>(value >> 24)};
    if (std::fwrite(bytes, 1, 4, file) != 4) std::exit(1);
}
static void u16(FILE* file, uint16_t value) {
    const unsigned char bytes[] = {static_cast<unsigned char>(value), static_cast<unsigned char>(value >> 8)};
    if (std::fwrite(bytes, 1, 2, file) != 2) std::exit(1);
}

int main(int argc, char** argv) {
    if (argc != 5) {
        std::fprintf(stderr, "Usage: %s DEVICE_UID FRAMES OUTPUT.wav METADATA.json\n"
            "Separate authorized HAL experiment only; no default/raw microphone fallback.\n", argv[0]);
        return 2;
    }
    if (std::strncmp(argv[1], "org.mixyard.input.", 18) || std::strlen(argv[1]) != 54) {
        std::fprintf(stderr, "Only an explicit Mixyard endpoint UID is accepted.\n"); return 2;
    }
    for (unsigned i = 0; i < 36; ++i) {
        const char ch = argv[1][18 + i];
        if (i == 8 || i == 13 || i == 18 || i == 23) { if (ch != '-') return 2; }
        else if (!((ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f'))) return 2;
    }
    errno = 0;
    char* end = nullptr;
    const auto frames = std::strtoull(argv[2], &end, 10);
    if (errno || !end || *end || argv[2][0] < '0' || argv[2][0] > '9' ||
        !frames || frames > 48000 * 30) return 2;
    CFStringRef uid = CFStringCreateWithCString(nullptr, argv[1], kCFStringEncodingUTF8);
    if (!uid) return 2;
    AudioObjectPropertyAddress translate{kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    const auto lookup = AudioObjectGetPropertyData(kAudioObjectSystemObject, &translate,
        sizeof(uid), &uid, &size, &device);
    CFRelease(uid);
    if (!status(lookup, "UID translation") || size != sizeof(device) || device == kAudioObjectUnknown) return 1;
    AudioObjectPropertyAddress formatProperty{kAudioDevicePropertyStreamFormat,
        kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain};
    AudioStreamBasicDescription format{};
    size = sizeof(format);
    if (!status(AudioObjectGetPropertyData(device, &formatProperty, 0, nullptr, &size, &format), "input format") ||
        size != sizeof(format) || format.mSampleRate != 48000 || format.mFormatID != kAudioFormatLinearPCM ||
        format.mFormatFlags != (kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian | kAudioFormatFlagIsPacked) ||
        format.mChannelsPerFrame != 2 || format.mBitsPerChannel != 32 || format.mFramesPerPacket != 1 ||
        format.mBytesPerFrame != 8 || format.mBytesPerPacket != 8) {
        std::fprintf(stderr, "Endpoint did not negotiate fixed 48k stereo interleaved Float32.\n"); return 1;
    }
    Capture capture;
    capture.target = frames;
    capture.pcm.resize((frames + 8192) * 2);
    capture.spans.resize(frames + 8192); // one frame/callback is the worst permitted case
    AudioDeviceIOProcID proc = nullptr;
    if (!status(AudioDeviceCreateIOProcID(device, record, &capture, &proc), "create IOProc")) return 1;
    if (!status(AudioDeviceStart(device, proc), "start IOProc")) {
        AudioDeviceDestroyIOProcID(device, proc); return 1;
    }
    const double deadline = CFAbsoluteTimeGetCurrent() + static_cast<double>(frames) / 48000 + 10;
    while (!capture.done.load(std::memory_order_acquire) && CFAbsoluteTimeGetCurrent() < deadline)
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
    const bool stopped = status(AudioDeviceStop(device, proc), "stop IOProc");
    const bool destroyed = status(AudioDeviceDestroyIOProcID(device, proc), "destroy IOProc");
    if (!destroyed) {
        // HAL may still hold the callback/context. Avoid freeing it while callbacks can run.
        std::fprintf(stderr, "BLOCKED: IOProc ownership not released; capture artifacts unavailable.\n");
        std::_Exit(1);
    }
    const auto count = capture.count.load(std::memory_order_acquire);
    // Even failures retain their complete received prefix. Exclusive creation prevents overwrite.
    FILE* wav = std::fopen(argv[3], "wbx");
    if (!wav) { std::perror("create WAV"); return 1; }
    const uint32_t bytes = static_cast<uint32_t>(count * 8);
    std::fwrite("RIFF", 1, 4, wav); u32(wav, 48 + bytes); std::fwrite("WAVEfmt ", 1, 8, wav);
    u32(wav, 16); u16(wav, 3); u16(wav, 2); u32(wav, 48000); u32(wav, 384000);
    u16(wav, 8); u16(wav, 32); std::fwrite("fact", 1, 4, wav); u32(wav, 4); u32(wav, static_cast<uint32_t>(count));
    std::fwrite("data", 1, 4, wav); u32(wav, bytes);
    const bool written = std::fwrite(capture.pcm.data(), 8, count, wav) == count && !std::ferror(wav);
    const bool closed = std::fclose(wav) == 0;
    const bool valid = stopped && destroyed && written && closed && count >= frames &&
        !capture.invalid.load(std::memory_order_relaxed);
    FILE* metadata = std::fopen(argv[4], "wx");
    if (!metadata) { std::perror("create metadata"); return 1; }
    const int result = std::fprintf(metadata,
        "{\"schema_version\":1,\"evidence_kind\":\"external_HAL_IOProc\","
        "\"qualified\":false,\"capture_complete\":%s,\"uid\":\"%s\",\"device_id\":%u,"
        "\"sample_rate\":48000,\"channels\":2,\"format\":\"Float32-interleaved\","
        "\"requested_frames\":%llu,\"received_frames\":%llu,"
        "\"first_sample_time\":%.17g,\"first_host_time\":%llu,"
        "\"last_callback_sample_time\":%.17g,\"last_callback_host_time\":%llu,"
        "\"acceptance_analysis\":\"BLOCKED: compare complete first/tail frames to admitted fixture\",\"callbacks\":[",
        valid ? "true" : "false", argv[1], device, frames, static_cast<unsigned long long>(count),
        capture.first.mSampleTime, static_cast<unsigned long long>(capture.first.mHostTime),
        capture.last.mSampleTime, static_cast<unsigned long long>(capture.last.mHostTime));
    for (uint64_t i = 0; i < capture.spanCount; ++i) {
        const auto& span = capture.spans[i];
        std::fprintf(metadata, "%s{\"offset\":%llu,\"frames\":%llu,\"sample_time\":%.17g,\"host_time\":%llu}",
            i ? "," : "", static_cast<unsigned long long>(span.offset),
            static_cast<unsigned long long>(span.frames), span.time.mSampleTime,
            static_cast<unsigned long long>(span.time.mHostTime));
    }
    std::fprintf(metadata, "]}\n");
    const bool metadataWritten = !std::ferror(metadata);
    const bool metadataClosed = std::fclose(metadata) == 0;
    return valid && result > 0 && metadataWritten && metadataClosed ? 0 : 1;
}

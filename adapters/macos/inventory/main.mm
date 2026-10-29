// SPDX-License-Identifier: MIT
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <Foundation/Foundation.h>
#include <AvailabilityMacros.h>
#if __MAC_OS_X_VERSION_MAX_ALLOWED < 140200
#error "The inventory helper requires macOS SDK 14.2 or newer."
#endif
#include "../../windows/inventory/protocol.hpp"
#include <cmath>
#include <cstring>
#include <limits>
#include <unistd.h>

namespace inv = mixyard::inventory;
static AudioObjectPropertyAddress address(AudioObjectPropertySelector selector,
    AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    return {selector, scope, kAudioObjectPropertyElementMain};
}

template<class T> static bool read(AudioObjectID object, AudioObjectPropertySelector selector,
    T& value, AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    auto property = address(selector, scope);
    UInt32 size = sizeof(T);
    return AudioObjectGetPropertyData(object, &property, 0, nullptr, &size, &value) == noErr && size == sizeof(T);
}
template<class T> static T required(AudioObjectID object, AudioObjectPropertySelector selector) {
    T value{};
    if (!read(object, selector, value)) throw inv::Unavailable();
    return value;
}

static std::vector<AudioObjectID> objects(AudioObjectID object, AudioObjectPropertySelector selector,
    AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    auto property = address(selector, scope);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(object, &property, 0, nullptr, &size) != noErr ||
        size > inv::max_property_bytes || size % sizeof(AudioObjectID) ||
        size / sizeof(AudioObjectID) > inv::max_items) throw inv::Unavailable();
    std::vector<AudioObjectID> result(size / sizeof(AudioObjectID));
    const UInt32 capacity = size;
    if (size && AudioObjectGetPropertyData(object, &property, 0, nullptr, &size, result.data()) != noErr)
        throw inv::Unavailable();
    // A changed-size result must be retried by the owner, never silently truncated.
    if (size != capacity) throw inv::Unavailable();
    return result;
}

static std::string text(AudioObjectID object, AudioObjectPropertySelector selector, bool allow_empty = false) {
    CFStringRef value = nullptr;
    const bool valid = read(object, selector, value);
    // CoreAudio CF properties transfer ownership to the caller.
    NSString* owned = CFBridgingRelease(value);
    if (!valid || !owned || CFGetTypeID((__bridge CFTypeRef)owned) != CFStringGetTypeID()) throw inv::Unavailable();
    if (!owned.length && allow_empty) return {};
    // Bound before constructing a UTF-8 allocation; never truncate opaque IDs.
    if (owned.length > inv::max_identifier_bytes) throw inv::Unavailable();
    NSData* bytes = [owned dataUsingEncoding:NSUTF8StringEncoding allowLossyConversion:NO];
    if (!bytes || !bytes.length || bytes.length > inv::max_identifier_bytes) throw inv::Unavailable();
    std::string result(static_cast<const char*>(bytes.bytes), bytes.length);
    inv::identifier(result);
    return result;
}

static std::optional<inv::Format> query_format(const std::vector<AudioObjectID>& streams) {
    // A multi-stream device has no one proven stream format. Do not sum channels
    // or combine a nominal device rate with an unrelated stream's channel count.
    if (streams.size() != 1) return std::nullopt;
    AudioStreamBasicDescription f{};
    if (!read(streams.front(), kAudioStreamPropertyVirtualFormat, f) ||
        f.mFormatID != kAudioFormatLinearPCM || !std::isfinite(f.mSampleRate) ||
        f.mSampleRate < 1 || f.mSampleRate > std::numeric_limits<std::uint32_t>::max() ||
        std::trunc(f.mSampleRate) != f.mSampleRate) return std::nullopt;
    // Schema 2 cannot carry encoding/layout. Keep unfamiliar encodings unknown.
    constexpr auto known_flags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsBigEndian |
        kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked | kAudioFormatFlagIsAlignedHigh |
        kAudioFormatFlagIsNonInterleaved;
    if (f.mFormatFlags & ~known_flags) return std::nullopt;
    const bool floating = (f.mFormatFlags & kAudioFormatFlagIsFloat) != 0;
    if (floating && (f.mFormatFlags & kAudioFormatFlagIsSignedInteger)) return std::nullopt;
    if (floating ? (f.mBitsPerChannel != 32 && f.mBitsPerChannel != 64) :
        (f.mBitsPerChannel != 8 && f.mBitsPerChannel != 16 && f.mBitsPerChannel != 24 && f.mBitsPerChannel != 32))
        return std::nullopt;
    if (!f.mBytesPerFrame || !f.mFramesPerPacket || !f.mBytesPerPacket) return std::nullopt;
    return inv::format(f.mChannelsPerFrame, static_cast<std::uint32_t>(f.mSampleRate));
}

static inv::Snapshot snapshot() {
    inv::Snapshot result("macos");
    for (const auto device : objects(kAudioObjectSystemObject, kAudioHardwarePropertyDevices)) {
        if (required<UInt32>(device, kAudioDevicePropertyDeviceIsAlive) == 0) throw inv::Unavailable();
        const auto input = objects(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
        const auto output = objects(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
        if (input.empty() && output.empty()) continue;
        const auto uid = text(device, kAudioDevicePropertyDeviceUID);
        if (!input.empty()) result.mic(uid, query_format(input));
        if (!output.empty()) result.output(uid, query_format(output));
    }
    if (@available(macOS 14.2, *)) {
        for (const auto process : objects(kAudioObjectSystemObject, kAudioHardwarePropertyProcessObjectList)) {
            if (!required<UInt32>(process, kAudioProcessPropertyIsRunningOutput)) continue;
            const auto bundle = text(process, kAudioProcessPropertyBundleID, true);
            // Unbundled executables have no bundle identity. Never substitute PID,
            // process name, executable path, or a media-session catalog identity.
            if (bundle.empty()) continue;
            const auto pid = required<pid_t>(process, kAudioProcessPropertyPID);
            if (pid <= 0) throw inv::Unavailable();
            const bool producer = pid == getpid() || bundle == "org.mixyard.coreaudio-probe" || bundle == "io.mixyard.desktop";
            result.app(bundle, producer); // Multiple matching processes stay ambiguous.
        }
    }
    return result;
}

int main(int argc, char** argv) {
    if (argc != 2 || std::strcmp(argv[1], "snapshot") != 0) {
        std::fputs("native-inventory-invalid-arguments\n", stderr);
        return 2;
    }
    @autoreleasepool {
        @try {
            try { return inv::emit(snapshot()); }
            catch (...) { return inv::unavailable("macos"); }
        } @catch (NSException* exception) {
            (void)exception;
            return inv::unavailable("macos");
        }
    }
}

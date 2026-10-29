// SPDX-License-Identifier: MIT
#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#include <AvailabilityMacros.h>
#if __MAC_OS_X_VERSION_MAX_ALLOWED < 140200
#error "Use an Apple macOS SDK 14.2 or newer for process taps."
#endif

#include "capture_writer.hpp"
#include "native_worker.hpp"
#include <algorithm>
#include <chrono>
#include <csignal>
#include <memory>
#include <thread>
#include <vector>

using namespace std::chrono_literals;
static volatile sig_atomic_t interrupted = 0;
static void onSignal(int) { interrupted = 1; }

struct ProbeError : std::runtime_error {
    std::string code;
    ProbeError(const char* code, const std::string& message) : std::runtime_error(message), code(code) {}
};

static NSString* str(const std::string& s) { return [NSString stringWithUTF8String:s.c_str()]; }
static void check(OSStatus status, const char* operation) {
    if (status == noErr) return;
    const uint32_t n = static_cast<uint32_t>(status);
    char fourcc[5] = {char(n >> 24), char(n >> 16), char(n >> 8), char(n), 0};
    for (int i = 0; i < 4; ++i) if (fourcc[i] < 32 || fourcc[i] > 126) fourcc[i] = '?';
    const std::string detail = std::string(operation) + ": OSStatus " + std::to_string(status) + " ('" + fourcc + "')";
    if (status == kAudioDevicePermissionsError)
        throw ProbeError("permission-denied", detail + "; allow system audio recording for this probe/launching terminal in System Settings > Privacy & Security. No settings are changed by the probe.");
    throw ProbeError("coreaudio-error", detail + "; capture/start errors can also indicate missing audio-recording permission. Check permission and OS support on the Mac.");
}

template<class T> static T property(AudioObjectID id, AudioObjectPropertySelector selector,
                                   AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    AudioObjectPropertyAddress address{selector, scope, kAudioObjectPropertyElementMain};
    T value{};
    UInt32 size = sizeof(value);
    check(AudioObjectGetPropertyData(id, &address, 0, nullptr, &size, &value), "read Core Audio property");
    if (size != sizeof(value)) throw ProbeError("invalid-property", "Core Audio returned an unexpected property size");
    return value;
}

static NSString* stringProperty(AudioObjectID id, AudioObjectPropertySelector selector) {
    CFStringRef value = property<CFStringRef>(id, selector);
    if (!value) throw ProbeError("invalid-property", "Core Audio returned a null string");
    return CFBridgingRelease(value);
}

static std::vector<unsigned char> bytesProperty(AudioObjectID id, AudioObjectPropertySelector selector,
                                              AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
    AudioObjectPropertyAddress address{selector, scope, kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    check(AudioObjectGetPropertyDataSize(id, &address, 0, nullptr, &size), "size Core Audio property");
    if (size > 1024 * 1024) throw ProbeError("invalid-property", "Core Audio property exceeds probe bound");
    std::vector<unsigned char> bytes(size);
    if (size) check(AudioObjectGetPropertyData(id, &address, 0, nullptr, &size, bytes.data()), "read Core Audio list");
    if (size > bytes.size()) throw ProbeError("discovery-changed", "Core Audio inventory changed; retry discovery");
    bytes.resize(size);
    return bytes;
}

static std::vector<AudioObjectID> objects(AudioObjectPropertySelector selector) {
    auto bytes = bytesProperty(kAudioObjectSystemObject, selector);
    if (bytes.size() % sizeof(AudioObjectID)) throw ProbeError("invalid-property", "Unaligned Core Audio object list");
    std::vector<AudioObjectID> ids(bytes.size() / sizeof(AudioObjectID));
    if (!bytes.empty()) std::memcpy(ids.data(), bytes.data(), bytes.size());
    return ids;
}

static UInt32 channels(AudioObjectID device, AudioObjectPropertyScope scope) {
    auto bytes = bytesProperty(device, kAudioDevicePropertyStreamConfiguration, scope);
    if (bytes.empty()) return 0;
    if (bytes.size() < offsetof(AudioBufferList, mBuffers))
        throw ProbeError("invalid-property", "Truncated stream configuration");
    const auto* buffers = reinterpret_cast<const AudioBufferList*>(bytes.data());
    if (buffers->mNumberBuffers > (bytes.size() - offsetof(AudioBufferList, mBuffers)) / sizeof(AudioBuffer))
        throw ProbeError("invalid-property", "Truncated stream buffer list");
    UInt32 count = 0;
    for (UInt32 i = 0; i < buffers->mNumberBuffers; ++i) count += buffers->mBuffers[i].mNumberChannels;
    return count;
}

static NSMutableDictionary* envelope() {
    return [@{@"schemaVersion": @1, @"platform": @"macos",
              @"capabilities": [@{@"deviceDiscovery": @NO, @"processCapture": @NO,
                                   @"virtualInputs": @NO, @"independentMixes": @NO, @"routeRecovery": @NO} mutableCopy],
              @"devices": [NSMutableArray array], @"diagnostics": [NSMutableArray array]} mutableCopy];
}

static void diagnostic(NSMutableDictionary* result, NSString* code, NSString* message, NSString* severity) {
    [result[@"diagnostics"] addObject:@{@"code": code, @"message": message, @"severity": severity}];
    std::fprintf(stderr, "%s: %s\n", code.UTF8String, message.UTF8String);
}

static void emit(NSDictionary* result) {
    NSError* error = nil;
    NSData* data = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingSortedKeys error:&error];
    if (!data) { std::fprintf(stderr, "JSON serialization failed\n"); std::_Exit(1); }
    if (std::fwrite(data.bytes, 1, data.length, stdout) != data.length || std::putchar('\n') == EOF || std::fflush(stdout) != 0)
        std::_Exit(1);
}

static void discover(NSMutableDictionary* result) {
    for (auto id : objects(kAudioHardwarePropertyDevices)) {
        NSString* uid = stringProperty(id, kAudioDevicePropertyDeviceUID);
        if (uid.length == 0) throw ProbeError("missing-device-uid", "A device has no stable UID; labels cannot identify it");
        NSString* name = stringProperty(id, kAudioObjectPropertyName);
        const auto input = channels(id, kAudioDevicePropertyScopeInput);
        const auto output = channels(id, kAudioDevicePropertyScopeOutput);
        if (!input && !output) continue;
        const auto transport = property<UInt32>(id, kAudioDevicePropertyTransportType);
        const bool virtualDevice = transport == kAudioDeviceTransportTypeVirtual || transport == kAudioDeviceTransportTypeAggregate;
        const double rate = property<Float64>(id, kAudioDevicePropertyNominalSampleRate);
        if (!std::isfinite(rate) || rate <= 0) throw ProbeError("invalid-format", "Invalid device nominal sample rate");
        [result[@"devices"] addObject:@{@"id": uid, @"name": name,
            @"direction": input && output ? @"duplex" : input ? @"input" : @"output",
            @"virtual": @(virtualDevice), @"inputChannels": @(input), @"outputChannels": @(output),
            @"nominalSampleRate": @(rate), @"transportType": @(transport)}];
    }
    [result[@"devices"] sortUsingComparator:^NSComparisonResult(NSDictionary* a, NSDictionary* b) {
        return [a[@"id"] compare:b[@"id"]];
    }];
    result[@"capabilities"][@"deviceDiscovery"] = @YES;
}

struct Process {
    AudioObjectID object;
    pid_t pid;
    std::string bundle;
    bool output;
};

static std::vector<Process> processes() {
    std::vector<Process> list;
    for (auto id : objects(kAudioHardwarePropertyProcessObjectList)) {
        list.push_back({id, property<pid_t>(id, kAudioProcessPropertyPID),
                        stringProperty(id, kAudioProcessPropertyBundleID).UTF8String,
                        property<UInt32>(id, kAudioProcessPropertyIsRunningOutput) != 0});
    }
    std::sort(list.begin(), list.end(), [](const Process& a, const Process& b) { return a.pid < b.pid; });
    return list;
}

static bool producer(const Process& p) {
    return p.pid == getpid() || p.bundle.rfind("org.mixyard", 0) == 0 || p.bundle.rfind("com.mixyard", 0) == 0;
}

static void inventory(NSMutableDictionary* result) {
    NSMutableArray* list = [NSMutableArray array];
    for (const auto& p : processes()) [list addObject:@{@"pid": @(p.pid), @"bundleId": str(p.bundle),
        @"runningOutput": @(p.output), @"captureExcluded": @(producer(p))}];
    result[@"processes"] = list; // PIDs are transient runtime selectors, never persisted identities.
}

// The legacy WAV command explicitly requests a stereo Float32 mixdown. The
// generic native transport itself accepts exact Float32/Float64 ordered channels.
class TapWavQueue {
    mixyard::worker::Queue& transport;
    mixyard::worker::HostClock clock;
    mixyard::Chunk current;
public:
    std::atomic<uint32_t> dropped{0}, invalid{0};
    explicit TapWavQueue(mixyard::worker::Queue& queue) : transport(queue) {}
    const mixyard::Chunk* front() {
        const auto* block = transport.front(clock.now());
        if (transport.fault() != mixyard::worker::Fault::None)
            throw std::runtime_error("Tap worker revoked or expired; queued capture is unqualified");
        if (!block) return nullptr;
        current.frames = block->frames;
        std::memcpy(current.samples.data(), block->pcm.data(), current.frames * 8);
        if (transport.fault() != mixyard::worker::Fault::None)
            throw std::runtime_error("Tap worker revoked during drain");
        return &current;
    }
    void pop() noexcept { transport.release(); }
};

struct CaptureState {
    mixyard::worker::Queue transport;
    TapWavQueue queue;
    mixyard::worker::CaptureIngress ingress;
    std::atomic<uint32_t> clockFault{0};
    CaptureState(uint32_t rate, const AudioStreamBasicDescription& actual, std::string tapUid)
        : transport({rate, {"FL", "FR"}, mixyard::worker::Precision::Float32}, {1, 1}, 1,
                    {mixyard::worker::Origin::ProcessTap, std::move(tapUid)}),
          queue(transport), ingress(transport, actual, 2000000000) {}
};

static OSStatus receive(AudioObjectID, const AudioTimeStamp* now, const AudioBufferList* input,
                        const AudioTimeStamp* inputTime, AudioBufferList*, const AudioTimeStamp*, void* context) noexcept {
    auto& s = *static_cast<CaptureState*>(context);
    if (s.ingress.receive(now, input, inputTime)) return noErr;
    const auto fault = s.transport.fault();
    if (fault == mixyard::worker::Fault::Overflow)
        s.queue.dropped.fetch_add(1, std::memory_order_relaxed);
    else if (fault == mixyard::worker::Fault::Shape || fault == mixyard::worker::Fault::Nonfinite)
        s.queue.invalid.fetch_add(1, std::memory_order_relaxed);
    else
        s.clockFault.store(1, std::memory_order_relaxed);
    return noErr;
}

struct OwnedCapture {
    AudioObjectID tap = kAudioObjectUnknown;
    AudioObjectID aggregate = kAudioObjectUnknown;
    AudioDeviceIOProcID io = nullptr;
    bool started = false;
    bool close(NSMutableDictionary* result) {
        bool ok = true;
        auto cleanup = [&](OSStatus status, const char* operation) {
            if (status != noErr) {
                ok = false;
                diagnostic(result, @"owned-cleanup-failed", str(std::string(operation) + ": OSStatus " + std::to_string(status)), @"error");
            }
        };
        if (started) cleanup(AudioDeviceStop(aggregate, io), "stop owned IOProc");
        if (io) cleanup(AudioDeviceDestroyIOProcID(aggregate, io), "destroy owned IOProc");
        if (aggregate != kAudioObjectUnknown) cleanup(AudioHardwareDestroyAggregateDevice(aggregate), "destroy owned aggregate");
        if (@available(macOS 14.2, *)) {
            if (tap != kAudioObjectUnknown) cleanup(AudioHardwareDestroyProcessTap(tap), "destroy owned tap");
        }
        started = false; io = nullptr; aggregate = tap = kAudioObjectUnknown;
        return ok;
    }
};

static uint32_t validateFormat(const AudioStreamBasicDescription& f) {
    const auto flags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
    if (f.mFormatID != kAudioFormatLinearPCM || f.mChannelsPerFrame != 2 || f.mBitsPerChannel != 32 ||
        !(f.mFormatFlags & kAudioFormatFlagIsFloat) || !(f.mFormatFlags & kAudioFormatFlagIsPacked) ||
        (f.mFormatFlags & ~flags) || f.mBytesPerFrame != ((f.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? 4u : 8u) ||
        f.mFramesPerPacket != 1 || f.mBytesPerPacket != f.mBytesPerFrame ||
        !std::isfinite(f.mSampleRate) || f.mSampleRate < 8000 || f.mSampleRate > 192000 || std::floor(f.mSampleRate) != f.mSampleRate)
        throw ProbeError("unsupported-format", "Tap must negotiate packed little-endian stereo Float32 at an integer 8–192 kHz rate; no implicit conversion or resampling");
    return static_cast<uint32_t>(f.mSampleRate);
}

static long positiveNumber(const char* value, long maximum) {
    char* end = nullptr;
    errno = 0;
    const long n = std::strtol(value, &end, 10);
    if (errno || end == value || *end || n < 1 || n > maximum)
        throw ProbeError("invalid-arguments", "Expected a positive integer within the documented bound");
    return n;
}

static void capture(int argc, const char* argv[], NSMutableDictionary* result) {
    if (@available(macOS 14.2, *)) {
        std::vector<pid_t> pids;
        std::vector<std::string> bundles;
        const char* path = nullptr;
        unsigned seconds = 0;
        for (int i = 2; i < argc; i += 2) {
            if (i + 1 >= argc) throw ProbeError("invalid-arguments", "Capture arguments require values");
            const std::string arg = argv[i];
            if (arg == "--pid") pids.push_back(static_cast<pid_t>(positiveNumber(argv[i + 1], INT32_MAX)));
            else if (arg == "--bundle-id") bundles.emplace_back(argv[i + 1]);
            else if (arg == "--wav" && !path) path = argv[i + 1];
            else if (arg == "--seconds" && !seconds) seconds = static_cast<unsigned>(positiveNumber(argv[i + 1], 300));
            else throw ProbeError("invalid-arguments", "Unknown or duplicate capture argument");
        }
        if (!path || !*path || !seconds || (pids.empty() && bundles.empty()) || pids.size() + bundles.size() > 32)
            throw ProbeError("invalid-arguments", "capture requires 1–32 explicit --pid/--bundle-id selectors, --seconds 1–300, and --wav NEW_FILE");
        auto observed = processes();
        for (const auto& bundle : bundles) {
            if (bundle.empty()) throw ProbeError("invalid-arguments", "Bundle ID must not be empty");
            std::vector<pid_t> matches;
            for (const auto& p : observed) if (p.bundle == bundle) matches.push_back(p.pid);
            if (matches.size() != 1) throw ProbeError(matches.empty() ? "source-missing" : "source-ambiguous",
                "Bundle selector must resolve to exactly one process; use inventory and explicit runtime PIDs for helpers");
            pids.push_back(matches.front());
        }
        std::sort(pids.begin(), pids.end());
        if (std::adjacent_find(pids.begin(), pids.end()) != pids.end())
            throw ProbeError("source-ambiguous", "Duplicate process selection");
        NSMutableArray<NSNumber*>* ids = [NSMutableArray array];
        NSMutableArray* selected = [NSMutableArray array];
        std::vector<Process> sources;
        for (auto pid : pids) {
            const auto it = std::find_if(observed.begin(), observed.end(), [&](const Process& p) { return p.pid == pid; });
            if (it == observed.end()) throw ProbeError("source-missing", "Selected PID is not a Core Audio process");
            if (producer(*it)) throw ProbeError("producer-recapture", "Refusing Mixyard/self producer capture");
            sources.push_back(*it); [ids addObject:@(it->object)];
            [selected addObject:@{@"pid": @(it->pid), @"bundleId": str(it->bundle)}];
        }
        OwnedCapture owned;
        std::unique_ptr<CaptureState> state;
        std::unique_ptr<mixyard::CaptureWriter> writer;
        std::exception_ptr failure;
        uint32_t rate = 0;
        try {
            CATapDescription* description = [[CATapDescription alloc] initStereoMixdownOfProcesses:ids];
            description.name = @"Mixyard selected-process capture";
            [description setPrivate:YES];
            description.muteBehavior = CATapUnmuted;
            check(AudioHardwareCreateProcessTap(description, &owned.tap), "create selected-process tap");
            const auto format = property<AudioStreamBasicDescription>(owned.tap, kAudioTapPropertyFormat);
            rate = validateFormat(format);
            NSString* tapUID = stringProperty(owned.tap, kAudioTapPropertyUID);
            state = std::make_unique<CaptureState>(rate, format, std::string(tapUID.UTF8String));
            NSDictionary* aggregate = @{
                @(kAudioAggregateDeviceNameKey): @"Mixyard private capture",
                @(kAudioAggregateDeviceUIDKey): [@"org.mixyard.capture." stringByAppendingString:NSUUID.UUID.UUIDString],
                @(kAudioAggregateDeviceIsPrivateKey): @YES,
                @(kAudioAggregateDeviceTapAutoStartKey): @YES,
                @(kAudioAggregateDeviceTapListKey): @[@{@(kAudioSubTapUIDKey): tapUID,
                                                       @(kAudioSubTapDriftCompensationKey): @NO}]
            };
            check(AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)aggregate, &owned.aggregate), "create private tap-only aggregate");
            writer = std::make_unique<mixyard::CaptureWriter>(state->queue, path, rate);
            check(AudioDeviceCreateIOProcID(owned.aggregate, receive, state.get(), &owned.io), "create capture IOProc");
            std::signal(SIGINT, onSignal); std::signal(SIGTERM, onSignal);
            check(AudioDeviceStart(owned.aggregate, owned.io), "start capture (system audio-recording permission required)");
            owned.started = true;
            const auto begin = std::chrono::steady_clock::now();
            auto nextCheck = begin;
            auto lastProgress = begin;
            uint32_t previousFrames = 0;
            while (!interrupted && std::chrono::steady_clock::now() - begin < std::chrono::seconds(seconds)) {
                if (state->queue.dropped.load(std::memory_order_relaxed)) throw ProbeError("capture-overflow", "Bounded capture queue overflowed; WAV contains a gap and is not qualification evidence");
                if (state->queue.invalid.load(std::memory_order_relaxed)) throw ProbeError("capture-buffer-error", "Unexpected/oversize native buffer; WAV is not qualification evidence");
                if (state->clockFault.load(std::memory_order_relaxed)) throw ProbeError("capture-clock-gap", "Missing/discontinuous tap sample timestamps; WAV is not qualification evidence");
                if (writer->failed.load(std::memory_order_acquire)) throw ProbeError("disk-write-failed", writer->error);
                const auto now = std::chrono::steady_clock::now();
                const uint32_t progress = writer->writtenFrames.load(std::memory_order_relaxed);
                if (progress != previousFrames) { previousFrames = progress; lastProgress = now; }
                if (progress && now - lastProgress > 2s)
                    throw ProbeError("capture-stalled", "Native capture/writer stopped advancing for two seconds; partial WAV is unqualified");
                if (now >= nextCheck) {
                    for (const auto& p : sources) {
                        bool unchanged = false;
                        try {
                            unchanged = property<pid_t>(p.object, kAudioProcessPropertyPID) == p.pid &&
                                std::string(stringProperty(p.object, kAudioProcessPropertyBundleID).UTF8String) == p.bundle;
                        } catch (const ProbeError&) {
                            throw ProbeError("source-lost", "Selected Core Audio process is no longer readable; stopped without substituting another source");
                        }
                        if (!unchanged)
                            throw ProbeError("source-lost", "Selected process changed; stopped without substituting another source");
                    }
                    const auto current = property<AudioStreamBasicDescription>(owned.tap, kAudioTapPropertyFormat);
                    if (validateFormat(current) != rate || current.mFormatFlags != format.mFormatFlags)
                        throw ProbeError("capture-format-changed", "Tap format changed; stopped instead of mislabeling PCM");
                    nextCheck = now + 1s;
                }
                if (now - begin > 5s && writer->writtenFrames.load(std::memory_order_relaxed) == 0)
                    throw ProbeError("capture-no-data", "No native PCM arrived; check permission, selected-process playback and aggregate support");
                std::this_thread::sleep_for(10ms);
            }
        } catch (...) { failure = std::current_exception(); }
        // Stop callback delivery before draining/finalizing or releasing callback storage.
        if (!owned.close(result)) {
            diagnostic(result, @"capture-unqualified", @"Owned teardown failed; terminating without freeing possibly live callback storage. Recheck owned resources on the Mac.", @"error");
            emit(result); std::_Exit(1);
        }
        if (writer && !writer->stopAndWait(2s)) {
            diagnostic(result, @"disk-write-timeout", @"WAV writer did not complete draining/finalization within two seconds after owned capture cleanup; partial WAV is unqualified.", @"error");
            diagnostic(result, @"capture-unqualified", @"Terminating without freeing storage still referenced by the blocked writer.", @"error");
            emit(result); std::_Exit(1);
        }
        if (writer && writer->opened) {
            result[@"capture"] = @{@"path": str(path), @"sampleRate": @(rate), @"channels": @2,
                @"encoding": @"float32-le", @"frames": @(writer->frames), @"hasNonzeroSamples": @(writer->hasSignal),
                @"transport": @"bounded-tap-worker", @"engineIntegrated": @NO,
                @"clockDomain": @"tap-session-local",
                @"sourceClockId": @(state->transport.clockId()), @"backendAvailable": @NO, @"audioApplied": @NO,
                @"droppedBuffers": @(state->queue.dropped.load()), @"invalidBuffers": @(state->queue.invalid.load()),
                @"clockGap": @(state->clockFault.load() != 0), @"interrupted": @(interrupted != 0), @"selectedProcesses": selected};
        }
        if (failure) std::rethrow_exception(failure);
        if (writer && writer->failed.load()) throw ProbeError("disk-write-failed", writer->error);
        if (state->queue.dropped.load() || state->queue.invalid.load() || state->clockFault.load())
            throw ProbeError("capture-gap", "Capture failed during shutdown; check buffer/clock diagnostics");
        if (!writer || !writer->frames) throw ProbeError("capture-no-data", "No PCM captured; permission and native capture remain unproved");
        if (!interrupted && writer->frames + mixyard::maxFrames < uint64_t(seconds) * rate)
            throw ProbeError("capture-short", "Captured fewer frames than requested wall-clock duration (beyond one maximum buffer); investigate native gaps/startup/clock behavior");
        if (!writer->hasSignal) throw ProbeError("capture-silent-or-denied", "Only zero PCM captured: source may be silent or permission withheld. This is not proof of permitted capture; retry with an explicit audible test source");
        result[@"capabilities"][@"processCapture"] = @YES;
        if (interrupted) diagnostic(result, @"capture-interrupted", @"Stopped by signal and finalized partial WAV; requested duration was not completed", @"warning");
    } else {
        throw ProbeError("unsupported-os", "Process tap capture requires macOS 14.2 or newer");
    }
}

int main(int argc, const char* argv[]) {
    @autoreleasepool {
        auto result = envelope();
        int exitCode = 0;
        diagnostic(result, @"transport-unverified", @"Tap capture does not publish virtual microphones. Arbitrary named mix devices, PCM injection and multi-device fan-out are unimplemented; two inputs are only the first transport proof.", @"warning");
        try {
            if (argc == 2 && std::strcmp(argv[1], "snapshot") == 0) {
                discover(result);
                if (@available(macOS 14.2, *)) {
                    diagnostic(result, @"capture-permission-unverified", @"Process-tap API is available; snapshot neither requests permission nor opens a tap, so processCapture remains false until explicit capture succeeds.", @"warning");
                } else diagnostic(result, @"capture-unavailable", @"Discovery is available; process taps require macOS 14.2+.", @"warning");
            } else if (argc == 2 && std::strcmp(argv[1], "processes") == 0) {
                if (@available(macOS 14.2, *)) inventory(result);
                else throw ProbeError("unsupported-os", "Core Audio process inventory requires macOS 14.2+");
            } else if (argc > 2 && std::strcmp(argv[1], "capture") == 0) capture(argc, argv, result);
            else throw ProbeError("invalid-arguments", "Usage: mixyard-coreaudio-probe snapshot | processes | capture (--pid N | --bundle-id ID)... --seconds 1..300 --wav NEW_FILE");
        } catch (const ProbeError& e) {
            diagnostic(result, str(e.code), str(e.what()), @"error"); exitCode = 1;
        } catch (const std::exception& e) {
            diagnostic(result, @"unavailable", str(e.what()), @"error"); exitCode = 1;
        }
        emit(result);
        return exitCode;
    }
}

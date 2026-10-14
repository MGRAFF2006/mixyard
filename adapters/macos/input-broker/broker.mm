// SPDX-License-Identifier: MIT
// Disposable app-owned XPC service and anonymous PCM listener. Never a HAL host.
#import "protocol.h"
#include "../virtual-input/fixture_api.h"
#include <algorithm>
#include <array>
#include <cassert>
#include <cmath>
#include <mutex>
#include <unistd.h>

namespace {
struct State {
    std::mutex mutex; // NSXPC/control threads only; never used by driver callbacks
    AudioServerPlugInDriverRef driver = nullptr;
    uint64_t epoch = 0, nextGeneration = 0, active = 0;
    uint64_t start = 0, end = 0, revision = 0, packets = 0, hellos = 0;
    std::array<uint64_t, 2> cursors{};
    uint32_t credits() const {
        const auto consumed = std::clamp(std::min(cursors[0], cursors[1]), start, end);
        return mixyard::creditFrames - static_cast<uint32_t>(end - consumed);
    }
} state;
OSStatus changed(AudioServerPlugInHostRef, AudioObjectID, UInt32, const AudioObjectPropertyAddress *) { return noErr; }
OSStatus copyStorage(AudioServerPlugInHostRef, CFStringRef, CFPropertyListRef *out) { *out = nullptr; return kAudioHardwareUnknownPropertyError; }
OSStatus writeStorage(AudioServerPlugInHostRef, CFStringRef, CFPropertyListRef) { return noErr; }
OSStatus deleteStorage(AudioServerPlugInHostRef, CFStringRef) { return noErr; }
OSStatus request(AudioServerPlugInHostRef, AudioObjectID, UInt64, void *) { return kAudioHardwareUnsupportedOperationError; }
AudioServerPlugInHostInterface host{changed, copyStorage, writeStorage, deleteStorage, request};
NSDictionary *status(NSString *value) { return @{@"status": value}; }
}

@interface MYSession : NSObject <MYPCMTransport> {
    uint64_t _generation;
    BOOL _closed;
}
- (void)close;
@end
@implementation MYSession
- (void)hello:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    if (_closed || _generation || state.active || state.nextGeneration == UINT64_MAX) {
        reply(status(@"busy")); return;
    }
    const auto generation = ++state.nextGeneration;
    if (MixyardInputTest_Admit(2, generation) != MY_INPUT_OK) {
        reply(status(@"admission_failed")); return;
    }
    _generation = generation; state.active = generation; ++state.hellos;
    state.start = state.end = std::max(state.cursors[0], state.cursors[1]);
    state.revision = 0;
    reply(@{@"status": @"admitted", @"generation": @(generation), @"epoch": @(state.epoch),
            @"position": @(state.end), @"credit_frames": @(state.credits()),
            @"sample_rate": @48000, @"channels": @2, @"format": @"Float32-interleaved"});
}
- (void)packet:(NSData *)data reply:(void (^)(NSDictionary *))reply {
    // Foundation decoding is OS-owned. Application validates before any PCM copy;
    // max message bytes is 32824, not a pre-decode allocation guarantee for NSXPC.
    mixyard::Header h{};
    if (![data isKindOfClass:NSData.class] || data.length < sizeof(h) ||
        data.length > sizeof(h) + mixyard::packetFrames * 8) {
        reply(status(@"invalid")); return;
    }
    std::memcpy(&h, data.bytes, sizeof(h));
    if (h.version != 1 || h.sampleRate != 48000 || h.channels != 2 || h.format != 1 ||
        !h.frames || h.frames > mixyard::packetFrames || h.bytes != h.frames * 8 ||
        data.length != sizeof(h) + h.bytes || !h.generation || !h.epoch || !h.revision ||
        h.position > mixyard::maxPosition - h.frames) { reply(status(@"invalid")); return; }
    std::array<float, mixyard::packetFrames * 2> pcm{};
    std::memcpy(pcm.data(), static_cast<const char *>(data.bytes) + sizeof(h), h.bytes);
    for (uint32_t i = 0; i < h.frames * 2; ++i)
        if (!std::isfinite(pcm[i])) { reply(status(@"invalid")); return; }
    std::lock_guard<std::mutex> lock(state.mutex);
    if (_closed || !_generation || state.active != _generation) {
        reply(status(@"not_admitted")); return;
    }
    if (h.generation != _generation || h.epoch != state.epoch ||
        h.position != state.end || h.revision < state.revision) {
        reply(status(@"stale")); return;
    }
    if (h.frames > state.credits()) { reply(status(@"backpressure")); return; }
    if (MixyardInputTest_Packet(2, h.generation, h.epoch, h.revision, h.position,
                              h.frames, pcm.data(), h.bytes) != MY_INPUT_OK) {
        reply(status(@"ring_rejected")); return;
    }
    state.end += h.frames; state.revision = h.revision; ++state.packets;
    reply(@{@"status": @"queued", @"generation": @(_generation),
            @"next_position": @(state.end), @"credit_frames": @(state.credits()),
            @"effective_delivery_ack": @NO});
}
- (void)close {
    std::lock_guard<std::mutex> lock(state.mutex);
    _closed = YES;
    if (_generation && state.active == _generation) {
        (void)MixyardInputTest_Disconnect(2, _generation);
        state.active = 0;
    }
}
@end

@interface MYBroker : NSObject <NSXPCListenerDelegate, MYFixtureControl>
@property(strong) NSXPCListener *anonymous;
@property(strong) NSString *requirement;
@property(strong) NSMutableSet<NSXPCConnection *> *connections;
@end
@implementation MYBroker
- (BOOL)listener:(NSXPCListener *)listener shouldAcceptNewConnection:(NSXPCConnection *)connection {
    NSString *requirement = listener == self.anonymous ? self.requirement : mixyard::fixturePolicy()[@"producer"];
    if (!mixyard::pin(connection, requirement)) return NO;
    @synchronized (self) {
        // Finite fixture budget, not product capacity. Reclaimed on invalidation.
        if (self.connections.count >= 8) return NO;
        [self.connections addObject:connection];
    }
    MYSession *session = nil;
    if (listener == self.anonymous) {
        session = [MYSession new]; connection.exportedInterface = pcmInterface();
        connection.exportedObject = session;
    } else {
        connection.exportedInterface = controlInterface(); connection.exportedObject = self;
    }
    __weak MYBroker *weakSelf = self;
    __weak NSXPCConnection *weakConnection = connection;
    // Both interruption and invalidation revoke immediately outside RT. No retry,
    // no retained producer authority, and a reconnect must perform fresh admission.
    connection.interruptionHandler = ^{ [session close]; [weakConnection invalidate]; };
    connection.invalidationHandler = ^{
        [session close];
        MYBroker *owner = weakSelf;
        NSXPCConnection *closed = weakConnection;
        @synchronized (owner) { if (closed) [owner.connections removeObject:closed]; }
    };
    [connection resume]; return YES;
}
- (void)endpoint:(void (^)(NSXPCListenerEndpoint *))reply { reply(self.anonymous.endpoint); }
- (void)snapshot:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    reply(@{@"active": @(state.active), @"generation": @(state.nextGeneration),
            @"packets": @(state.packets), @"hellos": @(state.hellos),
            @"credit_frames": @(state.credits()), @"pid": @(getpid()), @"uid": @(getuid())});
}
- (void)read:(uint32_t)reader frame:(uint64_t)frame count:(uint32_t)count
       reply:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    if (reader >= 2 || !count || count > mixyard::packetFrames ||
        frame != state.cursors[reader] || frame > mixyard::maxPosition - count) {
        reply(status(@"invalid")); return;
    }
    std::array<float, mixyard::packetFrames * 2> pcm{};
    AudioServerPlugInIOCycleInfo cycle{};
    cycle.mInputTime.mFlags = kAudioTimeStampSampleTimeValid;
    cycle.mInputTime.mSampleTime = static_cast<double>(frame);
    if ((*state.driver)->DoIOOperation(state.driver, 2, 3, reader + 1,
        kAudioServerPlugInIOOperationReadInput, count, &cycle, pcm.data(), nullptr) != noErr) {
        reply(status(@"read_failed")); return;
    }
    MixyardInputTestEvidence e{};
    if (MixyardInputTest_Evidence(2, reader + 1, &e) != MY_INPUT_OK) {
        reply(status(@"evidence_failed")); return;
    }
    state.cursors[reader] = frame + count;
    reply(@{@"status": @"fixture_read", @"pcm": [NSData dataWithBytes:pcm.data() length:count * 8],
            @"generation": @(e.generation), @"epoch": @(e.clockSeed),
            @"effective_revision": @(e.effectiveRevision), @"first_frame": @(e.firstFrame),
            @"next_frame": @(e.nextFrame), @"delivered_frames": @(e.deliveredFrames),
            @"zero_frames": @(e.zeroFrames), @"credit_frames": @(state.credits()),
            @"external_consumer_pcm": @NO});
}
@end

int main() {
    @autoreleasepool {
        if (@available(macOS 13.0, *)) {} else { return 2; }
        NSString *requirement = mixyard::fixturePolicy()[@"transport"];
        if (!mixyard::exactRequirement(requirement)) return 2;
        state.driver = static_cast<AudioServerPlugInDriverRef>(MixyardInput_Create(nullptr, kAudioServerPlugInTypeUUID));
        if (!state.driver || (*state.driver)->Initialize(state.driver, &host)) return 2;
        for (uint32_t i = 1; i <= 2; ++i) {
            AudioServerPlugInClientInfo info{i, getpid(), true, CFSTR("org.mixyard.fixture.reader")};
            if ((*state.driver)->AddDeviceClient(state.driver, 2, &info) ||
                (*state.driver)->StartIO(state.driver, 2, i)) return 2;
        }
        Float64 sample = 0; UInt64 time = 0, epoch = 0;
        if ((*state.driver)->GetZeroTimeStamp(state.driver, 2, 1, &sample, &time, &epoch)) return 2;
        state.epoch = epoch;
        MYBroker *broker = [MYBroker new]; broker.requirement = requirement;
        broker.connections = [NSMutableSet new]; broker.anonymous = NSXPCListener.anonymousListener;
        broker.anonymous.delegate = broker; [broker.anonymous resume];
        NSXPCListener *service = NSXPCListener.serviceListener;
        service.delegate = broker; [service resume];
        dispatch_main();
    }
}

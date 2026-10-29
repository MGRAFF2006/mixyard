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
void authorityEvent(NSDictionary *event) {
    NSMutableDictionary *record = [event mutableCopy];
    record[@"broker_pid"] = @(getpid());
    NSData *json = [NSJSONSerialization dataWithJSONObject:record options:0 error:nil];
    NSString *path = [[mixyard::fixtureRoot() stringByAppendingPathComponent:@"authority-events"]
        stringByAppendingPathComponent:[NSUUID.UUID.UUIDString stringByAppendingString:@".json"]];
    assert(json && [json writeToFile:path atomically:YES] && "fixture authority evidence write failed");
}
}

@interface MYSession : NSObject <MYPCMTransport, MYFixtureControl> {
    uint64_t _generation;
    uint64_t _identityRequests, _authenticationRequests, _confirmationRequests, _helloRequests, _packetRequests;
    mixyard::Readiness _readiness;
}
@property(strong) NSXPCListenerEndpoint *endpoint;
@property(strong) NSDictionary *context;
@property(copy) void (^beginProbe)(void);
- (BOOL)probeCompleted;
- (void)expire:(NSXPCConnection *)connection;
- (void)closeLocked;
- (void)close;
@end
@implementation MYSession
- (void)identify:(void (^)(NSDictionary *))reply {
    // Reply only: no reverse invocation can race this receiver-identity check.
    std::lock_guard<std::mutex> lock(state.mutex);
    ++_identityRequests;
    reply(status(_readiness.closed ? @"closed" : @"identity"));
}
- (void)authenticate:(void (^)(NSDictionary *))reply {
    void (^probe)(void) = nil;
    {
        std::lock_guard<std::mutex> lock(state.mutex);
        ++_authenticationRequests;
        _readiness.authenticate(reply);
        if (!_readiness.closed) { probe = self.beginProbe; self.beginProbe = nil; }
    }
    if (probe) probe();
}
- (BOOL)probeCompleted {
    std::lock_guard<std::mutex> lock(state.mutex);
    return _readiness.probeCompleted();
}
- (void)confirm:(NSString *)challenge reply:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    ++_confirmationRequests;
    const bool accepted = _readiness.confirm(challenge);
    if (!accepted) [self closeLocked];
    reply(status(accepted ? @"ready" : @"rejected"));
}
- (void)expire:(NSXPCConnection *)connection {
    // A finite pending-session lifetime only revokes; time never grants authority.
    {
        std::lock_guard<std::mutex> lock(state.mutex);
        if (_readiness.ready) return;
        [self closeLocked];
    }
    [connection invalidate];
}
- (void)hello:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    ++_helloRequests;
    if (_readiness.closed || !_readiness.ready || self.endpoint) { reply(status(@"not_ready")); return; }
    if (_generation || state.active || state.nextGeneration == UINT64_MAX) {
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
            @"sample_rate": @48000, @"channels": @2, @"format": @"Float32-interleaved",
            @"provenance": @"authenticated-anonymous-XPC-fixture", @"backendAvailable": @NO, @"audioApplied": @NO});
}
- (void)packet:(NSData *)data reply:(void (^)(NSDictionary *))reply {
    { std::lock_guard<std::mutex> lock(state.mutex); ++_packetRequests; }
    // Foundation decoding is OS-owned. Application validates before any PCM copy;
    // max message bytes is 32824, not a pre-decode allocation guarantee for NSXPC.
    mixyard::Header h{};
    if (![data isKindOfClass:NSData.class] || data.length < sizeof(h) ||
        data.length > sizeof(h) + mixyard::packetFrames * 8) {
        reply(status(@"invalid")); return;
    }
    std::memcpy(&h, data.bytes, sizeof(h));
    if (!mixyard::packetShape(h, data.length)) { reply(status(@"invalid")); return; }
    std::array<float, mixyard::packetFrames * 2> pcm{};
    std::memcpy(pcm.data(), static_cast<const char *>(data.bytes) + sizeof(h), h.bytes);
    for (uint32_t i = 0; i < h.frames * 2; ++i)
        if (!std::isfinite(pcm[i])) { reply(status(@"invalid")); return; }
    std::lock_guard<std::mutex> lock(state.mutex);
    if (_readiness.closed || !_readiness.ready || self.endpoint || !_generation || state.active != _generation) {
        reply(status(@"not_admitted")); return;
    }
    const auto admission = mixyard::packetAdmission(h, _generation, state.epoch, state.end, state.revision, state.credits());
    if (admission == mixyard::PacketAdmission::Stale) {
        reply(status(@"stale")); return;
    }
    if (admission == mixyard::PacketAdmission::Backpressure) { reply(status(@"backpressure")); return; }
    if (MixyardInputTest_Packet(2, h.generation, h.epoch, h.revision, h.position,
                              h.frames, pcm.data(), h.bytes) != MY_INPUT_OK) {
        reply(status(@"ring_rejected")); return;
    }
    state.end += h.frames; state.revision = h.revision; ++state.packets;
    reply(@{@"status": @"queued", @"generation": @(_generation),
            @"next_position": @(state.end), @"credit_frames": @(state.credits()),
            @"effective_delivery_ack": @NO});
}
- (void)closeLocked {
    const bool wasClosed = _readiness.closed, wasReady = _readiness.ready;
    _readiness.close();
    self.beginProbe = nil;
    if (_generation && state.active == _generation) {
        (void)MixyardInputTest_Disconnect(2, _generation);
        state.active = 0;
    }
    if (!wasClosed) {
        NSMutableDictionary *event = [self.context mutableCopy];
        event[@"outcome"] = @"session_closed";
        event[@"verified"] = @(_readiness.verified); event[@"ready"] = @(wasReady);
        event[@"requests"] = @[@(_identityRequests), @(_authenticationRequests), @(_confirmationRequests),
            @(_helloRequests), @(_packetRequests)];
        event[@"state"] = @{@"active": @(state.active), @"generation": @(state.nextGeneration),
            @"packets": @(state.packets), @"hellos": @(state.hellos),
            @"credit_frames": @(state.credits()), @"cursor0": @(state.cursors[0]),
            @"cursor1": @(state.cursors[1]), @"start": @(state.start), @"end": @(state.end),
            @"revision": @(state.revision)};
        authorityEvent(event);
    }
}
- (void)close {
    std::lock_guard<std::mutex> lock(state.mutex);
    [self closeLocked];
}
- (void)endpoint:(void (^)(NSXPCListenerEndpoint *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    reply(!_readiness.closed && _readiness.ready ? self.endpoint : nil);
}
- (void)snapshot:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    if (_readiness.closed || !_readiness.ready || !self.endpoint) { reply(status(@"not_ready")); return; }
    reply(@{@"active": @(state.active), @"generation": @(state.nextGeneration),
            @"packets": @(state.packets), @"hellos": @(state.hellos),
            @"credit_frames": @(state.credits()), @"cursor0": @(state.cursors[0]),
            @"cursor1": @(state.cursors[1]), @"start": @(state.start), @"end": @(state.end),
            @"revision": @(state.revision), @"pid": @(getpid()), @"uid": @(getuid())});
}
- (void)read:(uint32_t)reader frame:(uint64_t)frame count:(uint32_t)count
       reply:(void (^)(NSDictionary *))reply {
    std::lock_guard<std::mutex> lock(state.mutex);
    if (_readiness.closed || !_readiness.ready || !self.endpoint) { reply(status(@"not_ready")); return; }
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

@interface MYBroker : NSObject <NSXPCListenerDelegate>
@property(strong) NSXPCListener *anonymous;
@property(strong) NSString *requirement;
@property(strong) NSMutableSet<NSXPCConnection *> *connections;
@end
@implementation MYBroker
- (BOOL)listener:(NSXPCListener *)listener shouldAcceptNewConnection:(NSXPCConnection *)connection {
    NSString *requirement = listener == self.anonymous ? self.requirement : mixyard::fixturePolicy()[@"producer"];
    NSDictionary *context = @{@"channel": listener == self.anonymous ? @"pcm" : @"control",
        @"peer_pid": @(connection.processIdentifier), @"requirement": requirement ?: @"<unavailable>",
        @"connection": NSUUID.UUID.UUIDString};
    // Anonymous connections inherit their listener pin (single-assignment).
    // serviceListener does not support listener pins; pin its connection before
    // resume, replies, reverse probes or authority instead.
    if (!mixyard::exactRequirement(requirement) ||
        (listener != self.anonymous && !mixyard::pin(connection, requirement))) {
        std::lock_guard<std::mutex> lock(state.mutex);
        NSMutableDictionary *event = [context mutableCopy]; event[@"outcome"] = @"policy_unavailable";
        // Return NO before creating/exporting any session; capture actual state
        // so a generic client connection error cannot substitute for this branch.
        event[@"terminal_rejection"] = @YES; event[@"session_created"] = @NO;
        event[@"state"] = @{@"active": @(state.active), @"generation": @(state.nextGeneration),
            @"packets": @(state.packets), @"hellos": @(state.hellos),
            @"credit_frames": @(state.credits()), @"cursor0": @(state.cursors[0]),
            @"cursor1": @(state.cursors[1]), @"start": @(state.start), @"end": @(state.end),
            @"revision": @(state.revision)};
        authorityEvent(event); return NO;
    }
    if (listener != self.anonymous) {
        NSMutableDictionary *event = [context mutableCopy]; event[@"outcome"] = @"connection_pinned";
        authorityEvent(event);
    }
    @synchronized (self) {
        // Finite fixture budget, not product capacity. Reclaimed on invalidation.
        if (self.connections.count >= 8) return NO;
        [self.connections addObject:connection];
    }
    MYSession *session = [MYSession new];
    session.context = context;
    if (listener == self.anonymous) {
        connection.exportedInterface = pcmInterface();
        connection.exportedObject = session;
    } else {
        session.endpoint = self.anonymous.endpoint;
        connection.exportedInterface = controlInterface(); connection.exportedObject = session;
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
    connection.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYAuthorityProbe)];
    __weak MYSession *weakSession = session;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC),
        dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{ [weakSession expire:weakConnection]; });
    // The client checks an identify reply before asking for this reverse probe.
    // The incoming connection's exact requirement checks received messages.
    session.beginProbe = ^{
        MYSession *pending = weakSession;
        NSXPCConnection *live = weakConnection;
        if (!pending || !live) return;
        [(id<MYAuthorityProbe>)[live remoteObjectProxyWithErrorHandler:^(NSError *error) {
            [pending close]; [weakConnection invalidate];
            NSMutableDictionary *event = [context mutableCopy]; event[@"outcome"] = @"probe_error";
            event[@"error"] = mixyard::errorDetails(error); authorityEvent(event);
        }] probe:^{
            const BOOL accepted = [pending probeCompleted];
            NSMutableDictionary *event = [context mutableCopy];
            event[@"outcome"] = accepted ? @"probe_reply" : @"late_probe_ignored";
            authorityEvent(event);
        }];
    };
    [connection resume];
    return YES;
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
        if (!mixyard::pinListener(broker.anonymous, requirement)) return 2;
        authorityEvent(@{@"channel": @"pcm", @"outcome": @"listener_pinned", @"requirement": requirement});
        broker.anonymous.delegate = broker; [broker.anonymous resume];
        NSXPCListener *service = NSXPCListener.serviceListener;
        // The delegate pins each service connection or rejects unavailable policy.
        service.delegate = broker; [service resume];
        dispatch_main();
    }
}

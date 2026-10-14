// SPDX-License-Identifier: MIT
// Actual native subprocess/XPC assertions; all PCM reads remain ordinary-process ABI.
#import "../../adapters/macos/input-broker/protocol.h"
#include <array>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <limits>
#include <unistd.h>

@interface MYResult : NSObject
@property(strong) NSCondition *condition;
@property(strong) id value;
@property BOOL done;
- (void)finish:(id)value;
- (id)wait;
@end
@implementation MYResult
- (instancetype)init { if ((self = [super init])) _condition = [NSCondition new]; return self; }
- (void)finish:(id)value {
    [_condition lock];
    if (!_done) { _value = value; _done = YES; [_condition signal]; }
    [_condition unlock];
}
- (id)wait {
    [_condition lock];
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:10];
    while (!_done && [_condition waitUntilDate:deadline]) {}
    assert(_done && "native XPC reply/error timed out; timeout is never auth evidence");
    id value = _value; [_condition unlock]; return value;
}
@end

@interface MYProbe : NSObject <MYAuthorityProbe>
@property(strong) MYResult *held;
@end
@implementation MYProbe
- (void)probe:(void (^)(void))reply { if (self.held) [self.held finish:[reply copy]]; else reply(); }
@end
static void exportProbe(NSXPCConnection *connection, MYProbe *probe = nil) {
    connection.exportedInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYAuthorityProbe)];
    connection.exportedObject = probe ?: [MYProbe new];
}
static NSDictionary *rejection(id result, bool enforcingSide) {
    NSLog(@"native rejection (enforcing=%d): %@", enforcingSide, result);
    assert([result isKindOfClass:NSError.class]);
    NSError *error = result;
    assert([error.domain isEqual:NSCocoaErrorDomain]);
    assert(error.code == NSXPCConnectionCodeSigningRequirementFailure ||
           (!enforcingSide && error.code == NSXPCConnectionInvalid));
    return mixyard::errorDetails(error);
}

static id proxy(NSXPCConnection *connection, MYResult *result) {
    return [connection remoteObjectProxyWithErrorHandler:^(NSError *error) { [result finish:error]; }];
}
static id authenticate(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; [(id<MYReadiness>)proxy(connection, r)
        authenticate:^(NSDictionary *value) { [r finish:value]; }]; return [r wait];
}
static id confirm(NSXPCConnection *connection, NSString *challenge) {
    MYResult *r = [MYResult new]; [(id<MYReadiness>)proxy(connection, r)
        confirm:challenge reply:^(NSDictionary *value) { [r finish:value]; }]; return [r wait];
}
// Only called after receiving a pinned response from the actual receiver.
static id ready(NSXPCConnection *connection) {
    id challenge = authenticate(connection);
    if ([challenge isKindOfClass:NSError.class]) return challenge;
    assert([challenge[@"status"] isEqual:@"challenge"]);
    assert([challenge[@"challenge"] isKindOfClass:NSString.class]);
    return confirm(connection, challenge[@"challenge"]);
}
static void actorReady(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; [(id<MYAuthorityProbe>)proxy(connection, r)
        probe:^{ [r finish:@YES]; }]; assert([[r wait] isEqual:@YES]);
}
static id hello(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; [(id<MYPCMTransport>)proxy(connection, r)
        hello:^(NSDictionary *value) { [r finish:value]; }]; return [r wait];
}
static NSData *packet(mixyard::Header h, float sample = 0.25f) {
    NSMutableData *data = [NSMutableData dataWithLength:sizeof(h) + h.frames * 8];
    std::memcpy(data.mutableBytes, &h, sizeof(h));
    for (uint32_t i = 0; i < h.frames * 2; ++i)
        std::memcpy(static_cast<char *>(data.mutableBytes) + sizeof(h) + i * 4, &sample, 4);
    return data;
}
static id send(NSXPCConnection *connection, NSData *data) {
    MYResult *r = [MYResult new]; [(id<MYPCMTransport>)proxy(connection, r)
        packet:data reply:^(NSDictionary *value) { [r finish:value]; }]; return [r wait];
}
static id fetchEndpoint(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; [(id<MYFixtureControl>)proxy(connection, r)
        endpoint:^(NSXPCListenerEndpoint *endpoint) { [r finish:endpoint]; }]; return [r wait];
}
static NSDictionary *snapshot(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; [(id<MYFixtureControl>)proxy(connection, r)
        snapshot:^(NSDictionary *value) { [r finish:value]; }];
    id value = [r wait]; assert([value isKindOfClass:NSDictionary.class]); return value;
}
static void expect(id result, NSString *value) {
    if (![result isKindOfClass:NSDictionary.class] || ![result[@"status"] isEqual:value]) {
        NSLog(@"expected %@, got %@", value, result); abort();
    }
}
static NSXPCConnection *connect(NSXPCListenerEndpoint *endpoint, NSString *requirement, MYProbe *probe = nil) {
    NSXPCConnection *c = [[NSXPCConnection alloc] initWithListenerEndpoint:endpoint];
    c.remoteObjectInterface = pcmInterface();
    if (!mixyard::pin(c, requirement)) { [c invalidate]; return nil; }
    exportProbe(c, probe);
    [c resume]; return c;
}
static id readResult(NSXPCConnection *control, uint32_t reader, uint64_t frame, uint32_t count) {
    MYResult *r = [MYResult new]; [(id<MYFixtureControl>)proxy(control, r)
        read:reader frame:frame count:count reply:^(NSDictionary *value) { [r finish:value]; }];
    return [r wait];
}
static NSDictionary *read(NSXPCConnection *control, uint32_t reader, uint64_t frame, uint32_t count) {
    id value = readResult(control, reader, frame, count); expect(value, @"fixture_read");
    assert(![value[@"external_consumer_pcm"] boolValue]); return value;
}
static void samples(NSDictionary *read, float sample, uint32_t count) {
    NSData *pcm = read[@"pcm"]; assert(pcm.length == count * 8);
    for (uint32_t i = 0; i < count * 2; ++i) {
        float actual; std::memcpy(&actual, static_cast<const char *>(pcm.bytes) + i * 4, 4);
        assert(actual == sample);
    }
}
static void disconnected(NSXPCConnection *control) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:5];
    while ([snapshot(control)[@"active"] unsignedLongLongValue]) {
        assert([deadline timeIntervalSinceNow] > 0 && "producer authority not revoked");
    }
}

// Same binary/signature as the trusted producer when built as CrashRelay.xpc.
// Different signed executable when compiled with MIXYARD_STRANGER. Endpoint is
// passed via a native XPC message, never archived Mach rights into a disk/PID path.
@interface MYActor : NSObject <MYStranger, NSXPCListenerDelegate>
@property(strong) NSString *producerRequirement;
@end
@implementation MYActor
- (void)probe:(void (^)(void))reply { reply(); }
- (BOOL)listener:(NSXPCListener *)listener shouldAcceptNewConnection:(NSXPCConnection *)connection {
    (void)listener;
    if (!mixyard::pin(connection, self.producerRequirement)) return NO;
    connection.exportedInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
    connection.exportedObject = self; [connection resume]; return YES;
}
- (void)attempt:(NSXPCListenerEndpoint *)endpoint requirement:(NSString *)requirement
          reply:(void (^)(NSDictionary *))reply {
    NSXPCConnection *c = connect(endpoint, requirement); assert(c);
    mixyard::Header h; h.generation = h.epoch = h.revision = 1; h.frames = 1; h.bytes = 8;
    id early = send(c, packet(h, 0.875f));
#ifdef MIXYARD_STRANGER
    // This path intentionally differs in executable code, not only bundle name.
    NSDictionary *error = rejection(early, false);
    reply(@{@"rejection_observed": @YES, @"pid": @(getpid()), @"uid": @(getuid()), @"error": error});
    [c invalidate];
#else
    expect(early, @"not_admitted");
    expect(ready(c), @"ready");
    NSDictionary *admitted = hello(c); expect(admitted, @"admitted");
    h.generation = [admitted[@"generation"] unsignedLongLongValue];
    h.epoch = [admitted[@"epoch"] unsignedLongLongValue];
    h.position = [admitted[@"position"] unsignedLongLongValue];
    expect(send(c, packet(h, 0.5f)), @"queued");
    (void)reply;
    std::_Exit(0); // Abrupt producer loss after queued PCM; no graceful invalidation.
#endif
}
@end

int main(int argc, char **argv) {
    @autoreleasepool {
        if (@available(macOS 13.0, *)) {} else { return 2; }
        if ([[NSBundle.mainBundle objectForInfoDictionaryKey:@"CFBundlePackageType"] isEqual:@"XPC!"]) {
            MYActor *actor = [MYActor new];
            actor.producerRequirement = mixyard::fixturePolicy()[@"producer"];
            if (!mixyard::exactRequirement(actor.producerRequirement)) return 2;
            NSXPCListener *listener = NSXPCListener.serviceListener;
            listener.delegate = actor; [listener resume]; dispatch_main();
        }
        assert(argc == 5);
        NSString *brokerRequirement = [NSString stringWithUTF8String:argv[1]];
        NSString *strangerRequirement = [NSString stringWithUTF8String:argv[2]];
        NSString *crashRequirement = [NSString stringWithUTF8String:argv[3]];
        NSString *mode = [NSString stringWithUTF8String:argv[4]];
        assert(mixyard::exactRequirement(brokerRequirement));
        NSXPCConnection *control = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.broker.fixture"];
        control.remoteObjectInterface = controlInterface(); assert(mixyard::pin(control, brokerRequirement));
        exportProbe(control);
        [control resume];
        id controlReady = ready(control);
        if ([mode isEqual:@"positive"]) expect(controlReady, @"ready");
        if (![mode isEqual:@"positive"]) {
            NSDictionary *error = rejection(controlReady, false);
            NSData *json = [NSJSONSerialization dataWithJSONObject:@{@"status": @"rejection_observed",
                @"case": mode, @"producer_pid": @(getpid()), @"error": error} options:0 error:nil];
            assert(json); fwrite(json.bytes, 1, json.length, stdout); putchar('\n');
            [control invalidate]; return 0;
        }
        MYResult *endpointResult = [MYResult new]; [(id<MYFixtureControl>)proxy(control, endpointResult)
            endpoint:^(NSXPCListenerEndpoint *endpoint) { [endpointResult finish:endpoint]; }];
        id endpointValue = [endpointResult wait];
        assert([endpointValue isKindOfClass:NSXPCListenerEndpoint.class]);
        NSXPCListenerEndpoint *endpoint = endpointValue;
        auto initial = snapshot(control); assert([initial[@"active"] unsignedLongLongValue] == 0);
        assert([initial[@"generation"] unsignedLongLongValue] == 0);
        assert([initial[@"cursor0"] unsignedLongLongValue] == 0 &&
               [initial[@"cursor1"] unsignedLongLongValue] == 0);
        mixyard::Header earlyHeader; earlyHeader.generation = earlyHeader.epoch = earlyHeader.revision = 1;
        earlyHeader.frames = 1; earlyHeader.bytes = 8;
        // Hold an actual native reverse reply: no sleeps or timing-based readiness.
        MYProbe *heldPCM = [MYProbe new]; heldPCM.held = [MYResult new];
        NSXPCConnection *pendingPCM = connect(endpoint, brokerRequirement, heldPCM);
        void (^releasePCM)(void) = [heldPCM.held wait];
        expect(hello(pendingPCM), @"not_ready");
        expect(send(pendingPCM, packet(earlyHeader)), @"not_admitted");
        MYResult *pendingAuthentication = [MYResult new];
        [(id<MYReadiness>)proxy(pendingPCM, pendingAuthentication) authenticate:^(NSDictionary *value) {
            [pendingAuthentication finish:value];
        }];
        // Early confirmation closes the session, including a pending authenticate.
        expect(confirm(pendingPCM, @"early"), @"rejected");
        expect([pendingAuthentication wait], @"closed");
        releasePCM();
        expect(authenticate(pendingPCM), @"closed");
        expect(hello(pendingPCM), @"not_ready");
        expect(send(pendingPCM, packet(earlyHeader)), @"not_admitted");
        assert([snapshot(control) isEqual:initial]);

        MYProbe *heldControl = [MYProbe new]; heldControl.held = [MYResult new];
        NSXPCConnection *pendingControl = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.broker.fixture"];
        pendingControl.remoteObjectInterface = controlInterface();
        assert(mixyard::pin(pendingControl, brokerRequirement)); exportProbe(pendingControl, heldControl);
        [pendingControl resume];
        void (^releaseControl)(void) = [heldControl.held wait];
        assert(!fetchEndpoint(pendingControl));
        expect(snapshot(pendingControl), @"not_ready");
        expect(readResult(pendingControl, 0, 0, 8), @"not_ready");
        expect(confirm(pendingControl, @"early"), @"rejected");
        releaseControl();
        expect(authenticate(pendingControl), @"closed");
        assert(!fetchEndpoint(pendingControl));
        expect(readResult(pendingControl, 1, 0, 8), @"not_ready");
        assert([snapshot(control) isEqual:initial]);

        NSXPCConnection *badConfirmation = connect(endpoint, brokerRequirement);
        id challenge = authenticate(badConfirmation); expect(challenge, @"challenge");
        expect(hello(badConfirmation), @"not_ready");
        expect(send(badConfirmation, packet(earlyHeader)), @"not_admitted");
        expect(confirm(badConfirmation, @"wrong"), @"rejected");
        // A late correct token cannot reopen a failed session.
        expect(confirm(badConfirmation, challenge[@"challenge"]), @"rejected");
        expect(hello(badConfirmation), @"not_ready");
        assert([snapshot(control) isEqual:initial]);
        [badConfirmation invalidate];

        // Supervisor wrong-receiver pin fails before endpoint/read authority.
        NSXPCConnection *staleControl = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.broker.fixture"];
        staleControl.remoteObjectInterface = controlInterface();
        assert(mixyard::pin(staleControl, @"cdhash H\"0000000000000000000000000000000000000000\""));
        exportProbe(staleControl); [staleControl resume];
        NSDictionary *staleControlError = rejection(authenticate(staleControl), true);
        [staleControl invalidate];
        assert([snapshot(control) isEqual:initial]);
        // Locally invalid/broad requirements fail closed before any XPC resume.
        assert(!connect(endpoint, @"garbage") && !connect(endpoint, @"identifier \"org.mixyard.fixture\""));
        // Reverse authority: a stale broker requirement invalidates the native connection.
        NSXPCConnection *stale = connect(endpoint, @"cdhash H\"0000000000000000000000000000000000000000\"");
        assert(stale); NSDictionary *staleError = rejection(authenticate(stale), true); [stale invalidate];
        assert([snapshot(control)[@"hellos"] unsignedLongLongValue] == 0);
        assert([snapshot(control) isEqual:initial]);
        // A valid requirement for a different signed executable is also a wrong receiver.
        NSXPCConnection *wrongReceiver = connect(endpoint, strangerRequirement);
        NSDictionary *wrongReceiverError = rejection(authenticate(wrongReceiver), true);
        [wrongReceiver invalidate];
        assert([snapshot(control) isEqual:initial]);
        // Same UID, different signed executable gets the genuine anonymous endpoint.
        NSXPCConnection *stranger = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.stranger.fixture"];
        stranger.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
        assert(mixyard::pin(stranger, strangerRequirement)); [stranger resume]; actorReady(stranger);
        MYResult *attack = [MYResult new]; [(id<MYStranger>)proxy(stranger, attack)
            attempt:endpoint requirement:brokerRequirement reply:^(NSDictionary *value) { [attack finish:value]; }];
        id rejected = [attack wait]; assert([rejected isKindOfClass:NSDictionary.class]);
        assert([rejected[@"rejection_observed"] boolValue] && [rejected[@"uid"] unsignedIntValue] == getuid());
        assert([rejected[@"pid"] intValue] != [initial[@"pid"] intValue]);
        assert([snapshot(control)[@"packets"] unsignedLongLongValue] == 0);
        assert([snapshot(control)[@"hellos"] unsignedLongLongValue] == 0);
        samples(read(control, 0, 0, 8), 0, 8); samples(read(control, 1, 0, 8), 0, 8);
        NSXPCConnection *producer = connect(endpoint, brokerRequirement); assert(producer);
        mixyard::Header h; h.generation = h.epoch = h.revision = 1;
        h.position = 8; h.frames = 8; h.bytes = 64;
        expect(send(producer, packet(h)), @"not_admitted");
        expect(hello(producer), @"not_ready");
        expect(ready(producer), @"ready");
        NSDictionary *admitted = hello(producer); expect(admitted, @"admitted");
        h.generation = [admitted[@"generation"] unsignedLongLongValue];
        h.epoch = [admitted[@"epoch"] unsignedLongLongValue];
        assert([admitted[@"position"] unsignedLongLongValue] == 8);
        auto good = packet(h);
        // Every rejected mutation preserves admission, ring position and credit.
        for (unsigned which = 0; which < 14; ++which) {
            auto bad = h;
            switch (which) {
                case 0: bad.version = 2; break;
                case 1: bad.sampleRate = 44100; break;
                case 2: bad.channels = 1; break;
                case 3: bad.format = 2; break;
                case 4: bad.generation += 1; break;
                case 5: bad.epoch += 1; break;
                case 6: bad.position += 1; break;
                case 7: bad.position = UINT64_MAX; break;
                case 8: bad.revision = 0; break;
                case 9: bad.frames = 0; break;
                case 10: bad.bytes -= 1; break;
                default: break;
            }
            NSMutableData *data = [packet(bad) mutableCopy];
            if (which == 11) [data setLength:data.length - 1];
            if (which == 12) data = [packet(bad, std::numeric_limits<float>::infinity()) mutableCopy];
            if (which == 13) data = [packet(bad, std::numeric_limits<float>::quiet_NaN()) mutableCopy];
            id result = send(producer, data);
            assert([result isKindOfClass:NSDictionary.class] && ![result[@"status"] isEqual:@"queued"]);
            assert([snapshot(control)[@"packets"] unsignedLongLongValue] == 0);
            assert([snapshot(control)[@"credit_frames"] unsignedIntValue] == 4096);
        }
        expect(send(producer, [NSMutableData dataWithLength:sizeof(h) - 1]), @"invalid");
        expect(send(producer, [NSMutableData dataWithLength:sizeof(h) + 4096 * 8 + 1]), @"invalid");
        NSDictionary *queued = send(producer, good); expect(queued, @"queued");
        assert(![queued[@"effective_delivery_ack"] boolValue] && [queued[@"credit_frames"] unsignedIntValue] == 4088);
        expect(send(producer, good), @"stale"); // replay / out of order
        h.position = 16; h.frames = 4088; h.bytes = h.frames * 8; h.revision = 2;
        expect(send(producer, packet(h, -0.25f)), @"queued");
        h.position = 4104; h.frames = 1; h.bytes = 8;
        expect(send(producer, packet(h)), @"backpressure");
        NSDictionary *fast = read(control, 0, 8, 8); samples(fast, 0.25f, 8);
        assert([fast[@"effective_revision"] unsignedLongLongValue] == 1);
        assert([fast[@"credit_frames"] unsignedIntValue] == 0); // stalled second reader
        expect(send(producer, packet(h)), @"backpressure");
        NSDictionary *slow = read(control, 1, 8, 8); samples(slow, 0.25f, 8);
        assert([slow[@"credit_frames"] unsignedIntValue] == 8);
        h.revision = 1; expect(send(producer, packet(h)), @"stale");
        h.revision = 3; expect(send(producer, packet(h)), @"queued");
        [producer invalidate]; disconnected(control);
        // Queued/unread PCM must become zeros on disconnect, for both readers.
        samples(read(control, 0, 16, 4088), 0, 4088); samples(read(control, 1, 16, 4088), 0, 4088);
        NSXPCConnection *fresh = connect(endpoint, brokerRequirement);
        expect(ready(fresh), @"ready");
        NSDictionary *newAdmission = hello(fresh); expect(newAdmission, @"admitted");
        assert([newAdmission[@"generation"] unsignedLongLongValue] > h.generation);
        expect(send(fresh, packet(h)), @"stale"); // stale generation, otherwise valid new position
        h.generation = [newAdmission[@"generation"] unsignedLongLongValue];
        h.frames = 8; h.bytes = 64; h.revision = 1;
        expect(send(fresh, packet(h, 0.5f)), @"queued");
        for (uint32_t i = 0; i < 2; ++i) samples(read(control, i, 4104, 8), 0.5f, 8);
        [fresh invalidate]; disconnected(control);
        // Exact trusted executable abruptly exits after enqueueing one frame.
        NSXPCConnection *crash = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.crash.fixture"];
        crash.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
        assert(mixyard::pin(crash, crashRequirement)); [crash resume]; actorReady(crash);
        MYResult *crashed = [MYResult new]; [(id<MYStranger>)proxy(crash, crashed)
            attempt:endpoint requirement:brokerRequirement reply:^(NSDictionary *value) { [crashed finish:value]; }];
        assert([[crashed wait] isKindOfClass:NSError.class]); disconnected(control);
        assert([snapshot(control)[@"packets"] unsignedLongLongValue] == 5);
        for (uint32_t i = 0; i < 2; ++i) samples(read(control, i, 4112, 8), 0, 8);
        // Tail exceeds retained ring, no trimming or stale/default/sine fallback.
        for (uint32_t n = 0; n < 5; ++n)
            for (uint32_t i = 0; i < 2; ++i) samples(read(control, i, 4120 + n * 4096, 4096), 0, 4096);
        NSDictionary *finalState = snapshot(control);
        assert([finalState[@"hellos"] unsignedLongLongValue] == 3);
        assert([finalState[@"pid"] intValue] != getpid());
        NSData *json = [NSJSONSerialization dataWithJSONObject:@{@"schema_version": @1, @"status": @"pass",
            @"evidence_kind": @"native_authenticated_anonymous_XPC_with_in_process_Apple_ABI_reads",
            @"producer_pid": @(getpid()), @"broker": finalState, @"stranger": rejected,
            @"stale_broker_error": staleError, @"stale_supervisor_broker_error": staleControlError,
            @"wrong_broker_error": wrongReceiverError,
            @"authentication_before_authority": @YES, @"early_authority_unchanged": initial,
            @"requirement_failure_code": @(NSXPCConnectionCodeSigningRequirementFailure),
            @"anonymous_transport_authentication": @"executed", @"system_bootstrap": @"BLOCKED",
            @"hosted_HAL_peer_identity": @"BLOCKED", @"external_consumer_pcm": @"BLOCKED", @"production_apply": @NO,
            @"checks": @[@"exact_peer_both_directions", @"same_UID_different_signed_executable", @"early_messages",
                @"held_native_probe_no_authority", @"late_probe_closed_session", @"failed_confirmation_terminal",
                @"supervisor_read_endpoint_cursor_credit_gated", @"bad_stale_requirements", @"format_length_finite_position_epoch_generation_revision",
                @"replay_out_of_order_overflow_truncation", @"credited_backpressure_independent_readers",
                @"queued_not_delivery_ACK", @"disconnect_fresh_generation", @"producer_crash_zero_tail"]}
            options:NSJSONWritingPrettyPrinted error:nil];
        assert(json); fwrite(json.bytes, 1, json.length, stdout); putchar('\n');
        [pendingPCM invalidate]; [pendingControl invalidate];
        [stranger invalidate]; [crash invalidate]; [control invalidate]; return 0;
    }
}

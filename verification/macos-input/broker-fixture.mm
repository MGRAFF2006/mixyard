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
@property(copy) NSString *stage;
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
    if (!_done) NSLog(@"TIMEOUT stage=%@", _stage ?: @"unlabeled");
    assert(_done && "native XPC reply/error timed out; timeout is never auth evidence");
    id value = _value; [_condition unlock]; return value;
}
@end

@interface MYProbe : NSObject <MYAuthorityProbe>
@property(strong) MYResult *held;
@property BOOL receiverVerified;
@end
@implementation MYProbe
- (void)probe:(void (^)(void))reply {
    @synchronized (self) { assert(self.receiverVerified && "reverse invocation preceded pinned identity reply"); }
    if (self.held) [self.held finish:[reply copy]]; else reply();
}
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
    NSLog(@"RPC stage=%@ peer=%d", result.stage ?: @"unlabeled", connection.processIdentifier);
    return [connection remoteObjectProxyWithErrorHandler:^(NSError *error) { [result finish:error]; }];
}
static MYResult *beginAuthentication(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; r.stage = @"identify";
    // First receive a reply from the pinned receiver. Only then may it invoke
    // our reverse probe; a rejected invocation cannot hide the initial 4102.
    [(id<MYReadiness>)proxy(connection, r) identify:^(NSDictionary *identity) {
        if ([identity[@"status"] isEqual:@"closed"]) { [r finish:identity]; return; }
        assert([identity[@"status"] isEqual:@"identity"]);
        MYProbe *probe = connection.exportedObject;
        @synchronized (probe) { probe.receiverVerified = YES; }
        r.stage = @"authenticate after pinned identity reply";
        [(id<MYReadiness>)proxy(connection, r) authenticate:^(NSDictionary *value) { [r finish:value]; }];
    }];
    return r;
}
static id authenticate(NSXPCConnection *connection) { return [beginAuthentication(connection) wait]; }
static id confirm(NSXPCConnection *connection, NSString *challenge) {
    MYResult *r = [MYResult new]; r.stage = @"confirm"; [(id<MYReadiness>)proxy(connection, r)
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
static id hello(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; r.stage = @"hello"; [(id<MYPCMTransport>)proxy(connection, r)
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
    MYResult *r = [MYResult new]; r.stage = @"send"; [(id<MYPCMTransport>)proxy(connection, r)
        packet:data reply:^(NSDictionary *value) { [r finish:value]; }]; return [r wait];
}
static id fetchEndpoint(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; r.stage = @"fetchEndpoint"; [(id<MYFixtureControl>)proxy(connection, r)
        endpoint:^(NSXPCListenerEndpoint *endpoint) { [r finish:endpoint]; }]; return [r wait];
}
static NSDictionary *snapshot(NSXPCConnection *connection) {
    MYResult *r = [MYResult new]; r.stage = @"snapshot"; [(id<MYFixtureControl>)proxy(connection, r)
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
static NSXPCConnection *actorConnection(NSString *service, NSString *requirement, MYProbe *probe = nil) {
    NSXPCConnection *c = [[NSXPCConnection alloc] initWithServiceName:service];
    c.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
    assert(mixyard::pin(c, requirement)); exportProbe(c, probe); [c resume]; return c;
}
static id attempt(NSXPCConnection *actor, NSXPCListenerEndpoint *endpoint, NSString *requirement) {
    MYResult *r = [MYResult new]; r.stage = @"actor attempt";
    [(id<MYStranger>)proxy(actor, r) attempt:endpoint requirement:requirement
        reply:^(NSDictionary *value) { [r finish:value]; }]; return [r wait];
}
static id readResult(NSXPCConnection *control, uint32_t reader, uint64_t frame, uint32_t count) {
    MYResult *r = [MYResult new]; r.stage = @"readResult"; [(id<MYFixtureControl>)proxy(control, r)
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
static void actorEvent(NSDictionary *context, NSString *outcome, uint64_t uses, NSError *error = nil) {
    NSMutableDictionary *record = [context mutableCopy];
    record[@"outcome"] = outcome; record[@"endpoint_uses"] = @(uses);
    if (error) record[@"error"] = mixyard::errorDetails(error);
    NSData *json = [NSJSONSerialization dataWithJSONObject:record options:0 error:nil];
    NSString *path = [[mixyard::fixtureRoot() stringByAppendingPathComponent:@"authority-events"]
        stringByAppendingPathComponent:[NSUUID.UUID.UUIDString stringByAppendingString:@".json"]];
    assert(json && [json writeToFile:path atomically:YES]);
}
@interface MYActor : NSObject <MYStranger, NSXPCListenerDelegate> {
    mixyard::Readiness _readiness;
    uint64_t _endpointUses;
}
@property(strong) NSString *producerRequirement;
@property(strong) NSDictionary *context;
@property(strong) NSMutableSet<NSXPCConnection *> *connections;
@property(copy) void (^beginProbe)(void);
- (void)close;
@end
@implementation MYActor
- (void)identify:(void (^)(NSDictionary *))reply {
    @synchronized (self) { reply(@{@"status": _readiness.closed ? @"closed" : @"identity"}); }
}
- (void)authenticate:(void (^)(NSDictionary *))reply {
    void (^probe)(void) = nil;
    @synchronized (self) {
        _readiness.authenticate(reply);
        if (!_readiness.closed) { probe = self.beginProbe; self.beginProbe = nil; }
    }
    if (probe) probe();
}
- (void)confirm:(NSString *)challenge reply:(void (^)(NSDictionary *))reply {
    @synchronized (self) {
        const bool accepted = _readiness.confirm(challenge);
        actorEvent(self.context, accepted ? @"ready" : @"confirmation_rejected", _endpointUses);
        reply(@{@"status": accepted ? @"ready" : @"rejected"});
    }
}
- (void)close {
    @synchronized (self) {
        _readiness.close(); actorEvent(self.context, @"closed", _endpointUses);
        self.beginProbe = nil;
    }
}
- (BOOL)listener:(NSXPCListener *)listener shouldAcceptNewConnection:(NSXPCConnection *)connection {
    (void)listener;
    // serviceListener cannot carry a listener requirement. Install its exact
    // peer pin once, before resume or any exported/reverse invocation.
    if (!mixyard::pin(connection, self.producerRequirement)) return NO;
    @synchronized (self) {
        if (self.connections.count >= 8) return NO;
        [self.connections addObject:connection];
    }
    MYActor *session = [MYActor new];
    session.context = @{@"channel": @"actor", @"actor_pid": @(getpid()),
        @"peer_pid": @(connection.processIdentifier), @"requirement": self.producerRequirement,
        @"connection": NSUUID.UUID.UUIDString};
    connection.exportedInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
    connection.exportedObject = session;
    connection.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYAuthorityProbe)];
    __weak NSXPCConnection *weakConnection = connection;
    __weak MYActor *weakSelf = self;
    connection.interruptionHandler = ^{
        [session close]; [weakConnection invalidate];
        @synchronized (session) { actorEvent(session.context, @"interrupted", session->_endpointUses); }
    };
    connection.invalidationHandler = ^{
        [session close];
        @synchronized (session) { actorEvent(session.context, @"invalidated", session->_endpointUses); }
        MYActor *owner = weakSelf; NSXPCConnection *closed = weakConnection;
        @synchronized (owner) { if (closed) [owner.connections removeObject:closed]; }
    };
    __weak MYActor *weakSession = session;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC),
        dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            MYActor *pending = weakSession;
            if (!pending) return;
            @synchronized (pending) {
                if (pending->_readiness.ready) return;
                [pending close]; [weakConnection invalidate];
            }
        });
    session.beginProbe = ^{
        MYActor *pending = weakSession;
        NSXPCConnection *live = weakConnection;
        if (!pending || !live) return;
        [(id<MYAuthorityProbe>)[live remoteObjectProxyWithErrorHandler:^(NSError *error) {
            [pending close]; [weakConnection invalidate];
            @synchronized (pending) { actorEvent(pending.context, @"probe_error", pending->_endpointUses, error); }
        }] probe:^{
            @synchronized (pending) {
                actorEvent(pending.context, pending->_readiness.probeCompleted() ? @"probe_reply" : @"late_probe_ignored",
                           pending->_endpointUses);
            }
        }];
    };
    [connection resume];
    return YES;
}
- (void)attempt:(NSXPCListenerEndpoint *)endpoint requirement:(NSString *)requirement
          reply:(void (^)(NSDictionary *))reply {
    @synchronized (self) {
        if (_readiness.closed || !_readiness.ready) {
            actorEvent(self.context, @"attempt_not_ready", _endpointUses);
            reply(@{@"status": @"not_ready", @"connection": self.context[@"connection"],
                    @"endpoint_uses": @(_endpointUses)}); return;
        }
        // Expected broker authority is fixture policy, never supplied by the peer.
        if (![requirement isEqual:mixyard::fixturePolicy()[@"broker"]]) {
            reply(@{@"status": @"invalid"}); return;
        }
        ++_endpointUses; actorEvent(self.context, @"endpoint_use", _endpointUses);
        NSXPCConnection *c = connect(endpoint, requirement); assert(c);
        mixyard::Header h; h.generation = h.epoch = h.revision = 1; h.frames = 1; h.bytes = 8;
        id early = send(c, packet(h, 0.875f));
#ifdef MIXYARD_STRANGER
        // This path intentionally differs in executable code, not only bundle name.
        NSDictionary *error = rejection(early, true);
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
}
@end

int main(int argc, char **argv) {
    @autoreleasepool {
        if (@available(macOS 13.0, *)) {} else { return 2; }
        if ([[NSBundle.mainBundle objectForInfoDictionaryKey:@"CFBundlePackageType"] isEqual:@"XPC!"]) {
            MYActor *actor = [MYActor new]; actor.connections = [NSMutableSet new];
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
            NSDictionary *error = rejection(controlReady, [mode isEqual:@"inbound_requirement_mismatch"]);
            NSData *json = [NSJSONSerialization dataWithJSONObject:@{@"status": @"rejection_observed",
                @"case": mode, @"producer_pid": @(getpid()), @"error": error} options:0 error:nil];
            assert(json); fwrite(json.bytes, 1, json.length, stdout); putchar('\n');
            [control invalidate]; return 0;
        }
        MYResult *endpointResult = [MYResult new]; endpointResult.stage = @"initial endpoint"; [(id<MYFixtureControl>)proxy(control, endpointResult)
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
        NSLog(@"CASE held producer readiness");
        MYProbe *heldPCM = [MYProbe new]; heldPCM.held = [MYResult new]; heldPCM.held.stage = @"held PCM reverse probe";
        NSXPCConnection *pendingPCM = connect(endpoint, brokerRequirement, heldPCM);
        MYResult *pendingAuthentication = beginAuthentication(pendingPCM);
        void (^releasePCM)(void) = [heldPCM.held wait];
        expect(hello(pendingPCM), @"not_ready");
        expect(send(pendingPCM, packet(earlyHeader)), @"not_admitted");
        // Early confirmation closes the session, including a pending authenticate.
        expect(confirm(pendingPCM, @"early"), @"rejected");
        expect([pendingAuthentication wait], @"closed");
        releasePCM();
        expect(authenticate(pendingPCM), @"closed");
        expect(hello(pendingPCM), @"not_ready");
        expect(send(pendingPCM, packet(earlyHeader)), @"not_admitted");
        assert([snapshot(control) isEqual:initial]);

        NSLog(@"CASE held supervisor readiness");
        MYProbe *heldControl = [MYProbe new]; heldControl.held = [MYResult new]; heldControl.held.stage = @"held control reverse probe";
        NSXPCConnection *pendingControl = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.broker.fixture"];
        pendingControl.remoteObjectInterface = controlInterface();
        assert(mixyard::pin(pendingControl, brokerRequirement)); exportProbe(pendingControl, heldControl);
        [pendingControl resume];
        MYResult *pendingControlAuthentication = beginAuthentication(pendingControl);
        void (^releaseControl)(void) = [heldControl.held wait];
        assert(!fetchEndpoint(pendingControl));
        expect(snapshot(pendingControl), @"not_ready");
        expect(readResult(pendingControl, 0, 0, 8), @"not_ready");
        expect(confirm(pendingControl, @"early"), @"rejected");
        expect([pendingControlAuthentication wait], @"closed");
        releaseControl();
        expect(authenticate(pendingControl), @"closed");
        assert(!fetchEndpoint(pendingControl));
        expect(readResult(pendingControl, 1, 0, 8), @"not_ready");
        assert([snapshot(control) isEqual:initial]);

        NSLog(@"CASE wrong producer confirmation");
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
        NSLog(@"CASE stale supervisor receiver");
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
        NSLog(@"CASE stale producer receiver");
        NSXPCConnection *stale = connect(endpoint, @"cdhash H\"0000000000000000000000000000000000000000\"");
        assert(stale); NSDictionary *staleError = rejection(authenticate(stale), true); [stale invalidate];
        assert([snapshot(control)[@"hellos"] unsignedLongLongValue] == 0);
        assert([snapshot(control) isEqual:initial]);
        // A valid requirement for a different signed executable is also a wrong receiver.
        NSLog(@"CASE wrong producer receiver");
        NSXPCConnection *wrongReceiver = connect(endpoint, strangerRequirement);
        NSDictionary *wrongReceiverError = rejection(authenticate(wrongReceiver), true);
        [wrongReceiver invalidate];
        assert([snapshot(control) isEqual:initial]);
        NSMutableArray *actorChecks = [NSMutableArray new];
        NSMutableArray<NSXPCConnection *> *pendingActors = [NSMutableArray new];
        for (NSString *service in @[@"org.mixyard.stranger.fixture", @"org.mixyard.crash.fixture"]) {
            NSString *expectedPeer = [service isEqual:@"org.mixyard.stranger.fixture"] ? strangerRequirement : crashRequirement;
            NSLog(@"CASE actor readiness service=%@", service);
            MYProbe *held = [MYProbe new]; held.held = [MYResult new]; held.held.stage = @"held actor reverse probe";
            NSXPCConnection *pending = actorConnection(service, expectedPeer, held);
            MYResult *authentication = beginAuthentication(pending);
            void (^release)(void) = [held.held wait];
            id earlyAttempt = attempt(pending, endpoint, brokerRequirement); expect(earlyAttempt, @"not_ready");
            expect(confirm(pending, @"early"), @"rejected");
            expect([authentication wait], @"closed");
            release();
            expect(authenticate(pending), @"closed");
            expect(attempt(pending, endpoint, brokerRequirement), @"not_ready");
            [actorChecks addObject:@{@"case": @"held_then_closed", @"service": service,
                @"connection": earlyAttempt[@"connection"]}];
            [pendingActors addObject:pending]; // Keep live until the late reply is observed by CI.

            for (NSNumber *replay in @[@NO, @YES]) {
                NSXPCConnection *bad = actorConnection(service, expectedPeer);
                id token = authenticate(bad); expect(token, @"challenge");
                expect(attempt(bad, endpoint, brokerRequirement), @"not_ready");
                if (replay.boolValue) expect(confirm(bad, token[@"challenge"]), @"ready");
                expect(confirm(bad, replay.boolValue ? token[@"challenge"] : @"wrong"), @"rejected");
                expect(confirm(bad, token[@"challenge"]), @"rejected");
                id denied = attempt(bad, endpoint, brokerRequirement); expect(denied, @"not_ready");
                [actorChecks addObject:@{@"case": replay.boolValue ? @"replayed_confirmation" : @"wrong_confirmation",
                    @"service": service, @"connection": denied[@"connection"]}];
                [bad invalidate];
            }
            // Actual invalidation while unconfirmed must revoke, never open authority.
            NSXPCConnection *abandoned = actorConnection(service, expectedPeer);
            expect(authenticate(abandoned), @"challenge");
            id denied = attempt(abandoned, endpoint, brokerRequirement); expect(denied, @"not_ready");
            MYResult *invalidated = [MYResult new]; invalidated.stage = @"actor local invalidation";
            abandoned.invalidationHandler = ^{ [invalidated finish:@YES]; };
            [abandoned invalidate]; assert([[invalidated wait] isEqual:@YES]);
            [actorChecks addObject:@{@"case": @"invalidated_unconfirmed", @"service": service,
                @"connection": denied[@"connection"]}];
            // Even a direct authority request must have zero endpoint use when
            // the client has not authenticated the actor's actual executable.
            for (NSString *pin in @[@"cdhash H\"0000000000000000000000000000000000000000\"", brokerRequirement]) {
                NSXPCConnection *wrong = actorConnection(service, pin);
                NSDictionary *error = rejection(attempt(wrong, endpoint, brokerRequirement), true);
                [actorChecks addObject:@{@"case": @"wrong_receiver", @"service": service, @"error": error}];
                [wrong invalidate];
            }
            assert([snapshot(control) isEqual:initial]);
        }
        NSLog(@"CASE accepted stranger rejection");
        // Same UID, different signed executable gets the genuine anonymous endpoint.
        NSXPCConnection *stranger = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.stranger.fixture"];
        stranger.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
        assert(mixyard::pin(stranger, strangerRequirement)); exportProbe(stranger); [stranger resume]; expect(ready(stranger), @"ready");
        MYResult *attack = [MYResult new]; attack.stage = @"stranger attempt"; [(id<MYStranger>)proxy(stranger, attack)
            attempt:endpoint requirement:brokerRequirement reply:^(NSDictionary *value) { [attack finish:value]; }];
        id rejected = [attack wait]; assert([rejected isKindOfClass:NSDictionary.class]);
        assert([rejected[@"rejection_observed"] boolValue] && [rejected[@"uid"] unsignedIntValue] == getuid());
        assert([rejected[@"pid"] intValue] != [initial[@"pid"] intValue]);
        assert([snapshot(control)[@"packets"] unsignedLongLongValue] == 0);
        assert([snapshot(control)[@"hellos"] unsignedLongLongValue] == 0);
        samples(read(control, 0, 0, 8), 0, 8); samples(read(control, 1, 0, 8), 0, 8);
        NSLog(@"CASE producer packet and credit corpus");
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
        NSLog(@"CASE fresh producer generation");
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
        NSLog(@"CASE trusted abrupt crash");
        // Exact trusted executable abruptly exits after enqueueing one frame.
        NSXPCConnection *crash = [[NSXPCConnection alloc] initWithServiceName:@"org.mixyard.crash.fixture"];
        crash.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(MYStranger)];
        assert(mixyard::pin(crash, crashRequirement)); exportProbe(crash); [crash resume]; expect(ready(crash), @"ready");
        MYResult *crashed = [MYResult new]; crashed.stage = @"trusted crash attempt"; [(id<MYStranger>)proxy(crash, crashed)
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
            @"actor_readiness_checks": actorChecks,
            @"authentication_before_authority": @YES, @"early_authority_unchanged": initial,
            @"receiver_identity_reply_before_probe": @YES,
            @"requirement_failure_code": @(NSXPCConnectionCodeSigningRequirementFailure),
            @"anonymous_transport_authentication": @"executed", @"system_bootstrap": @"BLOCKED",
            @"hosted_HAL_peer_identity": @"BLOCKED", @"external_consumer_pcm": @"BLOCKED", @"production_apply": @NO,
            @"checks": @[@"exact_peer_both_directions", @"same_UID_different_signed_executable", @"early_messages",
                @"held_native_probe_no_authority", @"late_probe_closed_session", @"failed_confirmation_terminal",
                @"actor_server_readiness_endpoint_use_gated", @"actor_wrong_receiver_zero_authority",
                @"supervisor_read_endpoint_cursor_credit_gated", @"bad_stale_requirements", @"format_length_finite_position_epoch_generation_revision",
                @"replay_out_of_order_overflow_truncation", @"credited_backpressure_independent_readers",
                @"queued_not_delivery_ACK", @"disconnect_fresh_generation", @"producer_crash_zero_tail"]}
            options:NSJSONWritingPrettyPrinted error:nil];
        assert(json); fwrite(json.bytes, 1, json.length, stdout); putchar('\n');
        [pendingPCM invalidate]; [pendingControl invalidate];
        for (NSXPCConnection *pending in pendingActors) [pending invalidate];
        [stranger invalidate]; [crash invalidate]; [control invalidate]; return 0;
    }
}

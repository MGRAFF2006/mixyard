// SPDX-License-Identifier: MIT
#pragma once
#import <Foundation/Foundation.h>
#import <Security/Security.h>
#include <cstdint>
#include <cstring>
#include <limits>

// Native little-endian Float32 stereo interleaved at 48 kHz. One connection owns
// one preselected fixture endpoint; packets cannot choose device IDs or controls.
namespace mixyard {
constexpr uint64_t maxPosition = (uint64_t{1} << 53) - 1;
constexpr uint32_t packetFrames = 4096, creditFrames = 4096;
static_assert(sizeof(float) == 4 && std::numeric_limits<float>::is_iec559);
#if __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "The fixture packet contract requires native little-endian Float32."
#endif
struct Header {
    uint32_t version = 1, sampleRate = 48000, channels = 2, format = 1;
    uint64_t generation = 0, epoch = 0, revision = 0, position = 0;
    uint32_t frames = 0, bytes = 0;
};
static_assert(sizeof(Header) == 56);
// Reject broad identities and malformed/unavailable requirements before resume.
// This artifact accepts only per-build exact cdhash authority, not a caller PID/path.
inline bool oneHash(NSString *text) {
    if (![text isKindOfClass:NSString.class] || text.length != 50 ||
        ![text hasPrefix:@"cdhash H\""] || ![text hasSuffix:@"\""]) return false;
    for (NSUInteger i = 9; i < 49; ++i) {
        const unichar c = [text characterAtIndex:i];
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) return false;
    }
    return true;
}
inline bool exactRequirement(NSString *text) {
    if (![text isKindOfClass:NSString.class]) return false;
    NSArray<NSString *> *parts = [text componentsSeparatedByString:@" or "];
    if (parts.count < 1 || parts.count > 2) return false;
    for (NSString *part in parts) if (!oneHash(part)) return false;
    SecRequirementRef compiled = nullptr;
    const auto status = SecRequirementCreateWithString((__bridge CFStringRef)text,
                                                      kSecCSDefaultFlags, &compiled);
    if (compiled) CFRelease(compiled);
    return status == errSecSuccess;
}
// Disposable fixture policy is generated AFTER signing to avoid circular cdhash
// dependencies. It is outside the bundles, manifest-hashed, and not peer supplied.
// Same-user arbitrary policy/memory tampering is outside this fixture threat model.
// Native NSXPC checks the live peer; these files are expected policy, not PID identity.
inline NSString *fixtureRoot() {
    NSString *path = NSBundle.mainBundle.bundlePath;
    const bool service = [[NSBundle.mainBundle objectForInfoDictionaryKey:@"CFBundlePackageType"] isEqual:@"XPC!"];
    for (unsigned i = 0; i < (service ? 4u : 1u); ++i) path = path.stringByDeletingLastPathComponent;
    return path;
}
inline NSDictionary *fixturePolicy() {
    NSString *path = [fixtureRoot() stringByAppendingPathComponent:@"fixture-authority.plist"];
    NSDictionary *attributes = [NSFileManager.defaultManager attributesOfItemAtPath:path error:nil];
    if (!attributes || [attributes[NSFileSize] unsignedLongLongValue] > 4096) return nil;
    NSData *data = [NSData dataWithContentsOfFile:path];
    if (!data || data.length > 4096) return nil;
    id policy = [NSPropertyListSerialization propertyListWithData:data options:NSPropertyListImmutable format:nil error:nil];
    return [policy isKindOfClass:NSDictionary.class] ? policy : nil;
}
inline bool pin(NSXPCConnection *connection, NSString *requirement) {
    if (!connection || !exactRequirement(requirement)) return false;
    if (@available(macOS 13.0, *)) {
        @try { [connection setCodeSigningRequirement:requirement]; return true; }
        @catch (NSException *error) { NSLog(@"peer requirement rejected: %@", error.name); }
    }
    return false;
}
// Only anonymousListener/initWithMachServiceName support this setter. Bundled
// serviceListener delegates must pin each accepted connection before resume.
inline bool pinListener(NSXPCListener *listener, NSString *requirement) {
    if (!listener || !exactRequirement(requirement)) return false;
    if (@available(macOS 13.0, *)) {
        @try { [listener setConnectionCodeSigningRequirement:requirement]; return true; }
        @catch (NSException *error) { NSLog(@"listener requirement rejected: %@", error.name); }
    }
    return false;
}

// Fixture diagnostics only; identity is still enforced by Foundation, never PID.
inline NSDictionary *errorDetails(NSError *error) {
    return @{@"domain": error.domain, @"code": @(error.code),
             @"description": error.localizedDescription, @"user_info": error.userInfo.description};
}
}

// Shared connection-local readiness contract. Callers serialize every access on
// their non-RT session lock; close is terminal even if a probe replies later.
namespace mixyard {
struct Readiness {
    bool closed = false, verified = false, challengeSent = false, ready = false;
    NSString *challenge = nil;
    void (^pending)(NSDictionary *) = nil;
    NSDictionary *challengeReply() {
        challengeSent = true;
        return @{@"status": @"challenge", @"challenge": challenge};
    }
    void authenticate(void (^reply)(NSDictionary *)) {
        if (closed) { reply(@{@"status": @"closed"}); return; }
        if (challengeSent || pending) { reply(@{@"status": @"pending"}); return; }
        if (verified) reply(challengeReply());
        else pending = [reply copy];
    }
    bool probeCompleted() {
        if (closed) return false;
        verified = true; challenge = NSUUID.UUID.UUIDString;
        if (pending) { pending(challengeReply()); pending = nil; }
        return true;
    }
    void close() {
        closed = true; ready = false; challenge = nil;
        if (pending) { pending(@{@"status": @"closed"}); pending = nil; }
    }
    bool confirm(NSString *token) {
        if (closed || ready || !verified || !challengeSent ||
            ![token isKindOfClass:NSString.class] || ![challenge isEqual:token]) {
            close(); return false;
        }
        challenge = nil; ready = true; return true;
    }
};
}

// A reply gives the enforcing broker an error callback for its native peer check.
// No PCM, endpoint or control authority is granted by this diagnostic method.
@protocol MYAuthorityProbe
- (void)probe:(void (^)(void))reply;
@end

// No endpoint, admission or PCM authority. The client receives a pinned broker
// reply before echoing its connection-local challenge; failed confirmation closes
// the session permanently. Pinning alone does not gate outgoing requests.
@protocol MYReadiness
- (void)identify:(void (^)(NSDictionary *))reply;
- (void)authenticate:(void (^)(NSDictionary *))reply;
- (void)confirm:(NSString *)challenge reply:(void (^)(NSDictionary *))reply;
@end

@protocol MYPCMTransport <MYReadiness>
- (void)hello:(void (^)(NSDictionary *))reply;
- (void)packet:(NSData *)packet reply:(void (^)(NSDictionary *))reply;
@end

// Test supervisor only. Separate authenticated bootstrap channel. The PCM
// interface above has no read, endpoint, rename, registry or selection authority.
@protocol MYFixtureControl <MYReadiness>
- (void)endpoint:(void (^)(NSXPCListenerEndpoint *))reply;
- (void)read:(uint32_t)reader frame:(uint64_t)frame count:(uint32_t)count
       reply:(void (^)(NSDictionary *))reply;
- (void)snapshot:(void (^)(NSDictionary *))reply;
@end

@protocol MYStranger <MYReadiness>
- (void)attempt:(NSXPCListenerEndpoint *)endpoint requirement:(NSString *)requirement
          reply:(void (^)(NSDictionary *))reply;
@end

inline NSXPCInterface *pcmInterface() {
    return [NSXPCInterface interfaceWithProtocol:@protocol(MYPCMTransport)];
}
inline NSXPCInterface *controlInterface() {
    return [NSXPCInterface interfaceWithProtocol:@protocol(MYFixtureControl)];
}

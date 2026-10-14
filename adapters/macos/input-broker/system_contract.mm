// SPDX-License-Identifier: MIT
#import "protocol.h"

// SOURCE ONLY / UNEXECUTED. No call from the release plug-in or fixture.
// The privileged option selects system bootstrap; there is no user lookup fallback.
// A future broker must separately authenticate the actual coreaudiod/host identity;
// the fixture producer cdhash does not establish hosted AudioServerPlugIn identity.
NSXPCConnection *MixyardFutureSystemBroker(NSString *exactBrokerRequirement) {
    if (!mixyard::oneHash(exactBrokerRequirement)) return nil;
    NSXPCConnection *connection = [[NSXPCConnection alloc]
        initWithMachServiceName:@"org.mixyard.input.broker"
        options:NSXPCConnectionPrivileged];
    if (!mixyard::pin(connection, exactBrokerRequirement)) {
        [connection invalidate]; return nil;
    }
    // Inert: caller would configure protocol/lifetime outside RT before resume.
    return connection;
}

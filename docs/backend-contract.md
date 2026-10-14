# Native discovery envelope v1

Initial probe command: `snapshot`. Emit exactly one JSON object to stdout;
diagnostics go to stderr. Native tools may additionally offer capture commands
with their own documented arguments. This is discovery, not a realtime audio IPC.

```json
{
  "schemaVersion": 1,
  "platform": "linux",
  "capabilities": {
    "deviceDiscovery": true,
    "processCapture": false,
    "virtualInputs": false,
    "independentMixes": false,
    "routeRecovery": false
  },
  "devices": [
    {"id": "platform-uid", "name": "Example", "direction": "input", "virtual": false}
  ],
  "diagnostics": [
    {"code": "transport-unverified", "message": "Virtual-input transport is not proved", "severity": "warning"}
  ]
}
```

Platform: linux, windows or macos. Device direction: input, output or duplex.
IDs are opaque platform UIDs where available; never labels alone. Probes must not
claim virtual inputs merely because a third-party endpoint was enumerated. A true
capability indicates functionality implemented by the adapter and available in
this runtime, not an OS feature or a planned implementation. IndependentMixes
requires real separate sends; routeRecovery requires an implemented recovery
path. Audio qualification remains separate evidence even if capabilities are true.

Use empty arrays when nothing is observed. Errors retain a valid envelope where
possible with severity error, and a nonzero exit status. Explicit unavailable,
permission-denied and unsupported-OS diagnostics must not silently report success.
Do not log microphone samples, secrets, or unrelated app data.

Policy/config shape is owned by the policy thread, not this probe envelope. First
native capture spikes do not claim to execute the eventual shared policy graph.

# Authenticated device control integration

This Linux harness runs the actual `mixyard-control` process and the same
`Api.forControl` class used by the Vue console. It provisions two disposable
loopback TLS identities and profile stores. It never substitutes a contract
server, disables certificate validation, changes a personal runtime, or captures
audio. Secrets remain in memory or private disposable state, excluded from Git
and published artifacts.

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo build --locked --manifest-path control/Cargo.toml --bin mixyard-control --jobs 2
MIXYARD_CONTROL_BINARY="$PWD/control/target/debug/mixyard-control" \
  node --test verification/device-control/service.test.ts
```

Node 24 is required. No frontend dependencies or browser download are needed.
With a separate Cargo target, set `MIXYARD_CONTROL_BINARY` to that binary.
On constrained hosts, coordinate a private target/cache budget before building.

The checks cover certificate mismatch, one-use pairing, missing authorization,
view versus stored-control rights, A/B store isolation, concurrent stale writes,
discarded in-flight responses after reconnect, stable stored intent after service
restart, changed service authority, and credential revocation. Unsupported remote
automation arming remains denied. Every mutation is explicit; the clients never
retry an uncertain write automatically.

This is real client/service qualification over owned loopback TLS. It does not
qualify physical remote reachability, browser certificate provisioning, native
mobile secure storage, tray behavior, native audio, or media permissions. The
separate Vue browser tests cover rendered target switching and host-local drafts.

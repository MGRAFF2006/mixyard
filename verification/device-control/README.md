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

Node 24 and the existing `apps/web` client dependencies are required: lifecycle
state uses Vue's reactivity even when only the API is exercised. The API-only
run does not require a browser download. Use the existing compatible dependency
cache, or `pnpm --dir apps/web install --frozen-lockfile` on a build host.
With a separate Cargo target, set `MIXYARD_CONTROL_BINARY` to that binary.
On constrained hosts, coordinate a private target/cache budget before building.

The checks cover certificate mismatch, one-use pairing, missing authorization,
view versus stored-control rights, A/B store isolation, concurrent stale writes,
discarded in-flight responses after reconnect, stable stored intent after service
restart, changed service authority, credential revocation and rotation. Unsupported remote
automation arming remains denied. Every mutation is explicit; the clients never
retry an uncertain write automatically.

This is real client/service qualification over owned loopback TLS. It does not
qualify physical remote reachability, browser certificate provisioning, native
mobile secure storage, tray behavior, native audio, or media permissions. The
same harness can also render the actual Vue App with the existing Vite/Playwright
dependencies. Set `MIXYARD_CONTROL_BROWSER=1` alongside the binary environment
variable. Set `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` when reusing cached Chromium.

The browser path mounts the complete production App/Api. A test-only injected
binding pins each exact owned TLS endpoint and keeps A/B requests separate; it
does not replace service operations with predetermined responses. It exercises
host-local drafts, B-only persistence, switching while a real A save response is
held, revoked-session recovery, and the 390 px layout. This binding is not the
production Tauri/Kotlin/Swift bridge and does not qualify its native certificate
or credential storage. The ordinary Vue browser suite separately checks the
complete editor and close/unload guards.

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
  node --test verification/device-control/*.test.ts
```

Node 24, OpenSSL and the existing `apps/web` client dependencies are required: lifecycle
state uses Vue's reactivity even when only the API is exercised. The API-only
run does not require a browser download. Use the existing compatible dependency
cache, or `pnpm --dir apps/web install --frozen-lockfile` on a build host.
With a separate Cargo target, set `MIXYARD_CONTROL_BINARY` to that binary.
On constrained hosts, coordinate a private target/cache budget before building.

The test binding verifies TLS 1.3, normal certificate/name validation and the
exact provisioned leaf DER before HTTP is sent. A separate owned TLS regression
proves that a different leaf signed by the same trusted identity is rejected
before credentials, even after an ordinary-trust caller has primed Node's global
connection pool, and that TLS 1.2 cannot silently qualify. Each binding owns its
pin-policy agent, permits one HTTP exchange at a time, and disables TLS session
caching; it never borrows another policy's socket or retries a request. Transport errors
use fetch-style TypeError invalidation; structured HTTP errors retain their status.
The checks cover certificate mismatch, one-use pairing, missing authorization,
view versus stored-control rights, a singleton Settings grant completing the
actual settings editor's read/save flow without View, A/B store isolation, concurrent stale writes,
discarded in-flight responses after reconnect, stable stored intent after service
restart, changed service authority, credential revocation and rotation. Unsupported remote
automation arming remains denied. Every mutation is explicit; the clients never
retry an uncertain write automatically.
The offline regression also retains a real old reply across service shutdown:
transport failure must drop authority and keep drafts, discard that old reply,
and require explicit refresh after the same owned endpoint restarts. A new port
is never silently substituted for a frozen target.

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

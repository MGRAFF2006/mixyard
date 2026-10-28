# Local HTTP service

`mixyard-web` serves the built browser interface and the API specified in
[web-interface.md](web-interface.md). From the repository root, use
`scripts/run-web.sh` to build the interface and start the service, or run:

```sh
cargo run --locked --bin mixyard-web -- --assets apps/web/dist
```

Build `apps/web` first when running the binary directly. Missing `index.html`
causes startup to fail with the build instruction. The service never starts or
stops `mixyard-runtime`, applies audio, captures audio, or changes native routes.
`audioAvailable` is always false. A disconnected runtime is a normal status
result, with `connected: false`, `snapshot: null`, and an explanatory error.

Options are `--store DIRECTORY`, `--instance NAME`, `--port PORT`, and
`--assets DIRECTORY`. Defaults are the existing core profile directory, runtime
instance `default`, port `8384`, and `apps/web/dist` relative to the working
directory. `--port 0` requests an available port. Startup prints the actual URL,
for example `Mixyard web: http://127.0.0.1:8384`. Use that exact URL; `localhost`
and alternate Host values are rejected. No listen-address option exists.

The service uses Hyper for HTTP/1 parsing and framing, Tokio for bounded socket
work, and the operating system random source for a 256-bit process-lifetime
session token. Fetch `GET /api/session`, then use `POST`, JSON content type, and
`X-Mixyard-Token` for every other API endpoint. The token changes after restart.
Requests without Origin work with the token; supplied Origins must exactly
match the service origin. There are no CORS headers or cross-origin preflights.
Duplicate security headers, absolute request targets, unexpected Hosts and
Origins, unknown JSON fields, and caller-supplied paths are rejected.
Responses have `Cache-Control: no-store`, `nosniff`, a restrictive content
security policy, and no referrer. Static content and the API share one origin.

Static assets are loaded once at startup, with at most 2048 files, 16 directory
levels, 16 MiB per file and 64 MiB total. Hidden files are skipped; symlinks and
special files within the asset tree are rejected. Only exact cached file URLs
are served, with `/` mapped to `index.html`. Missing files return 404; there is
no SPA fallback. Percent-encoded or traversal paths cannot escape the cache.
Restart after rebuilding assets. The asset directory is trusted local build
output and must not be modified by an untrusted local process during startup.

Resource limits are 32 concurrent HTTP connections, four concurrent core
operations, 1 MiB per request body, 8 MiB serialized JSON response, and 8 MiB
profile-store preflight/read. Connections close after each response. Excess
connections are closed; excess operations return JSON 503 `service-busy`.
Each connection has a 15-second overall deadline, including incomplete headers
and response writes. Headers use at most a 16 KiB parser buffer and 64 fields.
A body deadline returns JSON 408 after three seconds; the operation deadline
returns JSON 504 after six seconds. The runtime client also enforces its own
five-second connection/snapshot deadline. Invalid JSON returns 400, policy
rejection 422, missing profile 404, and store conflicts/busy/recovery 409.

Blocking operations retain their slot until they actually finish, even after a
504 or client disconnect. A timed-out write may still complete. Reread the
store, preserve local edits and review before retrying; never automatically
retry edits. Very large accumulated stores may exceed the web limits; use the
existing profiles CLI to inspect/export/manage them. A write can commit before
its response exceeds the serialization limit, so a response error also requires
a fresh read. No store is truncated, migrated, repaired, or unlocked by the web
service. The core's private path checks, expected-revision conflicts, atomic
replacement and manual recovery behavior remain authoritative.

Inspection also checks expansion before constructing the core result: a
conservative cubic work estimate over configuration/inventory entities must not
exceed 50 million, and a byte-weighted estimate of repeated path findings and
cloned mixes must fit 32 MiB. Requests outside these web-specific budgets return
422 `inspection-too-large`; use the existing inspect CLI for larger graphs.
No routing path is truncated and the core's policy/resource limits are unchanged.

Run focused checks with two build jobs:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --lib --bins --test web_service --test profiles_cli --test diagnostics_cli
cargo fmt --all --check
```

`tests/web_service.rs` launches the actual binary on isolated loopback ports and
uses disposable stores. CI runs these HTTP, persistence and security scenarios
on Linux, Windows and macOS. These checks establish service behavior, not native
audio functionality or browser-interface usability.

# Authenticated device control

This independent Rust workspace adds an explicitly opted-in TLS 1.3 management
service. It reuses the existing typed web handlers and profile/automation owners;
it edits **stored intent**, never audio or OS routes. See the complete
[protocol and operator contract](../docs/remote-control.md).

```sh
cargo build --locked --manifest-path control/Cargo.toml
cargo test --locked --manifest-path control/Cargo.toml --test security
cargo fmt --manifest-path control/Cargo.toml -- --check
```

For isolated testing, initialize a dedicated private state directory, create an
invitation locally, then start the service. Capture invitation output privately;
it is a short-lived secret. `trust` exports only the public certificate.

```sh
mixyard-control --state ./control-state init --ip 127.0.0.1
mixyard-control --state ./control-state invite --permissions view,control,settings
mixyard-control --state ./control-state serve --enable-control \
  --bind 127.0.0.1:0 --store ./profile-store --assets ./web-assets \
  --instance isolated-control
```

`web-assets/index.html` must exist: the shared canonical adapter validates its
normal asset tree, although this API-only edge does not expose static files.
No default state directory or listener is created implicitly. Do not commit the
identity store, invitations, credentials, or private TLS key.

The service supports an exact, locally provisioned private LAN/Tailscale address,
as well as loopback. Wildcards and public addresses are refused. Actual LAN and
browser trust installation are qualification gaps; real loopback TLS tests do
not establish native audio or physical-device compatibility.

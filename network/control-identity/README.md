# Control identity bridge

Optional standalone Rust workspace using the real local-owner control identity
export with the raw v1 network endpoint. Neither control nor network enables it
implicitly. See [the contract and proof limits](../../docs/network-control-identity.md).

Run on a build host with adequate disk space:

```sh
cargo fmt --manifest-path network/control-identity/Cargo.toml -- --check
cargo test --locked --manifest-path network/control-identity/Cargo.toml
cargo clippy --locked --manifest-path network/control-identity/Cargo.toml --all-targets -- -D warnings
```

Tests create private, temporary synthetic control stores and exchange fixture PCM
over IPv4 loopback. They remove their stores on exit and use no application
service, personal identity, audio device or microphone.

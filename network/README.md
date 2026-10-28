# Mixyard network foundation

Standalone Rust workspace for paired, authenticated QUIC audio transport. It does
not open an audio device, mix sources, publish native endpoints or modify defaults.
See [the protocol, security boundary and qualification gates](../docs/network-audio.md).

Run the isolated proof (ephemeral certificates, loopback sockets only):

```sh
cargo run --locked --manifest-path network/Cargo.toml --example loopback -j2
cargo test --locked --manifest-path network/Cargo.toml -j2
cargo clippy --locked --manifest-path network/Cargo.toml --all-targets -j2 -- -D warnings
```

On a constrained host, place build and download caches in a task-owned temporary
directory. Check free space before building; reserve at least 10 GiB on `/tmp`:

```sh
df -h /tmp
network_build_dir=$(mktemp -d /tmp/mixyard-network.XXXXXX)
export CARGO_HOME="$network_build_dir/cargo"
export CARGO_TARGET_DIR="$network_build_dir/target"
```

The proof compares S24LE 96 kHz six-channel, F64LE 192 kHz four-channel and S32LE
44.1 kHz stereo payloads byte for byte in both directions, restarts a stream with
a new generation and checks disconnect silence. Additional tests cover all five
raw formats, eight channels, rates up to 768 kHz, source/peer rejection, bounded
queues, loss/reorder and malformed control. This is a transport exercise, not
real-time pacing, audio-quality, latency or device qualification.

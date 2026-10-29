# Optional network codecs

Callable bounded raw, Zstd and explicitly lossy Opus workers, independent of the
root and existing network Cargo workspaces. No audio devices or routing changes.
See [the contract, evidence and limits](../../docs/network-codecs.md).

Default features provide raw envelopes without native dependencies. Enable
`system-zstd` and/or `system-opus` to link installed public libraries; this crate
does not download, build or install them. libzstd >= 1.4 and libopus >= 1.3 with
linker-visible `zstd`/`opus` libraries are required for those features. Cargo uses
the std-only `build.rs`; per-library `MIXYARD_ZSTD_LIB_DIR`,
`MIXYARD_ZSTD_LINK_KIND` (`dylib` or `static`) and `MIXYARD_ZSTD_LIB_NAME`
select an explicit directory, linkage and bare library name. The same variables
with `OPUS` replace `ZSTD` for Opus. Defaults are platform linker paths, `dylib`
and `zstd`/`opus`. A supplied directory must be absolute and exist; malformed
settings fail. Missing native libraries fail the build, without codec fallback.
No extra installation or probing occurs. Direct-rustc `verify.sh` passes the
same selections with `-L`/`-l` and is a Unix/Bash verification path.

See [platform build/loader and hosted evidence](../../docs/network-codec-platforms.md)
for Windows MSVC/vcpkg, macOS/Homebrew and Linux package-manager paths. Optional
library presence alone is not device, platform or distribution qualification.
Mixyard code is MIT; upstream Zstd offers BSD/GPL alternatives and Opus a BSD
license plus its upstream patent terms. Distributors must retain applicable
upstream notices and audit their actual binary dependencies.

With a private build allocation and installed libraries:

```sh
bash network/codecs/verify.sh /tmp/my-private-codec-target
```

This uses dependency-free `rustc` and system FFI, overwrites its own output files,
and runs both feature modes plus real synthetic TCP loopback. Observed final
output size is about 12 MiB on Linux x86_64; that is not a portable hard disk or
compiler-memory bound. Follow the current host/build allowance before running.

Normal Cargo commands when a Cargo build is authorized:

```sh
cargo test --locked --manifest-path network/codecs/Cargo.toml
cargo test --locked --manifest-path network/codecs/Cargo.toml --all-features
cargo run --locked --manifest-path network/codecs/Cargo.toml --all-features --example loopback
```

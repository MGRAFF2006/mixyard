#!/usr/bin/env python3
"""Compile the actual root library and owned Linux PCM tests with rustc.

Existing compatible Cargo dependency artifacts are read only. This verifies the
unchanged full src/lib.rs module graph, not Cargo manifests, bins or OS SDKs.
Nothing is installed or written into the dependency directory.
"""
import argparse
import os
from pathlib import Path
import resource
import shutil
import subprocess
import tempfile


def floors():
    for path, minimum in (("/", 2 * 2**30), ("/tmp", 10 * 2**30), ("/dev/shm", 10 * 2**30)):
        if shutil.disk_usage(path).free < minimum:
            raise SystemExit(f"BLOCKED: free space floor for {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dependency-dir", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    deps = args.dependency_dir.resolve(strict=True)
    external = []
    for name in ("serde", "serde_json", "libc", "sha2", "getrandom"):
        candidates = list(deps.glob(f"lib{name}-*.rlib"))
        if len(candidates) != 1:
            raise SystemExit(f"BLOCKED: expected one compatible {name} rlib in dependency directory")
        external += ["--extern", f"{name}={candidates[0]}"]
    floors()
    resource.setrlimit(resource.RLIMIT_FSIZE, (48 * 2**20, 48 * 2**20))
    env = {**os.environ, "CARGO_INCREMENTAL": "0", "CARGO_BUILD_JOBS": "1", "CARGO_MANIFEST_DIR": str(root)}
    with tempfile.TemporaryDirectory(prefix="mixyard-native-linux-", dir="/dev/shm") as directory:
        target = Path(directory)
        env["TMPDIR"] = directory
        env["MIXYARD_LINUX_TEST_DIR"] = directory
        common = ["-C", "debuginfo=0", "-C", "codegen-units=1", "--edition=2021", "-L", f"dependency={deps}"]

        def run(command):
            floors()
            subprocess.run(command, check=True, env=env, cwd=root)
            size = sum(p.stat().st_size for p in target.rglob("*") if p.is_file())
            if size > 50 * 2**20:
                raise SystemExit("BLOCKED: generated artifacts exceed 50 MiB")
            print(f"Private generated artifacts: {size} bytes", flush=True)

        library = target / "libmixyard.rlib"
        run(["rustc", "--crate-name", "mixyard", "--crate-type", "rlib", str(root / "src/lib.rs"),
             "-o", str(library), *common, *external])
        binary = target / "native-linux-pcm"
        run(["rustc", "--test", str(root / "tests/native_linux_pcm.rs"), "-o", str(binary),
             "--extern", f"mixyard={library}", *common, *external])
        run([str(binary), "--nocapture"])
    print("Actual library/owned PCM checks passed; private generated artifacts removed. Cargo/PipeWire/hardware checks were not run.")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Local forecast guard. Does not download, prune, install, or invoke an SDK."""
import argparse
from pathlib import Path
import os

GIB = 1024**3
MIB = 1024**2


def tree_bytes(root):
    total = 0
    for base, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = [name for name in dirs if not Path(base, name).is_symlink()]
        for name in files:
            total += Path(base, name).lstat().st_size
    return total


def check(forecast, output=None):
    if not 0 <= forecast <= 2 * MIB:
        raise RuntimeError("forecast must fit the authorized <=2 MiB generated-output budget")
    for mount, reserve in (("/", 2 * GIB), ("/tmp", 2 * GIB), ("/dev/shm", 10 * GIB + 16 * MIB)):
        stat = os.statvfs(mount)
        free = stat.f_bavail * stat.f_frsize
        if free - forecast < reserve:
            raise RuntimeError(f"{mount}: {free} free, need {reserve} reserve + {forecast} forecast")
    native = Path(__file__).resolve().parents[1]
    used = sum(tree_bytes(native / name) for name in ("native-host", "standard-consumer", "private-audio"))
    if used + forecast > 384 * MIB:
        raise RuntimeError(f"combined native subtree cap: {used} + {forecast} > 384 MiB")
    if output:
        path = Path(output)
        shm = Path("/dev/shm").resolve()
        if not path.is_absolute() or not path.resolve().is_relative_to(shm):
            raise RuntimeError("all generated artifacts require an owned directory in /dev/shm")
        if path.exists():
            stat = path.lstat()
            if path.is_symlink() or not path.is_dir() or stat.st_uid != os.getuid() or stat.st_mode & 0o777 != 0o700:
                raise RuntimeError("existing output must be an owned 0700 non-symlink directory")
    print(f"guard OK: forecast={forecast} combinedNativeBytes={used}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--forecast", type=int, required=True)
    parser.add_argument("--output")
    args = parser.parse_args()
    check(args.forecast, args.output)

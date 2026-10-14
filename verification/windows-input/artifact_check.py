#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Inspect an unsigned x64 native package; never install or infer PCM evidence."""
import hashlib
import json
from pathlib import Path
import struct
import sys


def validate(package: Path) -> dict:
    files = [package / f"MixyardIngress.{ext}" for ext in ("sys", "inf", "cat")]
    files.append(package / "LICENSE")
    for path in files:
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"missing/empty artifact: {path.name}")
    binary = files[0].read_bytes()
    if binary[:2] != b"MZ":
        raise ValueError("driver missing DOS header")
    pe = struct.unpack_from("<I", binary, 0x3C)[0]
    if binary[pe:pe + 4] != b"PE\0\0":
        raise ValueError("driver missing PE header")
    if struct.unpack_from("<H", binary, pe + 4)[0] != 0x8664:
        raise ValueError("driver is not x64")
    optional = pe + 24
    if struct.unpack_from("<H", binary, optional)[0] != 0x20B:
        raise ValueError("driver is not PE32+")
    if struct.unpack_from("<H", binary, optional + 68)[0] != 1:
        raise ValueError("driver is not native subsystem")
    if struct.unpack_from("<II", binary, optional + 112 + 4 * 8) != (0, 0):
        raise ValueError("expected unsigned driver, found embedded certificate table")
    return {
        "schema_version": 1,
        "kind": "unsigned-private-ingress-build",
        "files": {p.name: {"bytes": p.stat().st_size,
                            "sha256": hashlib.sha256(p.read_bytes()).hexdigest()} for p in files},
        "endpoint_published": False,
        "native_execution": "blocked",
        "external_consumer_pcm": "blocked",
        "sample_effective_ack": "unimplemented",
        "production_apply": "disabled",
    }


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: artifact_check.py UNSIGNED_PACKAGE_DIRECTORY")
    directory = Path(sys.argv[1])
    result = validate(directory)
    (directory / "artifact.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print("PASS: x64 native unsigned package headers/hash inventory only; no load or audio evidence")

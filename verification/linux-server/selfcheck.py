#!/usr/bin/env python3
"""Small negative check of all-frame evidence rejection; no graph qualification."""
import importlib.util
import json
from pathlib import Path
import struct
import tempfile

spec = importlib.util.spec_from_file_location("probe", Path(__file__).parents[1] / "linux_server_ingress.py")
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
with tempfile.TemporaryDirectory() as directory:
    pcm = Path(directory) / "capture.f32"
    meta = {"frames": 2, "status": "RECORDED", "format": "F32LE", "wire_format": "F32P", "rate": 48000,
            "channels": ["FL", "FR"], "drops": {"trace": 0},
            "packets": [{"offset": 0, "frames": 2, "duration": 2, "position": 100, "flags": 0, "clock_id": 1}]}
    def write(frames):
        pcm.write_bytes(b"".join(struct.pack("<ff", *frame) for frame in frames))
        Path(str(pcm) + ".json").write_text(json.dumps(meta))
        return probe.analyze(pcm)
    assert write([(.125, -.125)] * 2)["privacy"] == "pass"
    assert write([(.75, -.75), (.125, -.125)])["privacy"] == "fail"  # first
    assert write([(.125, -.125), (.75, -.75)])["privacy"] == "fail"  # tail
    assert write([(.125, -.125), (float("nan"), 0)])["privacy"] == "fail"
    assert write([(.125, -.125)])["status"] == "fail"  # missing tail
    meta["drops"]["trace"] = 1
    assert write([(.125, -.125)] * 2)["privacy"] == "blocked"
    assert not probe.analyze(pcm, True)["sentinel_reproduced"]
print("7 focused all-frame evidence checks passed; synthetic only")

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
            "channels": ["FL", "FR"], "clock_scope": "graph-driver", "interleaved": True, "drops": {"trace": 0},
            "packets": [{"offset": 0, "frames": 2, "duration": 2, "position": 100, "flags": 0, "clock_id": 1, "clock_rate_num": 1, "clock_rate_denom": 48000, "source_frames": 2, "chunk_flags": [0, 0]}]}
    def write(frames):
        pcm.write_bytes(b"".join(struct.pack("<ff", *frame) for frame in frames))
        Path(str(pcm) + ".json").write_text(json.dumps(meta))
        return probe.analyze(pcm)
    assert write([(.125, -.125)] * 2)["privacy"] == "pass"
    assert write([(.75, -.75), (.125, -.125)])["privacy"] == "fail"  # first
    assert write([(.125, -.125), (.75, -.75)])["privacy"] == "fail"  # tail
    assert write([(.125, -.125), (float("nan"), 0)])["privacy"] == "fail"
    assert write([(.125, -.125)])["status"] == "fail"  # missing tail
    assert write([(0, 0)] * 2)["status"] == "fail"
    assert probe.analyze(pcm, phase="retired")["status"] == "pass"
    assert probe.analyze(pcm, sentinel_reproduced=False)["status"] == "blocked"
    for key, value in (("format", "F32BE"), ("wire_format", "F32LE"), ("rate", 44100), ("channels", ["FR", "FL"])):
        old = meta[key]; meta[key] = value
        assert write([(.125, -.125)] * 2)["status"] != "pass"
        meta[key] = old
    for key, value in (("clock_rate_num", 2), ("clock_rate_denom", 44100), ("clock_id", 4294967295)):
        old = meta["packets"][0][key]; meta["packets"][0][key] = value
        assert write([(.125, -.125)] * 2)["status"] != "pass"
        meta["packets"][0][key] = old
    write([(.125, -.125)] * 2)
    assert probe.analyze(pcm, expected_clock_id=7)["status"] != "pass"
    write([(.75, -.75)] * 2)
    assert probe.analyze(pcm, True)["sentinel_reproduced"]
    write([(.5, -.5)] * 2)
    assert not probe.analyze(pcm, True)["sentinel_reproduced"]
    meta["drops"]["trace"] = 1
    assert write([(.125, -.125)] * 2)["privacy"] == "blocked"
    assert not probe.analyze(pcm, True)["sentinel_reproduced"]
print("20 focused all-frame evidence checks passed; synthetic only")

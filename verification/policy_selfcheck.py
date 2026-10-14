#!/usr/bin/env python3
"""Corrupt a real CLI's replies to check the oracle. Model evidence only."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from policy_check import check


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="already built CLI; never built/imported here")
    parser.add_argument("--examples", type=Path, default=Path(__file__).resolve().parents[1] / "examples")
    args = parser.parse_args()
    binary = args.binary.resolve()
    try:
        baseline = check(binary, args.examples)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        baseline = {"status": "blocked", "reason": str(error), "native_routing_verified": False}
    if baseline["status"] != "pass":
        print(json.dumps(baseline, indent=2))
        return 2 if baseline["status"] == "blocked" else 1
    mutations = {
        "wrong mix": 'for output in plan["outputs"]: output["mix"] = "call"',
        "wrong target": 'for output in plan["outputs"]: output["target"] = {"kind": "device", "deviceUid": "unrequested"}',
        "wrong gain": 'for output in plan["outputs"]: output["gain"] = 0',
        "wrong mute": 'for output in plan["outputs"]: output["mute"] = not output["mute"]',
        "empty outputs": 'plan["outputs"] = []',
        "wrong resolved output": 'for output in plan["outputs"]: output["observedIds"] = ["simulated:output:4"]',
        "empty resolved outputs": 'for output in plan["outputs"]: output["observedIds"] = []',
        "duplicate resolved outputs": 'for output in plan["outputs"]: output["observedIds"] *= 2',
        "wrong effective gain": 'for output in plan["outputs"]: output["effectiveGain"] = 0',
        "effective gain ignores mute": 'for output in plan["outputs"]: output["effectiveGain"] = output["gain"]',
        "empty players": 'plan["players"] = []',
        "private player": 'plan["players"][0].update(sessionId="simulated:session:private-player", applicationId="example:private-player")',
        "wrong action": 'plan["players"][0]["plannedAction"] = "play"',
        "duplicate players": 'plan["players"] *= 2',
    }
    with tempfile.TemporaryDirectory(prefix="mixyard-policy-oracle-") as directory:
        wrapper = Path(directory) / "corrupt_cli.py"
        for label, mutation in mutations.items():
            command = "plan-media" if "players" in mutation else "plan"
            wrapper.write_text(
                "import json, subprocess, sys\n"
                f"process = subprocess.run([{str(binary)!r}, *sys.argv[1:]], capture_output=True, text=True, timeout=5)\n"
                "out = process.stdout\n"
                f"if process.returncode == 0 and sys.argv[1] == {command!r}:\n"
                "    plan = json.loads(out)\n"
                f"    {mutation}\n"
                "    out = json.dumps(plan)\n"
                "sys.stdout.write(out)\n"
                "sys.stderr.write(process.stderr)\n"
                "sys.exit(process.returncode)\n")
            report = check(wrapper, args.examples, runner=(sys.executable, "-B"))
            failures = {r["case"] for r in report["results"] if r["status"] == "fail"}
            expected = {r["case"] for r in baseline["results"]
                        if r["case"] not in {"public-v2-validate", "public-v2-plan-media-argument-order-no-dispatch",
                                              "duplicate-stable-id-rejected", "superseded-v1-not-used-as-v2-contract"}}
            if command == "plan-media":
                expected = {"public-v2-plan-media-argument-order-no-dispatch"}
            elif label == "effective gain ignores mute":
                expected = {"output-gain-mute-preserves-siblings"}
            if report["status"] != "fail" or failures != expected:
                raise AssertionError((label, report))
    print(json.dumps({"status": "pass", "checks": len(mutations) + 1,
                      "evidence_kind": "black-box-policy-oracle-corruption-selfcheck",
                      "native_routing_verified": False}))
    return 0


if __name__ == "__main__":
    sys.exit(main())

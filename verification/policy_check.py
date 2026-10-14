#!/usr/bin/env python3
"""Black-box schema-2 CLI checks. Simulated policy, never native audio/events."""

import argparse
import copy
import json
from pathlib import Path
import subprocess
import tempfile


def check(binary, examples):
    config = json.loads((examples / "config.json").read_text())
    inventory = json.loads((examples / "inventory.json").read_text())
    if config.get("schemaVersion") != 2 or inventory.get("schemaVersion") != 2:
        return {"status": "blocked", "reason": "Schema-2 public config/inventory fixtures not present; fixed-three v1 expectations are superseded.",
                "native_routing_verified": False}
    if not isinstance(config.get("mixes"), list) or not config["mixes"] or not isinstance(config.get("outputs"), list):
        return {"status": "blocked", "reason": "Schema-2 mixes/outputs command fixture contract unavailable.",
                "native_routing_verified": False}
    media_request = json.loads((examples / "media-request.json").read_text())
    if media_request.get("schemaVersion") != 2:
        return {"status": "blocked", "reason": "Schema-2 public media request fixture unavailable.",
                "native_routing_verified": False}
    results = []
    with tempfile.TemporaryDirectory(prefix="mixyard-policy-v2-") as directory:
        root = Path(directory)

        def run(label, desired, command="plan", reject=False):
            desired_path, observed_path = root / "config.json", root / "inventory.json"
            desired_path.write_text(json.dumps(desired))
            observed_path.write_text(json.dumps(inventory))
            arguments = [str(binary), command, str(desired_path)]
            if command == "plan":
                arguments.append(str(observed_path))
            elif command == "plan-media":
                request_path = root / "request.json"
                request_path.write_text(json.dumps(media_request))
                arguments.extend((str(observed_path), str(request_path)))
            process = subprocess.run(arguments, capture_output=True, text=True, timeout=10)
            diagnostic = process.stderr.strip()[:1000]
            try:
                if reject:
                    error = json.loads(process.stderr)
                    passed = process.returncode == 3 and error["code"] in ("invalid-policy", "invalid-json")
                else:
                    plan = json.loads(process.stdout)
                    passed = process.returncode == 0 and plan["schemaVersion"] == 2
                    if command == "validate":
                        passed = passed and plan["valid"] is True
                    elif command == "plan-media":
                        passed = passed and plan["mode"] == "dry-run-media-control" \
                            and plan["requestedAction"] == media_request["action"] \
                            and plan["configRevision"] == desired["revision"] \
                            and plan["inventoryRevision"] == inventory["revision"] \
                            and all(p["dispatchOutcome"] == "not-dispatched" for p in plan["players"])
                    else:
                        passed = passed and len(plan["mixes"]) == len(desired["mixes"]) \
                            and {m["id"] for m in plan["mixes"]} == {m["id"] for m in desired["mixes"]} \
                            and len(plan["outputs"]) == len(desired["outputs"]) \
                            and {o["id"] for o in plan["outputs"]} == {o["id"] for o in desired["outputs"]}
            except (ValueError, KeyError, TypeError):
                passed = False
                diagnostic = diagnostic or "Public CLI result did not match the schema-2 contract."
            results.append({"case": label, "status": "pass" if passed else "fail",
                            "diagnostic": "" if passed else diagnostic})

        run("public-v2-validate", config, command="validate")
        run("public-v2-plan-media-argument-order-no-dispatch", config, command="plan-media")
        run("all-mix-and-output-ids-accounted-for", config)
        expanded = copy.deepcopy(config)
        ids = {m["id"] for m in expanded["mixes"]}
        serial = 0
        while len(expanded["mixes"]) < 4:
            serial += 1
            identity = f"qualification-extra-{serial}"
            if identity in ids:
                continue
            mix = copy.deepcopy(expanded["mixes"][0])
            mix.update(id=identity, name=f"自由 / mix {serial}")
            expanded["mixes"].append(mix)
            ids.add(identity)
        run("four-plus-arbitrary-mixes-no-truncation", expanded)
        renamed = copy.deepcopy(expanded)
        for mix in renamed["mixes"]:
            mix["name"] = "Same display label / 音"
        run("rename-duplicate-labels-stable-ids", renamed)
        created = copy.deepcopy(expanded)
        extra = copy.deepcopy(created["mixes"][0])
        extra.update(id="qualification-lifecycle-extra", name="Extra lifecycle mix")
        created["mixes"].append(extra)
        run("create-extra-mix", created)
        created["mixes"].pop()
        run("delete-extra-mix-preserves-existing-ids", created)
        duplicate = copy.deepcopy(expanded)
        duplicate["mixes"].append(copy.deepcopy(duplicate["mixes"][0]))
        run("duplicate-stable-id-rejected", duplicate, reject=True)
        legacy = copy.deepcopy(config)
        legacy["schemaVersion"] = 1
        run("superseded-v1-not-used-as-v2-contract", legacy, reject=True)
    return {"status": "fail" if any(r["status"] == "fail" for r in results) else "pass",
            "evidence_kind": "black-box-schema-2-cli-simulated-inventory", "native_routing_verified": False,
            "checks": len(results), "results": results,
            "unexecuted_cases": ["full device-scoped-media lifecycle: see media-control-scenarios.json; native attribution/dispatch proof required",
                                 "runtime capacity, held endpoints and native lifecycle: native evidence required"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="already built CLI; this script never builds it")
    parser.add_argument("--examples", type=Path, default=Path(__file__).resolve().parents[1] / "examples")
    args = parser.parse_args()
    try:
        report = check(args.binary.resolve(), args.examples)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        report = {"status": "blocked", "reason": str(error), "native_routing_verified": False}
    print(json.dumps(report, indent=2))
    raise SystemExit({"pass": 0, "fail": 1, "blocked": 2}[report["status"]])

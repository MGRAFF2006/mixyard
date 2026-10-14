#!/usr/bin/env python3
"""Black-box schema-2 CLI checks. Simulated policy, never native audio/events."""

import argparse
import copy
import json
from pathlib import Path
import subprocess
import tempfile


def check(binary, examples, runner=()):
    config = json.loads((examples / "config.json").read_text())
    inventory = json.loads((examples / "inventory.json").read_text())
    if config.get("schemaVersion") != 2 or inventory.get("schemaVersion") != 2:
        return {"status": "blocked", "reason": "Schema-2 public config/inventory fixtures not present; fixed-three v1 expectations are superseded.",
                "native_routing_verified": False}
    if not isinstance(config.get("mixes"), list) or not config["mixes"] \
            or not isinstance(config.get("outputs"), list) or not config["outputs"]:
        return {"status": "blocked", "reason": "Schema-2 mixes/outputs command fixture contract unavailable.",
                "native_routing_verified": False}
    media_request = json.loads((examples / "media-request.json").read_text())
    if media_request.get("schemaVersion") != 2:
        return {"status": "blocked", "reason": "Schema-2 public media request fixture unavailable.",
                "native_routing_verified": False}
    # Hand-written public fixture oracle, not a second media resolver.
    if (media_request.get("deviceUid"), media_request.get("action")) != ("example:device-a", "pause"):
        return {"status": "blocked", "reason": "No independent media expectation for this request fixture.",
                "native_routing_verified": False}
    expected_players = [("simulated:session:spotify", "example:spotify", "pause", "planned", "not-dispatched")]
    # Explicit public fixture resolutions; never derive these from CLI replies.
    expected_resolved_outputs = {
        "call-input": {"observedIds": ["simulated:output:0"], "effectiveGain": 1.0},
        "record-input": {"observedIds": ["simulated:output:1"], "effectiveGain": 1.0},
        "headphones": {"observedIds": ["simulated:output:2"], "effectiveGain": 1.0},
        "speaker-a": {"observedIds": ["simulated:output:3"], "effectiveGain": 1.0},
        "speaker-b": {"observedIds": ["simulated:output:4"], "effectiveGain": 1.0},
        "speaker-c": {"observedIds": ["simulated:output:5"], "effectiveGain": 0.5},
        "party": {"observedIds": ["simulated:output:6"], "effectiveGain": 1.0},
    }
    if {o["id"] for o in config["outputs"]} != set(expected_resolved_outputs):
        return {"status": "blocked", "reason": "No independent output resolutions for this config fixture.",
                "native_routing_verified": False}
    # Unaffected public fixture sends, including resolved state, not a CLI baseline.
    expected_sends = [
        {"source": source, "mix": mix, "gain": gain, "mute": False,
         "observedId": observed, "effectiveGain": gain, "channelMapping": mapping,
         "rateConversionRequired": False}
        for source, mix, gain, observed, mapping in (
            ("mic", "call", 1.0, "simulated:source:0", "mono"),
            ("mic", "monitor", 0.0, "simulated:source:0", "duplicate-mono"),
            ("mic", "record", 0.5, "simulated:source:0", "duplicate-mono"),
            ("return", "monitor", 0.5, "simulated:source:3", "stereo"),
            ("return", "record", 0.25, "simulated:source:3", "stereo"),
            ("spotify", "desk-a", 1.0, "simulated:source:1", "stereo"),
            ("spotify", "monitor", 0.5, "simulated:source:1", "stereo"),
            ("spotify", "record", 0.25, "simulated:source:1", "stereo"),
            ("spotify", "shared", 0.5, "simulated:source:1", "stereo"),
            ("vlc", "desk-b", 1.0, "simulated:source:2", "stereo"),
            ("vlc", "shared", 0.5, "simulated:source:2", "stereo"),
        )
    ]
    results = []
    with tempfile.TemporaryDirectory(prefix="mixyard-policy-v2-") as directory:
        root = Path(directory)

        def run(label, desired, command="plan", reject=False, resolved_outputs=expected_resolved_outputs):
            desired_path, observed_path = root / "config.json", root / "inventory.json"
            desired_path.write_text(json.dumps(desired))
            observed_path.write_text(json.dumps(inventory))
            arguments = [*runner, str(binary), command, str(desired_path)]
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
                            and plan["deviceUid"] == media_request["deviceUid"] \
                            and plan["status"] == "ready" \
                            and sorted((p["sessionId"], p["applicationId"], p["plannedAction"],
                                        p["decision"], p["dispatchOutcome"]) for p in plan["players"]) == expected_players
                    else:
                        fields = ("id", "mix", "target", "gain", "mute", "allowMicMonitor", "observedIds", "effectiveGain")
                        expected_outputs = [{**o, **resolved_outputs[o["id"]], "mute": o.get("mute", False),
                                             "allowMicMonitor": o.get("allowMicMonitor", False)}
                                            for o in desired["outputs"]]
                        expected_masters = [
                            {"id": m["id"], "master": {"gain": 1.0, "mute": False, **m.get("master", {})}}
                            for m in desired["mixes"]
                        ]
                        passed = passed and sorted([{k: m[k] for k in ("id", "master")} for m in plan["mixes"]],
                                                   key=lambda m: m["id"]) == sorted(expected_masters, key=lambda m: m["id"]) \
                            and sorted(plan["sends"], key=lambda s: (s["source"], s["mix"])) == expected_sends \
                            and sorted([{k: o[k] for k in fields} for o in plan["outputs"]], key=lambda o: o["id"]) \
                            == sorted([{k: o[k] for k in fields} for o in expected_outputs], key=lambda o: o["id"])
            except (ValueError, KeyError, TypeError):
                passed = False
                diagnostic = diagnostic or "Public CLI result did not match the schema-2 contract."
            results.append({"case": label, "status": "pass" if passed else "fail",
                            "diagnostic": "" if passed else diagnostic or "Public CLI result did not match independent fixture expectations."})

        run("public-v2-validate", config, command="validate")
        run("public-v2-plan-media-argument-order-no-dispatch", config, command="plan-media")
        run("all-mix-and-output-ids-accounted-for", config)
        output_levels = copy.deepcopy(config)
        output_levels["outputs"][0].update(gain=.25, mute=True)
        muted_resolution = copy.deepcopy(expected_resolved_outputs)
        muted_resolution[output_levels["outputs"][0]["id"]]["effectiveGain"] = 0.0
        run("output-gain-mute-preserves-siblings", output_levels, resolved_outputs=muted_resolution)
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

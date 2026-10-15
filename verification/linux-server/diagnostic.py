#!/usr/bin/env python3
"""Enumeration matrix and allowed fixture-0 transport only; no qualification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from linux_server_ingress import BUILD, CONFIG, INSTALL, Probe, ROOT, analyze, wait


def hashes(paths):
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}


def capture(probe, state, clock_id):
    name = "allowed-capture"
    path = probe.evidence / (name + ".f32")
    child = probe.start([BUILD / "fixture", "capture", name, path], name)
    identity = {}

    def endpoint():
        graph = probe.graph(name + "-graph")
        nodes = [o["id"] for o in graph if o.get("info", {}).get("props", {}).get("node.name") == name]
        ports = [o["id"] for o in graph if o["type"] == "PipeWire:Interface:Port" and
                 str(o.get("info", {}).get("props", {}).get("node.id")) in set(map(str, nodes))]
        if len(nodes) != 1 or len(ports) != 1:
            return None
        node = next(o for o in graph if o["id"] == nodes[0])
        port = next(o for o in graph if o["id"] == ports[0])
        props = port["info"]["props"]
        if props.get("port.exclusive") is not True or props.get("port.direction") != "in":
            raise RuntimeError("capture port must be exclusive input before link")
        identity.update(node_id=node["id"], node_serial=node["info"]["props"]["object.serial"],
                        port_id=port["id"], port_serial=props["object.serial"], local_port_id=props["port.id"])
        probe.save("allowed-capture-identity", identity)
        return ports[0]

    port = wait(endpoint, "allowed capture port")
    linked = probe.run(["pw-link", "-L", str(state["output"]["port_id"]), str(port)])
    if linked.returncode:
        raise RuntimeError("allowed public-output link failed: " + linked.stderr)
    states = []
    for _ in range(10):
        time.sleep(.1)
        states.append(probe.command("STATUS"))
    probe.save("allowed-running", states[-1])
    probe.save("allowed-sustained", states)
    graph = probe.graph("allowed-running-graph")
    links = [o for o in graph if o["type"] == "PipeWire:Interface:Link" and
             o["info"]["output-node-id"] == state["output"]["node_id"] and
             o["info"]["output-port-id"] == state["output"]["port_id"] and
             o["info"]["input-node-id"] == identity["node_id"] and
             o["info"]["input-port-id"] == identity["port_id"]]
    code = probe.stop(child)
    # Reuse completeness/clock validation, but do not claim privacy without its
    # separately blocked acceptance gate. This milestone scores transport only.
    result = analyze(path, sentinel_reproduced=False, expected_clock_id=clock_id)
    frames = list(struct.iter_unpack("<ff", path.read_bytes()))
    result["capture_returncode"] = code
    result["expected_clock_id"] = clock_id
    result["capture_endpoint"] = identity
    result["public_link_active"] = len(links) == 1 and links[0]["info"]["state"] == "active"
    result["sustained_generation"] = all(s["generation"] == state["generation"] and
        s["generation"] > 0 and s["link_serial"] == state["link_serial"] and
        s["ingress"]["no_mixer"] and s["ingress"]["exclusive"] and
        s["ingress"]["mix_count"] == 1 and not s["format_errors"] for s in states)
    result["transport_status"] = "pass" if (code == 0 and result.get("complete") and
        result["sustained_generation"] and result["public_link_active"] and
        frames and all(f == (.125, -.125) for f in frames)) else "fail"
    result["privacy"] = "blocked"
    probe.save("allowed-transport", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--config", type=Path, default=CONFIG)
    parser.add_argument("--capture", action="store_true")
    args = parser.parse_args()
    if not Path("/.dockerenv").exists() or os.geteuid() == 0 or Path("/dev/snd").exists():
        parser.error("requires unprivileged isolated container")
    probe = Probe(args.evidence)
    # Protocol trace includes client core sync/done and parameter enumeration.
    probe.env["PIPEWIRE_DEBUG"] = "5"
    source = args.config.read_text()
    paths = [Path(__file__).resolve(), ROOT / "verification/linux_server_ingress.py",
             ROOT / "adapters/linux/server-proof/module.c", ROOT / "adapters/linux/server-proof/compile.sh",
             ROOT / "verification/linux-server/fixture.c", ROOT / "verification/linux-server/transport-check.c",
             ROOT / "verification/linux-server/compile.sh", ROOT / "verification/linux-server/run-diagnostic.sh",
             args.config, BUILD / "modules/libpipewire-module-mixyard-server-proof.so", BUILD / "fixture",
             BUILD / "transport-check", BUILD / "source/src/pipewire/private.h",
             INSTALL / "bin/pipewire", INSTALL / "bin/pw-dump", INSTALL / "bin/pw-link",
             INSTALL / "lib/libpipewire-0.3.so.0.1609.0", BUILD / "source/build/config.h"]
    probe.save("provenance", {"source_revision": os.environ.get("PROOF_SOURCE_REVISION", "UNKNOWN"),
        "sha256": hashes(paths), "enumeration_timeout_seconds": 10,
        "version": probe.run([INSTALL / "bin/pipewire", "--version"]).stdout})
    rows = []
    transport = None
    for case in ("without-module", "module-unselected", "module-selected-allowed"):
        row = {"case": case, "status": "fail"}
        first_child = len(probe.children)
        try:
            config = source
            if case == "without-module":
                config, count = re.subn(r"    \{ name = libpipewire-module-mixyard-server-proof args = \{.*?\} \}\n", "", config, flags=re.S)
                if count != 1:
                    raise RuntimeError("expected exactly one proof module block")
            consent = Path("/private/consent")
            consent.write_text("MIXYARD_PRIVATE_SERVER_PROOF_V1\n")
            consent.chmod(0o600)
            path = probe.evidence / (case + ".conf")
            path.write_text(config)
            daemon = probe.start([INSTALL / "bin/pipewire", "-c", path], case)
            wait(lambda: Path(probe.env["XDG_RUNTIME_DIR"], "mixyard-server-private").exists() or
                 daemon.poll() is not None, "private daemon socket")
            if daemon.poll() is not None:
                raise RuntimeError("daemon exited before enumeration")
            if case != "without-module":
                wait(lambda: Path("/private/control.sock").exists(), "control socket")
                state = probe.command("STATUS")
                probe.save(case + "-initial", state)
                if not (state["generation"] == 0 and state["ingress"]["no_mixer"] and
                        state["ingress"]["exclusive"] and state["ingress"]["mix_count"] == 0 and
                        not state["ingress"]["formatted"]):
                    raise RuntimeError("ingress must use exclusive native scheduling before selection/Format")
                if case == "module-selected-allowed":
                    reply = probe.select(state["fixtures"][0])
                    probe.save(case + "-selection", reply)
                    if reply.get("status") != "control_done" or not reply.get("generation"):
                        raise RuntimeError("fixture-0 SELECT failed")
            started = time.monotonic()
            graph = probe.graph(case + "-graph")
            row.update(status="pass", elapsed_seconds=time.monotonic() - started, globals=len(graph))
            if case != "without-module":
                state = probe.command("STATUS")
                probe.save(case + "-after-enumeration", state)
                if case == "module-selected-allowed" and not state["generation"]:
                    raise RuntimeError("selection retired during enumeration")
            if case == "module-selected-allowed" and args.capture:
                drivers = [o["id"] for o in graph if o.get("info", {}).get("props", {}).get("node.name") == "Private-Dummy-Driver"]
                if len(drivers) != 1:
                    raise RuntimeError("expected one private graph driver")
                transport = capture(probe, state, drivers[0])
        except (OSError, ValueError, RuntimeError, KeyError) as exc:
            row["error"] = str(exc)
        finally:
            for child in reversed(probe.children[first_child:]):
                probe.stop(child)
        rows.append(row)
    result = {"matrix": rows, "allowed_transport": transport,
              "status": "pass" if all(r["status"] == "pass" and "error" not in r for r in rows) and
              (not args.capture or transport and transport["transport_status"] == "pass") else "fail",
              "privacy_acceptance": "blocked", "production_apply": False,
              "artifacts": {str(p.relative_to(args.evidence)): hashlib.sha256(p.read_bytes()).hexdigest() for p in args.evidence.rglob("*")
                            if p.is_file() and p.name != "report.json"}}
    probe.save("report", result)
    print(json.dumps({k: v for k, v in result.items() if k != "artifacts"}, indent=2))
    return int(result["status"] != "pass")


if __name__ == "__main__":
    raise SystemExit(main())

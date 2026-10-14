#!/usr/bin/env python3
"""Private matched-source daemon probe. Never enables production routing."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import signal
import socket
import struct
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "verification/linux-server/build"
INSTALL = BUILD / "install"
CONFIG = ROOT / "verification/linux-server/pipewire.conf"


def analyze(path, negative=False):
    """Inspect every stored frame, including first/tails, without alignment/fitting."""
    path = Path(path)
    if not path.exists() or not Path(str(path) + ".json").exists():
        return {"status": "blocked", "reason": "missing PCM or packet trace"}
    meta = json.loads(Path(str(path) + ".json").read_text())
    data = path.read_bytes()
    if len(data) % 8 or len(data) != meta["frames"] * 8:
        return {"status": "fail", "reason": "truncated or inconsistent PCM"}
    frames = list(struct.iter_unpack("<ff", data))
    nonfinite = sum(not all(map(math.isfinite, frame)) for frame in frames)
    forbidden = sum(abs(left) > .4 or abs(right) > .4 for left, right in frames)
    allowed = sum(left == .125 and right == -.125 for left, right in frames)
    silent = sum(left == 0 and right == 0 for left, right in frames)
    other = len(frames) - allowed - silent
    packets = meta["packets"]
    gaps = sum(b["position"] != a["position"] + a["duration"]
               for a, b in zip(packets, packets[1:]))
    offset = 0
    bad_trace = 0
    for packet in packets:
        bad_trace += packet["offset"] != offset or packet["frames"] != packet["duration"] or bool(packet["flags"])
        offset += packet["frames"]
    loss = any(value for key, value in meta["drops"].items() if key != "process_calls")
    complete = bool(frames) and offset == len(frames) and not loss and not bad_trace and not gaps and meta["status"] == "RECORDED"
    result = {"frames": len(frames), "allowed_frames": allowed, "silent_frames": silent,
              "forbidden_frames": forbidden, "other_frames": other, "nonfinite_frames": nonfinite,
              "first_frame": frames[0] if frames else None, "last_frame": frames[-1] if frames else None,
              "position_gaps": gaps, "bad_packets": bad_trace, "drops": meta["drops"],
              "format": {key: meta[key] for key in ("format", "wire_format", "rate", "channels")},
              "clock_ids": sorted({p["clock_id"] for p in packets}), "complete": complete}
    if negative:
        result["status"] = "fail" if forbidden else "blocked"
        result["sentinel_reproduced"] = forbidden > 0 and nonfinite == 0
    else:
        result["privacy"] = "fail" if forbidden or nonfinite else "pass" if complete else "blocked"
        result["continuity"] = "pass" if complete and allowed == len(frames) else "fail" if frames else "blocked"
        result["status"] = "fail" if forbidden or nonfinite or other or gaps else "pass" if complete else "blocked"
    return result


def wait(predicate, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.03)
    raise RuntimeError("timeout: " + description)


class Probe:
    def __init__(self, evidence):
        self.evidence = evidence
        self.children = []
        self.checks = []
        self.sequence = 0
        self.env = dict(os.environ, PATH=str(INSTALL / "bin") + ":" + os.environ["PATH"],
                        LD_LIBRARY_PATH=str(INSTALL / "lib"),
                        SPA_PLUGIN_DIR=str(INSTALL / "lib/spa-0.2"),
                        PIPEWIRE_MODULE_DIR=str(BUILD / "modules") + ":" + str(INSTALL / "lib/pipewire-0.3"),
                        PIPEWIRE_CONFIG_DIR=str(INSTALL / "share/pipewire"),
                        PIPEWIRE_REMOTE="mixyard-server-private", PIPEWIRE_DEBUG="4", XDG_RUNTIME_DIR="/private/runtime",
                        XDG_CONFIG_HOME="/private/config", XDG_STATE_HOME="/private/state",
                        DBUS_SESSION_BUS_ADDRESS="unix:path=/private/no-dbus")
        for key in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_STATE_HOME"):
            Path(self.env[key]).mkdir(mode=0o700, exist_ok=True)

    def save(self, name, data):
        (self.evidence / (name + ".json")).write_text(json.dumps(data, indent=2))

    def start(self, args, name):
        with (self.evidence / (name + ".log")).open("w") as log:
            child = subprocess.Popen(list(map(str, args)), env=self.env, stdout=log, stderr=subprocess.STDOUT)
        self.children.append(child)
        return child

    def stop(self, child):
        if child.poll() is None:
            child.send_signal(signal.SIGTERM)
            try:
                child.wait(8)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        return child.returncode

    def run(self, args):
        return subprocess.run(list(map(str, args)), env=self.env, text=True, capture_output=True, timeout=10)

    def graph(self, name):
        result = self.run(["pw-dump"])
        if result.returncode:
            raise RuntimeError(result.stderr)
        graph = json.loads(result.stdout)
        self.save(name, graph)
        return graph

    def command(self, command):
        with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET) as control:
            control.settimeout(3)
            control.connect("/private/control.sock")
            control.sendall(command.encode())
            result = json.loads(control.recv(4096))
        self.sequence += 1
        self.save(f"control-{self.sequence}", {"command": command, "reply": result,
                  "monotonic_ns": time.monotonic_ns(), "pcm_ack": False})
        return result

    def select(self, endpoint):
        return self.command("SELECT " + " ".join(str(endpoint[key]) for key in
                            ("node_id", "node_serial", "port_id", "port_serial")) + " 4294967295 0")

    def daemon(self, negative, name):
        consent = Path("/private/consent")
        consent.write_text("MIXYARD_INSECURE_NEGATIVE_CONTROL_V1\n" if negative else "MIXYARD_PRIVATE_SERVER_PROOF_V1\n")
        consent.chmod(0o600)
        config = CONFIG.read_text()
        if negative:
            config = config.replace("fixture.sources = true", "fixture.sources = true\n        negative.control = true")
        path = self.evidence / (name + ".conf")
        path.write_text(config)
        daemon = self.start([INSTALL / "bin/pipewire", "-c", path], name)
        wait(lambda: Path("/private/control.sock").exists() or daemon.poll() is not None, "daemon socket")
        if daemon.poll() is not None:
            raise RuntimeError("daemon exited: " + (self.evidence / (name + ".log")).read_text()[-3000:])
        return daemon, self.command("STATUS")

    def capture(self, state, name):
        path = self.evidence / (name + ".f32")
        capture = self.start([BUILD / "fixture", "capture", name, path], name)
        def endpoint():
            graph = self.graph(name + "-graph")
            nodes = [o for o in graph if o.get("info", {}).get("props", {}).get("node.name") == name]
            if not nodes:
                return None
            ports = [o for o in graph if o["type"] == "PipeWire:Interface:Port" and
                     str(o.get("info", {}).get("props", {}).get("node.id")) == str(nodes[0]["id"])]
            return (nodes[0]["id"], ports[0]["id"]) if ports else None
        node, port = wait(endpoint, name + " capture port")
        link = self.start([BUILD / "fixture", "attack", state["output"]["node_id"], state["output"]["port_id"], node, port], name + "-link")
        return capture, link, path

    def attack(self, source, ingress, name, port=None, regrant=False):
        args = [BUILD / "fixture", "attack"] + (["--self-regrant"] if regrant else [])
        args += [source["node_id"], source["port_id"], ingress["node_id"], ingress["port_id"] if port is None else port]
        child = self.start(args, name)
        time.sleep(.12)
        return child

    def execute(self):
        # Sentinel reproduction happens before any protected absence is scored.
        daemon, state = self.daemon(True, "insecure-negative")
        selected = self.select(state["fixtures"][1])
        self.save("negative-selection", selected)
        capture, link, path = self.capture(state, "negative-sentinel")
        time.sleep(.8)
        self.stop(capture); self.stop(link)
        negative = analyze(path, True)
        self.checks.append({"case": "insecure-sentinel-control", **negative})
        self.stop(daemon)
        wait(lambda: not Path("/private/control.sock").exists(), "negative unload")

        daemon, state = self.daemon(False, "protected")
        self.save("protected-initial", state)
        graph = self.graph("protected-before-select")
        hidden = {state["ingress"]["node_id"], state["ingress"]["port_id"]}
        self.checks.append({"case": "preselection-registry-visibility", "status": "fail" if hidden & {o["id"] for o in graph} else "pass"})
        attacks = [self.attack(state["fixtures"][1], state["ingress"], "before-select")]
        selection = self.select(state["fixtures"][0])
        self.checks.append({"case": "exact-allowed-selection", "status": "pass" if selection.get("status") == "control_done" else "fail", "reply": selection})
        state = self.command("STATUS")
        capture, link, path = self.capture(state, "protected-allframes")
        fanout, fanlink, fanpath = self.capture(state, "fanout-allframes")
        attacks += [self.attack(state["fixtures"][1], state["ingress"], "unrestricted-regrant", regrant=True),
                    self.attack(state["fixtures"][1], state["ingress"], "local-port-fallback", port=0)]
        name_attack = self.run(["pw-link", "mixyard.proof.forbidden:stereo", "mixyard.proof.ingress:stereo"])
        self.save("name-attack", {"returncode": name_attack.returncode, "stdout": name_attack.stdout, "stderr": name_attack.stderr})
        # Only private profiles; all hardware monitors and DBus are disabled.
        wpdir = Path("/private/wp/wireplumber.conf.d"); wpdir.mkdir(parents=True)
        (wpdir / "proof.conf").write_text("wireplumber.profiles = { main = { monitor.alsa = disabled monitor.bluez = disabled monitor.bluez-midi = disabled monitor.v4l2 = disabled monitor.libcamera = disabled support.reserve-device = disabled support.logind = disabled } }\n")
        self.env["WIREPLUMBER_CONFIG_DIR"] = "/private/wp"
        wp = self.start(["wireplumber"], "wireplumber-hostile")
        time.sleep(.6)
        self.save("during-hostile", self.command("STATUS"))
        self.graph("during-hostile-graph")
        self.command("DESTROY_LINK")
        attacks.append(self.attack(state["fixtures"][1], state["ingress"], "after-destroy-regrant", regrant=True))
        time.sleep(.4)
        self.save("after-destroy", self.command("STATUS"))
        self.select(state["fixtures"][0])
        time.sleep(.4)
        self.stop(capture); self.stop(link); self.stop(fanout); self.stop(fanlink)
        for label, pcm in (("protected-allframes", path), ("output-fanout", fanpath)):
            result = analyze(pcm)
            if result.get("privacy") == "pass" and not negative.get("sentinel_reproduced"):
                result["privacy"] = result["status"] = "blocked"
                result["reason"] = "forbidden sentinel not reproduced"
            self.checks.append({"case": label, **result})
        for child in attacks: self.stop(child)
        self.stop(wp)
        capture, link, path = self.capture(state, "reopen-allframes")
        time.sleep(.4); self.stop(capture); self.stop(link)
        self.checks.append({"case": "output-reopen", **analyze(path)})
        stale = dict(state["fixtures"][0]); stale["port_serial"] += 1
        wrong = self.select(stale)
        self.checks.append({"case": "wrong-port-serial", "status": "pass" if wrong.get("status") == "error" else "fail", "reply": wrong})
        self.select(state["fixtures"][0])
        self.command("DESTROY_SOURCE 0")
        removed = self.command("STATUS")
        stale_reply = self.select(state["fixtures"][0])
        self.checks.append({"case": "source-removal-no-fallback", "status": "pass" if removed["generation"] == 0 and stale_reply.get("status") == "error" else "fail", "state": removed, "reply": stale_reply})
        for case in ("startup-before-observation-race", "forced-id-reuse-wrong-serial", "port-removal-alone", "format-channel-rate-mutation", "external-producer-selection", "error-recovery-pcm", "explicit-module-unload"):
            self.checks.append({"case": case, "status": "blocked", "reason": "not yet exercised by this bounded run"})
        self.command("RETIRE"); self.command("RETIRE")
        self.stop(daemon)
        wait(lambda: not Path("/private/control.sock").exists(), "protected daemon unload")
        daemon, restart = self.daemon(False, "restart")
        self.checks.append({"case": "server-restart-no-selected-fallback", "status": "pass" if restart["generation"] == 0 and restart["link_serial"] == 0 else "fail", "state": restart})
        self.stop(daemon)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    if not Path("/.dockerenv").exists() or os.geteuid() == 0 or Path("/dev/snd").exists():
        parser.error("requires unprivileged isolated container")
    probe = Probe(args.evidence)
    probe.save("resources", {"loadavg": Path("/proc/loadavg").read_text(), "cpu_stat": Path("/proc/stat").read_text(), "rtprio": resource.getrlimit(resource.RLIMIT_RTPRIO), "memlock": resource.getrlimit(resource.RLIMIT_MEMLOCK), "latency": "unknown", "clock_scope": "private dummy graph, no physical clocks"})
    error = None
    try:
        probe.execute()
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError) as exc:
        error = str(exc)
    finally:
        for child in reversed(probe.children): probe.stop(child)
    result = {"status": "fail" if error or any(c["status"] == "fail" and c["case"] != "insecure-sentinel-control" for c in probe.checks) else "blocked" if any(c["status"] == "blocked" for c in probe.checks) else "pass", "error": error, "checks": probe.checks, "production_apply": False, "audio_qualification": False,
              "threat_scope": "graph route substitution; excludes compromised selected producer/server and arbitrary same-user memory/config/control-socket access", "artifacts": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in args.evidence.iterdir() if p.is_file() and p.name != "report.json"}}
    probe.save("report", result)
    print(json.dumps({"status": result["status"], "error": error, "checks": [(c["case"], c["status"]) for c in probe.checks]}, indent=2))
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())

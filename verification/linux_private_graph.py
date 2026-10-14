#!/usr/bin/env python3
"""PRIVATE native graph probe. Run only inside linux-private/run.sh's container.
No production policy integration; qualify.py remains the independent PCM oracle.
"""
import argparse
import hashlib
import json
import math
import struct
import os
import re
import resource
from pathlib import Path
import signal
import subprocess
import sys
import time

import qualify

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "verification/linux-private"


def wait(predicate, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.03)
    raise RuntimeError(f"timeout: {description}")


def require_fixture_consent(route):
    """Fixed synthetic routing declaration only; no production policy planner."""
    if route["mic_consent"] is not True:
        raise ValueError("fixture mic-to-physical-like route needs consent even at zero/mute")


def qualify_revision_acks(traces, acks, commanded_revisions, consumer_pcm, mixes, sources,
                          pre_capture_revisions=()):
    """Correlate internal handoffs with independently checked external packets.

    Only commands issued during this capture are sample-effective candidates.
    Earlier handoffs are explicitly uncovered by this capture, even when their
    steady PCM is checked separately. No empty set earns a qualification pass.
    """
    errors = []
    matched = 0
    for revision in commanded_revisions:
        for mix in range(mixes):
            key = (mix, revision)
            rows = [r for r in traces if (r[1], r[9]) == key]
            if key not in acks:
                errors.append({"mix_revision": key, "reason": "missing internal handoff ACK"})
            if not rows:
                errors.append({"mix_revision": key, "reason": "commanded revision absent from trace"})
                continue
            frame = min(r[2] for r in rows)
            first = [r for r in rows if r[2] == frame]
            if key not in acks:
                continue
            if acks[key] != frame:
                errors.append({"mix_revision": key, "reason": "ACK/first callback frame mismatch",
                               "callback_frame": frame, "ack_frame": acks[key]})
                continue
            if len(first) != sources or {r[0] for r in first} != set(range(sources)):
                errors.append({"mix_revision": key, "reason": "missing source trace at ACK frame"})
                continue
            packets = {(r[4], r[10], r[6]) for r in first}
            if len(packets) != 1 or not packets.issubset(consumer_pcm.get(mix, set())):
                errors.append({"mix_revision": key,
                               "reason": "acknowledged frame absent from verified external consumer PCM"})
                continue
            matched += 1
    return {"status": "fail" if errors else "pass" if commanded_revisions else "blocked",
            "scope": "revisions commanded during this capture; callback ACKs alone are internal handoffs",
            "commanded_revisions": list(commanded_revisions),
            "pre_capture_revisions_uncovered": list(pre_capture_revisions),
            "matched_revision_frames": matched, "errors": errors}


def resource_snapshot(processes=()):
    def read(path):
        try:
            return Path(path).read_text().strip()
        except FileNotFoundError:
            return "unavailable"
    cpu = list(map(int, Path("/proc/stat").read_text().splitlines()[0].split()[1:]))
    threads = []
    for child in processes:
        if child.poll() is not None:
            continue
        for tid_path in Path(f"/proc/{child.pid}/task").iterdir():
            try:
                tid = int(tid_path.name)
                threads.append({"process": child.proof_name, "tid": tid,
                                "policy": os.sched_getscheduler(tid),
                                "priority": os.sched_getparam(tid).sched_priority})
            except ProcessLookupError:
                continue
    return {"monotonic_ns": time.monotonic_ns(), "host_loadavg": read("/proc/loadavg"),
            "guest_cpu_ticks": cpu, "guest_steal_ticks": cpu[7],
            "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "cpu_pressure": read("/proc/pressure/cpu"),
            "cgroup_cpu_max": read("/sys/fs/cgroup/cpu.max"),
            "cgroup_cpu_stat": read("/sys/fs/cgroup/cpu.stat"),
            "cgroup_cpu_pressure": read("/sys/fs/cgroup/cpu.pressure"),
            "rtprio_limit": resource.getrlimit(resource.RLIMIT_RTPRIO),
            "memlock_limit": resource.getrlimit(resource.RLIMIT_MEMLOCK),
            "affinity": sorted(os.sched_getaffinity(0)), "threads": threads,
            "policy_names": {"0": "SCHED_OTHER", "1": "SCHED_FIFO", "2": "SCHED_RR"}}


class Probe:
    def __init__(self, evidence):
        self.evidence = evidence
        self.processes = []
        self.env = dict(os.environ, XDG_RUNTIME_DIR="/tmp/mixyard-runtime",
                        XDG_STATE_HOME="/tmp/mixyard-state", XDG_CONFIG_HOME="/tmp/mixyard-config",
                        PIPEWIRE_REMOTE="mixyard-private", MIXYARD_PRIVATE_PROOF="1", WIREPLUMBER_DEBUG="3",
                        WIREPLUMBER_CONFIG_DIR="/tmp/mixyard-wp", DBUS_SESSION_BUS_ADDRESS="unix:path=/tmp/no-dbus")
        for name in ("XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "WIREPLUMBER_CONFIG_DIR"):
            Path(self.env[name]).mkdir(mode=0o700, exist_ok=True)
        self.checks = []
        self.exits = []
        self.stopped = set()
        self.runtime_count = 0
        self.current_revision = 0
        self.gate_first = [2**64 - 1, 2**64 - 1]
        self.coefficient_history = {}
        self.commanded_revisions = []
        self.process_names = {}

    def run(self, args, **kwargs):
        return subprocess.run(args, env=self.env, text=True, capture_output=True, timeout=10, **kwargs)

    def start(self, args, name, control=False):
        self.process_names[name] = self.process_names.get(name, 0) + 1
        unique_name = name if self.process_names[name] == 1 else f"{name}-{self.process_names[name]}"
        log = self.evidence / f"{unique_name}.log"
        with log.open("w") as output:
            p = subprocess.Popen(args, env=self.env, stdout=output, stderr=subprocess.STDOUT,
                                 stdin=subprocess.PIPE if control else subprocess.DEVNULL,
                                 text=True, start_new_session=True)
        p.proof_name = unique_name
        self.processes.append(p)
        return p, log

    def stop(self, p):
        if p in self.stopped:
            return
        self.stopped.add(p)
        if p.poll() is None:
            os.killpg(p.pid, signal.SIGTERM)
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)
                p.wait(timeout=3)
        self.exits.append({"process": p.proof_name, "exit": p.returncode})

    def graph(self, name):
        r = self.run(["pw-dump"])
        if r.returncode:
            raise RuntimeError(r.stderr)
        graph = json.loads(r.stdout)
        (self.evidence / f"{name}.json").write_text(json.dumps(graph, indent=2))
        if any(x["type"] == "PipeWire:Interface:Device" for x in graph):
            raise RuntimeError("unexpected hardware Device in private graph")
        return graph

    def command(self, command, expect=None):
        if self.runtime.poll() is not None:
            raise RuntimeError("runtime exited: " + self.runtime_log.read_text())
        before = self.runtime_log.stat().st_size
        self.runtime.stdin.write(command + "\n")
        self.runtime.stdin.flush()
        if expect:
            wait(lambda: expect in self.runtime_log.read_text()[before:], command + ": " + expect)
            if expect == "prepared":
                self.current_revision = int(re.search(r"prepared (\d+)", self.runtime_log.read_text()[before:])[1])
                self.commanded_revisions.append(self.current_revision)
                time.sleep(.08)
                self.command("status", "status")
        else:
            time.sleep(.08)

    def record(self, endpoint, name, sink=False):
        return self.start(["pw-record", "--target", endpoint, "--rate", "48000", "--channels", "2",
                           "--channel-map", "FL,FR", "--format", "s16", "--latency", "256",
                           "--properties", f"node.name=capture.{name} node.dont-fallback=true node.dont-move=true node.dont-reconnect=true stream.capture.sink={str(sink).lower()}",
                           str(self.evidence / f"{name}.wav")], "capture." + name)[0]

    def analyze(self, name, targets, offset=.3, duration=.5):
        try:
            result = qualify.analyze(self.evidence / f"{name}.wav", offset=offset, duration=duration,
                                    targets=[{f: channel.get(f, 0) for f in qualify.FREQUENCIES} for channel in targets])
        except ValueError as exc:
            result = {"status": "blocked", "reason": str(exc)}
        self.checks.append({"case": name, "oracle": result})
        (self.evidence / f"{name}.analysis.json").write_text(json.dumps(result, indent=2))
        return result["status"] == "pass"

    def compile(self):
        cflags = self.run(["pkg-config", "--cflags", "libpipewire-0.3"]).stdout.split()
        libs = self.run(["pkg-config", "--libs", "libpipewire-0.3"]).stdout.split()
        version = self.run(["pkg-config", "--modversion", "libpipewire-0.3"]).stdout.strip()
        if tuple(map(int, version.split(".")[:2])) < (1, 6):
            raise RuntimeError("PipeWire >=1.6 required for exclusive ingress ports")
        for source, name in ((FIXTURE / "capture.c", "capture"), (ROOT / "adapters/linux/native/mixyard_pw.c", "runtime"),
                             (FIXTURE / "producer.c", "producer")):
            r = self.run(["gcc", "-std=gnu11", "-O2", "-Wall", "-Wextra", "-Werror", *cflags,
                          str(source), "-o", str(self.evidence / name), *libs, "-lm"])
            (self.evidence / f"{name}.build.log").write_text(r.stdout + r.stderr)
            if r.returncode:
                raise RuntimeError(r.stderr)
        packages = self.run(["dpkg-query", "-W", "pipewire", "pipewire-bin", "libpipewire-0.3-dev",
                             "libspa-0.2-dev", "wireplumber", "gcc", "libc6-dev"]).stdout
        (self.evidence / "versions.txt").write_text(packages)
        return version

    def server(self):
        # This policy-only profile never loads a hardware monitor or session D-Bus.
        wp = Path(self.env["WIREPLUMBER_CONFIG_DIR"]) / "wireplumber.conf"
        wp.write_text(Path("/usr/share/wireplumber/wireplumber.conf").read_text() + '''
wireplumber.profiles = {
  proof = {
    inherits = [ policy, mixin.systemwide-session, mixin.stateless ]
    hardware.audio = disabled
    hardware.bluetooth = disabled
    hardware.video-capture = disabled
    support.mpris = disabled
    support.dbus = disabled
    support.system-dbus = disabled
    support.modem-manager = disabled
    api.notifications = disabled
  }
}
''')
        self.pw, _ = self.start(["pipewire", "-c", str(FIXTURE / "pipewire.conf")], "pipewire")
        wait(lambda: (Path(self.env["XDG_RUNTIME_DIR"]) / "mixyard-private").is_socket(), "private socket")
        self.wp, _ = self.start(["wireplumber", "--profile", "proof"], "wireplumber")
        time.sleep(.6)
        if self.wp.poll() is not None:
            raise RuntimeError("WirePlumber failed: " + (self.evidence / "wireplumber.log").read_text())

    def producer(self, name, left, right):
        return self.start([str(self.evidence / "producer"), name, str(left), str(right)], "producer." + name)[0]

    def runtime_start(self, names):
        self.runtime_count += 1
        self.gate_first = [2**64 - 1, 2**64 - 1]
        self.coefficient_history = {}
        self.commanded_revisions = []
        self.current_revision = 0
        self.runtime, self.runtime_log = self.start([str(self.evidence / "runtime"), "fixture.mic", "fixture.media",
                                                    "--", *names], f"runtime-{self.runtime_count}", control=True)
        wait(lambda: "private runtime" in self.runtime_log.read_text(), "runtime startup")
        time.sleep(.5)

    def diagnose_frames(self, names, stage="startup", open_gate=False, edit=False):
        pre_capture_revisions = self.commanded_revisions[:]
        selections = [("fixture.mic", "out", [(600, 1)], [(600, 1)]),
                      ("fixture.media", "out", [(1100, 1)], [(1700, 1)])]
        for i, name in enumerate(names):
            selections.append((name, "capture", [(600, 1)] if i == 0 else [(600, .5), (1100, .25)] if i == 1 else [(1100, 1)],
                               [(600, 1)] if i == 0 else [(600, .5), (1700, .25)] if i == 1 else [(1700, 1)]))
        captures = []
        for i, (endpoint, prefix, _, _) in enumerate(selections):
            node = f"frame-{stage}-{i}"
            child, log = self.start([str(self.evidence / "capture"), node, str(self.evidence / f"{stage}-frames-{i}.bin")], node, control=True)
            captures.append((child, log))
        time.sleep(.2)
        self.graph(f"frame-{stage}-observed")
        for i, (endpoint, prefix, _, _) in enumerate(selections):
            for channel in ("FL", "FR"):
                r = self.run(["pw-link", f"{endpoint}:{prefix}_{channel}", f"frame-{stage}-{i}:in_{channel}"])
                if r.returncode:
                    raise RuntimeError(r.stderr)
        self.graph(f"frame-{stage}-selected")
        for child, _ in captures:
            child.stdin.write("start\n")
            child.stdin.flush()
        coefficients = [[1, 0] if i == 0 else [.5, .25] if i == 1 else [0, 1] for i in range(len(names))]
        self.coefficient_history[self.current_revision] = [row[:] for row in coefficients]
        loads = []
        for iteration in range(15):
            if open_gate and iteration == 2:
                self.command("open", "barrier 3 validated")
                self.command("status", "status")
                status = self.runtime_log.read_text().rsplit("status revision=", 1)[-1]
                observed = re.findall(r"source (\d+).*?first=(\d+)", status)
                if len(observed) != 2 or any(int(frame) == 2**64 - 1 for _, frame in observed):
                    raise RuntimeError("missing first admitted frame acknowledgment")
                self.gate_first = [int(frame) for _, frame in sorted(observed)]
            if edit and iteration in (2, 5, 8, 11):
                m, source, gain, mute = {2: (0, 0, .25, 0), 5: (1, 0, .5, 1),
                                        8: (1, 0, .5, 0), 11: (0, 0, 1, 0)}[iteration]
                self.command(f"set {m} {source} {gain} {mute}", "prepared")
                coefficients[m][source] = 0 if mute else gain
                self.coefficient_history[self.current_revision] = [row[:] for row in coefficients]
            time.sleep(.2)
            loads.append(resource_snapshot(self.processes))
        (self.evidence / f"load-during-{stage}-capture.json").write_text(json.dumps(loads, indent=2))
        for child, _ in captures:
            self.stop(child)
            if child.returncode:
                raise RuntimeError("frame consumer dropped data or failed: " + str(child.returncode))
        self.command("trace", "trace-overflow")
        (self.evidence / f"frame-{stage}-trace.log").write_text(self.runtime_log.read_text())
        traces = [list(map(int, line.split()[1:])) for line in self.runtime_log.read_text().splitlines() if line.startswith("trace ")]
        trace_by_time = {(r[0], r[1], r[10]): r for r in traces}
        acks = {(int(m), int(rev)): int(frame) for m, rev, frame in re.findall(r"mix (\d+) ack=(\d+) frame=(\d+)", self.runtime_log.read_text())}
        overflows = [line for line in self.runtime_log.read_text().splitlines() if line.startswith("trace-overflow ") and not line.endswith(" 0")]
        if overflows:
            self.checks.append({"case": "trace-overflow", "status": "fail", "records": overflows})
        summary = []
        consumer_pcm = {}
        for i, (_, _, left, right) in enumerate(selections):
            raw = (self.evidence / f"{stage}-frames-{i}.bin").read_bytes()
            offset = 0
            received = [[], []]
            reference = [[], []]
            max_error = 0
            bad = 0
            jumps = []
            clocks = set()
            previous = None
            packet_errors = []
            metadata_errors = []
            while offset < len(raw):
                if offset + 36 > len(raw):
                    raise RuntimeError("truncated frame capture header")
                position, nsec, n, clock, num, denom, flags = struct.unpack_from("<QQIIIII", raw, offset)
                offset += 36
                if n != 256 or (num, denom) != (1, 48000) or offset + n * 8 > len(raw):
                    raise RuntimeError("unsupported/truncated frame packet; no conversion/alignment")
                samples = struct.unpack_from(f"<{n * 2}f", raw, offset)
                offset += n * 8
                clocks.add(clock)
                if previous is not None and position != previous + 256:
                    jumps.append({"previous": previous, "position": position})
                previous = position
                metadata_before = len(metadata_errors)
                if i >= 2:
                    for source in range(2):
                        trace = trace_by_time.get((source, i - 2, nsec))
                        if trace is None or trace[4] != position or trace[6] != n or (trace[8] and not (trace[8] == 2 and position < self.gate_first[source])):
                            metadata_errors.append({"position": position, "nsec": nsec, "source": source,
                                                    "trace": trace, "reason": "missing/mismatched source frame or queue fault"})
                peak_error = 0
                for f in range(n):
                    for c, tones in enumerate((left, right)):
                        # Predeclared same-clock DSP reference: zero integer-frame
                        # offset. No delay search, fitting, normalization or skipped
                        # leading/tail samples. A mismatch remains a failed gate.
                        if i < 2:
                            expected = sum(.08 * gain * math.sin(2 * math.pi * hz * ((position + f) % 480) / 48000)
                                           for hz, gain in tones)
                        else:
                            native = trace_by_time.get((0, i - 2, nsec))
                            gains = self.coefficient_history.get(native[9] if native else -1)
                            if gains is None:
                                raise RuntimeError("unknown revision in consumer clock reference")
                            expected = sum(.08 * gains[i - 2][source] * math.sin(2 * math.pi * hz * ((position + f) % 480) / 48000)
                                           for source, hz in ((0, 600), (1, 1100 if c == 0 else 1700))
                                           if position + f >= self.gate_first[source])
                        actual = samples[f * 2 + c]
                        error = abs(actual - expected)
                        max_error = max(max_error, error)
                        peak_error = max(peak_error, error)
                        bad += error > 1 / 32768
                        received[c].append(actual)
                        reference[c].append(expected)
                packet_errors.append({"position": position, "nsec": nsec, "clock_flags": flags, "max_error": peak_error})
                if (i >= 2 and peak_error <= 1 / 32768 and all(math.isfinite(v) for v in samples)
                        and len(metadata_errors) == metadata_before):
                    consumer_pcm.setdefault(i - 2, set()).add((position, nsec, n))
            qualify.write_pcm(self.evidence / f"{stage}-frames-{i}.wav", received)
            qualify.write_pcm(self.evidence / f"{stage}-frames-{i}-reference.wav", reference)
            comparison = qualify.compare(self.evidence / f"{stage}-frames-{i}-reference.wav", self.evidence / f"{stage}-frames-{i}.wav")
            passed = bool(received[0]) and not bad and not jumps and not metadata_errors and len(clocks) == 1
            item = {"case": f"continuous-{stage}-consumer-{i}", "status": "pass" if passed else "fail",
                    "endpoint": selections[i][0], "frames": len(received[0]), "capture_clock_ids": sorted(clocks),
                    "declared_alignment_frames": 0, "compared_all_samples": True,
                    "samples_outside_one_pcm16_lsb": bad, "max_float_error": max_error,
                    "clock_position_jumps": jumps, "queue_clock_mismatches": metadata_errors, "oracle": comparison,
                    "diagnosis": "frame/phase mismatch; correlate queue trace, not a fitted quality pass" if not passed else "same-clock frame reference matched"}
            self.checks.append(item)
            summary.append(item)
            (self.evidence / f"{stage}-frames-{i}.errors.json").write_text(json.dumps(packet_errors, indent=2))
        ack_check = qualify_revision_acks(traces, acks,
                                         self.commanded_revisions[len(pre_capture_revisions):],
                                         consumer_pcm, len(names), 2, pre_capture_revisions)
        self.checks.append({"case": f"{stage}-revision-frame-ack", **ack_check})
        (self.evidence / f"continuous-{stage}-analysis.json").write_text(json.dumps(summary, indent=2))

    def execute(self, names):
        version = self.compile()
        self.server()
        mic = self.producer("fixture.mic", 600, 600)
        self.producer("fixture.media", 1100, 1700)
        self.producer("fixture.private", 2300, 2300)
        self.producer("fixture.callreturn", 2900, 2900)
        self.runtime_start(names)
        self.graph("startup")
        for m in range(len(names)):
            self.command(f"set {m} 0 {1 if m == 0 else .5 if m == 1 else 0} 0", "prepared")
            self.command(f"set {m} 1 {0 if m == 0 else .25 if m == 1 else 1} 0", "prepared")
        self.command("bind", "barrier 1 validated")
        self.command("activate", "barrier 2 validated")
        time.sleep(.2)
        startup = [self.record(name, f"closed-{i}") for i, name in enumerate(names)]
        time.sleep(1)
        for p in startup:
            self.stop(p)
        for i in range(len(names)):
            self.analyze(f"closed-{i}", [{}, {}])
        graph = self.graph("staged-active-closed")
        for name in names:
            nodes = [o for o in graph if o["type"] == "PipeWire:Interface:Node" and
                     o.get("info", {}).get("props", {}).get("node.name") == name]
            if len(nodes) != 1 or nodes[0]["info"]["props"]["media.class"] != "Audio/Source":
                raise RuntimeError("exported Audio/Source identity missing")
        self.diagnose_frames(names, stage="startup", open_gate=True)
        self.graph("open")
        self.diagnose_frames(names, stage="edits", edit=True)
        captures = [self.record(name, f"baseline-{i}") for i, name in enumerate(names)]
        time.sleep(1.2)
        self.graph("consumers")
        for p in captures:
            self.stop(p)
        self.analyze("baseline-0", [{600: .08}, {600: .08}])
        for i in range(1, len(names)):
            self.analyze(f"baseline-{i}", [{600: .04, 1100: .02}, {600: .04, 1700: .02}] if i == 1 else [{1100: .08}, {1700: .08}])
        # Two hardware-shaped sinks are private null devices, never ALSA.
        # Consent here is an explicit fixture declaration, not production policy.
        routes = [{"target": "Private-speaker-A", "mix": names[-1], "mic_consent": True},
                  {"target": "Private-speaker-B", "mix": names[-1], "mic_consent": True}]
        (self.evidence / "private-output-consent.json").write_text(json.dumps({
            "evidence_only": True, "physical_hardware": False, "routes": routes,
            "zero_gain_mute_does_not_grant_consent": True}, indent=2))
        for state in ({"gain": 0, "mute": False}, {"gain": .5, "mute": True}):
            try:
                require_fixture_consent({"mic_consent": False, **state})
            except ValueError:
                pass
            else:
                raise RuntimeError("fixture consent waived by zero/mute")
        self.checks.append({"case": "private-fixture-consent-at-zero-mute", "status": "pass",
                            "production_policy_integration": False})
        for route in routes:
            require_fixture_consent(route)
            props = {"factory.name": "support.null-audio-sink", "node.name": route["target"],
                     "media.class": "Audio/Sink", "audio.position": "FL,FR", "node.virtual": True,
                     "object.linger": True, "node.pause-on-idle": False}
            r = self.run(["pw-cli", "create-node", "adapter", json.dumps(props)])
            if r.returncode:
                raise RuntimeError(r.stderr)
        time.sleep(.3)
        sink_graph = self.graph("dummy-outputs-created")
        sink_ids = {o["info"]["props"].get("node.name"): o["id"] for o in sink_graph
                    if o["type"] == "PipeWire:Interface:Node"}
        for route in routes:
            for channel in ("FL", "FR"):
                r = self.run(["pw-link", f"{names[-1]}:capture_{channel}", f"{route['target']}:playback_{channel}"])
                if r.returncode:
                    raise RuntimeError(r.stderr)
        self.graph("private-dummy-fanout")
        outputs = [self.record(route["target"], f"dummy-{i}", sink=True) for i, route in enumerate(routes)]
        time.sleep(1)
        self.graph("dummy-consumers")
        for child in outputs:
            self.stop(child)
        for i in range(len(routes)):
            self.analyze(f"dummy-{i}", [{600: .04, 1100: .02}, {600: .04, 1700: .02}] if len(names) == 2 else [{1100: .08}, {1700: .08}])
        # Delete only the exact private fixture nodes; the server is not restarted here.
        for route in routes:
            r = self.run(["pw-cli", "destroy", str(sink_ids[route["target"]])])
            if r.returncode:
                raise RuntimeError(r.stderr)
        self.graph("dummy-cleanup")

        # Multiple consumers of one published input constitute private fanout.
        fanout = [self.record(names[0], "fanout-a"), self.record(names[0], "fanout-b")]
        time.sleep(1)
        self.graph("fanout")
        for child in fanout:
            self.stop(child)
        for name in ("fanout-a", "fanout-b"):
            self.analyze(name, [{600: .08}, {600: .08}])

        # Record through gain/mute edits continuously; qualify steady intervals
        # independently. Full transition privacy and exact frame correlation
        # remain blocked until a capture-clock oracle exists.
        edits = [self.record(name, f"edits-{i}") for i, name in enumerate(names)]
        time.sleep(.8)
        self.command("set 0 0 .25 0", "prepared")
        time.sleep(.8)
        self.command("set 1 0 .5 1", "prepared")
        time.sleep(.8)
        self.command("set 1 0 .5 0", "prepared")
        time.sleep(.8)
        for child in edits:
            self.stop(child)
        self.analyze("edits-0", [{600: .02}, {600: .02}], offset=1.1)
        self.analyze("edits-1", [{1100: .02}, {1700: .02}], offset=2)
        self.analyze("edits-1", [{600: .04, 1100: .02}, {600: .04, 1700: .02}], offset=2.9)
        self.command("set 0 0 1 0", "prepared")

        siblings = self.record(names[0], "stall-sibling")
        self.command("stall 1 1", "stall prepared")
        time.sleep(1)
        self.command("status", "status")
        latest = self.runtime_log.read_text().rsplit("status revision=", 1)[-1]
        queues = re.findall(r"queue (\d+) (\d+) depth=(\d+) drops=(\d+)", latest)
        if not queues or any(int(depth) > 8 for _, _, depth, _ in queues):
            raise RuntimeError("unbounded or missing queue telemetry")
        if not all(int(drops) > 0 for _, mix, _, drops in queues if mix == "1"):
            raise RuntimeError("stalled egress did not report loss")
        self.stop(siblings)
        self.analyze("stall-sibling", [{600: .08}, {600: .08}])
        self.checks.append({"case": "bounded-logical-egress-stall", "status": "pass", "queues": queues,
                            "scheduling_isolation": "unproved; no RT callback was blocked"})
        self.command("stall 1 0", "stall prepared")
        time.sleep(.2)

        # Exclusive ports must reject concurrent fan-in before the first route
        # is removed. The attempt is owned by a separate private client.
        attempt = self.run(["pw-link", "fixture.private:out_FL", "guard.fixture.mic:in_FL"])
        (self.evidence / "second-link.txt").write_text(attempt.stdout + attempt.stderr)
        if attempt.returncode == 0:
            raise RuntimeError("exclusive ingress accepted a second link")
        self.checks.append({"case": "second-link", "status": "pass", "stderr": attempt.stderr.strip()})
        self.graph("second-link-rejected")

        self.stop(mic)
        self.producer("fixture.replacement-mic", 2300, 2300)
        time.sleep(.2)
        self.graph("source-replacement")
        missing = [self.record(name, f"missing-{i}") for i, name in enumerate(names)]
        time.sleep(1)
        for child in missing:
            self.stop(child)
        self.analyze("missing-0", [{}, {}])
        for i in range(1, len(names)):
            self.analyze(f"missing-{i}", [{1100: .02}, {1700: .02}] if i == 1 else [{1100: .08}, {1700: .08}])
        self.command("open", "barrier 3 blocked")
        self.command("quit")
        self.runtime.wait(timeout=5)
        cleanup = self.graph("runtime-cleanup")
        if any(o.get("info", {}).get("props", {}).get("node.name") in names + ["guard.fixture.mic", "guard.fixture.media"] for o in cleanup):
            raise RuntimeError("owned runtime node leaked after shutdown")
        self.checks.append({"case": "owned-runtime-cleanup", "status": "pass", "runtime_exit": self.runtime.returncode})
        # Disposable recovery creates a fresh closed runtime, never reuses an
        # ingress or searches the default microphone.
        self.producer("fixture.mic", 600, 600)
        self.runtime_start(names)
        for m in range(len(names)):
            self.command(f"set {m} 0 {1 if m == 0 else .5 if m == 1 else 0} 0", "prepared")
            self.command(f"set {m} 1 {0 if m == 0 else .25 if m == 1 else 1} 0", "prepared")
        self.command("bind", "barrier 1 validated")
        self.command("activate", "barrier 2 validated")
        time.sleep(.2)
        self.diagnose_frames(names, stage="recovery", open_gate=True)
        self.graph("recovery-open")
        recovery = self.record(names[0], "recovery")
        time.sleep(1)
        self.stop(recovery)
        self.analyze("recovery", [{600: .08}, {600: .08}])

        race = self.record(names[0], "hostile-relink")
        time.sleep(.3)
        self.command("hold-control", "holding control")
        before = self.graph("race-before")
        nodes = {o["info"]["props"].get("node.name"): o["id"] for o in before
                 if o["type"] == "PipeWire:Interface:Node"}
        owned = [o["id"] for o in before if o["type"] == "PipeWire:Interface:Link" and
                 o["info"]["input-node-id"] == nodes["guard.fixture.mic"]]
        if len(owned) != 2:
            raise RuntimeError("expected two owned ingress links")
        for identifier in owned:
            r = self.run(["pw-cli", "destroy", str(identifier)])
            if r.returncode:
                raise RuntimeError(r.stderr)
        for channel in ("FL", "FR"):
            r = self.run(["pw-link", f"fixture.private:out_{channel}", f"guard.fixture.mic:in_{channel}"])
            if r.returncode:
                raise RuntimeError(r.stderr)
        self.graph("race-relinked")
        time.sleep(2.2)
        self.stop(race)
        self.command("status", "status")
        # Never turn leaked samples into an expected-pass/xfail safety result.
        hostile = qualify.analyze(self.evidence / "hostile-relink.wav", offset=.6, duration=.5,
                                  targets=[{f: 0 for f in qualify.FREQUENCIES}] * 2)
        (self.evidence / "hostile-relink.analysis.json").write_text(json.dumps(hostile, indent=2))
        peaks = [channel["2300"][1] for channel in hostile["tone_peak_ranges"]]
        leak = max(peaks) > qualify.DEFAULT_LIMITS["leak_peak"]
        self.checks.append({"case": "hostile-relink-before-notification", "status": "fail" if leak else "blocked",
                            "private_tone_peaks": peaks, "oracle": hostile,
                            "reason": "unverified PCM admitted before public-client loss notification" if leak
                            else "not reproduced; unconditional public-client guarantee remains unproved"})
        self.command("quit")
        self.runtime.wait(timeout=5)
        self.runtime_start(names)
        for m in range(len(names)):
            self.command(f"set {m} 0 {1 if m == 0 else .5 if m == 1 else 0} 0", "prepared")
            self.command(f"set {m} 1 {0 if m == 0 else .25 if m == 1 else 1} 0", "prepared")
        self.command("bind", "barrier 1 validated")
        self.command("activate", "barrier 2 validated")
        time.sleep(.2)
        self.command("open", "barrier 3 validated")
        # Unlike holding the whole control loop (which can also stop buffer
        # negotiation), inject notification lag alone. This is explicitly a
        # simulated fault, not a claimed naturally reproduced SDK exploit.
        delayed = self.record(names[0], "delayed-observation")
        time.sleep(.3)
        self.command("delay-observer", "observer delay injected")
        snapshot = self.graph("delay-before")
        guard = next(o["id"] for o in snapshot if
                     o.get("info", {}).get("props", {}).get("node.name") == "guard.fixture.mic")
        selected_links = [o["id"] for o in snapshot if o["type"] == "PipeWire:Interface:Link" and
                          o["info"]["input-node-id"] == guard]
        if len(selected_links) != 2:
            raise RuntimeError("delayed observer requires exactly two owned input links")
        for identifier in selected_links:
            r = self.run(["pw-cli", "destroy", str(identifier)])
            if r.returncode:
                raise RuntimeError(r.stderr)
        for channel in ("FL", "FR"):
            r = self.run(["pw-link", f"fixture.private:out_{channel}", f"guard.fixture.mic:in_{channel}"])
            if r.returncode:
                raise RuntimeError(r.stderr)
        self.graph("delay-unowned-links")
        time.sleep(2.2)
        self.stop(delayed)
        self.command("status", "status")
        delayed_oracle = qualify.analyze(self.evidence / "delayed-observation.wav", offset=.7, duration=.5,
                                        targets=[{f: 0 for f in qualify.FREQUENCIES}] * 2)
        (self.evidence / "delayed-observation.analysis.json").write_text(json.dumps(delayed_oracle, indent=2))
        delayed_peaks = [c["2300"][1] for c in delayed_oracle["tone_peak_ranges"]]
        delayed_leak = max(delayed_peaks) > qualify.DEFAULT_LIMITS["leak_peak"]
        self.checks.append({"case": "injected-delayed-observation-relink", "status": "fail" if delayed_leak else "blocked",
                            "fault_injection": "registry loss invalidation deferred 2s; protocol and RT continue",
                            "natural_host_race_reproduced": False,
                            "private_tone_peaks": delayed_peaks, "oracle": delayed_oracle})
        return {"scheduling_mode": "triggered" if self.env.get("MIXYARD_TRIGGERED") == "1" else "automatic", "status": "fail" if leak or delayed_leak or any(c.get("status", c.get("oracle", {}).get("status")) == "fail" for c in self.checks) else "blocked", "sdk": version, "checks": self.checks,
                "identity_blocker": "exclusive ports prevent simultaneous fan-in, not replacement links before notification",
                "native_quality": "unqualified"}


def timing_diagnosis(evidence):
    """Classify all recorded timing evidence; never excuse an oracle failure."""
    producers = []
    for log in evidence.glob("producer*.log"):
        rows = [list(map(int, line.split()[1:])) for line in log.read_text().splitlines() if line.startswith("producer-frame ")]
        if not rows:
            continue
        producers.append({"log": log.name, "callbacks": len(rows),
                          "position_jumps": sum(b[0] != a[0] + a[3] for a, b in zip(rows, rows[1:])),
                          "max_callback_delay_ns": max(r[2] - r[1] for r in rows),
                          "callbacks_later_than_one_quantum": sum(r[2] - r[1] > r[3] * 1e9 / 48000 for r in rows),
                          "overflow": any(line.startswith("producer-trace-overflow ") and not line.endswith(" 0") for line in log.read_text().splitlines())})
    queues = []
    for log in evidence.glob("runtime-*.log"):
        unique = {}
        for line in log.read_text().splitlines():
            if line.startswith("trace "):
                row = list(map(int, line.split()[1:]))
                unique[tuple(row[:3])] = row
        for source, mix in sorted({tuple(row[:2]) for row in unique.values()}):
            rows = sorted((r for r in unique.values() if r[:2] == [source, mix]), key=lambda r: r[2])
            queues.append({"log": log.name, "source": source, "mix": mix, "callbacks": len(rows),
                           "underrun_callbacks": sum(bool(r[8] & 1) for r in rows),
                           "invalid_generation_callbacks": sum(bool(r[8] & 2) for r in rows),
                           "intentional_stall_callbacks": sum(bool(r[8] & 4) for r in rows),
                           "late_discard_callbacks": sum(bool(r[8] & 8) for r in rows),
                           "valid_frames_not_matching_ready_position": sum(r[8] == 0 and r[4] != r[11] for r in rows),
                           "max_queue_depth": max(r[7] for r in rows)})
    result = {"producer_timing": producers, "output_queue_timing": queues,
              "interpretation": "Packet reference mismatches, missing clock quanta, producer deadline misses, queue underruns/discards and probe loss remain separate failures. Same dummy clock is not physical-clock qualification."}
    (evidence / "timing-diagnosis.json").write_text(json.dumps(result, indent=2))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--mixes", nargs="+", default=["Call privacy", "Archive α", "Headphones private"])
    parser.add_argument("--automatic", action="store_true", help="retain original independent scheduling failure for diagnosis")
    args = parser.parse_args()
    if not Path("/.dockerenv").exists() or os.geteuid() == 0 or Path("/dev/snd").exists():
        parser.error("requires unprivileged isolated proof container without /dev/snd")
    if len(args.mixes) < 2 or len(set(args.mixes)) != len(args.mixes):
        parser.error("proof needs at least two distinct selectable mixes")
    p = Probe(args.evidence)
    p.env["MIXYARD_TRIGGERED"] = "0" if args.automatic else "1"
    result = {"status": "blocked"}
    try:
        (args.evidence / "resources-before.json").write_text(json.dumps(resource_snapshot(), indent=2))
        (args.evidence / "load-before.txt").write_text(Path("/proc/loadavg").read_text())
        result = p.execute(args.mixes)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        result = {"status": "fail", "reason": str(exc), "checks": p.checks}
    finally:
        for child in reversed(p.processes):
            p.stop(child)
        diagnosis = timing_diagnosis(args.evidence)
        if any(p["overflow"] for p in diagnosis["producer_timing"]):
            result["status"] = "fail"
            result["producer_trace_overflow"] = True
        (args.evidence / "resources-after.json").write_text(json.dumps(resource_snapshot(), indent=2))
        (args.evidence / "load-after.txt").write_text(Path("/proc/loadavg").read_text())
        result.update(evidence_kind="native-private-container-graph-and-external-consumer-pcm",
                      production_apply=False, host_identity_safety=False, physical_quality=False, process_exits=p.exits)
        result["artifacts"] = {f.name: hashlib.sha256(f.read_bytes()).hexdigest()
                               for f in args.evidence.iterdir() if f.is_file() and f.name != "report.json"}
        (args.evidence / "report.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k not in ("checks", "artifacts", "process_exits")}, indent=2))
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())

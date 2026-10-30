#!/usr/bin/env python3
"""Explicit Boat experiment: real canonical host -> separate standard recorders.

This does not activate the product runtime or qualify a system SDK. No GitHub
environment/provenance is manufactured. The source generator remains a fixture.
"""
import argparse
import ast
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import socket
import stat
import struct
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
NATIVE = ROOT / "adapters/linux/native-host"
sys.path.insert(0, str(NATIVE))
import comparison_sdk as sdk
from request import request
from process_capture import Capture
from qualify import isolation, parse_report, snapshot_bytes, verify_retirement

SOURCE = "5b3399a63468bdcd1cb0dbc4b40d0ecc5f14fc24"
WORK = Path("/home/user/mixyard-run")
NAMES = ("call", "record", "monitor")


def host(sandbox_id):
    if (not sandbox_id.startswith("bx_") or not socket.gethostname().startswith("box-node-")
            or os.getuid() != 1000):
        raise RuntimeError("requires explicitly selected disposable Boat user host")
    if subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip() != SOURCE:
        raise RuntimeError("experiment must start from exact frozen 5b source")
    # A conservative forecast for the small SDK/Rust build, retaining 10 GiB.
    space = os.statvfs(WORK.parent)
    if space.f_bavail * space.f_frsize < 15 * 1024**3:
        raise RuntimeError("insufficient cloud filesystem reserve")


def environment(prefix, directory):
    # No inherited PipeWire/default-session or GitHub state.
    return {"PATH": str(prefix / "bin") + ":/usr/bin:/bin",
            "HOME": str(directory / "home"), "XDG_RUNTIME_DIR": str(directory / "runtime"),
            "PIPEWIRE_RUNTIME_DIR": str(directory / "runtime"), "PIPEWIRE_REMOTE": "mixyard-private",
            "PIPEWIRE_CONFIG_DIR": str(directory / "config"), "PIPEWIRE_CONFIG_NAME": "client.conf",
            "XDG_CONFIG_HOME": str(directory / "config"), "XDG_STATE_HOME": str(directory / "home"),
            "LD_LIBRARY_PATH": str(prefix / "lib"),
            "PIPEWIRE_MODULE_DIR": str(prefix / "lib/pipewire-0.3"),
            "SPA_PLUGIN_DIR": str(prefix / "lib/spa-0.2"),
            "MIXYARD_PRIVATE_GRAPH": "hosted-owned-only"}


def prepare(sandbox_id):
    host(sandbox_id)
    WORK.mkdir(mode=0o700)
    archive = WORK / "source.tar.bz2"
    with urllib.request.urlopen(sdk.ARCHIVE_URL, timeout=30) as incoming, archive.open("xb") as out:
        size = 0
        while chunk := incoming.read(65536):
            size += len(chunk)
            if size > sdk.ARCHIVE_SIZE:
                raise RuntimeError("source archive exceeds pinned size")
            out.write(chunk)
    source = sdk.extract_verified(archive, WORK)  # Size/hash/critical blobs before build.
    sdk.write_json(WORK / "sdk-source-inputs.json", {"source": sdk.SOURCE, **sdk.manifest(source)})
    tree = ast.parse((NATIVE / "comparison_sdk.py").read_text())
    function = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "prepare")
    options = ast.literal_eval(next(n.value for n in function.body if isinstance(n, ast.Assign)
                                   and any(isinstance(t, ast.Name) and t.id == "options" for t in n.targets)))
    # Standard pw-record requires libsndfile; this is a declared experimental
    # build delta, not a change to the 5b comparison or graph's F64 contract.
    options += ["-Dsndfile=enabled", "-Dpw-cat=enabled"]
    prefix, build = WORK / "prefix", WORK / "sdk-build"
    for label, command in (
        ("meson", ["meson", "setup", str(build), str(source), "--prefix=" + str(prefix), *options]),
        ("sdk-compile", ["meson", "compile", "-C", str(build), "-j2"]),
        ("sdk-install", ["meson", "install", "-C", str(build), "--no-rebuild"]),
    ):
        with (WORK / (label + ".log")).open("xb") as log:
            subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True)
    sdk.write_json(WORK / "prefix-manifest.json", sdk.manifest(prefix))
    env = dict(os.environ, CARGO_BUILD_JOBS="1", CARGO_INCREMENTAL="0",
               CARGO_TARGET_DIR=str(WORK / "rust"), CARGO_PROFILE_RELEASE_DEBUG="0")
    with (WORK / "rust-build.log").open("xb") as log:
        subprocess.run(["cargo", "build", "--locked", "--release", "--manifest-path",
                        str(ROOT / "adapters/engine/Cargo.toml"), "-j1"], env=env,
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    tracked = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
               for name in subprocess.check_output(["git", "-C", str(ROOT), "ls-tree", "-r", "--name-only", "HEAD"], text=True).splitlines()
               if (ROOT / name).is_file() and not (ROOT / name).is_symlink()}
    sdk.write_json(WORK / "source-manifest.json", tracked)
    sdk.write_json(WORK / "boat-descriptor.json", {
        "schema": 1, "provider": "Boat", "sandboxId": sandbox_id, "ttlSeconds": 1800,
        "scope": "experimental-owned-private-recording-not-GitHub-qualification",
        "sourceHead": SOURCE, "sdkSource": sdk.SOURCE, "buildOptions": options,
        "sourceManifestSha256": sdk.digest(WORK / "source-manifest.json"),
        "prefixManifestSha256": sdk.digest(WORK / "prefix-manifest.json"),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "hostLibrarySha256": sdk.digest(WORK / "rust/release/libmixyard_host.so"),
        "platform": subprocess.check_output(["uname", "-a"], text=True).strip(),
        "runtimeVersion": subprocess.check_output([str(prefix / "bin/pipewire"), "--version"],
                                                   env=dict(os.environ, LD_LIBRARY_PATH=str(prefix / "lib")), text=True).strip()})


def build_graph(prefix):
    # Unmodified 5b native code and canonical host; no GitHub guard is spoofed.
    env = dict(os.environ, PKG_CONFIG_PATH=str(prefix / "lib/pkgconfig"))
    flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "libpipewire-0.3", "json-c"], env=env, text=True).split()
    subprocess.run(["cc", "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O2", "-g0",
                    "-I" + str(ROOT / "adapters/engine/include"), str(NATIVE / "graph.c"),
                    str(WORK / "rust/release/libmixyard_host.a"),
                    "-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free,--wrap=posix_memalign,--wrap=aligned_alloc",
                    *flags, "-lpthread", "-ldl", "-lm", "-lrt", "-lutil", "-o", str(WORK / "native-graph")], check=True)

    subprocess.run(["cc", "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O2", "-g0",
                    str(Path(__file__).with_name("registry_snapshot.c")), *flags,
                    "-o", str(WORK / "registry-snapshot")], check=True)


def snapshot(prefix, env):
    return json.loads(snapshot_bytes([str(WORK / "registry-snapshot")], env))


def private_with_recorders(objects):
    # Separate experimental topology, not a relaxation of 5b's four-node gate.
    expected = {"mixyard-private-driver", "mixyard-private-generator", "mixyard-private-engine",
                "mixyard-private-consumer", *("mixyard-private-standard-" + n for n in NAMES)}
    nodes = {o["id"]: o.get("info", {}).get("props", {}) for o in objects if o["type"] == "PipeWire:Interface:Node"}
    if set(p.get("node.name") for p in nodes.values()) != expected or len(nodes) != 7:
        raise RuntimeError("unexpected/unowned node in standard consumer graph")
    if any("device.api" in p or "api.alsa.path" in p for p in nodes.values()) or any(o["type"] == "PipeWire:Interface:Device" for o in objects):
        raise RuntimeError("physical device on experimental graph")
    # Reuse the strict server module/core/client checks on the initial projection.
    initial = [o for o in objects if o["type"] not in ("PipeWire:Interface:Port", "PipeWire:Interface:Link", "PipeWire:Interface:Node")
               or o["id"] in nodes and nodes[o["id"]].get("node.name") == "mixyard-private-driver"]
    isolation(initial)
    names = {p["node.name"]: i for i, p in nodes.items()}
    allowed = {(names["mixyard-private-generator"], names["mixyard-private-engine"]),
               (names["mixyard-private-engine"], names["mixyard-private-consumer"])}
    allowed |= {(names["mixyard-private-engine"], names["mixyard-private-standard-" + n]) for n in NAMES}
    links = [o for o in objects if o["type"] == "PipeWire:Interface:Link"]
    if len(links) != 9 or any((o["info"]["output-node-id"], o["info"]["input-node-id"]) not in allowed for o in links):
        raise RuntimeError("unexpected standard consumer links")
    if len({o["info"]["input-port-id"] for o in links}) != 9:
        raise RuntimeError("duplicate destination/recapture")
    return {"nodes": nodes, "links": links}


def pcm_oracle(path, route):
    raw = sdk.read_bounded(path, 8 * 1024**2)
    if not raw:
        raise RuntimeError("invalid bounded F32 stereo recording")
    if len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise RuntimeError("recording is not an actual WAV file")
    if struct.unpack_from("<I", raw, 4)[0] + 8 != len(raw):
        raise RuntimeError("incomplete or inconsistent RIFF extent")
    offset, chunks = 12, {}
    while offset < len(raw):
        if len(raw) - offset < 8:
            raise RuntimeError("incomplete WAV chunk header")
        kind, size = struct.unpack_from("<4sI", raw, offset); offset += 8
        if size + size % 2 > len(raw) - offset or len(chunks) >= 32:
            raise RuntimeError("invalid WAV chunks")
        if kind in chunks: raise RuntimeError("duplicate WAV chunk")
        chunks[kind] = raw[offset:offset+size]; offset += size + size % 2
    fmt, payload = chunks[b"fmt "], chunks[b"data"]
    code, channels, rate, byte_rate, align, bits = struct.unpack_from("<HHIIHH", fmt)
    if code == 0xfffe and len(fmt) >= 40: code = struct.unpack_from("<I", fmt, 24)[0]
    if (code, channels, rate, byte_rate, align, bits) != (3, 2, 48000, 384000, 8, 32):
        raise RuntimeError("WAV is not negotiated IEEE Float32/stereo/48000 output")
    if len(payload) % 8: raise RuntimeError("partial recorded frame")
    frames = list(struct.iter_unpack("<ff", payload))
    ordinary = ((0.25, -0.125), (0.125, 0.015625), (0.125, -0.0625))[route]
    marker = ((0.0, 0.0), (0.0625, 0.046875), (0.0, 0.0))[route]
    allowed = {(0.0, 0.0), ordinary, marker}
    if any(not all(math.isfinite(v) for v in frame) or frame not in allowed for frame in frames):
        raise RuntimeError("recorded PCM differs from canonical destination oracle")
    count = frames.count(ordinary)
    if count < 4096 or frames[-1024:] != [(0.0, 0.0)] * 1024:
        raise RuntimeError("insufficient mixed PCM or producer-loss silence")
    if route == 1 and frames.count(marker) < 64:
        raise RuntimeError("missing return-only record signature")
    return {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw), "pcmSha256": hashlib.sha256(payload).hexdigest(), "frames": len(frames),
            "encoding": "f32-le", "channels": ["FL", "FR"], "sampleRateHz": 48000,
            "ordinaryFrame": ordinary, "ordinaryFrames": count,
            "markerFrame": marker, "markerFrames": frames.count(marker), "tailZeroFrames": 1024,
            "scope": "file offsets are not native clock/sequence evidence"}


def sdk_binding(process, role, prefix):
    """Own live Popen only; bounded off-RT point observation, no lifetime claim."""
    before = time.monotonic_ns()
    if process.poll() is not None: raise RuntimeError("binding process already exited")
    raw = sdk.read_bounded(Path("/proc") / str(process.pid) / "maps", 1024 * 1024)
    if not raw.endswith(b"\n"): raise RuntimeError("incomplete binding snapshot")
    files = json.loads((WORK / "prefix-manifest.json").read_bytes())["files"]
    objects = {}
    for line in raw.decode("utf-8", errors="strict").splitlines():
        fields = line.split(maxsplit=5)
        if len(fields) < 6 or not fields[5].startswith("/"): continue
        path = Path(fields[5])
        if not (path.is_relative_to(prefix) or path.name.startswith(("libpipewire", "libspa-"))): continue
        if not path.resolve(strict=True).is_relative_to(prefix): raise RuntimeError("SDK fallback outside prefix")
        name = path.resolve(strict=True).relative_to(prefix).as_posix()
        item = files.get(name)
        if not item or not item["buildId"] or sdk.digest(path) != item["sha256"] or sdk.build_id(path) != item["buildId"]:
            raise RuntimeError("loaded SDK identity mismatch")
        objects[name] = {"sha256": item["sha256"], "buildId": item["buildId"]}
        if len(objects) > 64: raise RuntimeError("loaded SDK object bound")
    required = ["lib/libpipewire-0.3.so.0", "lib/spa-0.2/support/libspa-support.so"]
    if role == "daemon":
        from qualify import MODULES
        required += ["lib/pipewire-0.3/" + name + ".so" for name in MODULES]
    elif role == "graph": required += ["lib/spa-0.2/audiomixer/libspa-audiomixer.so"]
    else: required += ["lib/pipewire-0.3/libpipewire-module-adapter.so", "lib/spa-0.2/audioconvert/libspa-audioconvert.so"]
    if any((prefix / name).resolve().relative_to(prefix).as_posix() not in objects for name in required):
        raise RuntimeError("required role binding not observed")
    if process.poll() is not None: raise RuntimeError("process exited during binding observation")
    return {"scope": "declared pre-admission point only, not lifetime closure", "pid": process.pid,
            "beforeMonotonicNs": before, "afterMonotonicNs": time.monotonic_ns(), "objects": objects}


def start_capture(name, command, env):
    """Graph reports get the existing client cap; other runtime output is diagnostic."""
    return Capture(command, env, name, "client" if name == "graph" else "daemon")


def collect_diagnostics(captures, directory, report):
    """Persist only live-bounded buffers after owned readers finish, never raw files."""
    diagnostics = {}
    def reject(reason):
        report.setdefault("diagnosticFailures", []).append(reason)
        if report.get("result") != "failure":
            report["result"], report["failure"] = "failure", reason
    for capture in captures:
        try:
            capture.finish()
        except Exception:
            reject("diagnostic capture did not finish")
            diagnostics[capture.name] = {"finished": False, "captureProblem": "incomplete"}
            continue
        if capture.problem:
            reject("owned diagnostic capture failed")
        for channel in ("stdout", "stderr"):
            raw = bytes(capture.buffers[channel])
            limit = capture.limits[channel]
            if len(raw) > limit + 1:
                raise RuntimeError("bounded diagnostic capture invariant failed")
            if channel == "stderr" and raw:
                reject("unexpected " + capture.name + " diagnostic")
            # A single overflow witness byte is retained, never an unbounded log.
            descriptor = os.open(directory / (capture.name + "." + channel),
                                 os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as file:
                file.write(raw)
            diagnostics[capture.name + "." + channel] = {
                "bytes": len(raw), "limit": limit, "captureProblem": capture.problem,
                "finished": capture.finished, "sha256": hashlib.sha256(raw).hexdigest(),
                "prefix": raw[:2048].decode("utf-8", errors="replace")}
    return diagnostics


def run_recording(sandbox_id, attempt):
    host(sandbox_id)
    descriptor = json.loads((WORK / "boat-descriptor.json").read_bytes())
    if descriptor["provider"] != "Boat" or descriptor["sandboxId"] != sandbox_id or descriptor["sourceHead"] != SOURCE or descriptor["sdkSource"] != sdk.SOURCE:
        raise RuntimeError("explicit Boat descriptor mismatch")
    prefix = WORK / "prefix"
    if sdk.digest(WORK / "prefix-manifest.json") != descriptor["prefixManifestSha256"] or sdk.manifest(prefix) != json.loads((WORK / "prefix-manifest.json").read_bytes()):
        raise RuntimeError("private SDK changed")
    if sdk.digest(WORK / "rust/release/libmixyard_host.so") != descriptor["hostLibrarySha256"]:
        raise RuntimeError("canonical host changed")
    directory = WORK / ("ordinary-" + str(attempt)); directory.mkdir(mode=0o700)
    for name in ("home", "config", "runtime", "graph"):
        (directory / name).mkdir(mode=0o700)
    (directory / "config/private.conf").write_bytes((NATIVE / "private.conf").read_bytes())
    (directory / "config/client.conf").write_text("""context.properties = { support.dbus = false mem.warn-mlock = false }
context.spa-libs = { support.* = support/libspa-support audio.convert.* = audioconvert/libspa-audioconvert }
context.modules = [
 { name = libpipewire-module-protocol-native }
 { name = libpipewire-module-client-node }
 { name = libpipewire-module-metadata }
 { name = libpipewire-module-adapter }
]
""")
    env = environment(prefix, directory)
    owned, captures = [], []
    report = {"provider": "Boat", "sandboxId": sandbox_id, "sourceHead": SOURCE,
              "experimentSha256": sdk.digest(Path(__file__)), "claims": {"productRuntimeAudio": False,
              "physicalDevices": False, "nativeApps": False, "hardwareClock": False, "backendAvailable": False}}

    def start(name, command, extra=None):
        capture = start_capture(name, command, extra or env)
        captures.append(capture)
        owned.append((name, capture.process))
        return capture.process

    def await_condition(condition, seconds=10):
        end = time.monotonic() + seconds
        while not condition():
            if any(c.problem for c in captures):
                raise RuntimeError("owned diagnostic capture failed")
            failed = [(n, p.returncode) for n, p in owned if p.poll() is not None]
            if failed: raise RuntimeError("owned process exited before admission: " + repr(failed))
            if time.monotonic() >= end: raise RuntimeError("bounded standard consumer admission timeout")
            time.sleep(0.02)

    try:
        build_graph(prefix)
        daemon = start("daemon", [str(prefix / "bin/pipewire"), "-c", str(directory / "config/private.conf")],
                       dict(env, PIPEWIRE_CONFIG_NAME="private.conf"))
        await_condition(lambda: (directory / "runtime/mixyard-private").exists())
        sock = (directory / "runtime/mixyard-private").stat()
        if not stat.S_ISSOCK(sock.st_mode) or sock.st_uid != os.getuid(): raise RuntimeError("private socket ownership")
        report["initialIsolation"] = isolation(snapshot(prefix, env))
        graph = start("graph", [str(WORK / "native-graph"), str(directory / "graph"), "normal"])
        await_condition(lambda: (directory / "graph/topology.json").exists())
        for name in NAMES:
            properties = "{ node.name = mixyard-private-standard-" + name + " node.group = mixyard-private node.autoconnect = false node.dont-reconnect = true node.passive = true adapter.auto-port-config = { mode = convert position = preserve monitor = false } channelmix.disable = true resample.disable = true }"
            start(name, [str(prefix / "bin/pw-record"), "--remote", "mixyard-private", "--target", "0",
                         "--rate", "48000", "--channels", "2", "--channel-map", "FL,FR", "--format", "f32",
                         "--latency", "128", "--properties", properties, str(directory / (name + ".wav"))])
        objects = []
        def recorders_ready():
            nonlocal objects
            objects = snapshot(prefix, env)
            return len([o for o in objects if o["type"] == "PipeWire:Interface:Port"
                        and o.get("info", {}).get("props", {}).get("port.direction") == "in"]) >= 9
        await_condition(recorders_ready)
        nodes = {o["info"]["props"].get("node.name"): o["id"] for o in objects if o["type"] == "PipeWire:Interface:Node"}
        ports = [o for o in objects if o["type"] == "PipeWire:Interface:Port"]
        report["recorderProcesses"] = {n: p.pid for n, p in owned if n in NAMES}
        for index, name in enumerate(NAMES):
            source = [o for o in ports if int(o["info"]["props"].get("node.id", -1)) == nodes["mixyard-private-engine"] and o["info"]["props"].get("port.name") == "out-" + str(index)]
            sink = [o for o in ports if int(o["info"]["props"].get("node.id", -1)) == nodes["mixyard-private-standard-" + name] and o["info"]["props"].get("port.direction") == "in"]
            if len(source) != 1 or len(sink) != 1: raise RuntimeError("ambiguous recorder port")
            start("link-" + name, [str(prefix / "bin/pw-link"), "--remote", "mixyard-private", "--monitor", "--passive", str(source[0]["id"]), str(sink[0]["id"])])
        def links_ready():
            nonlocal objects
            objects = snapshot(prefix, env)
            return len([o for o in objects if o["type"] == "PipeWire:Interface:Link"]) == 9
        await_condition(links_ready)
        report["activeRegistry"] = objects
        report["activeTopology"] = private_with_recorders(objects)
        negotiated = [o for o in objects if o["type"] == "PipeWire:Interface:Port"
                      and int(o["info"]["props"].get("node.id", -1)) in
                      {nodes["mixyard-private-engine"], *(nodes["mixyard-private-standard-" + n] for n in NAMES)}]
        expected_format = {"format": 285, "rate": 48000, "channels": 2, "positions": [3, 4]}
        if len(negotiated) != 9 or any(o["info"].get("negotiatedFormat") != expected_format for o in negotiated):
            raise RuntimeError("actual engine/recorder public-port F64/48000/FLFR format missing or different")
        report["nativeNegotiatedFormats"] = negotiated
        report["conversion"] = {"input": expected_format, "output": "WAV IEEE Float32/stereo/48000 verified from actual header",
                                "owner": "each separate pw-record client-local adapter", "lowBitPreservation": False}
        report["sdkBindings"] = {name: sdk_binding(p, name if name in ("daemon", "graph") else "recorder", prefix)
                                 for name, p in owned if name in ("daemon", "graph", *NAMES)}
        report["staticHostArchiveSha256"] = sdk.digest(WORK / "rust/release/libmixyard_host.a")
        report["nativeGraphSha256"] = sdk.digest(WORK / "native-graph")
        topology = json.loads((directory / "graph/topology.json").read_bytes())
        (directory / "graph/request.new").write_text(json.dumps(request(topology)))
        (directory / "graph/request.new").rename(directory / "graph/request.json")
        report["admittedAtMonotonicNs"] = time.monotonic_ns()
        graph.wait(timeout=35)
        if graph.returncode: raise RuntimeError("native graph exit " + str(graph.returncode))
        report["recorderStops"] = {}
        for name, p in owned:
            if name in NAMES:
                if p.poll() is not None: raise RuntimeError("recorder exited before owned stop request")
                before = time.monotonic_ns(); p.send_signal(signal.SIGINT)
                report["recorderStops"][name] = {"pid": p.pid, "requestedSignal": int(signal.SIGINT),
                    "aliveBeforeRequest": True, "beforeSignalNs": before, "signalReturnedNs": time.monotonic_ns()}
        for name, p in owned:
            if name in NAMES:
                p.wait(timeout=5)
                report["recorderStops"][name].update(returncode=p.returncode, reapedAtMonotonicNs=time.monotonic_ns())
        graph_capture = next(c for c in captures if c.name == "graph")
        graph_capture.finish()
        graph_stdout, graph_stderr = graph_capture.decoded()
        if graph_stderr:
            raise RuntimeError("unexpected graph diagnostic")
        native = parse_report(graph_stdout)
        verify_retirement(native)
        if native["error"] or native["sampleMismatches"] or not native["sourceStopBeforeDestroy"] or not native["sourceRemovalObserved"]:
            raise RuntimeError("native source/mixer/loss oracle failed")
        report["nativeGraph"] = native
        report["recordings"] = {name: pcm_oracle(directory / (name + ".wav"), i) for i, name in enumerate(NAMES)}
        report["sampleVerification"] = "passed"
        exits = {n: p.returncode for n, p in owned if n in NAMES and p.returncode}
        if exits:
            # Preserve strict failure. Pinned pw-cat returns success only when
            # drained; SIGINT recording stop otherwise exits1. No generic waiver.
            report["result"], report["failure"] = "failure", "recordings verified, recorder commands returned nonzero: " + repr(exits)
        else: report["result"] = "success"
    except BaseException as error:
        report["result"], report["failure"] = "failure", type(error).__name__ + ": " + str(error)
        raise
    finally:
        for name, p in reversed(owned):
            if p.poll() is None: p.send_signal(signal.SIGTERM)
        for name, p in reversed(owned):
            try: p.wait(timeout=3)
            except subprocess.TimeoutExpired: p.kill(); p.wait(timeout=2)
        report["cleanup"] = [{"name": n, "pid": p.pid, "returncode": p.returncode, "reaped": p.poll() is not None} for n, p in owned]
        report["diagnostics"] = collect_diagnostics(captures, directory, report)
        sdk.write_json(directory / "report.json", report)
        print(json.dumps({"result": report["result"], "report": str(directory / "report.json"), "failure": report.get("failure"), "recordings": report.get("recordings")}), flush=True)
    if report["result"] != "success": raise RuntimeError(report["failure"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("prepare", "run"))
    parser.add_argument("--sandbox-id", required=True)
    parser.add_argument("--attempt", type=int, default=1)
    args = parser.parse_args()
    if args.command == "prepare": prepare(args.sandbox_id)
    else: run_recording(args.sandbox_id, args.attempt)

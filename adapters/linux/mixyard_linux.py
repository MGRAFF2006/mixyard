#!/usr/bin/env python3
"""PipeWire CLI prototype. No audio callbacks and no third-party Python modules."""

import argparse
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import sys

PREFIX = "mixyard."
TAG = "mixyard.owner"
MUTATION_UNAVAILABLE = (
    "external graph mutation unavailable: pw-* CLI linking cannot enforce observed "
    "node/port identities before audio flows, and command exit status does not "
    "acknowledge native gain/mute. Use snapshot, health, or offline plan; no override."
)


def props(obj):
    return obj.get("info", {}).get("props", {})


def kind(obj, suffix):
    return obj.get("type", "").endswith(":" + suffix)


def checked_keys(value, allowed, label):
    if not isinstance(value, dict) or set(value) - set(allowed):
        raise ValueError(f"{label}: expected object with keys {', '.join(allowed)}")


def gain(value, label, master=False):
    try:
        finite = isinstance(value, (int, float)) and math.isfinite(value)
    except OverflowError:
        finite = False
    if (isinstance(value, bool) or not finite or value < 0 or (master and value > 1)):
        raise ValueError(f"{label}: expected finite {'[0,1]' if master else 'nonnegative'} gain")
    return value


def selector(value):
    checked_keys(value, ("node.name", "device.serial", "application.id"), "selector")
    if not value.get("node.name") or any(not isinstance(v, str) or not v for v in value.values()):
        raise ValueError("selector requires exact nonempty node.name and string properties")
    if value["node.name"].startswith(PREFIX):
        raise ValueError("Mixyard output/producer recapture is forbidden")
    return value


def identifier(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"[a-z0-9_-]{1,40}", value):
        raise ValueError(f"{label} must match [a-z0-9_-]{{1,40}}")
    return value


def layout(value):
    if value not in (["MONO"], ["FL", "FR"]):
        raise ValueError("only MONO or FL/FR layouts are supported")
    return value


def peak(config, mix):
    return mix["master"] * sum(s["gains"].get(mix["id"], 0)
                               for s in config["sources"] if not s.get("mute", False))


def validate(config):
    checked_keys(config, ("instance", "sources", "mixes", "outputs"), "config")
    identifier(config.get("instance"), "instance")
    for key in ("sources", "mixes", "outputs"):
        if not isinstance(config.get(key), list):
            raise ValueError(f"{key} must be an array")
    mix_ids, source_ids, output_ids, selectors = set(), set(), set(), set()
    roles = {"mic", "application", "call-return"}
    for mix in config["mixes"]:
        checked_keys(mix, ("id", "name", "channels", "master", "virtualInput", "excludeRoles", "excludeSources"), "mix")
        mid = identifier(mix.get("id"), "mix ID")
        if mid in mix_ids:
            raise ValueError("duplicate mix ID")
        mix_ids.add(mid)
        if not isinstance(mix.get("name"), str) or not mix["name"].strip():
            raise ValueError("mix requires a nonempty display name")
        layout(mix.get("channels"))
        gain(mix.get("master"), f"{mid} master", master=True)
        if not isinstance(mix.get("virtualInput"), bool):
            raise ValueError("mix.virtualInput must be boolean")
        if not isinstance(mix.get("excludeRoles"), list) or any(r not in roles for r in mix["excludeRoles"]):
            raise ValueError("each mix requires explicit excludeRoles from mic/application/call-return")
        if not isinstance(mix.get("excludeSources", []), list):
            raise ValueError("excludeSources must be an array of source IDs")
    for source in config["sources"]:
        checked_keys(source, ("id", "selector", "role", "channels", "gains", "mute"), "source")
        sid = identifier(source.get("id"), "source ID")
        if sid in source_ids:
            raise ValueError("duplicate source ID")
        source_ids.add(sid)
        selected = json.dumps(selector(source.get("selector")), sort_keys=True)
        if selected in selectors:
            raise ValueError("duplicate source selector")
        selectors.add(selected)
        if source.get("role") not in roles:
            raise ValueError("source role must be mic, application or call-return")
        layout(source.get("channels"))
        checked_keys(source.get("gains"), mix_ids, "source gains")
        for mid, value in source["gains"].items():
            gain(value, f"{sid}/{mid}")
        if not isinstance(source.get("mute", False), bool):
            raise ValueError("source mute must be boolean")
        for mix in config["mixes"]:
            if (source["role"] in mix["excludeRoles"] or sid in mix.get("excludeSources", [])) and source["gains"].get(mix["id"], 0):
                raise ValueError(f"excluded source {sid} cannot enter {mix['id']}, including while muted")
    # One native stream uses SPA's channel layout; additional mixes are not count-limited.
    if sum(len(s["channels"]) for s in config["sources"]) > 64:
        raise ValueError("prototype supports at most 64 source channels per native capture stream")
    for mix in config["mixes"]:
        if any(sid not in source_ids for sid in mix.get("excludeSources", [])):
            raise ValueError("excludeSources references an unknown source")
        if peak(config, mix) > 1:
            raise ValueError(f"{mix['id']}: conservative peak bound exceeds 1; lower sends/master")
    sink_selectors = set()
    for output in config["outputs"]:
        checked_keys(output, ("id", "mix", "selector", "channels", "gain", "enabled"), "output")
        oid = identifier(output.get("id"), "output ID")
        if oid in output_ids:
            raise ValueError("duplicate output ID")
        output_ids.add(oid)
        if output.get("mix") not in mix_ids:
            raise ValueError("output references an unknown mix")
        selected = json.dumps(selector(output.get("selector")), sort_keys=True)
        layout(output.get("channels"))
        gain(output.get("gain"), f"{oid} output", master=True)
        if not isinstance(output.get("enabled", False), bool):
            raise ValueError("output.enabled must be boolean")
        if output.get("enabled", False):
            if selected in sink_selectors:
                raise ValueError("multiple enabled outputs select the same configured sink")
            sink_selectors.add(selected)
    return config


def matches(objects, selected, media):
    devices = {str(o["id"]): props(o) for o in objects if kind(o, "Device")}
    result = []
    for obj in objects:
        p = props(obj)
        if not kind(obj, "Node") or p.get("media.class") not in media:
            continue
        identity = dict(devices.get(str(p.get("device.id")), {}), **p)
        if all(identity.get(k) == v for k, v in selected.items()):
            if TAG in p or str(p.get("node.name", "")).startswith(PREFIX):
                raise ValueError("selected node is a Mixyard producer; feedback rejected")
            if p.get("bluez5.loopback") in (True, "true"):
                # Linking this placeholder asks WirePlumber to change the headset profile.
                continue
            result.append(obj)
    return result


def port_map(objects, node, direction, channels):
    ports = [o for o in objects if kind(o, "Port")
             and str(props(o).get("node.id")) == str(node["id"])
             and props(o).get("port.direction") == direction
             and not props(o).get("port.control", False)]
    result = []
    for channel in channels:
        found = [o for o in ports if props(o).get("audio.channel") == channel]
        if len(found) != 1:
            return None
        result.append(found[0]["id"])
    if len(ports) != len(channels):
        return None
    return result


def node_name(config, family, identifier, side):
    return f"{PREFIX}{config['instance']}.{family}.{identifier}.{side}"


def filter_module(config, family, item, sources, output_layout, virtual):
    """Native per-edge coefficients, with cascaded eight-input builtin mixers."""
    nodes, links, inputs, weights = [], [], [], [[] for _ in output_layout]
    for source in sources:
        for channel_index, _channel in enumerate(source["channels"]):
            name = f"edge_{source['id']}_{channel_index}"
            nodes.append({"type": "builtin", "name": name, "label": "copy"})
            inputs.append(f"{name}:In")
            value = source["gain"]
            for output in range(len(output_layout)):
                if len(output_layout) == 1:
                    weight = value / len(source["channels"])
                else:
                    weight = value if len(source["channels"]) == 1 or output == channel_index else 0
                weights[output].append((f"{name}:Out", weight))
    if not inputs:
        nodes.append({"type": "builtin", "name": "silence", "label": "copy"})
        inputs = ["silence:In"]
        weights = [[("silence:Out", 0)] for _ in output_layout]
    outputs = []
    for output, signals in enumerate(weights):
        level = 0
        while True:
            summed = []
            for offset in range(0, len(signals), 8):
                group = signals[offset:offset+8]
                name = f"sum{output}_{level}_{offset // 8}"
                nodes.append({"type": "builtin", "name": name, "label": "mixer",
                              "control": {f"Gain {i+1}": weight for i, (_, weight) in enumerate(group)}})
                links += [{"output": port, "input": f"{name}:In {i+1}"} for i, (port, _) in enumerate(group)]
                summed.append((f"{name}:Out", 1))
            if len(summed) == 1:
                outputs.append(summed[0][0])
                break
            signals, level = summed, level + 1
    name = node_name(config, family, item["id"], "out")
    common = {TAG: config["instance"], "node.autoconnect": False,
              "node.dont-fallback": True, "node.dont-reconnect": True,
              "node.dont-move": True, "node.linger": True, "node.virtual": True,
              "node.link-group": name.removesuffix(".out"),
              "stream.dont-remix": True, "node.pause-on-idle": False}
    capture = dict(common, **{"node.name": node_name(config, family, item["id"], "in"),
                             "media.class": "Stream/Input/Audio",
                             "audio.position": [f"AUX{i}" for i in range(len(inputs))]})
    description = "Mixyard " + item.get("name", item["id"])
    playback = dict(common, **{"node.name": name, "node.description": description,
                              "media.class": "Audio/Source" if virtual else "Stream/Output/Audio",
                              "audio.position": output_layout, "priority.session": 0})
    return {"name": "libpipewire-module-filter-chain", "args": {
        "node.description": description,
        "filter.graph": {"nodes": nodes, "links": links, "inputs": inputs, "outputs": outputs},
        "capture.props": capture, "playback.props": playback}}


def required_mic_consents(config):
    """Bind each configured mic/output path to its identities and controls, even muted."""
    result = []
    mixes = {mix["id"]: mix for mix in config["mixes"]}
    for output in config["outputs"]:
        if not output.get("enabled", False):
            continue
        mix = mixes[output["mix"]]
        for source in config["sources"]:
            if source["role"] != "mic" or not source["gains"].get(mix["id"], 0):
                continue
            path = {"instance": config["instance"], "source": source, "mix": mix, "output": output}
            token = hashlib.sha256(json.dumps(path, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
            result.append({"source": source["id"], "mix": mix["id"], "output": output["id"],
                           "sourceSelector": source["selector"], "outputSelector": output["selector"],
                           "token": token})
    return result


def plan(config, objects, arm_outputs=False, mic_consents=()):
    validate(config)
    diagnostics, routes, resolved_ids = [], [], set()
    mixes = {mix["id"]: mix for mix in config["mixes"]}
    required = required_mic_consents(config)
    blocked_outputs = {path["output"] for path in required if path["token"] not in mic_consents}
    active_outputs = [o for o in config["outputs"] if o.get("enabled", False) and arm_outputs
                      and o["id"] not in blocked_outputs]
    if arm_outputs:
        for oid in sorted(blocked_outputs):
            diagnostics.append({"code": "mic-consent-required", "output": oid,
                                "reason": "explicit consent required for every configured microphone path"})
    # Reject convergence even when disarmed, muted, attenuated or missing a port map.
    output_nodes, resolved_sinks = {}, set()
    for output in config["outputs"]:
        if not output.get("enabled", False):
            continue
        found = matches(objects, output["selector"], ("Audio/Sink",))
        output_nodes[output["id"]] = found
        if len(found) == 1:
            if found[0]["id"] in resolved_sinks:
                raise ValueError("multiple enabled outputs resolve to the same sink")
            resolved_sinks.add(found[0]["id"])
    active_mixes = {mid for mid, mix in mixes.items() if mix["virtualInput"]}
    active_mixes.update(o["mix"] for o in active_outputs)
    offset = 0
    for source in config["sources"]:
        found = matches(objects, source["selector"], ("Audio/Source", "Stream/Output/Audio"))
        ports = port_map(objects, found[0], "out", source["channels"]) if len(found) == 1 else None
        if ports is None:
            diagnostics.append({"code": "source-disconnected", "source": source["id"],
                                "reason": "missing, ambiguous, unsupported channel map, or profile-switch placeholder"})
        else:
            if found[0]["id"] in resolved_ids:
                raise ValueError("multiple selectors resolve to the same source")
            resolved_ids.add(found[0]["id"])
            for mid in sorted(active_mixes):
                mix = mixes[mid]
                if source["role"] in mix["excludeRoles"] or source["id"] in mix.get("excludeSources", []):
                    continue
                for index, channel in enumerate(source["channels"]):
                    routes.append({"from": source["selector"], "fromChannel": channel,
                                   "to": node_name(config, "mix", mid, "in"), "toChannel": f"AUX{offset+index}"})
        offset += len(source["channels"])
    for output in active_outputs:
        found = output_nodes[output["id"]]
        ports = port_map(objects, found[0], "in", output["channels"]) if len(found) == 1 else None
        if ports is None:
            diagnostics.append({"code": "output-disconnected", "output": output["id"], "reason": "explicit sink unavailable"})
            continue
        mix = mixes[output["mix"]]
        for index, channel in enumerate(mix["channels"]):
            routes.append({"from": node_name(config, "mix", mix["id"], "out"), "fromChannel": channel,
                           "to": node_name(config, "output", output["id"], "in"), "toChannel": f"AUX{index}"})
        for channel in output["channels"]:
            routes.append({"from": node_name(config, "output", output["id"], "out"), "fromChannel": channel,
                           "to": output["selector"], "toChannel": channel})
    if any(o.get("enabled", False) for o in config["outputs"]) and not arm_outputs:
        diagnostics.append({"code": "outputs-disarmed", "reason": "requires --arm-outputs"})
    modules = [{"name": "libpipewire-module-rt", "flags": ["ifexists", "nofail"]}]
    modules += [{"name": f"libpipewire-module-{m}"} for m in ("protocol-native", "client-node", "adapter")]
    for mix in config["mixes"]:
        sends = [dict(s, gain=0 if s.get("mute", False) else s["gains"].get(mix["id"], 0) * mix["master"])
                 for s in config["sources"]]
        modules.append(filter_module(config, "mix", mix, sends, mix["channels"], mix["virtualInput"]))
    for output in config["outputs"]:
        mix = mixes[output["mix"]]
        modules.append(filter_module(config, "output", output,
            [{"id": "mix", "channels": mix["channels"], "gain": output["gain"]}], output["channels"], False))
    return {"evidence": "command-plan-only", "mutationAvailable": False,
            "requiredMicConsents": required, "diagnostics": diagnostics, "routes": routes,
            "producerConfig": {"context.properties": {"log.level": 1},
                               "context.spa-libs": {"audio.convert.*": "audioconvert/libspa-audioconvert",
                                                    "support.*": "support/libspa-support"},
                               "context.modules": modules}}


def dump_graph(remote):
    try:
        result = subprocess.run(["pw-dump", "-r", remote], check=True, capture_output=True,
                                text=True, timeout=10)
    except subprocess.CalledProcessError as error:
        if any(text in (error.stderr or "").lower() for text in ("permission denied", "operation not permitted")):
            raise PermissionError("PipeWire denied access to the requested server") from error
        raise
    data = json.loads(result.stdout)
    if not isinstance(data, list):
        raise ValueError("pw-dump returned non-array")
    return data


def discover(remote):
    envelope = {"schemaVersion": 1, "platform": "linux", "capabilities": {
        "deviceDiscovery": False, "processCapture": False, "virtualInputs": False,
        "independentMixes": False, "routeRecovery": False}, "devices": [], "diagnostics": []}
    try:
        objects = dump_graph(remote)
        envelope["capabilities"]["deviceDiscovery"] = True
        for obj in objects:
            p = props(obj)
            direction = {"Audio/Source": "input", "Audio/Sink": "output"}.get(p.get("media.class"))
            if kind(obj, "Node") and direction:
                name = p.get("node.name")
                if not name:
                    continue
                envelope["devices"].append({"id": "pipewire:node.name:" + name,
                    "name": p.get("node.description", name), "direction": direction,
                    "virtual": p.get("node.virtual") in (True, "true") or TAG in p})
                if obj.get("info", {}).get("state") == "error":
                    envelope["diagnostics"].append({"code": "node-error", "severity": "error",
                                                    "message": f"Audio node {name} reports error state"})
        envelope["diagnostics"].append({"code": "native-transport-unverified", "severity": "warning",
            "message": "Discovery only; external mutation disabled pending identity-safe gated linking, acknowledged controls and native qualification. node.name IDs are profile-dependent; reject duplicate matches."})
        envelope["diagnostics"].append({"code": "xrun-counters-unavailable", "severity": "warning",
            "message": "pw-dump is not a gap/xrun measurement. Use native profiler/pw-top and PCM qualification; no counter does not mean zero dropouts."})
        return envelope, 0
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        code = "permission-denied" if isinstance(error, PermissionError) else "pipewire-unavailable"
        if isinstance(error, FileNotFoundError):
            code = "dependency-unavailable"
        envelope["diagnostics"].append({"code": code, "severity": "error",
                                        "message": str(error)})
        return envelope, 1


def health(remote, iterations):
    result = {"platform": "linux", "evidence": "native-counter-observation",
              "counterScope": "pw-top observation window; ERR combines xruns and errors",
              "ownedRows": [], "countersAvailable": False, "verifiedAudio": False,
              "gapsMeasured": False, "zeroDropouts": None}
    try:
        observed = subprocess.run(["pw-top", "-b", "-n", str(iterations), "-r", remote],
                                  check=True, capture_output=True, text=True, timeout=iterations * 2 + 10)
        for line in observed.stdout.splitlines():
            # Do not forward other applications' names from the profiler table.
            columns = line.split(maxsplit=9)
            if len(columns) != 10:
                continue
            name = re.search(r"(?:^|\s)[+=]?\s*(mixyard\.[a-z0-9_-]+\.(?:mix|output)\.[a-z0-9_-]+\.(?:in|out))\s*$", columns[9])
            if name:
                result["ownedRows"].append({"node": name[1], "state": columns[0],
                    "errorsAndXruns": int(columns[8]) if columns[8].isdigit() else None,
                    "nativeRow": line})
        result["countersAvailable"] = any(row["errorsAndXruns"] is not None for row in result["ownedRows"])
        result["diagnostic"] = "Native counters do not prove absence of PCM gaps or past dropouts"
        return result, 0 if result["countersAvailable"] else 1
    except (OSError, subprocess.SubprocessError) as error:
        result["diagnostic"] = f"xrun counters unavailable: {error}"
        return result, 1


def death_signal(parent):
    # Retained process-lifetime utility (Linux only); apply never starts clients.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGTERM, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "prctl(PR_SET_PDEATHSIG)")
    if os.getppid() != parent:
        os.kill(os.getpid(), signal.SIGTERM)


def spawn(command, env, **kwargs):
    parent = os.getpid()
    return subprocess.Popen(command, env=env, preexec_fn=lambda: death_signal(parent), **kwargs)


def topology(producer_config):
    result = json.loads(json.dumps(producer_config))
    for mod in result["context.modules"]:
        for node in mod.get("args", {}).get("filter.graph", {}).get("nodes", []):
            node.pop("control", None)
    return result


def controls(producer_config):
    return {mod["args"]["capture.props"]["node.name"]: {
                f"{node['name']}:{key}": value
                for node in mod["args"]["filter.graph"]["nodes"]
                for key, value in node.get("control", {}).items()}
            for mod in producer_config["context.modules"] if "args" in mod}


def apply(config_path, remote, runtime_dir, arm_outputs, interval, iterations=None):
    """Fail before any file, server or process access; the unsafe controller is retired."""
    raise ValueError(MUTATION_UNAVAILABLE)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    snapshot = commands.add_parser("snapshot")
    snapshot.add_argument("--remote", default=os.getenv("PIPEWIRE_REMOTE", "pipewire-0"))
    profiler = commands.add_parser("health")
    profiler.add_argument("--remote", required=True)
    profiler.add_argument("--iterations", type=int, choices=range(1, 21), default=3)
    for name in ("plan", "apply"):
        cmd = commands.add_parser(name, help="offline command plan" if name == "plan"
                                  else "unavailable: native identity/control safety gates unresolved")
        cmd.add_argument("config", type=Path)
        cmd.add_argument("--arm-outputs", action="store_true")
        if name == "plan":
            cmd.add_argument("--consent-mic-path", action="append", default=[], metavar="TOKEN",
                             help="consent to one exact microphone/output path from requiredMicConsents")
            cmd.add_argument("--dump", type=Path, required=True, help="saved pw-dump JSON; offline only")
        else:
            cmd.add_argument("--remote", required=True)
            cmd.add_argument("--runtime-dir", required=True)
            cmd.add_argument("--interval", type=float, default=1)
    args = parser.parse_args()
    if args.command == "snapshot":
        envelope, status = discover(args.remote)
        print(json.dumps(envelope))
        return status
    if args.command == "health":
        observed, status = health(args.remote, args.iterations)
        print(json.dumps(observed))
        return status
    try:
        if args.command == "plan":
            print(json.dumps(plan(json.loads(args.config.read_text()), json.loads(args.dump.read_text()),
                                  args.arm_outputs, args.consent_mic_path), indent=2))
        else:
            if not math.isfinite(args.interval) or args.interval < 0.1:
                raise ValueError("interval must be finite and >= 0.1 seconds")
            apply(args.config, args.remote, args.runtime_dir, args.arm_outputs, args.interval)
        return 0
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError) as error:
        print(f"mixyard-linux: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())

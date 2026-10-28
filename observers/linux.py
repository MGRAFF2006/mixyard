#!/usr/bin/env python3
"""Passive metadata only. No argv, environ, window names, audio, or mutations."""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import time
from zoneinfo import ZoneInfo

MAX_BYTES = 1024 * 1024
MAX_ENTRIES = 8192
MAX_DEVICES = 64


def fact(name, value=None, *, subject=None, status="known", reason=None,
         provider="linux", evidence="platform-observed", complete=True):
    return {"key": {"name": name, **({"subject": subject} if subject else {})},
            "status": status, "value": value, "provider": provider,
            "evidence": evidence, "observedAtMs": None, "validForMs": 5000,
            "completeness": "complete" if complete else "partial", "reason": reason}


def value(kind, data):
    return {"type": kind, "value": data}


def unavailable(names, provider, reason, status="unknown"):
    return [fact(n, status=status, reason=reason, provider=provider, complete=False) for n in names]


def read_text(path, limit=4096):
    with path.open("rb") as stream:
        raw = stream.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("file-size-limit")
    return raw.decode("utf-8").strip()


def entries(path, limit):
    result = []
    with os.scandir(path) as scan:
        for item in scan:
            if len(result) >= limit:
                raise ValueError("enumeration-limit")
            result.append(Path(item.path))
    return result


def applications(root, uid, mappings):
    provider = "linux.proc"
    if not mappings:
        return unavailable(["application.running"], provider, "exact-executable-mapping-required")
    found, incomplete = set(), False
    try:
        processes = entries(root, MAX_ENTRIES)
    except (OSError, ValueError):
        return unavailable(["application.running"], provider, "process-enumeration-unavailable")
    for process in processes:
        if not process.name.isdecimal():
            continue
        try:
            # Only consume UID, including for undumpable/root-owned proc entries.
            # Never collect process names, arguments, environments or other UIDs.
            status = read_text(process / "status", 65536)
            owners = [line.split()[1:] for line in status.splitlines() if line.startswith("Uid:")]
            if len(owners) != 1 or len(owners[0]) != 4 or not all(v.isdecimal() for v in owners[0]):
                raise ValueError("malformed-process-owner")
            if any(int(v) != uid for v in owners[0][:2]):
                continue
            executable = os.readlink(process / "exe")
            if len(executable.encode()) > 4096:
                raise ValueError("executable-size-limit")
            if executable.endswith(" (deleted)"):
                incomplete = True
                continue
            if executable in mappings:
                found.add(mappings[executable])
        except FileNotFoundError:
            # A PID disappearing between enumeration and inspection is not a
            # complete observation of absence (including a deleted executable).
            incomplete = True
        except (OSError, ValueError):
            incomplete = True
    if incomplete:
        return unavailable(["application.running"], provider, "process-enumeration-incomplete")
    return [fact("application.running", value("text-set", sorted(found)), provider=provider)]


def power(root):
    provider = "linux.sysfs"
    names = ["power.source", "power.battery-percent"]
    try:
        supplies = entries(root, 64)
        online, batteries, incomplete = [], [], False
        for supply in supplies:
            kind = read_text(supply / "type")
            if kind == "Battery":
                present = read_text(supply / "present") if (supply / "present").exists() else "1"
                if present not in ("0", "1"):
                    raise ValueError("invalid-battery-presence")
                if present == "1":
                    batteries.append(supply)
            elif kind in ("Mains", "USB", "USB_C", "USB_PD", "Wireless"):
                state = read_text(supply / "online")
                if state not in ("0", "1"):
                    raise ValueError("invalid-online-state")
                online.append(state == "1")
            else:
                incomplete = True
        if incomplete:
            return unavailable(names, provider, "unrecognized-power-supply")
        if any(online):
            source = fact("power.source", value("text", "external"), provider=provider)
        elif batteries:
            # A battery being present alone does not establish power source.
            states = [read_text(b / "status") for b in batteries]
            source = (fact("power.source", value("text", "battery"), provider=provider)
                      if all(s == "Discharging" for s in states)
                      else unavailable([names[0]], provider, "power-source-unverified")[0])
        else:
            source = unavailable([names[0]], provider, "no-power-source-observed")[0]
        if len(batteries) == 1:
            capacity = read_text(batteries[0] / "capacity")
            if not capacity.isdecimal() or not 0 <= int(capacity) <= 100:
                raise ValueError("invalid-capacity")
            percent = fact(names[1], value("number", int(capacity)), provider=provider)
        else:
            percent = unavailable([names[1]], provider, "no-battery" if not batteries else "multiple-batteries-no-weighted-total")[0]
        return [source, percent]
    except PermissionError:
        return unavailable(names, provider, "power-permission-denied")
    except OSError:
        return unavailable(names, provider, "power-files-unavailable")
    except ValueError:
        return unavailable(names, provider, "power-malformed-or-limit", "error")


def civil(timezone, timestamp=None):
    names = ["civil.timezone", "civil.date", "civil.weekday", "civil.minute-of-day"]
    try:
        moment = datetime.datetime.fromtimestamp(time.time() if timestamp is None else timestamp,
                                                ZoneInfo(timezone))
        values = [value("text", timezone), value("text", moment.date().isoformat()),
                  value("number", moment.isoweekday()), value("number", moment.hour * 60 + moment.minute)]
        return [fact(n, v, provider="civil.zoneinfo", evidence="derived") for n, v in zip(names, values)]
    except (KeyError, ValueError, OSError, OverflowError):
        return unavailable(names, "civil.zoneinfo", "timezone-unavailable-or-invalid", "error")


def bounded_command(command, timeout=2.0, limit=MAX_BYTES):
    """Bound both pipes while running; always reap the owned direct child."""
    child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE)
    try:
        result = bytearray()
        total = 0
        deadline = time.monotonic() + timeout
        with selectors.DefaultSelector() as selector:
            for pipe in (child.stdout, child.stderr):
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() >= deadline:
                    raise TimeoutError("child-timeout")
                for key, _ in selector.select(min(0.02, max(0, deadline - time.monotonic()))):
                    block = os.read(key.fileobj.fileno(), 65536)
                    if not block:
                        selector.unregister(key.fileobj)
                    else:
                        total += len(block)
                        if total > limit:
                            raise ValueError("child-output-limit")
                        if key.fileobj is child.stdout:
                            result.extend(block)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("child-timeout")
            if child.wait(timeout=remaining) != 0:
                raise OSError("child-failed")
        return bytes(result)
    finally:
        if child.poll() is None:
            child.kill()
        child.wait()
        child.stdout.close()
        child.stderr.close()


def load_adapter(path):
    spec = importlib.util.spec_from_file_location("mixyard_linux_readonly", path)
    adapter = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(adapter)
    return adapter


def audio_objects(objects, adapter):
    provider = "linux.pipewire"
    if not isinstance(objects, list) or len(objects) > MAX_ENTRIES:
        raise ValueError("audio-enumeration-limit")
    adapter.validate_observation(objects)
    devices = {str(o["id"]): adapter.props(o) for o in objects if adapter.kind(o, "Device")}
    endpoints = {}
    incomplete = False
    for obj in objects:
        if not adapter.kind(obj, "Node"):
            continue
        props = adapter.props(obj)
        direction = {"Audio/Source": "input", "Audio/Sink": "output"}.get(props.get("media.class"))
        if direction is None:
            continue
        if "device.id" in props and str(props["device.id"]) not in devices:
            incomplete = True
            continue
        identity = adapter.node_identity(obj, devices)
        if "node.name" not in identity or obj.get("info", {}).get("state") == "error":
            incomplete = True
            continue
        # Namespace and identity tuple are independent of profile names, labels,
        # PipeWire numeric object IDs and process IDs. No guessed default device.
        encoded = json.dumps([direction, identity], sort_keys=True, separators=(",", ":")).encode()
        uid = "context:pipewire:" + hashlib.sha256(encoded).hexdigest()
        if uid in endpoints:
            return unavailable(["device.connected"], provider, "duplicate-endpoint-identity", "ambiguous")
        endpoints[uid] = direction
        if len(endpoints) > MAX_DEVICES:
            raise ValueError("endpoint-count-limit")
    if incomplete:
        return unavailable(["device.connected"], provider, "endpoint-enumeration-incomplete")
    facts = [fact("device.connected", value("text-set", sorted(endpoints)), provider=provider)]
    for uid, direction in sorted(endpoints.items()):
        facts.append(fact("device.kind", value("text", direction), subject=uid, provider=provider))
        facts.append(fact("device.capabilities", value("text-set", ["capture" if direction == "input" else "playback"]),
                          subject=uid, provider=provider))
        for name, reason in [("device.channels", "negotiated-format-unobserved"),
                             ("device.sample-rate-hz", "negotiated-format-unobserved"),
                             ("device.hardware-monitor-declared", "explicit-user-declaration-required")]:
            facts.append(fact(name, subject=uid, status="unsupported", reason=reason, provider=provider, complete=False))
    return facts


def audio(adapter_path):
    try:
        adapter = load_adapter(adapter_path)
        raw = bounded_command(["pw-dump"])
        objects = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object)
        if len(objects) > MAX_ENTRIES:
            raise ValueError("audio-enumeration-limit")
        return audio_objects(objects, adapter)
    except FileNotFoundError:
        return unavailable(["device.connected"], "linux.pipewire", "audio-tool-unavailable")
    except (PermissionError, OSError, TimeoutError, subprocess.SubprocessError):
        return unavailable(["device.connected"], "linux.pipewire", "audio-command-unavailable")
    except (ValueError, TypeError, KeyError, AttributeError, RecursionError):
        return unavailable(["device.connected"], "linux.pipewire", "audio-malformed-or-limit", "error")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--adapter", required=True)
    args = parser.parse_args()
    raw = sys.stdin.buffer.read(16385)
    if len(raw) > 16384:
        raise ValueError("configuration-limit")
    config = json.loads(raw)
    facts = applications(Path("/proc"), os.geteuid(), config["applications"])
    facts += power(Path("/sys/class/power_supply"))
    facts += civil(config["timezone"])
    facts += audio(args.adapter)
    print(json.dumps(facts, allow_nan=False, separators=(",", ":")))


def unique_object(pairs):
    result = {}
    for key, item in pairs:
        if key in result:
            raise ValueError("duplicate-json-key")
        result[key] = item
    return result


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Disposable hosted private PipeWire graph qualification. Never opens hardware."""
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import time

from request import request

HERE = Path(__file__).resolve().parent
MODULES = {
    "libpipewire-module-protocol-native", "libpipewire-module-client-node",
    "libpipewire-module-access", "libpipewire-module-spa-node-factory",
    "libpipewire-module-link-factory",
}


def snapshot(env):
    result = subprocess.run(["pw-dump", "--remote", "mixyard-private"], env=env,
                            capture_output=True, timeout=5, check=True)
    if len(result.stdout) > 1024 * 1024 or result.stderr:
        raise RuntimeError("unbounded or ambiguous private graph observation")
    objects = json.loads(result.stdout)
    if not isinstance(objects, list) or len(objects) > 256:
        raise RuntimeError("unexpected private graph size")
    ids = [item["id"] for item in objects]
    if len(ids) != len(set(ids)):
        raise RuntimeError("ambiguous registry IDs")
    return objects


def isolation(objects, *, active=False):
    nodes, modules, links, cores = [], set(), [], []
    for item in objects:
        kind = item["type"]
        props = item.get("info", {}).get("props", {})
        if kind == "PipeWire:Interface:Device":
            raise RuntimeError("device object on isolated graph")
        if kind == "PipeWire:Interface:Module":
            modules.add(props["module.name"])
        if kind == "PipeWire:Interface:Core":
            cores.append(props)
        if kind == "PipeWire:Interface:Node":
            name = props.get("node.name")
            allowed = {"mixyard-private-driver"}
            if active:
                allowed |= {"mixyard-private-generator", "mixyard-private-engine", "mixyard-private-consumer"}
            if name not in allowed or "device.api" in props or "api.alsa.path" in props:
                raise RuntimeError("unowned or physical node on private graph")
            nodes.append((item["id"], name, props))
        if kind == "PipeWire:Interface:Link":
            links.append(item)
    if len(cores) != 1 or cores[0].get("core.name") != "mixyard-private":
        raise RuntimeError("wrong server core")
    if modules != MODULES:
        raise RuntimeError("unexpected private server module set: " + repr(modules))
    expected = 4 if active else 1
    if len(nodes) != expected or len(links) != (6 if active else 0):
        raise RuntimeError("unexpected node/link topology")
    if active:
        ids = {name: node for node, name, _ in nodes}
        for item in links:
            info = item["info"]
            pair = (info["output-node-id"], info["input-node-id"])
            if pair not in {
                (ids["mixyard-private-generator"], ids["mixyard-private-engine"]),
                (ids["mixyard-private-engine"], ids["mixyard-private-consumer"]),
            }:
                raise RuntimeError("recapture, duplicate monitor or unowned link")
        # One native link per destination port; no parallel monitor route.
        destinations = [item["info"]["input-port-id"] for item in links]
        if len(set(destinations)) != 6:
            raise RuntimeError("duplicate destination link")
    return {"nodeCount": len(nodes), "linkCount": len(links), "modules": sorted(modules),
            "coreName": cores[0]["core.name"]}


def stop_owned(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def await_file(path, process, timeout=10):
    deadline = time.monotonic() + timeout
    while not path.exists():
        if process.poll() is not None:
            raise RuntimeError("owned client exited before topology publication")
        if time.monotonic() >= deadline:
            raise RuntimeError("native topology publication timed out")
        time.sleep(0.02)
    # Native writer renames only a complete file into this private directory.
    return json.loads(path.read_text())


def atomic_json(path, value):
    staging = path.with_suffix(".new")
    staging.write_text(json.dumps(value))
    staging.rename(path)


def run(binary, env, directory, cancel=False):
    directory.mkdir(mode=0o700)
    stderr = directory / "stderr.log"
    stdout = directory / "stdout.log"
    with stderr.open("xb") as err, stdout.open("xb") as out:
        process = subprocess.Popen([str(binary), str(directory)], env=env, stdout=out, stderr=err)
        try:
            topology = await_file(directory / "topology.json", process)
            active = isolation(snapshot(env), active=True)
            if cancel:
                process.send_signal(signal.SIGTERM)
            else:
                atomic_json(directory / "request.json", request(topology))
            process.wait(timeout=35)
        finally:
            stop_owned(process)
    if process.returncode:
        raise RuntimeError(f"native client exit {process.returncode}: {stderr.read_text()} {stdout.read_text()}")
    report = json.loads(stdout.read_text())
    if report["error"] or report["sampleMismatches"]:
        raise RuntimeError("native sample oracle/lifecycle failed: " + repr(report))
    if not cancel and (report["stage"] != 10 or min(report["delivered"]) < 128):
        raise RuntimeError("insufficient actual native delivery")
    isolation(snapshot(env))
    return {"topology": topology, "activeIsolation": active, "result": report}


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise SystemExit("only disposable GitHub-hosted runners are authorized; no local native execution")
    binary = Path(sys.argv[1]).resolve(strict=True)
    evidence = Path(sys.argv[2]).resolve()
    with tempfile.TemporaryDirectory(prefix="mixyard-private-", dir=os.environ["RUNNER_TEMP"]) as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        runtime, config, home = [root / p for p in ("runtime", "config", "home")]
        for directory in (runtime, config, home):
            directory.mkdir(mode=0o700)
        (config / "private.conf").write_bytes((HERE / "private.conf").read_bytes())
        (config / "client.conf").write_text('''context.properties = { support.dbus = false mem.warn-mlock = false }
context.spa-libs = { support.* = support/libspa-support }
context.modules = [
    { name = libpipewire-module-protocol-native }
    { name = libpipewire-module-client-node }
]
''')
        env = {"PATH": "/usr/bin:/bin", "HOME": str(home), "XDG_RUNTIME_DIR": str(runtime),
               "PIPEWIRE_RUNTIME_DIR": str(runtime), "PIPEWIRE_REMOTE": "mixyard-private",
               "PIPEWIRE_CONFIG_DIR": str(config), "PIPEWIRE_CONFIG_NAME": "client.conf",
               "XDG_CONFIG_HOME": str(config), "XDG_STATE_HOME": str(home),
               "MIXYARD_PRIVATE_GRAPH": "hosted-owned-only"}
        with (root / "server.log").open("xb") as log:
            server = subprocess.Popen(["pipewire", "-c", str(config / "private.conf")], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 5
                while not (runtime / "mixyard-private").exists():
                    if server.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("private server startup: " + (root / "server.log").read_text())
                    time.sleep(0.02)
                socket = (runtime / "mixyard-private").stat()
                if not stat.S_ISSOCK(socket.st_mode) or socket.st_uid != os.getuid():
                    raise RuntimeError("private socket ownership mismatch")
                initial = isolation(snapshot(env))  # Required before connecting any PCM nodes.
                report = {"initialIsolation": initial,
                          "integrated": run(binary, env, root / "integrated"),
                          "cancelled": run(binary, env, root / "cancelled", cancel=True),
                          "claims": {"physicalDevices": False, "nativeApps": False,
                                     "physicalClocks": False, "backendAvailable": False}}
                evidence.write_text(json.dumps(report, indent=2) + "\n")
            finally:
                stop_owned(server)
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

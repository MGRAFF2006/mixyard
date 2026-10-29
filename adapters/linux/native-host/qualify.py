#!/usr/bin/env python3
"""Disposable hosted private PipeWire graph qualification. Never opens hardware."""
import json
import os
from pathlib import Path
import selectors
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
    "libpipewire-module-metadata",
}


def snapshot_bytes(command, env, timeout=5):
    """Owned control subprocess; bound capture before allocation, retain cause."""
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    buffers = {"stdout": bytearray(), "stderr": bytearray()}
    limits = {"stdout": 1024 * 1024, "stderr": 4096}
    deadline = time.monotonic() + timeout
    reason = None
    try:
        with selectors.DefaultSelector() as ready:
            ready.register(process.stdout, selectors.EVENT_READ, "stdout")
            ready.register(process.stderr, selectors.EVENT_READ, "stderr")
            while ready.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    reason = "timeout"
                    break
                events = ready.select(remaining)
                if not events:
                    reason = "timeout"
                    break
                for key, _ in events:
                    name = key.data
                    # One byte beyond the limit proves overflow without reading
                    # or allocating the unbounded remainder of either pipe.
                    chunk = os.read(key.fd, min(65536, limits[name] - len(buffers[name]) + 1))
                    if not chunk:
                        ready.unregister(key.fileobj)
                    else:
                        buffers[name].extend(chunk)
                        if len(buffers[name]) > limits[name]:
                            reason = name + "-limit"
                            break
                if reason:
                    break
        if not reason:
            try:
                process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                reason = "timeout"
    finally:
        if process.poll() is None:
            process.kill()  # This handle belongs only to our pw-dump child.
            process.wait(timeout=1)
        process.stdout.close()
        process.stderr.close()
    if reason or process.returncode or buffers["stderr"]:
        diagnostic = bytes(buffers["stderr"][:2048]).decode("utf-8", errors="replace")
        raise RuntimeError(f"private graph observation rejected: reason={reason or 'exit-or-stderr'} "
                           f"returncode={process.returncode} stdoutBytes={len(buffers['stdout'])} "
                           f"stderrBytes={len(buffers['stderr'])} stderrPrefix={diagnostic!r}")
    return bytes(buffers["stdout"])


def snapshot(env):
    try:
        raw = snapshot_bytes(["pw-dump", "--remote", "mixyard-private"], env)
    except RuntimeError as error:
        # Registry enumeration does not bind each advertised object. Preserve
        # actual IDs/types/versions for a bind failure; never infer type from a
        # numeric global ID or use this diagnostic to pass the isolation gate.
        try:
            registry = snapshot_bytes(["pw-cli", "--remote", "mixyard-private", "list-objects"], env)
            diagnostic = registry[:8192].decode("utf-8", errors="replace")
        except RuntimeError as registry_error:
            diagnostic = str(registry_error)[:8192]
        raise RuntimeError(f"{error}; readOnlyRegistryPrefix={diagnostic!r}") from error
    objects = json.loads(raw)
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


def client_log(path):
    with path.open("r", errors="replace") as file:
        text = file.read(65536)
        return text + (" [truncated]" if file.read(1) else "")


def run(binary, env, directory, cancel=False, server_loss=None, mode="normal"):
    directory.mkdir(mode=0o700)
    stderr = directory / "stderr.log"
    stdout = directory / "stdout.log"
    with stderr.open("xb") as err, stdout.open("xb") as out:
        process = subprocess.Popen([str(binary), str(directory), mode], env=env, stdout=out, stderr=err)
        failure = None
        try:
            topology = await_file(directory / "topology.json", process)
            active = isolation(snapshot(env), active=True)
            atomic_json(directory / "request.json", request(topology, mode))
            if cancel or server_loss:
                await_file(directory / "flowing", process)
                if cancel:
                    process.send_signal(signal.SIGTERM)
                else:
                    stop_owned(server_loss)
            process.wait(timeout=35)
        except Exception as error:
            failure = error
        finally:
            stop_owned(process)
    if failure:
        raise RuntimeError(f"{mode}: {failure}; native stderr={client_log(stderr)} stdout={client_log(stdout)}") from failure
    if server_loss:
        report = json.loads(stdout.read_text())
        if process.returncode != 1 or report["error"] != 46 or report["sampleMismatches"]:
            raise RuntimeError("server loss failed closed-exit check: " + repr(report))
        return {"topology": topology, "activeIsolation": active, "result": report}
    if process.returncode:
        raise RuntimeError(f"native client exit {process.returncode}: {client_log(stderr)} {client_log(stdout)}")
    report = json.loads(stdout.read_text())
    if report["error"] or report["sampleMismatches"]:
        raise RuntimeError("native sample oracle/lifecycle failed: " + repr(report))
    if not cancel:
        if mode == "normal" and (report["consumerTerminalClosures"][2] < 1 or
                report["consumerDropped"][2] < 1 or
                report["consumerTerminalClosures"][:2] != [0, 0] or
                report["consumerEpochs"][2] <= report["consumerEpochs"][0]):
            raise RuntimeError("owned consumer overflow/explicit fresh epoch/isolation not proved: " + repr(report))
        if report["stage"] != 10 or min(report["delivered"]) < (64 if mode == "output-failure" else 128):
            raise RuntimeError("insufficient actual native delivery")
        if mode == "output-failure" and (report["inactiveOutputs"][1] < 128 or
                report["inactiveOutputs"][0] or report["inactiveOutputs"][2] or
                report["delivered"][0] < 192 or report["delivered"][2] < 192):
            raise RuntimeError("destination failure did not isolate healthy native siblings")
        if mode == "tail-loss" and report["lossFrame"] == 2**64-1:
            raise RuntimeError("no actual native source loss was observed")
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
    # Register Metadata/3 marshalling for read-only pw-dump observation. Do not
    # suppress protocol errors or broaden the owned server's module whitelist.
    { name = libpipewire-module-metadata }
]
''')
        env = {"PATH": "/usr/bin:/bin", "HOME": str(home), "XDG_RUNTIME_DIR": str(runtime),
               "PIPEWIRE_RUNTIME_DIR": str(runtime), "PIPEWIRE_REMOTE": "mixyard-private",
               "PIPEWIRE_CONFIG_DIR": str(config), "PIPEWIRE_CONFIG_NAME": "client.conf",
               "XDG_CONFIG_HOME": str(config), "XDG_STATE_HOME": str(home),
               "MIXYARD_PRIVATE_GRAPH": "hosted-owned-only"}
        # PipeWire's environment selector takes precedence over -c. Clients
        # retain client.conf; only this owned daemon selects the server config.
        server_env = dict(env, PIPEWIRE_CONFIG_NAME="private.conf")
        report = {"serverEpochs": [],
                  "claims": {"physicalDevices": False, "nativeApps": False,
                             "physicalClocks": False, "backendAvailable": False}}
        for epoch in range(2):
            with (root / f"server-{epoch}.log").open("xb") as log:
                server = subprocess.Popen(["pipewire", "-c", str(config / "private.conf")], env=server_env, stdout=log, stderr=log)
                try:
                    deadline = time.monotonic() + 5
                    while not (runtime / "mixyard-private").exists():
                        if server.poll() is not None or time.monotonic() >= deadline:
                            raise RuntimeError("private server startup: " + (root / f"server-{epoch}.log").read_text())
                        time.sleep(0.02)
                    socket = (runtime / "mixyard-private").stat()
                    if not stat.S_ISSOCK(socket.st_mode) or socket.st_uid != os.getuid():
                        raise RuntimeError("private socket ownership mismatch")
                    initial = isolation(snapshot(env))  # Required before connecting any PCM nodes.
                    result = {"ownedServerPid": server.pid, "initialIsolation": initial,
                              "integrated": run(binary, env, root / f"integrated-{epoch}"),
                              "outputFailure": run(binary, env, root / f"output-failure-{epoch}", mode="output-failure"),
                              "retainedTailLoss": run(binary, env, root / f"tail-loss-{epoch}", mode="tail-loss"),
                              "cancelled": run(binary, env, root / f"cancelled-{epoch}", cancel=True)}
                    if epoch == 1:
                        result["serverLoss"] = run(binary, env, root / "server-loss", server_loss=server)
                    report["serverEpochs"].append(result)
                finally:
                    stop_owned(server)
        evidence.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

"""Real owned child pipes at the production gate; no PipeWire/SDK is loaded."""
import base64
import contextlib
import errno
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

import qualify
from process_capture import Captures, DIAGNOSTIC_LIMIT, STDOUT_LIMIT
from test_control import object_, private_snapshot


def active_snapshot():
    result = private_snapshot()
    result += [object_(10 + i, "Node", {"node.name": name}) for i, name in enumerate(
        ["mixyard-private-generator", "mixyard-private-engine", "mixyard-private-consumer"])]
    result += [object_(30 + i, "Link", **{
        "output-node-id": 10 if i < 3 else 11, "input-node-id": 11 if i < 3 else 12,
        "input-port-id": 40 + i}) for i in range(6)]
    return result


def passing_report(mode="normal", cancel=False, server_loss=False):
    result = {"stage": 10, "error": 0, "sampleMismatches": 0, "rendered": 450,
              "delivered": [450, 450, 262], "callbackHeapOperations": 0,
              "inactiveOutputs": [0, 0, 0], "lossFrame": 2**64 - 1,
              "consumerTerminalClosures": [0, 0, 1], "consumerDropped": [0, 0, 188],
              "consumerEpochs": [3, 3, 4], "sourceStopBeforeDestroy": True,
              "sourceRemovalObserved": True, "sourceStopFrame": 10000,
              "stopSignalsReady": True, "dataLoopRetired": True,
              "retiredOwnedCopiesZero": True, "cancelled": cancel,
              "stopSignal": signal.SIGTERM if cancel else 0,
              "expectedServerLoss": server_loss, "coreErrorCount": 0, "coreError": None}
    if mode == "output-failure":
        result.update(delivered=[206, 65, 206], inactiveOutputs=[0, 131, 0])
    if mode == "tail-loss":
        result["lossFrame"] = 10000
    if server_loss:
        result.update(error=46, coreErrorCount=1, coreError={
            "id": 0, "sequence": 37, "result": -errno.EPIPE,
            "messageComplete": True, "message": "connection error"})
    return result


CHILD = r'''
import base64, json, os, pathlib, signal, sys, time
root = pathlib.Path(sys.argv[1]); config = json.loads(sys.argv[2])
def publish(name, value):
    staging = root / (name + ".new")
    staging.write_text(json.dumps(value)); staging.rename(root / name)
publish("topology.json", {"clockId":42,"sources":["a","b","c"],"outputs":["x","y","z"]})
deadline = time.monotonic() + 5
while not (root / "request.json").exists():
    if time.monotonic() >= deadline: raise RuntimeError("request not published")
    time.sleep(.001)
report = config["report"]
if config.get("cancel"):
    def stop(number, _):
        report["stopSignal"] = number; report["cancelled"] = True
    signal.signal(signal.SIGTERM, stop); signal.signal(signal.SIGINT, stop)
    report["cancelled"] = False
publish("flowing", True)
if config.get("serverPid"):
    while True:
        try: os.kill(config["serverPid"], 0)
        except ProcessLookupError: break
        if time.monotonic() >= deadline: raise RuntimeError("owned server not reaped")
        time.sleep(.001)
if config.get("cancel"):
    while not report["cancelled"]:
        if time.monotonic() >= deadline: raise RuntimeError("no cancellation")
        time.sleep(.001)
payload = base64.b64decode(config["stdout"]) if "stdout" in config else (json.dumps(report)+"\n").encode()
sys.stdout.buffer.write(payload); sys.stdout.buffer.flush()
sys.stderr.buffer.write(base64.b64decode(config["stderr"])); sys.stderr.buffer.flush()
raise SystemExit(config.get("returncode", 0))
'''


class Diagnostics(unittest.TestCase):
    def launch_case(self, root, *, mode="normal", cancel=False, stderr=b"", stdout=None,
                    server=None, report=None):
        config = {"report": report if report is not None else passing_report(mode, cancel, server is not None),
                  "stderr": base64.b64encode(stderr).decode(), "cancel": cancel,
                  "returncode": 1 if server else 0}
        if stdout is not None:
            config["stdout"] = base64.b64encode(stdout).decode()
        if server:
            config["serverPid"] = server.process.pid
        real_popen = subprocess.Popen
        paths = []

        def launch(argv, **kwargs):
            paths.append(Path(argv[1]))
            return real_popen([sys.executable, "-I", "-u", "-c", CHILD, argv[1], json.dumps(config)], **kwargs)

        def epochs(binary, evidence, captures, aggregate):
            with tempfile.TemporaryDirectory(prefix="owned-graph-", dir=root) as temporary:
                with mock.patch.object(qualify.subprocess, "Popen", side_effect=launch), \
                        mock.patch.object(qualify, "snapshot", side_effect=[active_snapshot(), private_snapshot()]):
                    aggregate["serverEpochs"].append({"controlled": qualify.run(
                        binary, dict(os.environ), Path(temporary) / "client", mode=mode,
                        cancel=cancel, server_loss=server, captures=captures)})

        evidence = root / "evidence.json"
        environment = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted", "RUNNER_TEMP": str(root)}
        failure = None
        with mock.patch.dict(os.environ, environment), \
                mock.patch.object(qualify, "execute_epochs", side_effect=epochs), \
                mock.patch("sys.argv", [qualify.__file__, __file__, str(evidence)]), \
                contextlib.redirect_stdout(io.StringIO()):
            try:
                qualify.main()
            except (RuntimeError, ValueError) as error:
                failure = error
        result = json.loads(evidence.read_text())
        self.assertEqual(evidence.stat().st_mode & 0o777, 0o600)
        self.assertTrue(paths)
        self.assertTrue(all(not path.exists() for path in paths))
        self.assertTrue(all(item["finished"] for item in result["diagnostics"]))
        return result, failure

    def test_passing_native_report_with_warning_is_rejected_and_preserved(self):
        warning = b"[W] controlled runtime diagnostic must fail qualification\n"
        for mode, cancel in [("normal", False), ("output-failure", False), ("tail-loss", False), ("normal", True)]:
            for diagnostic in (b"", warning):
                with self.subTest(mode=mode, cancel=cancel, warning=bool(diagnostic)), \
                        tempfile.TemporaryDirectory(prefix="mixyard-diagnostic-case-", dir="/dev/shm") as temporary:
                    result, failure = self.launch_case(Path(temporary), mode=mode, cancel=cancel, stderr=diagnostic)
                    self.assertEqual(result["qualification"], "failure" if diagnostic else "success")
                    self.assertEqual(failure is not None, bool(diagnostic))
                    captured = result["diagnostics"][0]["stderr"]
                    self.assertEqual(base64.b64decode(captured["rawBase64"]), diagnostic)

    def test_arbitrary_non_utf8_and_oversized_client_output_cannot_qualify(self):
        cases = [(b"arbitrary bytes", None), (b"\xff", None),
                 (b"x" * (DIAGNOSTIC_LIMIT + 1), None),
                 (b"", b"\xff"), (b"", b"x" * (STDOUT_LIMIT + 1)),
                 (b"", b'{"error":0,"error":0}\n')]
        for stderr, stdout in cases:
            with self.subTest(stderr_bytes=len(stderr), stdout_bytes=len(stdout or b"")), \
                    tempfile.TemporaryDirectory(prefix="mixyard-diagnostic-invalid-", dir="/dev/shm") as temporary:
                result, failure = self.launch_case(Path(temporary), stderr=stderr, stdout=stdout)
                self.assertIsNotNone(failure)
                self.assertEqual(result["qualification"], "failure")
                for name, original in [("stdout", stdout), ("stderr", stderr)]:
                    captured = result["diagnostics"][0][name]
                    raw = base64.b64decode(captured["rawBase64"])
                    self.assertLessEqual(len(raw), captured["limit"] + 1)
                    if original is not None:
                        self.assertEqual(raw, original[:captured["limit"] + 1])

    def start_daemon(self, captures, root, diagnostic=b"", stream="stderr"):
        ready = root / "ready"
        script = '''import base64,pathlib,signal,sys,time
def stop(*_):
    target = getattr(sys, sys.argv[3]).buffer
    target.write(base64.b64decode(sys.argv[2])); target.flush()
    raise SystemExit(0)
signal.signal(signal.SIGTERM,stop)
pathlib.Path(sys.argv[1]).touch()
while True: time.sleep(.01)
'''
        daemon = captures.start([sys.executable, "-I", "-u", "-c", script, str(ready),
                                 base64.b64encode(diagnostic).decode(), stream],
                                dict(os.environ), "owned-daemon", "daemon")
        deadline = time.monotonic() + 5
        while not ready.exists():
            self.assertIsNone(daemon.process.poll())
            self.assertLess(time.monotonic(), deadline)
            time.sleep(.001)
        return daemon

    def test_late_daemon_diagnostics_and_bounds_are_gated_after_actual_stop(self):
        for diagnostic, stream in [(b"", "stderr"), (b"late daemon warning\n", "stderr"),
                                   (b"\xff", "stderr"), (b"x" * (DIAGNOSTIC_LIMIT + 1), "stderr"),
                                   (b"unexpected stdout", "stdout"), (b"x" * (DIAGNOSTIC_LIMIT + 1), "stdout")]:
            with self.subTest(stream=stream, bytes=len(diagnostic)), \
                    tempfile.TemporaryDirectory(prefix="mixyard-daemon-diagnostics-", dir="/dev/shm") as temporary:
                root = Path(temporary)
                captures = Captures()
                daemon = self.start_daemon(captures, root, diagnostic, stream)
                daemon.stop()
                failures = captures.finalize()
                completed = captures.write_evidence(root / "evidence.json", {}, "; ".join(failures) if failures else None)
                self.assertEqual(completed["qualification"], "failure" if diagnostic else "success")
                record = completed["diagnostics"][0]
                self.assertEqual(base64.b64decode(record[stream]["rawBase64"]), diagnostic)
                self.assertIsNotNone(daemon.process.returncode)
                self.assertFalse(daemon.reader.is_alive())

    def test_server_disconnect_exception_needs_exact_stopped_owner_and_core_cause(self):
        expected = f"native core error {-errno.EPIPE}: connection error\n".encode()
        variants = [(expected, None), (expected + b"[W] extra warning\n", None)]
        wrong = passing_report(server_loss=True)
        wrong["coreError"]["id"] = 7
        variants.append((expected, wrong))
        for diagnostic, report in variants:
            with self.subTest(extra=diagnostic != expected, wrong_core=report is not None), \
                    tempfile.TemporaryDirectory(prefix="mixyard-disconnect-diagnostics-", dir="/dev/shm") as temporary:
                root = Path(temporary)
                server_captures = Captures()
                daemon = self.start_daemon(server_captures, root)
                try:
                    result, failure = self.launch_case(root, server=daemon, stderr=diagnostic, report=report)
                    allowed = diagnostic == expected and report is None
                    self.assertEqual(failure is None, allowed)
                    self.assertEqual(result["qualification"], "success" if allowed else "failure")
                    self.assertEqual(base64.b64decode(result["diagnostics"][0]["stderr"]["rawBase64"]), diagnostic)
                finally:
                    self.assertEqual(server_captures.finalize(), [])


if __name__ == "__main__":
    unittest.main()

"""Bounded diagnostics for owned qualification children, never audio callbacks."""
import base64
import json
import os
import selectors
import signal
import subprocess
import threading
import time

STDOUT_LIMIT = 64 * 1024
DIAGNOSTIC_LIMIT = 16 * 1024
MAX_CAPTURES = 11  # Nine clients and two owned server epochs.
EVIDENCE_LIMIT = 4 * 1024 * 1024


def stop_owned(process):
    result = {"pid": process.pid, "requestedSignal": None, "escalated": False,
              "osClock": "CLOCK_MONOTONIC", "osTimeNamespace": None,
              "beforeSignalNs": None, "signalReturnedNs": None, "stopCompletedNs": None}
    try:
        result["osTimeNamespace"] = os.stat("/proc/self/ns/time").st_ino
    except OSError:
        pass  # Unavailable evidence never prevents stopping the owned child.
    def sample(field):
        try:
            result[field] = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
        except OSError:
            result[field] = None  # Unknown ordering never prevents owned shutdown.

    if process.poll() is None:
        result["requestedSignal"] = int(signal.SIGTERM)
        sample("beforeSignalNs")
        process.send_signal(signal.SIGTERM)
        sample("signalReturnedNs")
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            result["escalated"] = True
            process.kill()
            process.wait(timeout=5)
    sample("stopCompletedNs")
    result["returncode"] = process.returncode
    return result


class Capture:
    def __init__(self, command, env, name, role):
        self.name, self.role = name, role
        self.buffers = {"stdout": bytearray(), "stderr": bytearray()}
        self.limits = {"stdout": STDOUT_LIMIT if role == "client" else DIAGNOSTIC_LIMIT,
                       "stderr": DIAGNOSTIC_LIMIT}
        self.problem = None
        self.validation = "unavailable"
        self.rejection = None
        self.stop_result = None
        self.finished = False
        self.abort = threading.Event()
        self.process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.reader = threading.Thread(target=self._read, name="mixyard-diagnostics-" + name)
        try:
            self.reader.start()
        except BaseException:
            stop_owned(self.process)
            self.process.stdout.close()
            self.process.stderr.close()
            raise

    def _read(self):
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(self.process.stdout, selectors.EVENT_READ, "stdout")
                ready.register(self.process.stderr, selectors.EVENT_READ, "stderr")
                while ready.get_map():
                    if self.abort.is_set():
                        self.problem = self.problem or "capture did not reach EOF"
                        return
                    for key, _ in ready.select(0.1):
                        name = key.data
                        buffer = self.buffers[name]
                        chunk = os.read(key.fd, min(65536, self.limits[name] - len(buffer) + 1))
                        if not chunk:
                            ready.unregister(key.fileobj)
                            continue
                        buffer.extend(chunk)
                        if len(buffer) > self.limits[name]:
                            self.problem = name + "-limit"
                            self.process.kill()  # This Popen handle is owned by this capture.
                            return
        except Exception as error:
            self.problem = "diagnostic capture error: " + str(error)[:512]
            if self.process.poll() is None:
                self.process.kill()
        finally:
            self.process.stdout.close()
            self.process.stderr.close()

    def stop(self):
        if self.stop_result is None or self.process.poll() is None:
            self.stop_result = stop_owned(self.process)
        return self.stop_result

    def finish(self):
        if self.finished:
            return
        try:
            self.stop()
        finally:
            self.reader.join(timeout=5)
            if self.reader.is_alive():
                self.abort.set()
                self.reader.join(timeout=1)
                self.problem = self.problem or "diagnostic capture did not finish"
            if self.reader.is_alive():
                raise RuntimeError(self.name + ": diagnostic reader still active")
            self.finished = True

    def reject(self, reason):
        self.validation = "rejected"
        self.rejection = str(reason)[:2048]
        raise RuntimeError(self.name + ": " + self.rejection)

    def decoded(self):
        if not self.finished or self.reader.is_alive():
            self.reject("diagnostics are not complete")
        if self.problem:
            self.reject(self.problem)
        try:
            return tuple(bytes(self.buffers[name]).decode("utf-8", errors="strict")
                         for name in ("stdout", "stderr"))
        except UnicodeDecodeError:
            self.reject("undecodable UTF-8 diagnostics/report")

    def check_daemon(self):
        stdout, stderr = self.decoded()
        if stdout or stderr:
            self.reject("unexpected daemon diagnostic output")
        stop = self.stop_result
        if not stop or stop["requestedSignal"] != signal.SIGTERM or stop["escalated"] or stop["returncode"] not in (0, -signal.SIGTERM):
            self.reject("daemon did not complete the owned stop")
        self.validation = "clean"

    def record(self):
        result = {"name": self.name, "role": self.role, "pid": self.process.pid,
                  "returncode": self.process.returncode, "finished": self.finished,
                  "validation": self.validation, "rejection": self.rejection,
                  "captureProblem": self.problem, "stop": self.stop_result}
        for name, data in self.buffers.items():
            raw = bytes(data)
            try:
                text = raw.decode("utf-8", errors="strict")
            except UnicodeDecodeError:
                text = None
            result[name] = {"bytesCaptured": len(raw), "limit": self.limits[name],
                            "rawBase64": base64.b64encode(raw).decode("ascii"),
                            "utf8": text is not None,
                            "textPrefix": text[:2048] if text is not None else None}
        return result


class Captures:
    def __init__(self):
        self.items = []

    def start(self, command, env, name, role):
        if role not in ("client", "daemon") or any(c.name == name for c in self.items):
            raise RuntimeError("unknown role or duplicate owned diagnostic capture")
        if len(self.items) >= MAX_CAPTURES:
            raise RuntimeError("owned diagnostic capture count exceeded")
        capture = Capture(command, env, name, role)
        self.items.append(capture)
        return capture

    def finalize(self):
        failures = []
        for capture in self.items:
            try:
                capture.finish()
                if capture.role == "daemon":
                    capture.check_daemon()
                elif capture.validation not in ("clean", "expected-server-disconnect", "expected-post-owned-stop-unavailable"):
                    capture.reject(capture.rejection or "client did not pass complete qualification")
            except Exception as error:
                failures.append(str(error)[:2048])
        return failures

    def write_evidence(self, path, report, failure=None):
        report = dict(report, qualification="failure" if failure is not None else "success",
                      failure=(type(failure).__name__ + ": " + str(failure))[:2048] if failure is not None else None,
                      diagnostics=[capture.record() for capture in self.items])
        payload = json.dumps(report, separators=(",", ":")).encode("utf-8")
        if len(payload) > EVIDENCE_LIMIT:
            # Keep all bounded raw diagnostics, never truncate into a success.
            report = dict(qualification="failure", failure="qualification evidence size exceeded",
                          diagnostics=report["diagnostics"])
            payload = json.dumps(report, separators=(",", ":")).encode("utf-8")
            if len(payload) > EVIDENCE_LIMIT:
                raise RuntimeError("bounded diagnostic evidence exceeded its declared cap")
            failure = report["failure"]
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as file:
            file.write(payload + b"\n")
        return report

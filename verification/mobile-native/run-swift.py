#!/usr/bin/env python3
"""Provision trust only in an owned, disposable GitHub macOS runner.

This never runs on Mathis's machine. A fresh private keychain is added to the
runner search list temporarily. The production client still calls real Security
PKI/hostname verification and its exact certificate pin implementation.
"""
import os
from pathlib import Path
import re
import signal
import subprocess
import sys


def security(*args):
    return subprocess.run(["security", *args], check=True, capture_output=True, text=True).stdout


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_OS") != "macOS":
        raise RuntimeError("Swift trust qualification requires an owned hosted macOS runner")
    fixture = Path(os.environ["MIXYARD_NATIVE_FIXTURE"])
    keychain = fixture / "qualification.keychain-db"
    certificates = [fixture / name / "certificate.pem" for name in ("a", "b", "viewer")]
    original = re.findall(r'"([^"]+)"', security("list-keychains", "-d", "user"))
    trusted = []
    created = False
    child = None
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        security("create-keychain", "-p", "fixture-only", str(keychain)); created = True
        security("unlock-keychain", "-p", "fixture-only", str(keychain))
        security("list-keychains", "-d", "user", "-s", str(keychain), *original)
        for certificate in certificates:
            security("add-trusted-cert", "-r", "trustRoot", "-k", str(keychain), str(certificate))
            trusted.append(certificate)
        executable = Path(__file__).parent / "swift" / ".build" / "native-control-qualification"
        child = subprocess.Popen([str(executable)])
        return child.wait()
    finally:
        # All material belongs to this hosted fixture. Never remove another
        # certificate or keychain, and restore the original search list.
        if child is not None and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill(); child.wait()
        cleanup_failed = False
        for certificate in trusted:
            try:
                security("remove-trusted-cert", str(certificate))
            except subprocess.CalledProcessError:
                cleanup_failed = True
        try:
            security("list-keychains", "-d", "user", "-s", *original)
        finally:
            if created:
                security("delete-keychain", str(keychain))
        if cleanup_failed:
            raise RuntimeError("Owned fixture trust cleanup failed")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        # Captured Security output may contain paths; no credential contents or
        # provisioning state are ever printed by this driver.
        print("Owned hosted Swift trust qualification failed", file=sys.stderr)
        sys.exit(1)

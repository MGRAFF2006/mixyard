#!/usr/bin/python3
"""Owned discovery fixture; marker presence means both processes are ready."""
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).parent


def publish(name, pid):
    pending = root / (name + ".pending")
    pending.write_text(str(pid))
    pending.replace(root / name)


mode = (root / "mode").read_text()
if mode == "hang":
    child = subprocess.Popen(["/bin/sleep", "30"])
    publish("descendant", child.pid)
    # wait_pid() may cancel immediately after observing this final ready marker.
    publish("pid", os.getpid())
    child.wait()
if mode == "stderr":
    sys.stderr.write("private bind failure")
    sys.stderr.flush()
if mode == "fail":
    sys.exit(1)
if mode == "oversize":
    sys.stdout.write("x" * (4 * 1024 * 1024 + 1))
    sys.exit(0)
sys.stdout.write((root / "graph").read_text())

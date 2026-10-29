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
if mode in ("hang", "orphan"):
    program = pathlib.Path("/bin/sleep")
    if mode == "orphan":
        # Exercise /proc comm parsing as well as an already-exited group leader.
        program = root / "owned (child)"
        program.symlink_to("/bin/sleep")
    child = subprocess.Popen([str(program), "30"])
    publish("descendant", child.pid)
    # wait_pid() may cancel immediately after observing this final ready marker.
    publish("pid", os.getpid())
    if mode == "orphan":
        sys.exit(0)
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

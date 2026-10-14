#!/usr/bin/env python3
"""Offline correction checks; synthetic cases never count as audio qualification."""
import os
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

FIXTURE = Path(__file__).resolve().parent
sys.path.insert(0, str(FIXTURE.parent))
from linux_private_graph import Probe, qualify_revision_acks


def revision_checks():
    # Trace column layout is the runtime wire format. Consumer keys represent
    # packets that have independently passed the full expected-PCM comparison.
    traces = [[s, m, 512, 1000, 123456, 2, 256, 1, 0, 7, 999, 123456]
              for m in range(2) for s in range(2)]
    acks = {(m, 7): 512 for m in range(2)}
    pcm = {m: {(123456, 999, 256)} for m in range(2)}

    def check(rows=traces, ack=acks, revisions=(7,), packets=pcm, pre=(6,)):
        return qualify_revision_acks(rows, ack, revisions, packets, 2, 2, pre)

    valid = check()
    assert valid["status"] == "pass" and valid["matched_revision_frames"] == 2
    assert valid["pre_capture_revisions_uncovered"] == [6]
    assert check(ack={})["status"] == "fail"  # original false positive
    assert check(ack={(0, 7): 512})["status"] == "fail"
    assert check(revisions=(7, 8))["status"] == "fail"  # absent commanded revision
    assert check(rows=[])["status"] == "fail"
    assert check(rows=traces[:-1])["status"] == "fail"  # absent source trace
    assert check(ack={(m, 7): 768 for m in range(2)})["status"] == "fail"
    assert check(packets={})["status"] == "fail"  # missing/corrupt consumer PCM
    assert check(packets={0: pcm[0]})["status"] == "fail"
    assert check(packets={m: {(123712, 999, 256)} for m in range(2)})["status"] == "fail"
    assert check(packets={m: {(123456, 1000, 256)} for m in range(2)})["status"] == "fail"
    assert check(revisions=(), pre=(7,))["status"] == "blocked"
    print("PASS: revision ACK positive and 11 negative/uncovered cases")


def disk_checks():
    reserve = 10 * 1024**3
    cases = [(reserve, reserve, 0, 0), (reserve - 1, reserve, 0, 1),
             (reserve, reserve - 1, 0, 1), (reserve, reserve, 7, 7),
             (reserve, reserve - 1, 7, 1), ("invalid", reserve, 0, 1)]
    for script in ("build.sh", "run.sh"):
        for before, after, docker_exit, expected in cases:
            with tempfile.TemporaryDirectory(prefix="mixyard-native-proof-selfcheck-") as tmp:
                work = Path(tmp)
                # Both commands are mocks. Never build, execute a graph, or fill disk.
                (work / "df").write_text('''#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == "--output=avail -B1 ." ]]
echo df >> "$MOCK_LOG"
echo Avail
if [[ ! -f "$MOCK_SEEN" ]]; then
  touch "$MOCK_SEEN"
  echo "$MOCK_BEFORE"
else
  echo "$MOCK_AFTER"
fi
''')
                (work / "docker").write_text('''#!/usr/bin/env bash
set -euo pipefail
echo "docker $*" >> "$MOCK_LOG"
if [[ "$1" == image ]]; then echo synthetic-image; exit 0; fi
exit "$MOCK_DOCKER_EXIT"
''')
                for name in ("df", "docker"):
                    (work / name).chmod(0o700)
                env = dict(os.environ, PATH=f"{work}:{os.environ['PATH']}",
                           MOCK_LOG=str(work / "commands"), MOCK_SEEN=str(work / "seen"),
                           MOCK_BEFORE=str(before), MOCK_AFTER=str(after),
                           MOCK_DOCKER_EXIT=str(docker_exit))
                result = subprocess.run(["bash", str(FIXTURE / script), str(work / "evidence")],
                                        env=env, capture_output=True, text=True, timeout=10)
                assert result.returncode == expected, (script, before, after, result)
                commands = (work / "commands").read_text().splitlines()
                assert commands[0] == commands[-1] == "df" and commands.count("df") == 2
                if before == reserve - 1 or before == "invalid":
                    assert commands == ["df", "df"]  # no docker invocation
    print("PASS: 12 exact-byte build/run disk cases, including command failures")


def historical_replay(recordings, destination):
    """Read old packets; replay only the analyzer into a NEW evidence directory.

    No process, server, graph, start command or synthetic PCM is executed here.
    The old samples retain their exact positions, first/tail samples and gaps.
    """
    destination.mkdir(mode=0o700)
    results = []
    for stage in ("startup", "edits", "recovery"):
        class Replay(Probe):
            def __init__(self):
                # Avoid Probe's private server-directory setup on the host.
                self.evidence = destination
                self.checks = []
                self.processes = []
                self.current_revision = 6
                self.commanded_revisions = list(range(1, 7))
                self.coefficient_history = {}
                self.runtime_log = recordings / f"frame-{stage}-trace.log"
                status = self.runtime_log.read_text().rsplit("status revision=", 1)[-1]
                self.gate_first = [int(frame) for _, frame in sorted(
                    re.findall(r"source (\d+).*?first=(\d+)", status))]
                assert len(self.gate_first) == 2

            def start(self, args, name, control=False):
                target = Path(args[-1])
                shutil.copyfile(recordings / target.name, target)
                return SimpleNamespace(stdin=io.StringIO(), returncode=0), destination / f"{name}.log"

            def command(self, command, expect=None):
                if command.startswith("set "):
                    self.current_revision += 1
                    self.commanded_revisions.append(self.current_revision)

            def run(self, args, **kwargs):
                return SimpleNamespace(returncode=0)

            def graph(self, name):
                return []

            def stop(self, child):
                pass

        replay = Replay()
        with patch("linux_private_graph.time.sleep"), patch(
                "linux_private_graph.qualify_revision_acks", wraps=qualify_revision_acks) as ack:
            replay.diagnose_frames(["Conference mix", "Session archive α", "Headphones private"],
                                   stage=stage, open_gate=stage != "edits", edit=stage == "edits")
        results.extend(replay.checks)
        ack_result = replay.checks[-1]
        assert ack_result["status"] == ("pass" if stage == "edits" else "blocked")
        if stage == "edits":
            assert ack_result["matched_revision_frames"] == 12
            args = list(ack.call_args.args)
            args[1] = {}
            assert qualify_revision_acks(*args)["status"] == "fail"
            args = list(ack.call_args.args)
            args[0] = [row for row in args[0] if row[9] != 7]
            assert qualify_revision_acks(*args)["status"] == "fail"
            args = list(ack.call_args.args)
            args[3] = {}
            assert qualify_revision_acks(*args)["status"] == "fail"
        for check in replay.checks[:-1]:
            expected = "fail" if stage == "startup" and check["endpoint"] in (
                "fixture.mic", "Headphones private") else "pass"
            assert check["status"] == expected, check
            assert check["compared_all_samples"] and check["declared_alignment_frames"] == 0
    (destination / "offline-reanalysis.json").write_text(json.dumps({
        "kind": "offline analyzer replay of historical packets; no graph/audio rerun",
        "recorded_source_commit": "72f621430025959de87d9603f4f51de06d56a394",
        "checks": results}, indent=2))
    print("PASS: 15 historical full-sample comparisons retain both gaps; scoped ACK checks and real-packet negatives")


if __name__ == "__main__":
    revision_checks()
    disk_checks()
    if len(sys.argv) == 3:
        historical_replay(Path(sys.argv[1]), Path(sys.argv[2]))
    elif len(sys.argv) != 1:
        raise SystemExit("usage: selfcheck.py [HISTORICAL-RECORDINGS NEW-REANALYSIS-DIRECTORY]")

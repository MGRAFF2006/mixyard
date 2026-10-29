#!/usr/bin/env python3
"""Fixture/process tests only; no native discovery or audio is invoked."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location('qualification', Path(__file__).with_name('qualify.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
CORE = {'id': 0, 'type': 'PipeWire:Interface:Core'}


class EvidenceValidation(unittest.TestCase):
    def test_rejects_incomplete_or_ambiguous_metadata(self):
        for value in ({}, [], [CORE, CORE], [{'id': 1, 'type': 'PipeWire:Interface:Node'}],
                      [{'id': True, 'type': 'PipeWire:Interface:Core'}],
                      [{**CORE, 'id': -1}], [{**CORE, 'id': 2**32}], [CORE] * 8193):
            with self.subTest(value=str(value)[:60]), self.assertRaises(ValueError):
                module.inventory_summary(value)

    def test_observed_objects_are_counted_without_identity_disclosure(self):
        self.assertEqual(module.inventory_summary([
            CORE, {'id': 2**32 - 1, 'type': 'PipeWire:Interface:Node'}]),
            {'PipeWire:Interface:Core': 1, 'PipeWire:Interface:Node': 1})

    def test_arbitrary_type_suffix_is_not_reported(self):
        for kind in ('PipeWire:Interface:private-app-secret', 'PipeWire:Interface:', 'other'):
            with self.subTest(kind=kind), self.assertRaises(ValueError) as caught:
                module.inventory_summary([CORE, {'id': 1, 'type': kind}])
            self.assertNotIn(kind, str(caught.exception))

    def test_duplicate_keys_and_nonfinite_literals_are_rejected_at_decode(self):
        for raw in (b'[{"id":1,"id":0,"type":"PipeWire:Interface:Core"}]',
                    b'{"info":{"private":true,"private":false}}', b'[NaN]', b'[Infinity]'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                module.strict_json(raw)
        self.assertEqual(module.strict_json(b'[{"id":0,"type":"PipeWire:Interface:Core"}]'), [CORE])


class EvidencePaths(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / 'verification').mkdir()
        self.allowed = self.root / 'verification/local-evidence'
        self.outside = self.root / 'public'
        self.outside.mkdir()

    def tearDown(self):
        self.temp.cleanup()

    def test_root_and_intermediate_symlinks_never_receive_evidence(self):
        self.allowed.symlink_to(self.outside, target_is_directory=True)
        with self.assertRaises(OSError):
            with module.evidence_directory(self.root, self.allowed / 'new'):
                self.fail('followed symlink')
        self.allowed.unlink()
        self.allowed.mkdir(mode=0o700)
        (self.allowed / 'redirect').symlink_to(self.outside, target_is_directory=True)
        with self.assertRaises(OSError):
            with module.evidence_directory(self.root, self.allowed / 'redirect/new'):
                self.fail('followed symlink')
        self.assertEqual(list(self.outside.iterdir()), [])

    def test_outside_existing_and_nonprivate_directories_are_rejected(self):
        with self.assertRaises(ValueError):
            with module.evidence_directory(self.root, self.outside / 'new'):
                self.fail('accepted outside path')
        self.allowed.mkdir(mode=0o755)
        with self.assertRaises(ValueError):
            with module.evidence_directory(self.root, self.allowed / 'new'):
                self.fail('accepted shared evidence root')
        self.allowed.chmod(0o700)
        (self.allowed / 'existing').mkdir(mode=0o700)
        with self.assertRaises(ValueError):
            with module.evidence_directory(self.root, self.allowed / 'existing'):
                self.fail('overwrote existing evidence')

    def test_files_are_exclusive_private_and_relative_to_pinned_directory(self):
        output = self.allowed / 'nested/new'
        with module.evidence_directory(self.root, output) as fd:
            self.assertEqual(output.stat().st_mode & 0o777, 0o700)
            target = self.outside / 'untouched'
            target.write_text('original')
            (output / 'report.json').symlink_to(target)
            with self.assertRaises(FileExistsError):
                with module.private_file('report.json', fd):
                    self.fail('followed report symlink')
            self.assertEqual(target.read_text(), 'original')
            # Replacing the pathname cannot redirect pinned descriptor writes.
            moved = self.allowed / 'moved'
            output.rename(moved)
            output.symlink_to(self.outside, target_is_directory=True)
            with module.private_file('raw.json', fd) as stream:
                stream.write(b'{}')
            self.assertEqual((moved / 'raw.json').stat().st_mode & 0o777, 0o600)
            self.assertFalse((self.outside / 'raw.json').exists())


class OwnedProcesses(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def command(self, mode):
        code = ('import json,os,pathlib,subprocess,sys,time; '
                'child=subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"]); '
                f'pathlib.Path({str(self.root / "pids")!r}).write_text(json.dumps([os.getpid(),child.pid])); '
                + ('time.sleep(60)' if mode == 'hang' else 'print("{}")'))
        return [sys.executable, '-c', code]

    def assert_stopped(self, pids):
        deadline = time.monotonic() + 2
        while True:
            active = []
            for pid in pids:
                path = Path(f'/proc/{pid}/stat')
                try:
                    state = path.read_text().rsplit(')', 1)[1].split()[0]
                except FileNotFoundError:
                    continue
                if state != 'Z':
                    active.append(pid)
            if not active:
                return
            if time.monotonic() >= deadline:
                self.fail(f'owned processes still active: {active}')
            time.sleep(0.01)

    def test_timeout_cleans_owned_group_and_reaps_leader(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            module.capture(self.command('hang'), self.root / 'timeout.json', timeout=0.5)
        pids = json.loads((self.root / 'pids').read_text())
        self.assert_stopped(pids)
        with self.assertRaises(ChildProcessError):
            os.waitpid(pids[0], os.WNOHANG)

    def test_normal_exit_cleans_lingering_owned_descendant(self):
        self.assertEqual(module.capture(self.command('exit'), self.root / 'ok.json'), {})
        self.assert_stopped(json.loads((self.root / 'pids').read_text()))

    def test_sigterm_cancels_capture_before_reporting_completion(self):
        code = (f'import importlib.util,pathlib; s=importlib.util.spec_from_file_location("q",{str(Path(module.__file__))!r}); '
                'q=importlib.util.module_from_spec(s); s.loader.exec_module(q); '
                f'q.capture({self.command("hang")!r},pathlib.Path({str(self.root / "cancel.json")!r}))')
        runner = subprocess.Popen([sys.executable, '-B', '-c', code], stdout=subprocess.DEVNULL,
                                  stderr=subprocess.DEVNULL)
        pids = []
        try:
            deadline = time.monotonic() + 3
            while not (self.root / 'pids').exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.01)
            pids = json.loads((self.root / 'pids').read_text())
            runner.send_signal(signal.SIGTERM)
            self.assertNotEqual(runner.wait(timeout=3), 0)
            self.assert_stopped(pids)
        finally:
            if runner.poll() is None:
                try:
                    runner.send_signal(signal.SIGTERM)
                    runner.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    runner.kill()
                    runner.wait()

    def test_stderr_nonzero_oversize_and_invalid_json_never_succeed(self):
        for i, code in enumerate(('import sys; print("{}"); sys.stderr.write("private")',
                                  'raise SystemExit(1)', 'print("not json")',
                                  f'import sys; sys.stdout.write("x"*{module.LIMIT + 1})')):
            with self.subTest(code=code), self.assertRaises(ValueError) as caught:
                module.capture([sys.executable, '-c', code], self.root / f'bad{i}.json')
            self.assertNotIn('private', str(caught.exception))


if __name__ == '__main__':
    unittest.main()

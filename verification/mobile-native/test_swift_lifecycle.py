"""Wrapper ownership/failure units; never execute Security, Swift or PKI locally."""
import contextlib
import errno
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('swift_lifecycle', Path(__file__).with_name('run-swift.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class TrustLifecycle(unittest.TestCase):
    def test_partial_preparation_failure_restores_and_removes_only_owned_material(self):
        with tempfile.TemporaryDirectory(prefix='mixyard-swift-lifecycle-', dir=os.environ.get('RUNNER_TEMP', '/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir())) as root:
            calls = []
            def owned(label, deadline, *args, **kwargs):
                calls.append((label, args))
                if label == 'trust-a': raise RuntimeError('owned-failure')
                return '\"/owned-original.keychain-db\"' if label in ('startup-list', 'cleanup-verify') else ''
            with patch.dict(os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','MIXYARD_NATIVE_FIXTURE':root}), patch.object(module, 'security', side_effect=owned), patch.object(module, 'phase'), patch.object(module.signal, 'signal'), patch.object(module.subprocess, 'Popen') as child:
                with self.assertRaises(RuntimeError): module.main()
                child.assert_not_called()
            labels = [call[0] for call in calls]
            self.assertLess(labels.index('cleanup-search'), labels.index('cleanup-trust-a'))
            self.assertIn('cleanup-verify', labels)
            self.assertIn('cleanup-keychain', labels)
            restore = next(args for label,args in calls if label=='cleanup-search')
            self.assertEqual(restore[-1], '/owned-original.keychain-db')
            self.assertNotIn('cleanup-trust-b', labels)

    def test_failed_restoration_is_reported_but_other_owned_cleanup_is_attempted(self):
        with tempfile.TemporaryDirectory(prefix='mixyard-swift-lifecycle-', dir=os.environ.get('RUNNER_TEMP', '/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir())) as root:
            calls = []
            def owned(label, deadline, *args, **kwargs):
                calls.append(label)
                if label == 'cleanup-search': raise RuntimeError('owned-restore-failure')
                return '\"/owned-original.keychain-db\"' if label in ('startup-list','cleanup-verify') else ''
            with patch.dict(os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','MIXYARD_NATIVE_FIXTURE':root}), patch.object(module, 'security', side_effect=owned), patch.object(module, 'phase'), patch.object(module.signal, 'signal'), patch.object(module.subprocess, 'Popen') as child:
                child.return_value.wait.return_value = 0
                child.return_value.poll.return_value = 0
                with self.assertRaisesRegex(RuntimeError, 'owned-trust-cleanup-failed'): module.main()
            for label in ('cleanup-verify','cleanup-trust-a','cleanup-trust-b','cleanup-trust-viewer','cleanup-keychain'):
                self.assertIn(label, calls)

    def test_security_timeout_is_bounded_and_does_not_export_captured_output(self):
        error = subprocess.TimeoutExpired(['security','private-argument'], 1, output='private-output', stderr='private-stderr')
        with patch.object(module, 'phase'), patch.object(module.subprocess, 'run', side_effect=error) as run, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, '^owned-security-phase-failed$'):
                module.security('startup-list', time.monotonic()+1, 'list-keychains')
            self.assertLessEqual(run.call_args.kwargs['timeout'], 1)
            self.assertTrue(run.call_args.kwargs['capture_output'])

    def test_failure_classifications_are_fixed_numeric_and_never_reveal_captured_material(self):
        cases = [
            (subprocess.TimeoutExpired(['security','SECRET-ARG'], 4, output='SECRET-OUT', stderr='SECRET-ERR'), 'timeout', 4000),
            (subprocess.CalledProcessError(1, ['security','SECRET-ARG'], output='SECRET-OUT', stderr='SECRET-ERR'), 'exit', 1),
            (subprocess.CalledProcessError(-15, ['security','SECRET-ARG'], stderr='SECRET-ERR'), 'signal', 15),
            (OSError(errno.ENOENT, 'SECRET-PATH'), 'os-error', errno.ENOENT),
        ]
        for error,kind,value in cases:
            with self.subTest(kind=kind):
                output=io.StringIO()
                with patch.object(module.subprocess, 'run', side_effect=error), contextlib.redirect_stderr(output):
                    with self.assertRaisesRegex(RuntimeError, '^owned-security-phase-failed$'):
                        module.security('trust-a', time.monotonic()+10, 'SECRET-ARG')
                lines=output.getvalue().splitlines()
                report=json.loads(next(line.removeprefix('MIXYARD_SECURITY ') for line in lines if line.startswith('MIXYARD_SECURITY ')))
                self.assertEqual(report, {'phase':'trust-a','kind':kind,'value':value})
                self.assertNotIn('SECRET', output.getvalue())

    def test_expired_budget_starts_no_os_command_and_unknown_phase_cannot_inject_a_log(self):
        output=io.StringIO()
        with patch.object(module.subprocess, 'run') as run, contextlib.redirect_stderr(output):
            with self.assertRaises(RuntimeError): module.security('trust-a', time.monotonic()-1, 'SECRET-ARG')
            with self.assertRaises(RuntimeError): module.security('SECRET-PHASE', time.monotonic()+1, 'SECRET-ARG')
            run.assert_not_called()
        self.assertIn('"kind":"budget-exhausted"', output.getvalue())
        self.assertNotIn('SECRET', output.getvalue())

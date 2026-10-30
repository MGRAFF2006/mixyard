"""Wrapper ownership/failure units; never execute Security, Swift or PKI locally."""
import importlib.util
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
        with patch.object(module, 'phase'), patch.object(module.subprocess, 'run', side_effect=error) as run:
            with self.assertRaisesRegex(RuntimeError, '^owned-security-phase-failed$'):
                module.security('startup-list', time.monotonic()+1, 'list-keychains')
            self.assertLessEqual(run.call_args.kwargs['timeout'], 1)
            self.assertTrue(run.call_args.kwargs['capture_output'])

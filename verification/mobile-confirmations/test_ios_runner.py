"""Selection/cleanup regressions only; no local SDK, simulator or GUI calls."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('confirmation_runner', Path(__file__).with_name('run-ios.py'))
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)


def runtime(number, available=True):
    return {'identifier':'com.apple.CoreSimulator.SimRuntime.iOS-' + number.replace('.', '-'), 'version':number, 'isAvailable':available}


class SimulatorSelection(unittest.TestCase):
    def test_selected_sdk_wins_over_a_newer_arbitrary_installed_runtime(self):
        result = module.select_runtime('18.5', [runtime('18.4'), runtime('18.5'), runtime('26.2')], 'arm64')
        self.assertEqual(result['version'], '18.5')
        with self.assertRaises(RuntimeError): module.select_runtime('18.5', [runtime('26.2')], 'arm64')
        with self.assertRaises(RuntimeError): module.select_runtime('18.5', [runtime('18.5', False)], 'arm64')

    def test_unknown_architecture_does_not_silently_select_another_target(self):
        with self.assertRaises(RuntimeError): module.select_runtime('18.5', [runtime('18.5')], 'unknown')

    def test_failed_actual_test_preserves_single_architecture_and_disposes_only_owned_uuid(self):
        owned = '11111111-2222-3333-4444-555555555555'
        def capture(*args):
            if args[:2] == ('xcodebuild', '-version'): return 'Xcode16.4'
            if '--show-sdk-version' in args: return '18.5'
            if 'runtimes' in args: return json.dumps({'runtimes':[runtime('26.2'), runtime('18.5')]})
            if 'create' in args: return owned
            if '-showdestinations' in args: return '{ platform:iOS Simulator, arch:arm64, id:' + owned + ' }'
            raise AssertionError('Unexpected owned command')
        calls=[]
        def run(args, **kwargs):
            calls.append(args)
            if args[:2] == ['xcodebuild','test']: raise subprocess.CalledProcessError(65, 'owned-xcodebuild')
        with patch.dict(module.os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','RUNNER_TEMP':'/owned'}), patch.object(module.platform, 'machine', return_value='arm64'), patch.object(module, 'capture', side_effect=capture), patch.object(module.subprocess, 'run', side_effect=run), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(subprocess.CalledProcessError): module.main()
        command=next(args for args in calls if args[:2]==['xcodebuild','test'])
        self.assertIn('ARCHS=arm64', command); self.assertIn('ONLY_ACTIVE_ARCH=YES', command)
        self.assertIn('platform=iOS Simulator,id='+owned+',arch=arm64', command)
        self.assertEqual(calls[-2:], [['xcrun','simctl','shutdown',owned], ['xcrun','simctl','delete',owned]])

    def test_boot_timeout_stays_primary_when_shutdown_and_delete_also_time_out(self):
        calls=[]
        identifier='11111111-2222-3333-4444-555555555555'
        def capture(*args):
            if args[:2]==('xcodebuild','-version'): return 'Xcode16.4'
            if '--show-sdk-version' in args: return '18.5'
            if 'runtimes' in args: return json.dumps({'runtimes':[runtime('18.5')]})
            if 'create' in args: return identifier
            if '-showdestinations' in args: return '{ arch:arm64, id:'+identifier+' }'
            raise AssertionError('Unexpected owned capture')
        def run(args, **kwargs):
            calls.append((args,kwargs['timeout']))
            if any(value in args for value in ['bootstatus','shutdown','delete']):
                raise subprocess.TimeoutExpired(args, kwargs['timeout'])
        evidence=io.StringIO()
        with patch.dict(module.os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','RUNNER_TEMP':'/owned'}), patch.object(module.platform,'machine',return_value='arm64'), patch.object(module,'capture',side_effect=capture), patch.object(module.subprocess,'run',side_effect=run), contextlib.redirect_stdout(evidence):
            with self.assertRaises(subprocess.TimeoutExpired) as failure: module.main()
        self.assertEqual(failure.exception.timeout,180)
        self.assertIn('bootstatus',failure.exception.cmd)
        self.assertEqual(len(failure.exception.__cause__.exceptions),2)
        self.assertEqual([limit for args,limit in calls],[30,180,30,30])
        self.assertNotIn('actual-ui-test-start',evidence.getvalue())
        for label in ['bootstatus','cleanup-shutdown','cleanup-delete']:
            self.assertIn('"phase": "'+label+'", "kind": "timeout"',evidence.getvalue())

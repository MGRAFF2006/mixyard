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
    def test_tool_identity_is_read_only_and_rejects_a_different_xcode(self):
        developer='/Applications/OwnedXcode.app/Contents/Developer'
        def capture(label,*_args):
            return developer+'/usr/bin/simctl' if label=='resolved-simctl' else developer
        data=module.plistlib.dumps({'CFBundleShortVersionString':'1032.2','CFBundleVersion':'SECRET-NOT-NUMERIC'})
        evidence=io.StringIO()
        with patch.object(module,'capture',side_effect=capture),patch.object(module,'step') as step,patch.object(module.Path,'read_bytes',return_value=data),contextlib.redirect_stdout(evidence):
            module.toolchain_evidence()
        step.assert_called_once_with('first-launch-status',['xcodebuild','-checkFirstLaunchStatus'],30)
        self.assertIn('"simctlMatchesSelectedXcode": true',evidence.getvalue())
        self.assertNotIn('SECRET',evidence.getvalue());self.assertNotIn('/Applications',evidence.getvalue())
        with patch.object(module,'capture',side_effect=['/different/tool/simctl',developer]),patch.object(module,'step'),patch.object(module.Path,'read_bytes',return_value=data),contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(RuntimeError): module.toolchain_evidence()

    def test_explicit_comparison_requires_exact_preinstalled_inputs_without_fallback(self):
        with patch.dict(module.os.environ,{'MIXYARD_CONFIRMATION_COMPARISON':'xcode16.2-sdk18.2','DEVELOPER_DIR':'/Applications/Xcode_16.2.app/Contents/Developer'}),patch.object(module.Path,'is_dir',return_value=True),contextlib.redirect_stdout(io.StringIO()):
            module.validate_comparison('Xcode 16.2\nBuild version 16C5032a','18.2')
            for xcode,sdk in [('Xcode 16.4','18.2'),('Xcode 16.2','18.5')]:
                with self.assertRaises(RuntimeError):module.validate_comparison(xcode,sdk)
            with patch.object(module.Path,'is_dir',return_value=False):
                with self.assertRaises(RuntimeError):module.validate_comparison('Xcode 16.2','18.2')

    def test_destination_diagnostic_exposes_only_exact_owned_arch_platform_availability(self):
        owned='11111111-2222-3333-4444-555555555555'
        raw='Available destinations for SECRET scheme:\n{ platform:iOS Simulator, id:'+owned+', name:SECRET }\n{ platform:iOS Simulator, arch:arm64, id:OTHER, name:SECRET }'
        value=module.destination_evidence(raw,owned)
        self.assertEqual(value['records'],[{'platform':'iOS Simulator','arch':'missing-or-other','archPresent':False,'errorPresent':False,'availability':'available','duplicateFields':False}])
        self.assertNotIn('SECRET',json.dumps(value));self.assertNotIn(owned,json.dumps(value))
    def test_shutdown149_requires_exact_owned_shutdown_readback_within_existing_budget(self):
        owned='11111111-2222-3333-4444-555555555555'
        with patch.object(module,'step',side_effect=subprocess.CalledProcessError(149,'owned-shutdown')),patch.object(module,'capture',return_value=json.dumps({'devices':{'runtime':[{'udid':owned,'state':'Shutdown'}]}})) as capture,contextlib.redirect_stdout(io.StringIO()):
            module.shutdown_owned(owned)
        self.assertLessEqual(capture.call_args.kwargs['timeout'],3)
        for devices in [[{'udid':owned,'state':'Booted'}],[{'udid':'OTHER','state':'Shutdown'}],[]]:
            with patch.object(module,'step',side_effect=subprocess.CalledProcessError(149,'owned-shutdown')),patch.object(module,'capture',return_value=json.dumps({'devices':{'runtime':devices}})):
                with self.assertRaises(subprocess.CalledProcessError):module.shutdown_owned(owned)
        with patch.object(module,'step',side_effect=subprocess.CalledProcessError(1,'owned-shutdown')),patch.object(module,'capture') as capture:
            with self.assertRaises(subprocess.CalledProcessError):module.shutdown_owned(owned)
            capture.assert_not_called()

    def test_selected_sdk_wins_over_a_newer_arbitrary_installed_runtime(self):
        result = module.select_runtime('18.5', [runtime('18.4'), runtime('18.5'), runtime('26.2')], 'arm64')
        self.assertEqual(result['version'], '18.5')
        with self.assertRaises(RuntimeError): module.select_runtime('18.5', [runtime('26.2')], 'arm64')
        with self.assertRaises(RuntimeError): module.select_runtime('18.5', [runtime('18.5', False)], 'arm64')

    def test_unknown_architecture_does_not_silently_select_another_target(self):
        with self.assertRaises(RuntimeError): module.select_runtime('18.5', [runtime('18.5')], 'unknown')

    def test_failed_actual_test_preserves_single_architecture_and_disposes_only_owned_uuid(self):
        owned = '11111111-2222-3333-4444-555555555555'
        def capture(_label, *args):
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
        with patch.dict(module.os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','RUNNER_TEMP':'/owned'}), patch.object(module.platform, 'machine', return_value='arm64'), patch.object(module, 'capture', side_effect=capture), patch.object(module, 'process_evidence'),patch.object(module,'toolchain_evidence'), patch.object(module.subprocess, 'run', side_effect=run), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(subprocess.CalledProcessError): module.main()
        command=next(args for args in calls if args[:2]==['xcodebuild','test'])
        self.assertIn('SWIFT_ACTIVE_COMPILATION_CONDITIONS=DEBUG OWNED_CONFIRMATION_EVIDENCE',command)
        self.assertIn('ARCHS=arm64', command); self.assertIn('ONLY_ACTIVE_ARCH=YES', command)
        self.assertIn('platform=iOS Simulator,id='+owned+',arch=arm64', command)
        self.assertEqual(calls[-2:], [['xcrun','simctl','shutdown',owned], ['xcrun','simctl','delete',owned]])

    def test_boot_timeout_stays_primary_when_shutdown_and_delete_also_time_out(self):
        calls=[]
        identifier='11111111-2222-3333-4444-555555555555'
        def capture(_label, *args):
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
        with patch.dict(module.os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','RUNNER_TEMP':'/owned'}), patch.object(module.platform,'machine',return_value='arm64'), patch.object(module,'capture',side_effect=capture), patch.object(module,'process_evidence'),patch.object(module,'toolchain_evidence'), patch.object(module.subprocess,'run',side_effect=run), contextlib.redirect_stdout(evidence):
            with self.assertRaises(subprocess.TimeoutExpired) as failure: module.main()
        self.assertEqual(failure.exception.timeout,180)
        self.assertIn('bootstatus',failure.exception.cmd)
        self.assertEqual(len(failure.exception.__cause__.exceptions),2)
        self.assertEqual([limit for args,limit in calls],[30,180,30,30])
        self.assertNotIn('actual-ui-test-start',evidence.getvalue())
        for label in ['bootstatus','cleanup-shutdown','cleanup-delete']:
            self.assertIn('"phase": "'+label+'", "kind": "timeout"',evidence.getvalue())

    def test_discovery_capture_keeps_30s_bound_and_filters_process_evidence_without_retry(self):
        raw = '7 1 /private/SECRET/CoreSimulatorService\n8 1 /private/SECRET/unrelated-app\n9 1 /usr/bin/simctl\n'
        evidence=io.StringIO()
        with patch.object(module.subprocess,'check_output',side_effect=subprocess.TimeoutExpired(['owned-simctl'],30,output='SECRET-OUTPUT')) as run, patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],0,stdout=raw)), contextlib.redirect_stdout(evidence):
            with self.assertRaises(subprocess.TimeoutExpired): module.capture('runtime-discovery','xcrun','simctl','list','runtimes','--json')
        run.assert_called_once()
        self.assertEqual(run.call_args.kwargs['timeout'],30)
        self.assertIn('"phase": "runtime-discovery", "kind": "timeout", "value": 30',evidence.getvalue())
        self.assertIn('CoreSimulatorService',evidence.getvalue()); self.assertIn('simctl',evidence.getvalue())
        self.assertNotIn('SECRET',evidence.getvalue()); self.assertNotIn('unrelated-app',evidence.getvalue())

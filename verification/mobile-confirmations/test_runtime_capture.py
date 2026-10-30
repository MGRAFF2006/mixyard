"""Actual adapter boundary, mocked Apple tool or bounded own Python only."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import patch
import owned_simulator_state as state

RAW=json.dumps({'runtimes':[{'identifier':'com.apple.CoreSimulator.SimRuntime.iOS-18-5','version':'18.5','isAvailable':True}]}).encode()


def receipt(raw):
    return {'status':'COMPLETE','reason':'ok','toolValue':0,'stdoutBytes':len(raw),'stderrBytes':0,
            'directChildReaped':True,'ownedChildRetained':False,'pipesEof':True,'localPipesClosed':True}


class RuntimeAdapter(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec=importlib.util.spec_from_file_location('runtime_runner',Path(__file__).with_name('run-ios.py'))
        cls.runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(cls.runner)
    def run_adapter(self,raw=RAW,meta=None,fail=False,reader=None):
        output=io.StringIO();meta=receipt(raw) if meta is None else meta
        with patch.dict(state.os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS'}), \
             patch.object(state,'read_devices',side_effect=reader,return_value=(raw,meta)) as read,contextlib.redirect_stdout(output):
            if fail:
                with self.assertRaisesRegex(RuntimeError,'Owned runtime discovery capture failed'):
                    state.capture_runtime_catalogue()
                text=None
            else:text=state.capture_runtime_catalogue()
        read.assert_called_once();self.assertEqual(read.call_args.args[1],state.RUNTIMES)
        for secret in ['SECRET','/PRIVATE','--json','xcrun']:self.assertNotIn(secret,output.getvalue())
        lines=[line for line in output.getvalue().splitlines() if line.startswith('Owned simulator runtime capture: ')]
        self.assertEqual(len(lines),1)
        return text,json.loads(lines[0].split(': ',1)[1]),read.call_args.args[0]
    def test_complete_utf8_feeds_same_legacy_runtime_json_and_selection(self):
        start=time.monotonic();text,result,deadline=self.run_adapter()
        selected=self.runner.select_runtime('18.5',json.loads(text)['runtimes'],'arm64')
        self.assertEqual(selected['identifier'],'com.apple.CoreSimulator.SimRuntime.iOS-18-5')
        self.assertTrue(result['complete']);self.assertEqual(result['configuredSeconds'],30)
        self.assertLessEqual(deadline-start,30.01)
        self.assertTrue(result['capture']['localPipesClosed']);self.assertTrue(result['capture']['pipesEof'])
    def test_unknown_partial_retained_or_localclose_only_output_never_reaches_selector(self):
        changes=[{'status':'UNKNOWN'},{'toolValue':7},{'directChildReaped':False},{'ownedChildRetained':True},
                 {'pipesEof':False},{'localPipesClosed':False},{'stdoutBytes':state.MAX_STDOUT+1},{'stdoutBytes':0},{'stderrBytes':1}]
        for change in changes:
            meta=receipt(RAW);meta.update(change)
            with self.subTest(fields=list(change)):
                text,result,_=self.run_adapter(meta=meta,fail=True)
                self.assertIsNone(text);self.assertFalse(result['complete'])
        _,result,_=self.run_adapter(raw=b'\xff',fail=True)
        self.assertEqual(result['reason'],'utf8-invalid')
    def test_real_owned_child_timeout_nonzero_cap_and_eof_vs_local_close(self):
        original=state.read_devices
        for code,cap,reason in [('import time;time.sleep(10)',None,'timeout'),
                                ('raise SystemExit(7)',None,'tool-exit'),
                                ('import sys;sys.stdout.write("x"*10000)',128,'stdout-limit')]:
            def reader(deadline,command):
                self.assertEqual(command,state.RUNTIMES)
                return original(deadline,[sys.executable,'-B','-c',code])
            with patch.object(state,'RUNTIME_SECONDS',0.5),patch.object(state,'STOP_RESERVE',0.1), \
                 patch.object(state,'MAX_STDOUT',cap or (256*1024)):
                start=time.monotonic();_,result,_=self.run_adapter(fail=True,reader=reader)
                self.assertLess(time.monotonic()-start,2) # Scheduling tolerance, not a realtime guarantee.
            self.assertEqual(result['capture']['reason'],reason)
            self.assertTrue(result['capture']['directChildReaped'])
            self.assertTrue(result['capture']['localPipesClosed'])
            self.assertFalse(result['capture']['ownedChildRetained'])
            if reason=='timeout':self.assertFalse(result['capture']['pipesEof'])
    def test_retained_child_fails_without_another_spawn_or_false_completion(self):
        original=state.read_devices
        with patch.object(state,'_HELD_CHILD',object()),patch.object(state.subprocess,'Popen') as spawn:
            _,result,_=self.run_adapter(fail=True,reader=lambda deadline,command:original(deadline,command))
        spawn.assert_not_called();self.assertFalse(result['complete'])
        self.assertTrue(result['capture']['ownedChildRetained'])
        self.assertFalse(result['capture']['directChildReaped'])
    def test_adapter_failure_before_creation_has_no_postdeadline_process_evidence(self):
        module=self.runner;failure=RuntimeError('Owned runtime discovery capture failed')
        def capture(label,*args,**kwargs):
            if label=='selected-xcode':return 'Xcode16.4'
            if label=='selected-sdk':return '18.5'
            raise AssertionError('Creation must not follow discovery failure')
        with patch.dict(module.os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS'}), \
             patch.object(module,'step') as step,patch.object(module,'capture',side_effect=capture), \
             patch.object(module,'toolchain_evidence'),patch.object(module,'process_evidence') as evidence, \
             patch.object(module,'capture_runtime_catalogue',side_effect=failure),contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(RuntimeError) as caught:module.main()
        self.assertIs(caught.exception,failure)
        evidence.assert_called_once_with('before-runtime-discovery')
        step.assert_called_once() # Only the existing lock prerequisite, no simulator/cleanup before creation.

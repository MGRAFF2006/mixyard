"""Wrapper ownership/failure units; never execute Security, Swift or PKI locally."""
import contextlib
import errno
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import signal
import selectors
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('swift_lifecycle', Path(__file__).with_name('run-swift.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class TrustLifecycle(unittest.TestCase):
    def test_partial_preparation_failure_restores_search_before_owned_admin_cleanup(self):
        with tempfile.TemporaryDirectory(prefix='mixyard-swift-lifecycle-', dir=os.environ.get('RUNNER_TEMP', '/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir())) as root:
            calls=[]
            def owned(label, deadline, *args, **kwargs):
                calls.append(label)
                return '"/owned-original.keychain-db"' if label in ('startup-list','cleanup-verify') else ''
            with patch.dict(os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','MIXYARD_NATIVE_FIXTURE':root}), patch.object(module,'OwnedAdmin') as constructor, patch.object(module,'security',side_effect=owned), patch.object(module,'phase'), patch.object(module.signal,'signal'), patch.object(module.subprocess,'Popen') as child:
                admin=constructor.return_value; admin.names=('a','b','viewer')
                admin.add.side_effect=RuntimeError('owned-partial-add')
                admin.cleanup.side_effect=lambda _: calls.append('admin-cleanup') or True
                with self.assertRaisesRegex(RuntimeError,'owned-partial-add'): module.main()
                child.assert_not_called()
                admin.add.assert_called_once()
            self.assertLess(calls.index('cleanup-search'),calls.index('admin-cleanup'))
            self.assertLess(calls.index('cleanup-verify'),calls.index('admin-cleanup'))
            self.assertIn('cleanup-keychain',calls)

    def test_failed_restoration_is_reported_but_admin_and_keychain_cleanup_are_attempted(self):
        with tempfile.TemporaryDirectory(prefix='mixyard-swift-lifecycle-', dir=os.environ.get('RUNNER_TEMP', '/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir())) as root:
            calls=[]
            def owned(label, deadline, *args, **kwargs):
                calls.append(label)
                if label=='cleanup-search': raise RuntimeError('owned-restore-failure')
                return '"/owned-original.keychain-db"' if label in ('startup-list','cleanup-verify') else ''
            with patch.dict(os.environ, {'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','MIXYARD_NATIVE_FIXTURE':root}), patch.object(module,'OwnedAdmin') as constructor, patch.object(module,'security',side_effect=owned), patch.object(module,'phase'), patch.object(module.signal,'signal'), patch.object(module.subprocess,'Popen') as child:
                admin=constructor.return_value; admin.names=('a','b','viewer'); admin.cleanup.return_value=True
                child.return_value.wait.return_value=0; child.return_value.poll.return_value=0
                with self.assertRaisesRegex(RuntimeError,'owned-trust-cleanup-failed'): module.main()
                admin.cleanup.assert_called_once()
            self.assertIn('cleanup-verify',calls); self.assertIn('cleanup-keychain',calls)

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

    def test_privileged_timeout_relays_term_and_drains_without_killing_the_monitor(self):
        child = unittest.mock.Mock()
        child.returncode = -15
        child.communicate.side_effect = [subprocess.TimeoutExpired('owned',3.5), ('','')]
        with patch.dict(os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS'}), patch.object(module,'owned_process_evidence'), patch.object(module.subprocess,'Popen',return_value=child):
            with self.assertRaises(subprocess.TimeoutExpired): module.privileged_capture(['sudo','-n','/usr/bin/security'],4)
        child.terminate.assert_called_once(); child.kill.assert_not_called()
        budgets=[call.kwargs['timeout'] for call in child.communicate.call_args_list]
        self.assertGreater(budgets[0],0); self.assertLessEqual(budgets[0],3.5); self.assertGreater(budgets[1],0); self.assertLessEqual(budgets[1],0.5)
        self.assertIsNone(module.pending_command)

    def test_unproved_privileged_stop_retains_ownership_until_bounded_drain_completes(self):
        child = unittest.mock.Mock()
        child.returncode = -15
        child.communicate.side_effect = subprocess.TimeoutExpired('owned',1)
        with patch.dict(os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS'}), patch.object(module,'owned_process_evidence'), patch.object(module.subprocess,'Popen',return_value=child), patch.object(module,'phase'):
            with self.assertRaises(module.StopIncomplete): module.privileged_capture(['sudo','-n','/usr/bin/security'],4)
            self.assertIs(module.pending_command,child)
            self.assertFalse(module.drain_command(time.monotonic()+1))
            child.kill.assert_not_called()
            child.communicate.side_effect=None; child.communicate.return_value=('','')
            self.assertTrue(module.drain_command(time.monotonic()+1))
        self.assertIsNone(module.pending_command)

    def test_stop_metadata_is_limited_to_owned_security_ancestry_and_fixed_fields(self):
        child=unittest.mock.Mock(); child.pid=10; child.poll.return_value=None
        raw='10 1 501 S /private/SECRET/sudo\n11 10 0 S /usr/bin/security\n12 1 0 S /usr/bin/security\n13 10 501 S /private/SECRET/unrelated\n'
        output=io.StringIO()
        with patch.dict(os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS'}), patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],0,stdout=raw)) as run, contextlib.redirect_stderr(output):
            module.owned_process_evidence('before-privileged-term',child,0.1)
        data=json.loads(output.getvalue().removeprefix('MIXYARD_OWNED_PROCESS '))
        self.assertEqual([row['pid'] for row in data['processes']],[10,11])
        self.assertNotIn('SECRET',output.getvalue()); self.assertNotIn('unrelated',output.getvalue())
        self.assertEqual(run.call_args.kwargs['timeout'],0.1)

    def test_term_permission_error_is_classified_without_claiming_completion_or_leaking_exception(self):
        child=unittest.mock.Mock();child.pid=42
        child.terminate.side_effect=PermissionError(errno.EPERM,'SECRET-EXCEPTION')
        output=io.StringIO()
        with contextlib.redirect_stderr(output):
            with self.assertRaises(PermissionError):module.terminate_owned(child,'before-privileged-term')
        value=json.loads(output.getvalue().removeprefix('MIXYARD_OWNED_SIGNAL '))
        self.assertEqual(value,{'phase':'before-privileged-term','pid':42,'result':'os-error','errno':errno.EPERM})
        self.assertNotIn('SECRET',output.getvalue());child.communicate.assert_not_called()

    @unittest.skipUnless(os.name=='posix','POSIX signal inheritance mechanism')
    def test_actual_owned_unprivileged_exec_inherits_ignore_before_and_restores_default_after(self):
        script='import signal,time;print("ready:"+str(signal.getsignal(signal.SIGTERM)==signal.SIG_IGN),flush=True);time.sleep(30)'
        for repaired in (False,True):
            command=[sys.executable,'-B','-c',script]
            if repaired:command=module.command_with_default_term(command)
            original=signal.getsignal(signal.SIGTERM)
            child=None
            try:
                signal.signal(signal.SIGTERM,signal.SIG_IGN)
                try: child=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,start_new_session=True)
                finally: signal.signal(signal.SIGTERM,original)
                with selectors.DefaultSelector() as selector:
                    selector.register(child.stdout,selectors.EVENT_READ)
                    self.assertTrue(selector.select(timeout=1),'Owned child readiness boundary')
                    self.assertEqual(child.stdout.readline().strip(),'ready:'+str(not repaired))
                child.terminate() # Only our own UNPRIVILEGED direct child.
                if repaired:self.assertEqual(child.wait(timeout=1),-signal.SIGTERM)
                else:
                    with self.assertRaises(subprocess.TimeoutExpired):child.wait(timeout=0.05)
            finally:
                signal.signal(signal.SIGTERM,original)
                if child is not None:
                    if child.poll() is None:child.kill() # Never a sudo/root monitor.
                    child.wait(timeout=1)
                    child.stdout.close()

    def test_failed_owned_trust_cleanup_retains_recovery_keychain_and_still_fails(self):
        base='/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir()
        with tempfile.TemporaryDirectory(prefix='owned-recovery-retention-',dir=base) as root:
            calls=[]
            def owned(label,*args,**kwargs):calls.append(label);return '"/owned-original"' if label in ('startup-list','cleanup-verify') else ''
            with patch.dict(os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','MIXYARD_NATIVE_FIXTURE':root}),patch.object(module,'OwnedAdmin') as constructor,patch.object(module,'security',side_effect=owned),patch.object(module,'phase') as phase,patch.object(module.signal,'signal'),patch.object(module.subprocess,'Popen') as child:
                admin=constructor.return_value;admin.names=('a','b','viewer');admin.cleanup.return_value=False
                child.return_value.wait.return_value=0;child.return_value.poll.return_value=0
                with self.assertRaisesRegex(RuntimeError,'owned-trust-cleanup-failed'):module.main()
                self.assertNotIn('cleanup-keychain',calls)
                phase.assert_any_call('cleanup-recovery-retained','done')
    def test_direct_reap_receipt_never_claims_descendant_or_trust_absence(self):
        child=unittest.mock.Mock();child.pid=41;child.returncode=-15
        output=io.StringIO()
        with contextlib.redirect_stderr(output):module.completion_receipt(child,'privileged-timeout-reaped')
        value=json.loads(output.getvalue().removeprefix('MIXYARD_OWNED_RECEIPT '))
        self.assertTrue(value['directChildReaped']);self.assertTrue(value['pipesEof']);self.assertEqual(value['descendants'],'unproved')
        self.assertEqual(value['returncode'],-15)
        child.returncode=None
        with self.assertRaises(module.StopIncomplete):module.completion_receipt(child,'privileged-timeout-reaped')

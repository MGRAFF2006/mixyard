"""Admin command/partial cleanup contracts with OS boundaries mocked; no PKI changes."""
import base64
import contextlib
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
from admin_fixture import OwnedAdmin

spec=importlib.util.spec_from_file_location('owned_swift',Path(__file__).with_name('run-swift.py'))
module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)


@contextlib.contextmanager
def fixture():
    base=os.environ.get('RUNNER_TEMP','/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir())
    with tempfile.TemporaryDirectory(prefix='mixyard-admin-unit-',dir=base) as root:
        path=Path(root).resolve()
        for name in OwnedAdmin.names:
            (path/name).mkdir()
            # Deliberate mock-only DER bytes; actual hosted helper validates X509.
            value=base64.b64encode(('owned-mock-'+name).encode()).decode()
            (path/name/'certificate.pem').write_text('-----BEGIN CERTIFICATE-----\n'+value+'\n-----END CERTIFICATE-----\n')
        with patch.dict(os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS'}): yield path


def state(codes):
    return json.dumps({'statuses':codes,'ownedTrustAbsent':all(code in (-25300,-25263) for code in codes)})


class AdminFixture(unittest.TestCase):
    def test_partial_timeout_uses_noninteractive_admin_only_and_removes_exact_owned_certificate(self):
        with fixture() as root:
            commands=[]; output=io.StringIO()
            def command(args,**kwargs):
                commands.append(args)
                if 'add-trusted-cert' in args: raise subprocess.TimeoutExpired(args,4,stderr='SECRET-STDERR')
                return subprocess.CompletedProcess(args,0,stdout='')
            def run(label,deadline,args,**kwargs):
                if label=='admin-before': return state([-25300]*3)
                if label=='admin-precleanup': return state([0,-25300,-25300])
                if label=='admin-after': return state([-25300]*3)
                return module.run_owned(label,deadline,args,**kwargs)
            with contextlib.redirect_stderr(output), patch.object(module,'privileged_capture',side_effect=lambda args,timeout: command(args,timeout=timeout)), patch.object(module.subprocess,'run',side_effect=command):
                admin=OwnedAdmin(root,run); admin.admit(time.monotonic()+18)
                with self.assertRaises(RuntimeError): admin.add('a',root/'owned.keychain',time.monotonic()+18)
                self.assertEqual(admin.attempted,['a'])
                self.assertTrue(admin.cleanup(time.monotonic()+14))
            self.assertEqual(len(commands),3)
            self.assertEqual(commands[0][:2],['/usr/bin/security','add-certificates'])
            self.assertIn('-k',commands[0])
            for args in commands[1:]:
                self.assertEqual(args[:3],['sudo','-n','/usr/bin/security'])
                self.assertIn('-d',args)
                self.assertTrue(Path(args[-1]).parent==root and Path(args[-1]).name=='owned-admin-a.der')
            self.assertIn('add-trusted-cert',commands[1]); self.assertIn('remove-trusted-cert',commands[2])
            self.assertIn('"kind":"timeout","value":4000',output.getvalue())
            self.assertNotIn('SECRET',output.getvalue()); self.assertNotIn(str(root),output.getvalue())

    def test_preexisting_owned_fingerprint_is_never_overwritten_or_removed(self):
        with fixture() as root, contextlib.redirect_stderr(io.StringIO()):
            commands=[]
            def run(label,deadline,args,**kwargs): commands.append(label); return state([0,-25300,-25300])
            admin=OwnedAdmin(root,run)
            with self.assertRaisesRegex(RuntimeError,'already-trusted'): admin.admit(time.monotonic()+18)
            self.assertTrue(admin.cleanup(time.monotonic()+14))
            self.assertEqual(commands,['admin-before']); self.assertEqual(admin.attempted,[])

    def test_readback_rejects_unknown_status_extra_fields_and_secret_output_without_logging_it(self):
        for raw in [state([-25308,-25300,-25300]), '{"statuses":[-25300,-25300,-25300],"ownedTrustAbsent":true,"certificate":"SECRET"}', 'SECRET-not-json']:
            with fixture() as root, contextlib.redirect_stderr(io.StringIO()) as output:
                admin=OwnedAdmin(root,lambda *args,**kwargs:raw)
                with self.assertRaises(RuntimeError): admin.admit(time.monotonic()+18)
                self.assertNotIn('SECRET',output.getvalue())
                self.assertFalse(admin.admitted)

    def test_snapshot_change_blocks_privilege_and_failure_remains_explicit(self):
        with fixture() as root, contextlib.redirect_stderr(io.StringIO()):
            labels=[]
            def run(label,*args,**kwargs): labels.append(label); return state([-25300]*3)
            admin=OwnedAdmin(root,run); admin.admit(time.monotonic()+18)
            snapshot,_=admin.certificates['a']; snapshot.chmod(0o600); snapshot.write_bytes(b'changed')
            with self.assertRaisesRegex(RuntimeError,'certificate-changed'): admin.add('a',root/'owned',time.monotonic()+18)
            self.assertFalse(admin.cleanup(time.monotonic()+14))
            self.assertEqual(labels,['admin-before'])

    def test_personal_host_is_rejected_before_any_file_or_privileged_command(self):
        with patch.dict(os.environ,{'GITHUB_ACTIONS':'false','RUNNER_OS':'macOS'}):
            with self.assertRaisesRegex(RuntimeError,'owned-host-required'): OwnedAdmin(Path('/nonexistent'),lambda *args:None)

    def test_one_removal_error_does_not_skip_other_owned_records_or_the_final_readback(self):
        with fixture() as root, contextlib.redirect_stderr(io.StringIO()):
            labels=[]
            def run(label,*args,**kwargs):
                labels.append(label)
                if label in ('admin-before','admin-after'): return state([-25300]*3)
                if label in ('admin-ready','admin-precleanup'): return state([0]*3)
                if label=='cleanup-trust-a': raise RuntimeError('owned-remove-failed')
                return ''
            admin=OwnedAdmin(root,run); admin.admit(time.monotonic()+18)
            for name in admin.names: admin.add(name,root/'owned.keychain',time.monotonic()+18)
            admin.ready(time.monotonic()+18)
            self.assertFalse(admin.cleanup(time.monotonic()+14))
            self.assertEqual(labels[-4:],['cleanup-trust-a','cleanup-trust-b','cleanup-trust-viewer','admin-after'])

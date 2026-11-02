"""No local KVM access/privilege/emulator execution; all OS boundaries doubled."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('owned_kvm',Path(__file__).with_name('kvm-access.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)


def node(access=False, **change):
    value={'exists':True,'characterDevice':True,'kernelKvmMatch':True,'runnerUid':1001,'readable':access,'writable':access}
    value.update(change)
    return value, (1,2,3)


class HostedKvmAdmission(unittest.TestCase):
    def setUp(self):
        self.env=patch.dict(module.os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'Linux','MIXYARD_HOSTED_EMULATOR_APPROVED':'1'});self.env.start()
        self.output=io.StringIO();self.redirect=contextlib.redirect_stdout(self.output);self.redirect.__enter__()
    def tearDown(self): self.redirect.__exit__(None,None,None);self.env.stop()
    def test_absent_or_unverified_device_never_receives_privilege(self):
        for value in [node(exists=False),node(characterDevice=False,kernelKvmMatch=False),node(kernelKvmMatch=False)]:
            with patch.object(module,'inspect_node',return_value=value),patch.object(module.subprocess,'run') as run:
                with self.assertRaises(RuntimeError): module.main()
                run.assert_not_called()
    def test_already_accessible_real_kvm_needs_no_permission_change(self):
        with patch.object(module,'inspect_node',return_value=node(True)),patch.object(module.subprocess,'run') as run:
            module.main();run.assert_not_called()
    def test_acl_grants_only_current_uid_on_same_real_node_and_rechecks(self):
        with patch.object(module,'inspect_node',side_effect=[node(),node(True)]),patch.object(module.shutil,'which',side_effect=lambda name:'/usr/bin/'+name),patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],0)) as run:
            module.main()
        self.assertEqual(run.call_args.args[0],['/usr/bin/sudo','-n','/usr/bin/setfacl','-m','u:1001:rw','--','/dev/kvm'])
        self.assertEqual(run.call_args.kwargs['timeout'],5)
    def test_owner_grant_is_only_used_when_acl_tool_is_absent(self):
        with patch.object(module,'inspect_node',side_effect=[node(),node(True)]),patch.object(module.shutil,'which',side_effect=lambda name:None if name=='setfacl' else '/usr/bin/'+name),patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],0)) as run:
            module.main()
        self.assertEqual(run.call_args.args[0],['/usr/bin/sudo','-n','/usr/bin/chown','1001','--','/dev/kvm'])
    def test_permission_denial_does_not_fallback_or_emit_process_output(self):
        with patch.object(module,'inspect_node',return_value=node()),patch.object(module.shutil,'which',side_effect=lambda name:'/usr/bin/'+name),patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],1,stdout='SECRET',stderr='SECRET')) as run:
            with self.assertRaises(RuntimeError): module.main()
        self.assertEqual(run.call_count,1);self.assertNotIn('SECRET',self.output.getvalue())
    def test_changed_device_or_missing_access_stays_failed(self):
        for after in [(node(True)[0],(1,99,3)),node(False)]:
            with patch.object(module,'inspect_node',side_effect=[node(),after]),patch.object(module.shutil,'which',side_effect=lambda name:'/usr/bin/'+name),patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
                with self.assertRaises(RuntimeError): module.main()
    def test_unapproved_or_local_run_cannot_inspect_or_modify_device(self):
        with patch.dict(module.os.environ,{'GITHUB_ACTIONS':'false'}),patch.object(module,'inspect_node') as inspect:
            with self.assertRaises(RuntimeError): module.main()
            inspect.assert_not_called()

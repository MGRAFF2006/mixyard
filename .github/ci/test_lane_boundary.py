"""Exercise cross-lane pollution and qualification false-green boundaries."""
import importlib.util
from pathlib import Path
import unittest


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


boundary = module('lane_boundary')
gate = module('require_linux_fixtures')


class Isolation(unittest.TestCase):
    def test_export_restore_does_not_carry_codec_state_or_secrets_into_outputs(self):
        baseline = {'HOME': '/home/runner', 'PATH': '/usr/bin', 'GITHUB_TOKEN': 'private'}
        polluted = dict(baseline, CARGO_TARGET_DIR='/tmp/codec', LD_LIBRARY_PATH='/codec/lib',
                        pythonLocation='/python/3.13', PATH='/python/3.13/bin:/usr/bin')
        restored = boundary.restore(baseline, polluted)
        self.assertEqual(restored, {'PATH': '/usr/bin', 'CARGO_TARGET_DIR': '',
                                   'LD_LIBRARY_PATH': '', 'pythonLocation': ''})
        following = boundary.command_environment(baseline, dict(polluted, **restored))
        self.assertNotIn('CARGO_TARGET_DIR', following)
        self.assertNotIn('pythonLocation', following)
        self.assertNotIn('/python/3.13', following['PATH'])

    def test_declared_tools_override_stale_runner_path_and_pnpm_bootstrap(self):
        baseline = {'HOME': '/home/runner', 'PATH': '/usr/bin'}
        current = dict(baseline, PATH='/old-python:/old-node:/old-pnpm',
                       LANE_PYTHON='true', LANE_NODE='true', LANE_PNPM='true',
                       pythonLocation='/python/3.12', RUNNER_ARCH='X64',
                       RUNNER_TOOL_CACHE='/tools', PNPM_HOME='/pnpm', RUSTUP_TOOLCHAIN='1.99.0')
        result = boundary.command_environment(baseline, current)
        self.assertEqual(result['PATH'].split(':'), ['/python/3.12/bin', '/python/3.12',
                         '/tools/node/24.13.1/x64/bin', '/pnpm/bin', '/pnpm',
                         '/home/runner/.cargo/bin', '/usr/bin'])
        self.assertEqual(result['RUSTUP_TOOLCHAIN'], '1.99.0')


class QualificationGate(unittest.TestCase):
    def test_absent_inputs_are_not_a_pass_for_unexecuted_inventory(self):
        selected = {'qualification_offline': 'true', 'qualification_core': 'true'}
        for outcome in ('failure', 'skipped', 'cancelled', ''):
            steps = {'qualification_offline': {'outcome': outcome, 'outputs': {'core_present': 'false'}}}
            self.assertEqual(set(gate.gate_failures(selected, steps)), set(selected))
        inventory = {'outcome': 'success', 'outputs': {'core_present': 'false'}}
        self.assertEqual(gate.gate_failures(selected, {'qualification_offline': inventory}), [])
        inventory['outputs']['core_present'] = 'true'
        self.assertEqual(gate.gate_failures(selected, {'qualification_offline': inventory}), ['qualification_core'])

    def test_unrelated_failure_still_fails_after_inventory_success(self):
        self.assertEqual(gate.gate_failures({'qualification_offline': 'true', 'one': 'true'},
                         {'qualification_offline': {'outcome': 'success'}, 'one': {'outcome': 'failure'}}), ['one'])


class BeforeToolSetup(unittest.TestCase):
    def test_early_disk_checks_can_run_before_lane_setup_actions(self):
        baseline = {'HOME': '/home/runner', 'PATH': '/usr/bin'}
        env = dict(baseline, LANE_PYTHON='true', LANE_PNPM='true')
        self.assertIn('/usr/bin', boundary.command_environment(baseline, env)['PATH'])

    def test_shell_preserves_nonzero_exit_status(self):
        import json
        import os
        import subprocess
        import sys
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            workspace = root / 'workspace'
            workspace.mkdir()
            env = dict(os.environ, GITHUB_WORKSPACE=str(workspace), GITHUB_RUN_ID='test', GITHUB_RUN_ATTEMPT='1')
            helper = str(Path(__file__).with_name('lane_boundary.py'))
            subprocess.run([sys.executable, helper, 'snapshot'], env=env, check=True)
            script = root / 'fixture.sh'
            script.write_text('exit 7\n')
            result = subprocess.run([sys.executable, helper, 'shell', 'bash', str(script)], env=env)
            self.assertEqual(result.returncode, 7)
            subprocess.run([sys.executable, helper, 'clean'], env=env, check=True)
            self.assertFalse(list(root.glob('.mixyard-linux-environment-*')))

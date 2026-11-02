"""No builds: exercise selection against real local Git histories."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('selection', Path(__file__).with_name('select_linux_fixtures.py'))
selection = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selection)
CONFIG = json.loads(selection.CONFIG.read_text())


class Selection(unittest.TestCase):
    def test_shared_or_unknown_changes_run_every_lane(self):
        for paths in (None, [], ['Cargo.toml'], ['src/lib.rs'], ['.github/actions/foo/action.yml'],
                      ['.github/ci/select_linux_fixtures.py'], ['new-config.toml'], ['network/codecs/Cargo.toml'],
                      ['apps/web/pnpm-lock.yaml'], ['apps/desktop/package.json']):
            with self.subTest(paths=paths):
                self.assertEqual(selection.select(paths, CONFIG), set(CONFIG))

    def test_lane_paths_and_overlapping_windows_paths(self):
        self.assertEqual(selection.select(['verification/native-linux/selfcheck.py'], CONFIG),
                         {'native-linux-evidence-fixture-safety', 'qualification-offline', 'qualification-core', 'qualification-linux'})
        self.assertEqual(selection.select(['adapters/windows/inventory/check.cpp'], CONFIG),
                         {'platform-native-inventory-portable', 'windows-worker-portable', 'qualification-offline', 'qualification-core', 'qualification-linux', 'windows-native-host-linked', 'runtime-lifecycle'})
        self.assertEqual(selection.select(['verification/native-linux/deleted.py',
                                           'verification/windows-input/added.cpp'], CONFIG),
                         {'native-linux-evidence-fixture-safety', 'windows-input-transport', 'qualification-offline', 'qualification-core', 'qualification-linux'})

    def test_manual_and_invalid_history_are_fail_safe(self):
        for name, event in [('workflow_dispatch', {}), ('push', {}),
                            ('push', {'before': '0' * 40, 'after': 'a' * 40}),
                            ('push', {'before': '--help', 'after': 'a' * 40}),
                            ('pull_request', {'pull_request': {}})]:
            self.assertIsNone(selection.changed_paths('.', name, event))

    def test_merge_base_push_deletion_and_rename(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            def git(*args):
                return selection.git(repo, *args).decode().strip()
            def commit(message):
                git('add', '-A')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '-qm', message)
                return git('rev-parse', 'HEAD')
            git('init', '-q')
            old = repo / 'verification/native-linux/old.py'
            old.parent.mkdir(parents=True)
            old.write_text('fixture\n')
            base = commit('base')
            git('checkout', '-qb', 'feature')
            new = repo / 'verification/windows-input/renamed.py'
            new.parent.mkdir(parents=True)
            old.rename(new)
            head = commit('rename across lanes')
            paths = selection.changed_paths(repo, 'push', {'before': base, 'after': head})
            self.assertEqual(set(paths), {'verification/native-linux/old.py',
                                          'verification/windows-input/renamed.py'})
            self.assertEqual(selection.select(paths, CONFIG), {'native-linux-evidence-fixture-safety', 'windows-input-transport', 'qualification-offline', 'qualification-core', 'qualification-linux'})
            # Advance the base independently. Its unrelated change must not be
            # mistaken for a deletion introduced by the PR.
            git('checkout', '-q', '--detach', base)
            (repo / 'Cargo.toml').write_text('shared base advance\n')
            advanced = commit('base advanced')
            pr = {'pull_request': {'base': {'sha': advanced}, 'head': {'sha': head}}}
            self.assertEqual(set(selection.changed_paths(repo, 'pull_request', pr)), set(paths))
            git('checkout', '-q', '--detach', head)
            new.unlink()
            deleted = commit('delete')
            self.assertEqual(selection.changed_paths(repo, 'push', {'before': head, 'after': deleted}),
                             ['verification/windows-input/renamed.py'])
            self.assertIsNone(selection.changed_paths(repo, 'push', {'before': 'a' * 40, 'after': deleted}))


class FailurePropagation(unittest.TestCase):
    def test_selected_skipped_cancelled_or_failed_cannot_pass(self):
        spec = importlib.util.spec_from_file_location('gate', Path(__file__).with_name('require_linux_fixtures.py'))
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        for outcome in ('failure', 'cancelled', 'skipped', ''):
            with self.subTest(outcome=outcome):
                self.assertEqual(gate.failures({'one': 'true'}, {'ONE': outcome}), ['one'])
        self.assertEqual(gate.failures({'one': 'true', 'two': 'false'},
                                      {'ONE': 'success', 'TWO': 'skipped'}), [])


if __name__ == '__main__':
    unittest.main()

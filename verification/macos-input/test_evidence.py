#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Real, disposable Git histories; no SDK, network, or native execution."""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

EVIDENCE = pathlib.Path(__file__).with_name('evidence.py').resolve()
SCOPE = 'adapters/macos/virtual-input'
TARGET = 'refs/remotes/origin/main'


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='evidence-git-')
        self.addCleanup(temporary.cleanup)
        self.repo = pathlib.Path(temporary.name)
        self.env = dict(os.environ, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_AUTHOR_NAME='humunkulud', GIT_AUTHOR_EMAIL='graff.mathis@gmail.com',
                        GIT_COMMITTER_NAME='humunkulud', GIT_COMMITTER_EMAIL='graff.mathis@gmail.com')
        for name in tuple(self.env):
            if name.startswith('MIXYARD_') or name in ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE'):
                self.env.pop(name)
        self.git('init', '--quiet', '--template=', '--initial-branch=fixture')
        self.write('.gitignore', 'build/\n')
        self.write(f'{SCOPE}/driver.cpp', 'initial source\n')
        self.write(f'{SCOPE}/base-input.txt', 'declared base input\n')
        self.initial = self.commit('initial')
        self.write('base.txt', 'declared\n')
        self.declared = self.commit('declared base')
        self.write(f'{SCOPE}/driver.cpp', 'exact source\n')
        self.source = self.commit('PR source')
        self.git('checkout', '--quiet', '--detach', self.declared)
        self.write(f'{SCOPE}/base-input.txt', 'later approved base input\n')
        self.write(f'{SCOPE}/approved-only.txt', 'only on later approved base\n')
        self.actual_base = self.commit('later approved base')
        self.git('merge', '--quiet', '--no-ff', '-m', 'queued GitHub test merge', self.source)
        self.executed = self.git('rev-parse', 'HEAD')
        self.git('checkout', '--quiet', '--detach', self.actual_base)
        self.write('target.txt', 'fetched target is newer still\n')
        self.target = self.commit('target tip')
        self.git('update-ref', TARGET, self.target)
        self.git('checkout', '--quiet', '--detach', self.executed)
        self.env.update(MIXYARD_SOURCE_HEAD=self.source, MIXYARD_CURRENT_BASE=self.declared,
                        MIXYARD_TARGET_BASE_REF=TARGET)
        self.output = self.repo / 'verification/macos-input/build'

    def git(self, *args, input=None):
        return subprocess.run(['git', *args], cwd=self.repo, env=self.env, input=input,
                              text=True, capture_output=True, check=True).stdout.strip()

    def write(self, path, text):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def commit(self, message):
        self.git('add', '.')
        self.git('commit', '--quiet', '-m', message)
        return self.git('rev-parse', 'HEAD')

    def merge_commit(self, *parents):
        tree = self.git('rev-parse', self.executed + '^{tree}')
        args = ['commit-tree', tree]
        for parent in parents:
            args.extend(['-p', parent])
        commit = self.git(*args, input='synthetic parent rejection fixture\n')
        self.git('checkout', '--quiet', '--detach', commit)
        return commit

    def capture(self, status='incomplete', valid=True):
        result = subprocess.run([sys.executable, '-B', str(EVIDENCE), str(self.output), status],
                                cwd=self.repo, env=self.env, text=True, capture_output=True)
        manifest = json.loads((self.output / 'provenance.json').read_text())
        if valid:
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(manifest['provenance_errors'], [])
            self.assertIs(manifest['provenance_valid'], True)
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertTrue(manifest['provenance_errors'])
            self.assertIs(manifest['provenance_valid'], False)
        self.assertEqual(manifest['status'], status)
        return manifest

    def test_later_approved_test_merge_preserves_exact_source_and_tested_tree(self):
        m = self.capture()
        self.assertEqual(m['schema_version'], 3)
        self.assertEqual(m['checkout_kind'], 'test_merge')
        self.assertEqual(m['executed_head'], self.executed)
        self.assertEqual(m['executed_parents'], [self.actual_base, self.source])
        self.assertEqual(m['source_head'], self.source)
        self.assertEqual(m['source_parents'], [self.declared])
        self.assertEqual(m['declared_base'], self.declared)
        self.assertEqual(m['current_base'], self.declared)
        self.assertEqual(m['merge_base'], self.declared)
        self.assertEqual(m['actual_executed_base'], self.actual_base)
        self.assertEqual(m['target_base_ref'], TARGET)
        self.assertEqual(m['target_base'], self.target)
        self.assertIs(m['declared_base_ancestor_of_executed_base'], True)
        self.assertIs(m['executed_base_ancestor_of_target'], True)
        self.assertEqual(m['source_tree'], self.git('rev-parse', self.source + '^{tree}'))
        self.assertEqual(m['executed_tree'], self.git('rev-parse', self.executed + '^{tree}'))
        self.assertNotEqual(m['source_tree'], m['executed_tree'])
        path = f'{SCOPE}/base-input.txt'
        digest = lambda text: hashlib.sha256(text.encode()).hexdigest()
        self.assertEqual(m['source_sha256'], {
            f'{SCOPE}/driver.cpp': digest('exact source\n'),
            path: digest('declared base input\n')})
        self.assertEqual(m['executed_sha256'], {
            f'{SCOPE}/driver.cpp': digest('exact source\n'),
            path: digest('later approved base input\n'),
            f'{SCOPE}/approved-only.txt': digest('only on later approved base\n')})
        self.assertEqual(m['working_tree_sha256'], m['executed_sha256'])
        self.assertNotIn(f'{SCOPE}/approved-only.txt', m['source_sha256'])
        self.assertIn(f'{SCOPE}/approved-only.txt', m['executed_sha256'])
        self.assertIs(m['production_apply'], False)
        self.assertEqual(m['hosted_HAL_peer_identity'], 'BLOCKED')

    def test_exact_declared_base_test_merge(self):
        self.env['MIXYARD_CURRENT_BASE'] = self.actual_base
        m = self.capture()
        self.assertIs(m['source_contains_declared_base'], False)
        self.assertEqual(m['actual_executed_base'], self.actual_base)

    def test_exact_source_checkout_with_checked_declared_base(self):
        self.git('checkout', '--quiet', '--detach', self.source)
        m = self.capture('success')
        self.assertEqual(m['checkout_kind'], 'source')
        self.assertIsNone(m['actual_executed_base'])
        self.assertIs(m['source_contains_declared_base'], True)
        self.assertEqual(m['source_tree'], m['executed_tree'])
        self.assertEqual(m['source_sha256'], m['working_tree_sha256'])
        self.assertIn('CI artifact provenance only', m['qualification_limit'])

    def test_queued_main_push_binds_event_before_separately_from_fresh_target(self):
        # The pushed accepted main merge is older than the freshly fetched main
        # by the time its queued job runs. Event.before is its real baseline.
        self.git('checkout', '--quiet', '--detach', self.executed)
        self.write(f'{SCOPE}/later-main.txt', 'accepted after this main push was queued\n')
        newer_target = self.commit('newer accepted main')
        self.git('update-ref', TARGET, newer_target)
        self.git('checkout', '--quiet', '--detach', self.executed)
        self.env['MIXYARD_SOURCE_HEAD'] = self.executed
        # Old event wiring conflated the newer approved target with the baseline.
        self.env['MIXYARD_CURRENT_BASE'] = TARGET
        self.capture(valid=False)
        self.env['MIXYARD_CURRENT_BASE'] = self.actual_base
        m = self.capture()
        self.assertEqual(m['checkout_kind'], 'source')
        self.assertEqual(m['source_head'], self.executed)
        self.assertEqual(m['declared_base'], self.actual_base)
        self.assertEqual(m['target_base'], newer_target)
        self.assertEqual(m['source_tree'], m['executed_tree'])
        self.assertIs(m['source_contains_declared_base'], True)
        self.assertNotIn(f'{SCOPE}/later-main.txt', m['source_sha256'])

    def test_direct_source_must_contain_declared_base(self):
        self.git('checkout', '--quiet', '--detach', self.source)
        self.env['MIXYARD_CURRENT_BASE'] = self.actual_base
        m = self.capture(valid=False)
        self.assertIn('Direct source checkout must contain the declared base.', m['provenance_errors'])

    def test_wrong_second_parent_and_swapped_parent_order(self):
        for parents in ((self.actual_base, self.initial), (self.source, self.actual_base)):
            with self.subTest(parents=parents):
                self.merge_commit(*parents)
                m = self.capture(valid=False)
                self.assertIn('Test merge must have exactly [executed base, source HEAD] parents.',
                              m['provenance_errors'])

    def test_extra_parent_rejected(self):
        self.merge_commit(self.actual_base, self.source, self.initial)
        m = self.capture(valid=False)
        self.assertEqual(m['actual_executed_base'], self.actual_base)
        self.assertEqual(m['executed_parents'], [self.actual_base, self.source, self.initial])
        self.assertIn('Test merge must have exactly [executed base, source HEAD] parents.',
                      m['provenance_errors'])

    def test_nonmerge_unrelated_execution_rejected(self):
        self.git('checkout', '--quiet', '--detach', self.target)
        self.capture(valid=False)

    def test_executed_base_must_descend_declared_base(self):
        self.merge_commit(self.initial, self.source)
        m = self.capture(valid=False)
        self.assertIn('Executed base must descend from the declared base.', m['provenance_errors'])

    def test_divergent_executed_base_rejected(self):
        self.git('checkout', '--quiet', '--detach', self.declared)
        self.write('divergent.txt', 'unapproved divergent base\n')
        divergent = self.commit('divergent base')
        self.merge_commit(divergent, self.source)
        m = self.capture(valid=False)
        self.assertIn('Executed base must be an ancestor of the fetched target base.', m['provenance_errors'])

    def test_unapproved_later_base_rejected(self):
        self.git('update-ref', TARGET, self.declared)
        m = self.capture(valid=False)
        self.assertIn('Executed base must be an ancestor of the fetched target base.', m['provenance_errors'])

    def test_configured_pr_target_ref_supplies_approval_without_main_fallback(self):
        target_ref = 'refs/remotes/origin/release/input'
        self.git('update-ref', target_ref, self.target)
        self.git('update-ref', '-d', TARGET)
        self.env['MIXYARD_TARGET_BASE_REF'] = target_ref
        m = self.capture()
        self.assertEqual(m['target_base_ref'], target_ref)
        self.assertEqual(m['target_base'], self.target)

    def test_direct_source_declared_base_must_be_approved(self):
        self.git('checkout', '--quiet', '--detach', self.source)
        self.git('update-ref', TARGET, self.initial)
        m = self.capture(valid=False)
        self.assertIn('Declared base must be an ancestor of the fetched target base.', m['provenance_errors'])

    def test_shallow_checkout_rejected_even_with_available_required_tips(self):
        (self.repo / '.git/shallow').write_text(self.declared + '\n')
        m = self.capture(valid=False)
        self.assertIn('Complete ancestry required; shallow checkout cannot qualify.', m['provenance_errors'])

    def test_missing_commit_object_rejected(self):
        (self.repo / '.git/objects' / self.initial[:2] / self.initial[2:]).unlink()
        m = self.capture(valid=False)
        self.assertTrue(any('rev-list' in error for error in m['provenance_errors']))

    def test_graft_cannot_disguise_reversed_raw_parents(self):
        wrong = self.merge_commit(self.source, self.actual_base)
        raw = self.git('cat-file', 'commit', wrong)
        self.assertEqual([line[7:] for line in raw.splitlines() if line.startswith('parent ')],
                         [self.source, self.actual_base])
        self.write('.git/info/grafts', f'{wrong} {self.actual_base} {self.source}\n')
        # A quiet graft must not make synthetic history count as actual parents.
        self.env.update(GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='advice.graftFileDeprecated',
                        GIT_CONFIG_VALUE_0='false')
        self.assertEqual(self.git('show', '-s', '--format=%P', wrong).split(),
                         [self.actual_base, self.source])
        m = self.capture(valid=False)
        self.assertEqual(m['executed_parents'], [self.source, self.actual_base])

    def test_commit_graph_cannot_hide_missing_or_corrupt_ancestor(self):
        self.git('commit-graph', 'write', '--reachable')
        path = self.repo / '.git/objects' / self.initial[:2] / self.initial[2:]
        original = path.read_bytes()
        for contents in (None, b'not a compressed Git object'):
            with self.subTest(contents=contents):
                if contents is None:
                    path.unlink()
                else:
                    path.write_bytes(contents)
                # All required tips still resolve: ancestor objects must be read too.
                self.assertEqual(self.git('rev-parse', self.source), self.source)
                m = self.capture(valid=False)
                self.assertTrue(any('rev-list' in error for error in m['provenance_errors']))
                path.write_bytes(original)

    def test_missing_or_ambiguous_refs_rejected(self):
        for name, value in (('MIXYARD_SOURCE_HEAD', 'missing-source'),
                            ('MIXYARD_CURRENT_BASE', 'missing-base'),
                            ('MIXYARD_TARGET_BASE_REF', 'refs/remotes/origin/missing')):
            previous = self.env[name]
            with self.subTest(name=name):
                self.env[name] = value
                self.capture(valid=False)
            self.env[name] = previous
        self.git('branch', 'ambiguous', self.source)
        self.git('tag', 'ambiguous', self.declared)
        for name in ('MIXYARD_SOURCE_HEAD', 'MIXYARD_CURRENT_BASE'):
            previous = self.env[name]
            with self.subTest(ambiguous=name):
                self.env[name] = 'ambiguous'
                self.capture(valid=False)
            self.env[name] = previous

    def test_local_or_head_ref_cannot_supply_target_approval(self):
        for ref in ('HEAD', self.target, 'refs/heads/fixture', 'refs/remotes/origin/../main'):
            with self.subTest(ref=ref):
                self.env['MIXYARD_TARGET_BASE_REF'] = ref
                m = self.capture(valid=False)
                self.assertIn('Target base must be an explicit refs/remotes/origin/<base> ref.',
                              m['provenance_errors'])

    def test_dirty_or_missing_working_input_rejected_and_recorded(self):
        path = f'{SCOPE}/driver.cpp'
        self.write(path, 'uncommitted bytes\n')
        m = self.capture(valid=False)
        self.assertNotEqual(m['working_tree_sha256'][path], m['executed_sha256'][path])
        (self.repo / path).unlink()
        m = self.capture(valid=False)
        self.assertNotIn(path, m['working_tree_sha256'])

    def test_untracked_input_rejected(self):
        self.write(f'{SCOPE}/untracked.h', 'extra input\n')
        m = self.capture(valid=False)
        self.assertIn('Scoped working inputs must match the executed commit.', m['provenance_errors'])

    def test_partial_artifacts_keep_failure_status_even_when_provenance_fails(self):
        payload = b'partial artifact\x00\xff\n'
        artifact = 'verification/macos-input/build/fixture/partial.bin'
        file = self.repo / artifact
        file.parent.mkdir(parents=True)
        file.write_bytes(payload)
        for valid in (True, False):
            with self.subTest(valid=valid):
                if not valid:
                    self.env['MIXYARD_CURRENT_BASE'] = 'missing-base'
                m = self.capture('failure', valid=valid)
                self.assertEqual(m['artifact_sha256'], {artifact: hashlib.sha256(payload).hexdigest()})
        self.assertNotIn('provenance.json', m['artifact_sha256'])


if __name__ == '__main__':
    unittest.main()

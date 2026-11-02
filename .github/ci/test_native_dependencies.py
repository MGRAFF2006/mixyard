"""Keep automatic native consumers connected to consolidated Linux results."""
from pathlib import Path
import re
import unittest


WORKFLOWS = Path(__file__).parents[1] / 'workflows'


def job(text, name):
    return re.search(r'^  ' + re.escape(name) + r':\n(.*?)(?=^  [\w-]+:|\Z)',
                     text, re.MULTILINE | re.DOTALL).group(1)


class MacOSProvenance(unittest.TestCase):
    def setUp(self):
        self.workflow = (WORKFLOWS / 'linux-fixtures.yml').read_text()
        self.native = job(self.workflow, 'macos-input-artifact')
        self.condition = ' '.join(re.search(r'^    if: (.*?)(?=^    [\w-]+:)', self.native,
                                            re.MULTILINE | re.DOTALL).group(1).split())

    def eligible(self, event='pull_request', ref='refs/pull/1/merge', provenance='true',
                 cancelled=False, fixtures_result='success'):
        # GitHub implicitly gates needs on success unless a status function is
        # present. Reproduce the skipped-needs regression, not just boolean text.
        if 'always()' not in self.condition and fixtures_result != 'success':
            return False
        expression = self.condition.replace('always()', 'True').replace('!cancelled()', str(not cancelled))
        for key, value in [('github.event_name', event), ('github.ref', ref),
                           ('needs.fixtures.outputs.macos_input_success', provenance)]:
            expression = expression.replace(key, repr(value))
        return eval(expression.replace('&&', ' and ').replace('||', ' or '), {'__builtins__': {}})

    def test_automatic_artifact_needs_real_consolidated_provenance(self):
        self.assertRegex(self.native, r'(?m)^    needs: fixtures$')
        self.assertIn("macos_input_success: ${{ steps.macos_input_provenance.outcome == 'success' }}",
                      self.workflow)
        self.assertTrue(self.eligible())
        for outcome in ('false', '', 'failure', 'skipped'):
            self.assertFalse(self.eligible(provenance=outcome))

    def test_unrelated_linux_failure_does_not_suppress_successful_provenance(self):
        self.assertTrue(self.eligible(fixtures_result='failure'))
        self.assertFalse(self.eligible(provenance='false', fixtures_result='failure'))
        self.assertFalse(self.eligible(cancelled=True))

    def test_dispatch_and_feature_push_do_not_duplicate_native_artifact(self):
        self.assertFalse(self.eligible(event='workflow_dispatch', ref='refs/heads/main'))
        self.assertFalse(self.eligible(event='push', ref='refs/heads/t3/network-codecs'))
        self.assertTrue(self.eligible(event='push', ref='refs/heads/main'))
        manual = job((WORKFLOWS / 'macos-input.yml').read_text(), 'artifact')
        self.assertIn("if: github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'", manual)
        self.assertIn('needs: provenance', manual)


if __name__ == '__main__':
    unittest.main()

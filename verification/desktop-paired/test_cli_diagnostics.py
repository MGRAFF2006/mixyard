import importlib.util
from pathlib import Path
import unittest
import sys
sys.path.insert(0, str(Path(__file__).parent))

spec = importlib.util.spec_from_file_location('paired_qualify', Path(__file__).with_name('qualify.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class CliDiagnostics(unittest.TestCase):
    def test_classified_failure_keeps_reason_without_private_paths_or_opaque_material(self):
        secret = 'owned-synthetic-secret-' + 'x' * 48
        root = Path('/owned/private-fixture')
        diagnostic = module.cli_failure(f'private-store: unsafe-ancestor {root}/identity C:\\Users\\runneradmin\\private \\\\?\\Volume{{owned}}\\private {secret}', root)
        self.assertIn('private-store: unsafe-ancestor', diagnostic)
        for value in (str(root), 'runneradmin', 'Volume', secret):
            self.assertNotIn(value, diagnostic)

    def test_static_windows_reason_survives_without_allowing_opaque_secret_tokens(self):
        reason = 'windows-identity-state-insecure-ancestor-acl'
        diagnostic = module.cli_failure('unsafe-identity-store: ' + reason + ' ' + 'x' * 64, Path('/owned'))
        self.assertIn(reason, diagnostic)
        self.assertNotIn('x' * 64, diagnostic)

    def test_diagnostic_is_bounded_and_does_not_include_quoted_or_pem_values(self):
        diagnostic = module.cli_failure('private-store: \"owned secret\" -----BEGIN PRIVATE KEY-----\nprivate-material\n-----END PRIVATE KEY----- ' + 'word ' * 2000, Path('/owned'))
        self.assertLessEqual(len(diagnostic), 1024)
        self.assertNotIn('owned secret', diagnostic)
        self.assertNotIn('private-material', diagnostic)
        self.assertEqual(module.cli_failure('', Path('/owned')), '<no stderr>')

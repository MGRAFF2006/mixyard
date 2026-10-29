"""Production selector propagation without native codec compilation or Cargo."""
import os
import runpy
import sys
from unittest.mock import patch
from pathlib import Path
import subprocess
import tempfile
import unittest

from codec_source_links import parse_directives, selected_links


class SourceLinkTests(unittest.TestCase):
    def test_raw_does_not_read_invalid_native_configuration_or_run_selector(self):
        self.assertEqual(selected_links(None, None, [], {}, lambda _: self.fail()), [])

    def test_directives_are_literal_arguments_and_unknown_directives_fail(self):
        self.assertEqual(parse_directives('cargo:rustc-link-search=native=/tmp/a b\n'
                                         'cargo:rustc-link-lib=static=custom\n'),
                         ['-L', 'native=/tmp/a b', '-l', 'static=custom'])
        with self.assertRaises(ValueError):
            parse_directives('cargo:rustc-flags=-lwrong')

    def test_production_runner_propagates_selector_flags_to_every_link_stage(self):
        root = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix='mixyard-source-propagation-') as directory:
            cache = Path(directory) / 'cache'
            target = Path(directory) / 'target'
            deps = cache / 'target/debug/deps'
            deps.mkdir(parents=True)
            (cache / 'network').mkdir()
            (cache / 'network/Cargo.lock').write_bytes((root / 'Cargo.lock').read_bytes())
            for name in ('anyhow', 'quinn', 'rustls', 'serde', 'serde_json', 'tokio', 'rcgen'):
                (deps / f'lib{name}-fixture.rlib').touch()
            calls = []
            def invoke(command, **kwargs):
                calls.append(command)
                output = ''
                if command[0].endswith('codec-link-setup'):
                    output = ('cargo:rustc-link-search=native=/selected path\n'
                              'cargo:rustc-link-lib=static=selected_zstd\n')
                return subprocess.CompletedProcess(command, 0, stdout=output)
            argv = [str(root / 'verify-codec-source.py'), '--cache', str(cache),
                    '--target', str(target), '--features', 'system-zstd', '--test', 'codec_quic',
                    '--lib-test-filter', 'fragment_transport::codecs', '--clippy']
            with patch.object(sys, 'argv', argv), patch('subprocess.run', side_effect=invoke):
                runpy.run_path(str(root / 'verify-codec-source.py'), run_name='__main__')
            stages = [command for command in calls if any(
                str(argument).endswith(('codecs/src/lib.rs', 'src/lib.rs', 'tests/codec_quic.rs'))
                for argument in command)]
            self.assertEqual(len(stages), 4)  # Codec rlib, network rlib, unit and paired test.
            for command in stages:
                self.assertIn('native=/selected path', command)
                self.assertIn('static=selected_zstd', command)

    def test_actual_selector_custom_names_kind_feature_scope_and_removal(self):
        root = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix='mixyard-source-selector-') as directory:
            target = Path(directory)
            zstd, opus = target / 'zstd selection', target / 'opus selection'
            zstd.mkdir()
            opus.mkdir()
            env = dict(os.environ, MIXYARD_ZSTD_LIB_DIR=str(zstd),
                       MIXYARD_OPUS_LIB_DIR=str(opus), MIXYARD_ZSTD_LIB_NAME='lib',
                       MIXYARD_OPUS_LIB_NAME='selected_opus', MIXYARD_ZSTD_LINK_KIND='static',
                       MIXYARD_OPUS_LINK_KIND='static', CARGO_FEATURE_SYSTEM_OPUS='1')
            artifact = zstd / 'liblib.a'
            artifact.write_bytes(b'exact selected zstd')
            (opus / 'libselected_opus.a').write_bytes(b'exact selected opus')
            run = lambda command: subprocess.run(command, check=True, timeout=60)
            flags = selected_links(root, target, ['system-zstd'], env, run)
            self.assertEqual(flags[::2], ['-L', '-l'])
            self.assertTrue(flags[3].startswith('static=mixyard_codec_zstd_selected_'))
            self.assertEqual([p.read_bytes() for p in (target / 'selected-libraries').iterdir()],
                             [b'exact selected zstd'])
            flags = selected_links(root, target, ['system-zstd', 'system-opus'], env, run)
            self.assertEqual(flags[::2], ['-L', '-l', '-L', '-l'])
            self.assertTrue(flags[7].startswith('static=mixyard_codec_opus_selected_'))
            artifact.unlink()
            with self.assertRaises(subprocess.CalledProcessError):
                selected_links(root, target, ['system-zstd'], env, run)


if __name__ == '__main__':
    unittest.main()

"""Evidence boundaries only; tests never connect to a display or native app."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from window_evidence import capture, owned_processes


class WindowEvidenceTests(unittest.TestCase):
    def test_capture_refuses_a_nonisolated_session_before_any_command(self):
        with patch.dict('os.environ', {}, clear=True), patch('window_evidence.command') as command:
            with self.assertRaisesRegex(RuntimeError, 'wrapper-owned isolated display'):
                capture(Path('.'), 'failure', [])
        command.assert_not_called()

    def test_missing_titled_window_still_records_unnamed_window_metadata_and_capture_failure(self):
        def command(args, timeout=2):
            if args[:3] == ['xwininfo', '-root', '-tree']:
                return {'exit_code': 0, 'stdout': '0x100 (root)\n  0x200 (has no name)\n'}
            if args[0] == 'xdotool':
                return {'exit_code': 1, 'stdout': '', 'stderr': 'no matching window'}
            if args[0] == 'import':
                return {'error': 'screenshot timed out'}
            return {'exit_code': 0, 'stdout': 'captured property'}
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict('os.environ', {'MIXYARD_NATIVE_ISOLATED': '1', 'DISPLAY': ':99'}), \
                patch('window_evidence.owned_processes', return_value=[]), \
                patch('window_evidence.command', side_effect=command) as commands:
            process = Mock(pid=42); process.poll.return_value = None
            capture(directory, 'failure', [process])
            report = json.loads(Path(directory, 'failure-native.json').read_text())
        self.assertEqual(report['title_visible']['exit_code'], 1)
        self.assertEqual([window['id'] for window in report['windows']], ['0x100', '0x200'])
        self.assertTrue(any(call.args[0][:3] == ['xprop', '-id', '0x200'] for call in commands.call_args_list))
        self.assertEqual(report['root_screenshot']['error'], 'screenshot timed out')
        self.assertEqual(report['started_processes'], [{'pid': 42, 'exit_code': None}])

    def test_process_metadata_keeps_only_owned_roots_and_descendants(self):
        rows = '1 0 1001 S unrelated\n10 1 1001 S harness\n11 10 1001 S driver\n12 11 1001 S mixyard-desktop\n13 12 1001 S WebKit\n20 1 1001 S other-app\n'
        with patch('window_evidence.command', return_value={'exit_code': 0, 'stdout': rows}):
            result = owned_processes([10])
        self.assertEqual([row['pid'] for row in result], [10, 11, 12, 13])
        self.assertTrue(all('args' not in row for row in result))


if __name__ == '__main__':
    unittest.main()

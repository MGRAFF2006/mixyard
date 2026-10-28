"""Process exit observation only: no desktop, devices or WebDriver session."""
import subprocess
import sys
import unittest
from unittest.mock import patch

from webdriver import exited


class ProcessExitTests(unittest.TestCase):
    def test_process_disappearing_during_status_read_is_exited(self):
        with patch('webdriver.Path') as path:
            path.return_value.exists.return_value = True
            path.return_value.read_text.side_effect = FileNotFoundError()
            self.assertTrue(exited(123))

    def test_live_and_zombie_status_are_distinct(self):
        with patch('webdriver.Path') as path:
            path.return_value.exists.return_value = True
            for state, expected in [('R', False), ('S', False), ('Z', True)]:
                path.return_value.read_text.return_value = f'123 (mixyard) {state} 1 2 3'
                self.assertEqual(exited(123), expected)

    def test_other_read_errors_do_not_claim_process_exit(self):
        with patch('webdriver.Path') as path:
            path.return_value.exists.return_value = True
            path.return_value.read_text.side_effect = PermissionError()
            with self.assertRaises(PermissionError):
                exited(123)

    @unittest.skipUnless(sys.platform.startswith('linux'), '/proc lifecycle is Linux-specific')
    def test_owned_live_child_then_reaped_child(self):
        process = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
        try:
            self.assertFalse(exited(process.pid))
        finally:
            process.terminate()
            process.wait(timeout=5)
        self.assertTrue(exited(process.pid))


if __name__ == '__main__':
    unittest.main()

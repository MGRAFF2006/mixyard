"""Queued native events must be consumed before the next verifier action."""
import unittest
from unittest.mock import Mock, patch

from webdriver import native_button, submit_connect


class NativeInputTests(unittest.TestCase):
    def test_connect_waits_for_complete_text_and_tab_without_retyping(self):
        driver = Mock(window_id='42', input_attempts=[])
        driver.script.side_effect = ['runtime-instance', 'runtime-connect']
        states = [{'input': {'value': value}} for value in
                  ['default', 'default', 'native-1', 'native-123-late']]
        with patch('webdriver.native_focus'), patch('webdriver.native_button') as button, \
                patch('webdriver.xdotool', return_value='42') as keyboard, \
                patch('webdriver.input_state', side_effect=states), \
                patch('webdriver.time.sleep'):
            submit_connect(driver, 'native-123-late')
        button.assert_called_once_with(driver, '#runtime-connect')
        typed = [call.args for call in keyboard.call_args_list if call.args[0] == 'type']
        self.assertEqual(typed, [('type', '--clearmodifiers', '--delay', 10, 'native-123-late')])
        self.assertEqual(driver.input_attempts[0]['after']['input']['value'], 'native-123-late')

    def test_incomplete_text_never_activates_connect(self):
        driver = Mock(window_id='42', input_attempts=[])
        clock = iter([0, 0, 3])
        with patch('webdriver.native_focus'), patch('webdriver.native_button') as button, \
                patch('webdriver.xdotool', return_value='42'), \
                patch('webdriver.input_state', return_value={'input': {'value': 'native-1'}}), \
                patch('webdriver.time.monotonic', side_effect=lambda: next(clock)), \
                patch('webdriver.time.sleep'):
            with self.assertRaisesRegex(AssertionError, 'native runtime instance entry'):
                submit_connect(driver, 'native-123-late')
        button.assert_not_called()

    def test_button_waits_for_trusted_event_delivery(self):
        driver = Mock()
        driver.script.side_effect = [None, None, True]
        with patch('webdriver.native_focus'), patch('webdriver.xdotool') as keyboard, \
                patch('webdriver.time.sleep'):
            native_button(driver, '#runtime-connect')
        keyboard.assert_called_once_with('key', '--clearmodifiers', 'Return')

    def test_untrusted_event_is_not_accepted(self):
        driver = Mock()
        driver.script.return_value = False
        clock = iter([0, 0, 3])
        with patch('webdriver.native_focus'), patch('webdriver.xdotool'), \
                patch('webdriver.time.monotonic', side_effect=lambda: next(clock)), \
                patch('webdriver.time.sleep'):
            with self.assertRaisesRegex(AssertionError, 'trusted native key activation'):
                native_button(driver, '#runtime-connect')


if __name__ == '__main__':
    unittest.main()

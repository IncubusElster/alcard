#!/usr/bin/env python3
"""Fast protocol fixtures for the private terminal observer; no guest commands.

These checks do not claim application acceptance. They establish that split
UTF-8/ANSI output is observed consistently and that old PAM errors or private
terminal contents cannot become current/public success evidence.
"""
import importlib.util
import json
import os
from pathlib import Path
import pty
import pwd
import unittest


spec = importlib.util.spec_from_file_location('settings_observer', Path(__file__).with_name('observer.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class Base:
    USER = pwd.getpwuid(os.getuid()).pw_name


class ObserverTests(unittest.TestCase):
    def setUp(self):
        self.acceptance = module.Acceptance(Base())
        self.master, self.slave = pty.openpty()
        self.acceptance.master = self.master
        self.acceptance.screen = module.Screen()

    def tearDown(self):
        os.close(self.master)
        os.close(self.slave)

    @staticmethod
    def frame(title, body, footer, status=''):
        return ('\x1b[?1049h\x1b[2J\x1b[1;1H' + title +
                '\x1b[2;1H' + body + '\x1b[18;1H' + status +
                '\x1b[21;1H' + footer).encode()

    def test_split_utf8_and_csi_identify_actual_locale_and_selection(self):
        for width in (1, 2, 3, 17, 65536):
            with self.subTest(chunk_bytes=width):
                screen = module.Screen()
                data = self.frame('ALCARD / НАСТРОЙКИ', '  Система\r\n› Язык и клавиатура',
                                  '↑↓  4 Открыть  6 Обновить  8 Назад')
                for start in range(0, len(data), width):
                    screen.feed(data[start:start + width])
                self.acceptance.screen = screen
                state = self.acceptance.state()
                self.assertEqual((state['page'], state['locale'], state['selected'], state['dialog']),
                                 ('home', 'ru', 1, 'list'))

    def test_picker_and_confirmation_use_visible_content(self):
        self.acceptance.screen.feed(self.frame(
            'Idioma y teclado', 'Idioma del sistema\r\n\r\n  Inglés\r\n  Ruso\r\n› Español',
            '↑↓ Elegir   Enter Aplicar   Esc Volver'))
        state = self.acceptance.state()
        self.assertEqual((state['page'], state['locale'], state['selected'], state['dialog']),
                         ('language', 'es', 2, 'choose'))
        self.acceptance.screen.feed(self.frame(
            'Connection profile', 'Apply this change?\r\n\r\nForget',
            'Enter Apply   Esc Cancel', status='Done'))
        state = self.acceptance.state()
        self.assertEqual((state['page'], state['dialog'], state['status']),
                         ('profile', 'confirm', 'saved'))

    def test_old_authentication_failure_is_not_reused(self):
        screen = self.acceptance.screen
        screen.feed(b'\x1b[?1049h\x1b[?1049lPassword: \r\nAuthentication failure\r\n')
        self.assertTrue(self.acceptance.state()['auth_failed'])
        screen.feed(b'\x1b[?1049h\x1b[?1049lPassword: ')
        state = self.acceptance.state()
        self.assertTrue(state['auth_prompt'])
        self.assertFalse(state['auth_failed'])
        self.assertEqual((state['alternate_entries'], state['alternate_leaves']), (2, 2))

    def test_terminal_secrets_and_unrecognized_content_are_not_public(self):
        secret = 'private-terminal-fixture-value'
        self.acceptance.screen.feed(('\x1b[?1049lPassword: ' + secret).encode())
        reply = json.dumps(self.acceptance.state())
        self.assertNotIn(secret, reply)
        self.assertNotIn('Password:', reply)
        self.acceptance.screen.feed(self.frame('Unknown text', secret, secret))
        state = self.acceptance.state()
        self.assertIsNone(state['page'])
        self.assertIsNone(state['status'])
        self.assertNotIn(secret, json.dumps(state))


if __name__ == '__main__':
    unittest.main()

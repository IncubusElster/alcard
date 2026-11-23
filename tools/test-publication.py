#!/usr/bin/env python3
"""Isolated regressions for the source/history publication gate."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('publication_gate', Path(__file__).with_name('check-publication.py'))
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        subprocess.run(['git', '-C', str(self.repo), 'init', '-q'], check=True)
        subprocess.run(['git', '-C', str(self.repo), 'config', 'user.name', 'CI Test'], check=True)
        subprocess.run(['git', '-C', str(self.repo), 'config', 'user.email', 'ci@example.invalid'], check=True)

    def commit(self, name, content):
        target = self.repo / name
        if content is None:
            target.unlink()
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
        subprocess.run(['git', '-C', str(self.repo), 'add', '-A'], check=True)
        subprocess.run(['git', '-C', str(self.repo), 'commit', '-qm', 'test snapshot'], check=True)

    def test_clean_reachable_history(self):
        self.commit('README.md', 'safe content\n')
        results, counts = GATE.scan(self.repo)
        self.assertEqual(results, [])
        self.assertEqual(counts['commits'], 1)
        self.assertEqual(counts['blobs'], 1)

    def test_deleted_secret_still_detected_from_history_without_disclosing_value(self):
        fake = 'gh' + 'p_' + 'Z' * 25
        self.commit('local.conf', 'token=' + fake + '\n')
        self.commit('local.conf', None)
        self.commit('README.md', 'safe content\n')
        findings, counts = GATE.scan(self.repo)
        self.assertEqual(counts['commits'], 3)
        self.assertTrue(any(item[2] == 'GitHub credential' for item in findings))
        self.assertFalse(any(fake in str(item) for item in findings))

    def test_sensitive_path_even_without_a_key(self):
        self.commit('.env', 'DEBUG=1\n')
        found, _ = GATE.scan(self.repo)
        self.assertTrue(any(item[0] == 'FILENAME' and item[1] == '.env' for item in found))

    def test_examples_do_not_match_high_confidence_secret_patterns(self):
        self.commit('README.md', 'Use a variable, never store credentials here.\n')
        found, _ = GATE.scan(self.repo)
        self.assertEqual(found, [])


if __name__ == '__main__':
    unittest.main()

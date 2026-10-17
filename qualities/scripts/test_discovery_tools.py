#!/usr/bin/env python3
"""The live gate must fail closed before tests can silently skip missing tools."""
import unittest
from unittest.mock import patch
from discovery_tools import VERSIONS, probe, validate


class DiscoveryToolsTests(unittest.TestCase):
    def test_missing_tools_fail_every_probe(self):
        with patch('discovery_tools.shutil.which', return_value=None):
            checks = probe()
        self.assertEqual(len(checks), len(VERSIONS))
        self.assertTrue(all(c.get('error') == 'executable missing' for c in checks))

    def test_pinned_versions_and_json_capability(self):
        for name, version in VERSIONS.items():
            text = f'{name} {version}'
            if name == 'tokei':
                text += ' compiled with serialization support: json, yaml'
            self.assertIsNone(validate(name, text))
            self.assertIsNotNone(validate(name, f'{name} 0.0.0'))
        self.assertIsNotNone(validate('tokei', 'tokei 15.0.0'))
        self.assertIsNotNone(validate('tokei', 'tokei 15.0.0 serialization support: yaml'))


if __name__ == '__main__':
    unittest.main()

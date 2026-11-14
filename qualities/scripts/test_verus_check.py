#!/usr/bin/env python3
"""Drift detection between the Verus proof and the shipped code."""
import unittest
from verus_check import PROOF, SHIPPED, constants, drift, synced_bodies


def block(body, name='f'):
    return f'// verus-sync: begin {name}\n{body}\n// verus-sync: end {name}\n'


class DriftTests(unittest.TestCase):
    def test_whitespace_only_changes_are_not_drift(self):
        self.assertEqual(drift(block('if a  {\n b }'), block('if a { b }')), [])

    def test_body_change_is_drift(self):
        self.assertEqual(len(drift(block('a - 1'), block('a - 2'))), 1)

    def test_one_sided_and_missing_blocks_are_drift(self):
        self.assertTrue(drift(block('x'), 'fn f() {}'))
        self.assertTrue(drift('fn f() {}', block('x')))
        self.assertTrue(drift('fn f() {}', 'fn f() {}'))

    def test_constant_mismatch_is_drift(self):
        proof = 'const N: u64 = 1_024;\n' + block('x')
        self.assertEqual(drift(proof, 'pub(crate) const N: u64 = 1024;\n' + block('x')), [])
        self.assertEqual(len(drift(proof, 'const N: u64 = 2048;\n' + block('x'))), 1)

    def test_unbalanced_markers_are_rejected(self):
        with self.assertRaises(ValueError):
            synced_bodies('// verus-sync: begin f\n')
        with self.assertRaises(ValueError):
            synced_bodies('// verus-sync: end f\n')

    def test_repository_proof_matches_shipped_code(self):
        self.assertEqual(drift(PROOF.read_text(), SHIPPED.read_text()), [])
        self.assertEqual(constants(PROOF.read_text())['BUDGET_RESERVE'], 32768)


if __name__ == '__main__':
    unittest.main()

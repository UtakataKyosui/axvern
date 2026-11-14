#!/usr/bin/env python3
"""Parsing helpers of the relax spike."""
from types import SimpleNamespace
import unittest
from relax_spike import counts, summary


class SpikeTests(unittest.TestCase):
    def test_counts_sum_passed_and_failed_over_every_test_binary(self):
        result = SimpleNamespace(
            stdout='test result: ok. 10 passed; 0 failed; 0 ignored\ntest result: FAILED. 3 passed; 1 failed\n',
            stderr='')
        self.assertEqual(counts(result), (13, 1))
        self.assertEqual(counts(SimpleNamespace(stdout='running 0 tests\n', stderr='')), (0, 0))

    def test_summary_reports_the_median_not_the_mean(self):
        self.assertEqual(summary([4.0, 0.1, 0.3])['median_s'], 0.3)


if __name__ == '__main__':
    unittest.main()

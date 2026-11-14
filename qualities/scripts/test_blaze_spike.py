#!/usr/bin/env python3
"""Parsing helpers of the blaze spike."""
from types import SimpleNamespace
import unittest
from blaze_spike import first_error, passed, summary


class SpikeTests(unittest.TestCase):
    def test_passed_sums_every_test_binary(self):
        output = 'test result: ok. 10 passed; 0 failed; 0 ignored\ntest result: FAILED. 3 passed; 1 failed\n'
        self.assertEqual(passed(output), 13)
        self.assertEqual(passed('running 0 tests\n'), 0)

    def test_first_error_picks_the_first_error_line(self):
        result = SimpleNamespace(stderr='Compiling x\nerror: boom\nerror: later\n')
        self.assertEqual(first_error(result), 'error: boom')
        self.assertIsNone(first_error(SimpleNamespace(stderr='fine\n')))

    def test_summary_reports_the_median_not_the_mean(self):
        self.assertEqual(summary([4.0, 0.1, 0.3])['median_s'], 0.3)


if __name__ == '__main__':
    unittest.main()

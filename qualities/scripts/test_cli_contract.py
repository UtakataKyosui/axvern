"""Negative fixtures ensure malformed CLI responses fail validation."""
import json
import unittest
import os
from pathlib import Path
import sys
import tempfile
from cli_contract import invoke, validate_machine_output, validate_response


class ContractTests(unittest.TestCase):
    def test_general_json_rule_is_not_axvern_framing(self):
        self.assertEqual(validate_machine_output(b'{\n "data": []\n}\n'), {'data': []})
        # Terminal bytes encoded as user data are not terminal formatting.
        self.assertEqual(validate_machine_output(b'{"data":"\\u001b"}'), {'data': '\x1b'})
        for value in (b'progress\n{}', b'{}{}', b'{"a":1,"a":2}', b'NaN', b'\xff'):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, 'CLI-JSON'):
                validate_machine_output(value)
        with self.assertRaisesRegex(ValueError, 'CLI-COLOR'):
            validate_machine_output(b'\x1b[31m{}')

    def test_noninteractive_probe_detects_waiting_for_stdin(self):
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(ValueError, 'CLI-INPUT'):
                invoke(sys.executable, ['-c', 'input()'], Path(root), os.environ.copy(), timeout=0.2)
            result = invoke(sys.executable, ['-c', 'print("{}")'], Path(root), os.environ.copy())
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b'{}\n')

    def response(self, **changes):
        value = dict(schema_version=1, execution_id=None, state=None,
                     exit_code=None, output=None, error=None)
        value.update(changes)
        return json.dumps(value).encode() + b'\n'

    def test_valid(self):
        validate_response(self.response(), b'', 0)
        validate_response(self.response(state='failed', exit_code=23), b'', 10)
        validate_response(self.response(output={'executions': []},
                          error={'code': 'partial_result', 'message': 'offline'}), b'', 11)

    def test_rejects_invalid(self):
        cases = [
            (b'{}\n', b'', 0),
            (self.response(schema_version=True), b'', 0),
            (self.response(execution_id='axv_bad'), b'', 0),
            (self.response(state='unknown'), b'', 0),
            (self.response(exit_code=True), b'', 0),
            (self.response(output=[]), b'', 0),
            (self.response(), b'warning', 0),
            (self.response() + self.response(), b'', 0),
            (self.response()[:-1], b'', 0),
            (self.response(error={'code': 'invalid_input', 'message': 'bad'}), b'', 3),
            (self.response(error={'code': 'invented', 'message': 'bad'}), b'', 1),
            (self.response(), b'', 10),
            (self.response(), b'', 12),
            (b'{"schema_version":1,"schema_version":1}\n', b'', 0),
            (self.response(output={'value': float('nan')}), b'', 0),
        ]
        for stdout, stderr, code in cases:
            with self.subTest(stdout=stdout, code=code), self.assertRaises(ValueError):
                validate_response(stdout, stderr, code)


if __name__ == '__main__':
    unittest.main()

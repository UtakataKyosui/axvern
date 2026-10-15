"""Compiled CLI contract evaluator: --self-test FIXTURES or AXVERN_BINARY."""
from std.sys import argv
from probes import probe
from tests import test_fixtures, test_process_boundary


def main() raises:
    var args = argv()
    if len(args) == 3 and args[1] == "--self-test":
        test_fixtures(args[2])
        test_process_boundary()
    elif len(args) == 2:
        probe(args[1], "qualities/cli_contract/cases.json")
    else:
        raise Error(
            "usage: cli-contract AXVERN_BINARY | --self-test FIXTURES.json"
        )

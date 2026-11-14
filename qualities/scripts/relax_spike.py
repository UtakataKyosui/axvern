#!/usr/bin/env python3
"""Opt-in comparison of `relax test` and `cargo test` on a private copy of this workspace.

Never touches the checkout: files are copied to a temporary directory and edited there.
Writes JSON evidence; exits non-zero if the two tools disagree on a test count.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
TEST_RESULT = re.compile(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed')
PROBE = '\n#[cfg(test)]\nmod relax_spike_probe_{n} {{\n    #[test]\n    fn probe() {{\n        assert_eq!({n} + 1, {n} + 1);\n    }}\n}}\n'


def run(argv, cwd, env=None, timeout=3600):
    start = time.perf_counter()
    result = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    return result, time.perf_counter() - start


def copy_tree(destination):
    names = subprocess.check_output(['git', 'ls-files', '-z'], cwd=ROOT).decode().split('\0')
    for name in filter(None, names):
        source = ROOT / name
        if source.is_file() and not source.is_symlink():
            (destination / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination / name)


def counts(result):
    """Total (passed, failed) over every test binary in the run's output."""
    found = TEST_RESULT.findall(result.stdout + result.stderr)
    return sum(int(p) for p, _ in found), sum(int(f) for _, f in found)


def summary(samples):
    return {'median_s': round(statistics.median(samples), 3), 'min_s': round(min(samples), 3),
            'max_s': round(max(samples), 3), 'samples': [round(s, 3) for s in samples]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repeat', type=int, default=5, help='edit/retest rounds per tool')
    parser.add_argument('--package', default='axvern-core', help='package for the edit loop')
    parser.add_argument('--record', default=str(ROOT / 'target/relax-spike.json'))
    args = parser.parse_args()
    relax = os.environ.get('AXVERN_RELAX') or shutil.which('relax')
    if not relax:
        print('relax not found: set AXVERN_RELAX or put it on PATH; see qualities/index.md')
        return 0
    evidence = {'host': platform.platform(), 'repeat': args.repeat, 'package': args.package,
                'cargo_version': subprocess.run(['cargo', '--version'], capture_output=True, text=True).stdout.strip()}
    agree = True
    with tempfile.TemporaryDirectory(prefix='axvern-relax-') as scratch:
        work = Path(scratch) / 'tree'
        copy_tree(work)
        cargo_env = {**os.environ, 'CARGO_TARGET_DIR': str(Path(scratch) / 'cargo-target')}
        tools = {'cargo': (['cargo', 'test', '--locked', '-p', args.package], cargo_env),
                 'relax': ([relax, 'test', args.package], None)}
        cold = {}
        for tool, (argv, env) in tools.items():
            result, seconds = run(argv, work, env)
            cold[tool] = counts(result)
            evidence[f'{tool}_cold_test'] = {'seconds': round(seconds, 3), 'exit': result.returncode,
                                             'passed': cold[tool][0], 'failed': cold[tool][1]}
        agree &= cold['cargo'] == cold['relax']
        lib = work / 'logics/core/src/lib.rs'
        original, times = lib.read_text(), {'cargo': [], 'relax': []}
        for round_no in range(args.repeat):
            lib.write_text(original + ''.join(PROBE.format(n=n) for n in range(round_no + 1)))
            now = {}
            for tool, (argv, env) in tools.items():
                result, seconds = run(argv, work, env)
                times[tool].append(seconds)
                now[tool] = counts(result)
            # Each round adds one test, so both tools must see exactly one more than before.
            agree &= now['cargo'] == now['relax'] and now['relax'][0] == cold['relax'][0] + round_no + 1
        lib.write_text(original)
        for tool in times:
            evidence[f'{tool}_edit_retest'] = summary(times[tool])
        binary = Path(scratch) / 'cargo-target/debug/axvern'
        built, _ = run(['cargo', 'build', '--locked'], work, cargo_env)
        if built.returncode == 0 and binary.is_file():
            env = {**cargo_env, 'AXVERN_TEST_BIN': str(binary)}
            integration = {}
            for tool, argv in {'cargo': ['cargo', 'test', '--locked', '-p', 'axvern-tests'],
                               'relax': [relax, 'test', 'axvern-tests']}.items():
                result, seconds = run(argv, work, env)
                integration[tool] = counts(result)
                evidence[f'{tool}_integration_test'] = {'seconds': round(seconds, 3), 'exit': result.returncode,
                                                        'passed': integration[tool][0], 'failed': integration[tool][1]}
            agree &= integration['cargo'] == integration['relax']
        else:
            evidence['integration_skipped'] = 'cargo build of the test binary failed'
    evidence['tools_agree_on_counts'] = bool(agree)
    record = Path(args.record)
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps(evidence, indent=2))
    return 0 if agree else 1


if __name__ == '__main__':
    sys.exit(main())

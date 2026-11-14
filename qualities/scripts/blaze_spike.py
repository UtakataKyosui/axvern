#!/usr/bin/env python3
"""Opt-in comparison of `blaze` and `cargo` on a private copy of this workspace.

Never touches the checkout: files are copied to a temporary directory and edited there.
Writes JSON evidence; exits non-zero only if an edit is not visible in the built binary.
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
VERSION_LINE = 'println!("axvern {}", env!("CARGO_PKG_VERSION"));'
TEST_RESULT = re.compile(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed')


def run(argv, cwd, env=None, timeout=1800):
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


def summary(samples):
    return {'median_s': round(statistics.median(samples), 3), 'min_s': round(min(samples), 3),
            'max_s': round(max(samples), 3), 'samples': [round(s, 3) for s in samples]}


def first_error(result):
    return next((line.strip()[:300] for line in result.stderr.splitlines() if 'error' in line), None)


def passed(output):
    return sum(int(match.group(1)) for match in TEST_RESULT.finditer(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repeat', type=int, default=5, help='edit/rebuild rounds per tool')
    parser.add_argument('--record', default=str(ROOT / 'target/blaze-spike.json'))
    args = parser.parse_args()
    blaze = os.environ.get('AXVERN_BLAZE') or shutil.which('blaze')
    if not blaze:
        print('blaze not found: set AXVERN_BLAZE or put it on PATH; see qualities/index.md')
        return 0
    evidence = {'host': platform.platform(), 'repeat': args.repeat,
                'blaze_version': subprocess.run([blaze, '--version'], capture_output=True, text=True).stdout.strip(),
                'cargo_version': subprocess.run(['cargo', '--version'], capture_output=True, text=True).stdout.strip()}
    with tempfile.TemporaryDirectory(prefix='axvern-blaze-') as scratch:
        work = Path(scratch) / 'tree'
        copy_tree(work)
        main_rs = work / 'tools/cli/src/main.rs'
        original = main_rs.read_text()
        assert VERSION_LINE in original, 'the version line this spike edits has moved'
        cargo_env = {**os.environ, 'CARGO_TARGET_DIR': str(Path(scratch) / 'cargo-target')}
        built = {'cargo': Path(scratch) / 'cargo-target/debug/axvern', 'blaze': work / 'target/debug/axvern'}
        builds = {'cargo': ['cargo', 'build', '--locked'], 'blaze': [blaze, 'build']}
        envs = {'cargo': cargo_env, 'blaze': None}
        try:
            subprocess.run([blaze, 'stop'], capture_output=True)  # cold means no warm daemon
            for tool in builds:
                result, seconds = run(builds[tool], work, envs[tool])
                evidence[f'{tool}_cold_build'] = {'seconds': round(seconds, 3), 'exit': result.returncode,
                                                  'first_error': first_error(result) if result.returncode else None}
            times, correct = {'cargo': [], 'blaze': []}, True
            for round_no in range(args.repeat):
                marker = f'spike-{round_no}'
                main_rs.write_text(original.replace(
                    VERSION_LINE, f'println!("axvern {{}} {marker}", env!("CARGO_PKG_VERSION"));'))
                for tool in builds:
                    result, seconds = run(builds[tool], work, envs[tool])
                    shown = subprocess.run([built[tool], '--version'], capture_output=True, text=True).stdout
                    visible = result.returncode == 0 and marker in shown
                    correct &= visible
                    times[tool].append(seconds)
                    if not visible:
                        evidence.setdefault('edit_not_visible', []).append(f'{tool} round {round_no}')
            main_rs.write_text(original)
            for tool in times:
                evidence[f'{tool}_edit_rebuild'] = summary(times[tool])
            test_env = {**os.environ, 'AXVERN_TEST_BIN': str(built['blaze'])}
            subprocess.run([blaze, 'stop'], capture_output=True)
            steps = [('blaze_test_integration_cold_daemon', ['-p', 'axvern-tests', '--test', 'cli']),
                     ('blaze_test_core', ['-p', 'axvern-core']),
                     ('blaze_test_integration_after_core', ['-p', 'axvern-tests', '--test', 'cli'])]
            for name, extra in steps:
                result, _ = run([blaze, 'test', *extra], work, test_env)
                evidence[name] = {'exit': result.returncode, 'passed': passed(result.stdout),
                                  'first_error': first_error(result) if result.returncode else None}
        finally:
            subprocess.run([blaze, 'stop'], capture_output=True)
    record = Path(args.record)
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps(evidence, indent=2))
    return 0 if correct else 1


if __name__ == '__main__':
    sys.exit(main())

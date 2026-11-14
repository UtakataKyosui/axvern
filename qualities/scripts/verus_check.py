#!/usr/bin/env python3
"""Check that proved Verus code matches the shipped code, then run the proof."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
PROOF = ROOT / 'qualities/verus/budget.rs'
SHIPPED = ROOT / 'logics/core/src/results.rs'
MARKER = re.compile(r'//\s*verus-sync:\s*(begin|end)\s+(\w+)')
CONST = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+(\w+)\s*:\s*u64\s*=\s*([\d_]+)\s*;', re.M)


def synced_bodies(text):
    """Return {name: body without whitespace} for each begin/end marker pair."""
    bodies, open_name, start = {}, None, 0
    for match in MARKER.finditer(text):
        kind, name = match.groups()
        if kind == 'begin':
            if open_name:
                raise ValueError(f'verus-sync {open_name} is not closed')
            open_name, start = name, match.end()
        else:
            if open_name != name:
                raise ValueError(f'verus-sync end {name} has no matching begin')
            bodies[name] = re.sub(r'\s+', '', text[start:match.start()])
            open_name = None
    if open_name:
        raise ValueError(f'verus-sync {open_name} is not closed')
    return bodies


def constants(text):
    return {name: int(value.replace('_', '')) for name, value in CONST.findall(text)}


def drift(proof_text, shipped_text):
    """Describe every way the proof no longer describes the shipped code."""
    problems = []
    proved, shipped = synced_bodies(proof_text), synced_bodies(shipped_text)
    if not proved:
        problems.append('no verus-sync block in the proof')
    for name in sorted(set(proved) | set(shipped)):
        if name not in proved or name not in shipped:
            problems.append(f'{name}: block exists on one side only')
        elif proved[name] != shipped[name]:
            problems.append(f'{name}: body differs between proof and shipped code')
    shared = constants(proof_text)
    for name, value in constants(shipped_text).items():
        if name in shared and shared[name] != value:
            problems.append(f'{name}: proof says {shared[name]}, shipped code says {value}')
    return problems


def find_verus():
    explicit = os.environ.get('AXVERN_VERUS')
    return explicit if explicit and Path(explicit).is_file() else shutil.which('verus')


def main():
    problems = drift(PROOF.read_text(), SHIPPED.read_text())
    if problems:
        print('Verus proof drift:\n  ' + '\n  '.join(problems), file=sys.stderr)
        return 1
    verus = find_verus()
    if not verus:
        if 'AXVERN_REQUIRE_VERUS' in os.environ:
            print('verus required: install the pinned release, see qualities/index.md', file=sys.stderr)
            return 1
        print('verus not found: proof not run (drift check passed); set AXVERN_REQUIRE_VERUS=1 to require it')
        return 0
    result = subprocess.run([verus, '--crate-type=lib', str(PROOF)], capture_output=True, text=True)
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    summary = re.search(r'(\d+) verified, (\d+) errors', result.stdout + result.stderr)
    if result.returncode or not summary or summary.group(2) != '0' or summary.group(1) == '0':
        print('Verus verification failed', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())

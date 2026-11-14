#!/usr/bin/env python3
"""Select check groups from the complete event diff; unknown paths fail closed."""
import json
import os
from pathlib import Path
import subprocess


VERUS_SCRIPTS = ('qualities/scripts/verus_check.py', 'qualities/scripts/test_verus_check.py')
GROUPS = ('rust', 'site', 'links', 'verus')


def groups(paths):
    selected = set()
    for path in paths:
        if path.startswith('.github/issue-updates/'):
            continue
        if path.startswith('sites/axvern-document-site/'):
            selected.update(('site', 'links'))
        elif path.startswith(('logics/', 'tools/cli/')) or path in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'):
            selected.add('rust')
            # Proved code is copied from logics/core, so edits there can drift from the proof.
            if path.startswith('logics/core/'):
                selected.add('verus')
        elif path.startswith('qualities/verus/') or path in VERUS_SCRIPTS:
            selected.add('verus')
        elif path.endswith('.md'):
            selected.add('links')
        else:
            selected.update(('rust', 'site', 'links', 'verus'))
    return selected


def changed_paths(event, head):
    if 'pull_request' in event:
        base = event['pull_request']['base']['sha']
        revision = f'{base}...{head}'
    else:
        base = event.get('before', '')
        if not base or set(base) == {'0'}:
            return None
        revision = f'{base}..{head}'
    result = subprocess.run(['git', 'diff', '--name-only', '--no-renames', '-z', revision], capture_output=True)
    if result.returncode:
        return None
    return [p for p in result.stdout.decode('utf-8', errors='replace').split('\0') if p]


if __name__ == '__main__':
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    paths = changed_paths(event, os.environ['GITHUB_SHA'])
    selected = groups(paths) if paths is not None else set(GROUPS)
    print(json.dumps({'changed': paths, 'checks': sorted(selected)}))
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        for group in GROUPS:
            output.write(f'{group}={str(group in selected).lower()}\n')

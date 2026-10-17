#!/usr/bin/env python3
"""Fail the v0.2 acceptance gate on missing/unverified tools, recording versions."""
import json
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
VERSIONS = {
    'pueue': '4.0.4', 'pueued': '4.0.4', 'fd': '10.5.0', 'rg': '15.2.0',
    'bat': '0.26.1', 'ast-grep': '0.51.0', 'tokei': '15.0.0',
}


def validate(name, output):
    version = re.search(r'\b\d+\.\d+\.\d+\b', output)
    if not version or version.group() != VERSIONS[name]:
        return 'expected version ' + VERSIONS[name]
    if name == 'tokei':
        formats = output.partition('serialization support:')[2].splitlines()
        if not formats or 'json' not in [s.strip() for s in formats[0].split(',')]:
            return 'JSON serialization support required'
    return None


def probe():
    checks = []
    for name in VERSIONS:
        check = {'tool': name, 'expected_version': VERSIONS[name]}
        if not shutil.which(name):
            check['error'] = 'executable missing'
        else:
            try:
                result = subprocess.run([name, '--version'], capture_output=True, text=True,
                                        timeout=10, check=False)
                check['version_output'] = result.stdout.strip()
                error = validate(name, result.stdout) if result.returncode == 0 else 'version probe failed'
                if error:
                    check['error'] = error
            except (OSError, subprocess.TimeoutExpired, UnicodeError):
                check['error'] = 'version probe unavailable'
        checks.append(check)
    return checks


def main():
    checks = probe()
    passed = all('error' not in check for check in checks)
    target = ROOT / 'target/discovery-tools.json'
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({'passed': passed, 'tools': checks}, indent=2) + '\n')
    for check in checks:
        print(check['tool'] + ': ' + check.get('error', 'verified ' + check['expected_version']))
    return 0 if passed else 1


if __name__ == '__main__':
    raise SystemExit(main())

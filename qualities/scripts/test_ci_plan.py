#!/usr/bin/env python3
"""Exercise safety boundaries in CI selection, including stack bases and deletions."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from ci_plan import groups


class PlanTests(unittest.TestCase):
    def test_groups(self):
        cases = [
            (['logics/core/src/lib.rs'], {'rust'}),
            (['Cargo.lock'], {'rust'}),
            (['sites/axvern-document-site/docs/en/index.mdx'], {'site', 'links'}),
            (['README.md'], {'links'}),
            (['.github/issue-updates/12.md'], set()),
            (['.moon/workspace.yml'], {'rust', 'site', 'links'}),
            (['qualities/scripts/ci_plan.py'], {'rust', 'site', 'links'}),
            (['new-unknown-file'], {'rust', 'site', 'links'}),
            (['logics/core/src/lib.rs', 'README.md'], {'rust', 'links'}),
        ]
        for paths, expected in cases:
            with self.subTest(paths=paths):
                self.assertEqual(groups(paths), expected)

    def test_event_diff_and_fallback(self):
        script = Path(__file__).with_name('ci_plan.py').resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=root, text=True).strip()
            git('init', '-q')
            git('config', 'user.email', 'ci@example.invalid')
            git('config', 'user.name', 'CI test')
            (root / 'Cargo.lock').write_text('base')
            git('add', '.')
            git('commit', '-qm', 'base')
            base = git('rev-parse', 'HEAD')
            (root / 'Cargo.lock').unlink()  # deleted paths must trigger checks
            git('add', '-u')
            git('commit', '-qm', 'delete')
            head = git('rev-parse', 'HEAD')
            for event, expected in [
                ({'pull_request': {'base': {'sha': base}}}, {'rust'}),
                ({'before': base}, {'rust'}),
                ({'before': '0' * 40}, {'rust', 'site', 'links'}),
                ({'before': 'missing-revision'}, {'rust', 'site', 'links'}),
                ({}, {'rust', 'site', 'links'}),  # schedule has no diff base
            ]:
                (root / 'event.json').write_text(json.dumps(event))
                output = root / 'output'
                output.write_text('')
                subprocess.run(['python3', str(script)], cwd=root, check=True, capture_output=True,
                               env={**os.environ, 'GITHUB_EVENT_PATH': str(root / 'event.json'),
                                    'GITHUB_SHA': head, 'GITHUB_OUTPUT': str(output)})
                actual = {line.split('=')[0] for line in output.read_text().splitlines() if line.endswith('=true')}
                self.assertEqual(actual, expected)


if __name__ == '__main__':
    unittest.main()

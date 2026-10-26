#!/usr/bin/env python3
"""Exercise Project readiness and the actual pre-commit entrypoint offline."""
from copy import deepcopy
from datetime import date
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import project_gate as gate


POLICY = {'repository': 'UtakataKyosui/axvern', 'project_id': 'project',
          'project_url': 'https://github.com/users/UtakataKyosui/projects/30', 'timezone': 'Asia/Tokyo'}
FIELDS = {'Work type': 'Implementation', 'Status': 'Todo', 'Phase': '1 Execute',
          'Start date': '2026-11-02', 'Target date': '2026-11-13',
          'Paired issue': 'https://github.com/UtakataKyosui/axvern/issues/35'}


def connection(nodes):
    return {'nodes': nodes, 'pageInfo': {'hasNextPage': False}}


def issue(number=88, fields=None, state='OPEN', reason=None, blockers=None):
    fields = fields if fields is not None else FIELDS
    values = []
    for name, value in fields.items():
        kind = 'date' if name.endswith('date') else 'text' if name == 'Paired issue' else 'name'
        values.append({'field': {'name': name}, kind: value})
    return {'id': f'issue-{number}', 'number': number, 'state': state, 'stateReason': reason,
            'repository': {'nameWithOwner': POLICY['repository']},
            'blockedBy': connection(blockers if blockers is not None else [
                {'id': 'issue-35', 'number': 35, 'state': 'CLOSED'}]),
            'projectItems': connection([{'project': {'id': 'project'}, 'isArchived': False,
                                         'fieldValues': connection(values)}])}


class ReadinessTests(unittest.TestCase):
    def setUp(self):
        self.impl = issue()
        self.fields = deepcopy(FIELDS)
        self.req = issue(35, {'Work type': 'Requirements'}, 'CLOSED', 'COMPLETED', [])
        self.pair = self.req, {'Work type': 'Requirements'}

    def assess(self, **kwargs):
        return gate.assess(self.impl, self.fields, kwargs.pop('mode', 'develop'),
                           kwargs.pop('today', date(2026, 11, 2)),
                           kwargs.pop('requirement', self.pair), **kwargs)

    def test_closed_completed_requirement_is_satisfied(self):
        self.assertEqual(self.assess(), ([], []))

    def test_open_cancelled_and_wrong_type_requirements_are_rejected(self):
        for state, reason in [('OPEN', None), ('CLOSED', 'NOT_PLANNED')]:
            self.req.update(state=state, stateReason=reason)
            self.assertTrue(self.assess()[0])
        self.req.update(state='CLOSED', stateReason='COMPLETED')
        self.assertTrue(self.assess(requirement=(self.req, {'Work type': 'Follow-up'}))[0])

    def test_pair_and_native_blocker_are_both_required(self):
        self.assertTrue(self.assess(requirement=None)[0])
        self.impl['blockedBy'] = connection([])
        self.assertTrue(self.assess()[0])

    def test_early_start_does_not_bypass_open_blockers(self):
        self.assertTrue(self.assess(today=date(2026, 10, 27))[0])
        errors, warnings = self.assess(today=date(2026, 10, 27), early_reason='ユーザーが前倒しを指示')
        self.assertFalse(errors)
        self.assertTrue(warnings)
        self.impl['blockedBy']['nodes'].append({'id': 'issue-108', 'number': 108, 'state': 'OPEN'})
        self.assertTrue(self.assess(today=date(2026, 10, 27), early_reason='前倒し')[0])

    def test_missing_invalid_reversed_and_overdue_dates(self):
        for start, end in [(None, '2026-11-13'), ('invalid', '2026-11-13'),
                           ('2026-11-13', '2026-11-02')]:
            self.fields.update({'Start date': start, 'Target date': end})
            self.assertTrue(self.assess()[0])
        self.fields.update(FIELDS)
        errors, warnings = self.assess(today=date(2026, 11, 14))
        self.assertFalse(errors)
        self.assertTrue(warnings)

    def test_requirements_need_plan_mode_and_closed_work_cannot_restart(self):
        self.fields['Work type'] = 'Requirements'
        self.assertTrue(self.assess()[0])
        self.assertFalse(self.assess(mode='plan')[0])
        self.impl['state'] = 'CLOSED'
        self.assertTrue(self.assess(mode='plan')[0])

    def test_archived_wrong_project_and_truncated_information_fail_closed(self):
        self.assertEqual(gate.project_fields(self.impl, POLICY), FIELDS)
        for change in ['archived', 'wrong_project', 'truncated']:
            node = deepcopy(self.impl)
            item = node['projectItems']['nodes'][0]
            if change == 'archived':
                item['isArchived'] = True
            elif change == 'wrong_project':
                item['project']['id'] = 'other'
            else:
                item['fieldValues']['pageInfo']['hasNextPage'] = True
            with self.assertRaises(gate.GateError):
                gate.project_fields(node, POLICY)
        self.impl['blockedBy']['pageInfo']['hasNextPage'] = True
        with self.assertRaises(gate.GateError):
            self.assess()

    def test_pair_url_cannot_redirect_to_another_repository(self):
        fields = {**FIELDS, 'Paired issue': 'https://github.com/other/repo/issues/35'}
        with patch.object(gate, 'fetch_issue', return_value=issue(fields=fields)) as fetch:
            result = gate.inspect(POLICY, 88, 'develop')
        self.assertFalse(result['ready'])
        self.assertEqual(fetch.call_count, 1)

    def test_gh_errors_and_partial_responses_do_not_pass(self):
        with patch.object(gate, 'run', side_effect=gate.GateError('offline')):
            with self.assertRaises(gate.GateError):
                gate.graphql('query{}', {})
        for payload in ['not json', '{"data":null}', '{"data":{},"errors":[{}]}']:
            with patch.object(gate, 'run', return_value=payload):
                with self.assertRaises(gate.GateError):
                    gate.graphql('query{}', {})


class HookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git('init', '-q', '--initial-branch=main')
        source = Path(__file__).resolve().parents[2]
        for name in ['qualities/scripts/project_gate.py', '.githooks/pre-commit']:
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / name, target)
        (self.root / 'qualities/project-policy.json').write_text(json.dumps(POLICY))
        bindir = self.root / 'bin'
        bindir.mkdir()
        fake = bindir / 'gh'
        fake.write_text('#!/usr/bin/env python3\nimport json,os,sys\n'
                        'request=json.load(sys.stdin)\n'
                        'issues=json.load(open(os.environ["PROJECT_TEST_FIXTURE"]))\n'
                        'print(json.dumps({"data":{"repository":{"issue":issues[str(request["variables"]["number"])]}}}))\n')
        fake.chmod(0o755)
        self.fixture = self.root / 'fixture.json'
        self.impl = issue(fields={**FIELDS, 'Start date': '2000-01-01', 'Target date': '2099-01-01'})
        self.req = issue(35, {'Work type': 'Requirements'}, 'CLOSED', 'COMPLETED', [])
        self.write_fixture()
        self.env = {**os.environ, 'PATH': str(bindir) + os.pathsep + os.environ['PATH'],
                    'PROJECT_TEST_FIXTURE': str(self.fixture)}
        self.env.pop('AXVERN_ISSUE', None)

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True).strip()

    def write_fixture(self):
        self.fixture.write_text(json.dumps({'88': self.impl, '35': self.req}))

    def command(self, *args, env=None):
        return subprocess.run(['python3', 'qualities/scripts/project_gate.py', *args],
                              cwd=self.root, env=env or self.env, text=True, capture_output=True)

    def test_real_hook_checks_staged_work_and_reloads_remote_state(self):
        installed = self.command('install-hooks')
        self.assertEqual(installed.returncode, 0, installed.stderr)
        self.assertEqual(self.git('config', '--get', 'core.hooksPath'), '.githooks')
        (self.root / 'change.txt').write_text('change')
        self.git('add', 'change.txt')
        hook = ['./.githooks/pre-commit']
        missing = subprocess.run(hook, cwd=self.root, env=self.env, capture_output=True)
        self.assertNotEqual(missing.returncode, 0)
        selected = self.command('select', '88')
        self.assertEqual(selected.returncode, 0, selected.stderr)
        self.assertEqual(subprocess.run(hook, cwd=self.root, env=self.env, capture_output=True).returncode, 0)
        self.req.update(state='OPEN', stateReason=None)
        self.impl['blockedBy']['nodes'][0]['state'] = 'OPEN'
        self.write_fixture()
        self.assertNotEqual(subprocess.run(hook, cwd=self.root, env=self.env, capture_output=True).returncode, 0)

    def test_branch_switch_does_not_reuse_task_selection(self):
        self.assertEqual(self.command('select', '88').returncode, 0)
        self.git('symbolic-ref', 'HEAD', 'refs/heads/other')
        self.assertNotEqual(self.command('check').returncode, 0)
        self.git('symbolic-ref', 'HEAD', 'refs/heads/issue-88-work')
        self.assertEqual(self.command('check').returncode, 0)

    def test_existing_hooks_are_not_overwritten(self):
        self.git('config', 'core.hooksPath', 'custom-hooks')
        self.assertNotEqual(self.command('install-hooks').returncode, 0)
        self.assertEqual(self.git('config', '--get', 'core.hooksPath'), 'custom-hooks')
        self.git('config', '--unset', 'core.hooksPath')
        legacy = self.root / '.git/hooks/pre-commit'
        legacy.write_text('existing hook')
        self.assertNotEqual(self.command('install-hooks').returncode, 0)
        self.assertEqual(legacy.read_text(), 'existing hook')


if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""Offline tests for Claude's adapter and the repository harness wiring."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import agent_hook


class AgentHookTests(unittest.TestCase):
    def test_startup_does_not_require_network_or_task_selection(self):
        with patch.object(agent_hook.subprocess, 'run') as run:
            result = agent_hook.handle({'hook_event_name': 'SessionStart'})
        run.assert_not_called()
        self.assertIn('select N', result['hookSpecificOutput']['additionalContext'])

    def test_read_only_tools_do_not_run_gate(self):
        with patch.object(agent_hook.subprocess, 'run') as run:
            self.assertEqual({}, agent_hook.handle({
                'hook_event_name': 'PreToolUse', 'tool_name': 'Read'}))
        run.assert_not_called()

    def test_all_edit_tools_check_this_checkout_without_granting_permission(self):
        root = Path('/tmp/checkout with spaces')
        for tool in agent_hook.EDIT_TOOLS:
            with self.subTest(tool=tool), patch.object(
                agent_hook.subprocess, 'run',
                return_value=subprocess.CompletedProcess([], 0, '{}', ''),
            ) as run:
                result = agent_hook.handle({
                    'hook_event_name': 'PreToolUse', 'tool_name': tool,
                    'cwd': '/tmp/unrelated',
                }, root)
                self.assertEqual({}, result)
                self.assertEqual(root, run.call_args.kwargs['cwd'])
                self.assertEqual(
                    [sys.executable, str(root / 'qualities/scripts/project_gate.py'), 'check'],
                    run.call_args.args[0],
                )

    def test_failed_gate_denies_and_explains(self):
        for stdout, stderr in [('未完了Blocker: #1', ''), ('', '対象Issue未選択')]:
            with self.subTest(stderr=stderr), patch.object(
                agent_hook.subprocess, 'run',
                return_value=subprocess.CompletedProcess([], 1, stdout, stderr),
            ):
                result = agent_hook.handle({
                    'hook_event_name': 'PreToolUse', 'tool_name': 'Edit',
                })['hookSpecificOutput']
                self.assertEqual('deny', result['permissionDecision'])
                self.assertIn(stderr or stdout, result['permissionDecisionReason'])

    def test_timeout_or_missing_executable_denies(self):
        for error in [OSError(), subprocess.TimeoutExpired('gate', 50)]:
            with self.subTest(error=error), patch.object(
                agent_hook.subprocess, 'run', side_effect=error,
            ):
                result = agent_hook.handle({
                    'hook_event_name': 'PreToolUse', 'tool_name': 'Write',
                })
                self.assertEqual('deny', result['hookSpecificOutput']['permissionDecision'])

    def test_invalid_input_fails_closed(self):
        for value in ['bad json', '[]', 'null']:
            with self.subTest(value=value), patch('sys.stdin', io.StringIO(value)), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(2, agent_hook.main())

    def test_repository_wiring_and_shared_skill(self):
        root = agent_hook.ROOT
        settings = json.loads((root / '.claude/settings.json').read_text())
        for name in ['SessionStart', 'PreToolUse']:
            hooks = settings['hooks'][name]
            self.assertIn('qualities/scripts/agent_hook.py', hooks[0]['hooks'][0]['command'])
        self.assertEqual(agent_hook.EDIT_TOOLS,
                         set(settings['hooks']['PreToolUse'][0]['matcher'].split('|')))
        self.assertGreater(settings['hooks']['PreToolUse'][0]['hooks'][0]['timeout'], 50)
        self.assertIn('@AGENTS.md', (root / 'CLAUDE.md').read_text())
        link = root / '.claude/skills/project-workflow'
        self.assertTrue(link.is_symlink())
        self.assertEqual((root / '.agents/skills/project-workflow').resolve(), link.resolve())
        self.assertTrue((link / 'SKILL.md').is_file())


if __name__ == '__main__':
    unittest.main()

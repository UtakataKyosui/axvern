#!/usr/bin/env python3
"""Claude Code adapter for the shared Project gate; never grants tool permission."""
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]
EDIT_TOOLS = {'Edit', 'Write', 'NotebookEdit'}


def handle(event, root=ROOT):
    name = event.get('hook_event_name')
    if name == 'SessionStart':
        return {'hookSpecificOutput': {
            'hookEventName': name,
            'additionalContext': (
                'axvern: qualities/agent-harness.md の手順を使用する。'
                '編集前に対象IssueのACを読み、リポジトリルートで '
                'python3 qualities/scripts/project_gate.py select N を通す。'
                'BashやMCP経由の編集にも同じ事前検査が必要。'
                '調査のみなら inspect を使う。'
            ),
        }}
    if name != 'PreToolUse' or event.get('tool_name') not in EDIT_TOOLS:
        return {}
    try:
        # Always use this checkout, regardless of the tool's current directory.
        result = subprocess.run(
            [sys.executable, str(root / 'qualities/scripts/project_gate.py'), 'check'],
            cwd=root, capture_output=True, text=True, timeout=50,
        )
        if result.returncode == 0:
            # Do not return "allow": retain Claude's normal permission checks.
            return {}
        reason = (result.stderr or result.stdout).strip()
    except (OSError, subprocess.TimeoutExpired):
        reason = 'Project検査を実行できません（未導入またはタイムアウト）。'
    return {'hookSpecificOutput': {
        'hookEventName': name,
        'permissionDecision': 'deny',
        'permissionDecisionReason': (
            'Project gate未通過。select Nで対象Issueを選び、失敗理由を解消してください。\n'
            + reason[:6000]
        ),
    }}


def main():
    try:
        event = json.load(sys.stdin)
        if not isinstance(event, dict):
            raise ValueError('hook input must be an object')
        print(json.dumps(handle(event), ensure_ascii=False))
        return 0
    except (ValueError, TypeError):
        print('axvern hook: 不正なJSON入力です。', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())

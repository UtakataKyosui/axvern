#!/usr/bin/env python3
"""Read-only Project readiness checks and worktree-local task selection."""
import argparse
from datetime import date, datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from zoneinfo import ZoneInfo


class GateError(Exception):
    pass


def run(argv, cwd=None, input_text=None):
    try:
        result = subprocess.run(argv, cwd=cwd, input=input_text, text=True,
                                capture_output=True, timeout=20)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise GateError(f'{argv[0]} を実行できません（未導入またはタイムアウト）。') from exc
    if result.returncode:
        raise GateError(f'{argv[0]} が失敗しました。Git/ghの設定・認証・接続を確認してください。')
    return result.stdout.strip()


def graphql(query, variables):
    try:
        result = json.loads(run(['gh', 'api', 'graphql', '--input', '-'],
                                input_text=json.dumps({'query': query, 'variables': variables})))
    except json.JSONDecodeError as exc:
        raise GateError('GitHubの応答がJSONではありません。') from exc
    if result.get('errors') or not isinstance(result.get('data'), dict):
        raise GateError('GitHub GraphQL照会に失敗しました。Project閲覧権限を確認してください。')
    return result['data']


QUERY = '''query($owner:String!,$repo:String!,$number:Int!){
  repository(owner:$owner,name:$repo){issue(number:$number){
    id number state stateReason repository{nameWithOwner}
    blockedBy(first:100){nodes{id number state} pageInfo{hasNextPage}}
    projectItems(first:100){pageInfo{hasNextPage} nodes{
      isArchived project{id}
      fieldValues(first:100){pageInfo{hasNextPage} nodes{
        ... on ProjectV2ItemFieldSingleSelectValue{name field{... on ProjectV2FieldCommon{name}}}
        ... on ProjectV2ItemFieldTextValue{text field{... on ProjectV2FieldCommon{name}}}
        ... on ProjectV2ItemFieldDateValue{date field{... on ProjectV2FieldCommon{name}}}
      }}
    }}
  }}
}'''


def fetch_issue(policy, number):
    owner, repo = policy['repository'].split('/')
    data = graphql(QUERY, {'owner': owner, 'repo': repo, 'number': number})
    issue = (data.get('repository') or {}).get('issue')
    if not issue or issue['repository']['nameWithOwner'] != policy['repository']:
        raise GateError(f'対象Issue #{number} を取得できません。')
    return issue


def complete(connection):
    if connection.get('pageInfo', {}).get('hasNextPage'):
        raise GateError('関連情報が100件を超えています。省略分を確認するまで進めません。')
    return connection['nodes']


def project_fields(issue, policy):
    item = next((i for i in complete(issue['projectItems'])
                 if i['project']['id'] == policy['project_id']), None)
    if not item or item['isArchived']:
        raise GateError(f'有効なProject itemへ登録してください: {policy["project_url"]}')
    values = {}
    for value in complete(item['fieldValues']):
        if 'field' in value:
            values[value['field']['name']] = next(
                (value[key] for key in ('name', 'text', 'date') if key in value), None)
    return values


def assess(issue, fields, mode, today, requirement=None, early_reason=None):
    errors, warnings = [], []
    if issue['state'] != 'OPEN':
        errors.append('作業対象Issueはopenである必要があります。')
    allowed = {'develop': {'Implementation', 'Follow-up'},
               'plan': {'Requirements', 'Phase', 'Follow-up'}}[mode]
    if fields.get('Work type') not in allowed:
        errors.append(f'Work typeが{mode}作業の対象ではありません。')
    if fields.get('Status') not in {'Todo', 'In Progress'}:
        errors.append('StatusをTodoまたはIn Progressにしてください。')
    if not fields.get('Phase'):
        errors.append('Phaseが未設定です。')
    try:
        start = date.fromisoformat(fields['Start date'])
        target = date.fromisoformat(fields['Target date'])
    except (KeyError, TypeError, ValueError):
        errors.append('有効なStart date / Target dateを設定してください。')
    else:
        if target < start:
            errors.append('Target dateがStart dateより前です。')
        if today < start:
            if early_reason and early_reason.strip():
                warnings.append(f'前倒し着手の明示理由: {early_reason.strip()}')
            else:
                errors.append(f'着手予定日前です（{start}）。前倒しの指示が必要です。')
        if today > target:
            warnings.append(f'完了目安 {target} を超過しています。残作業と再計画を報告してください。')
    blockers = complete(issue['blockedBy'])
    if any(b['state'] != 'CLOSED' for b in blockers):
        errors.append('未完了Blocker: ' + ', '.join('#' + str(b['number'])
                      for b in blockers if b['state'] != 'CLOSED'))
    if fields.get('Work type') == 'Implementation':
        if requirement is None:
            errors.append('同じProjectのRequirements IssueをPaired issueへ設定してください。')
        else:
            paired, paired_fields = requirement
            if paired_fields.get('Work type') != 'Requirements':
                errors.append('ペアのWork typeがRequirementsではありません。')
            if paired['id'] not in {b['id'] for b in blockers}:
                errors.append('ペアの要件定義IssueをGitHubのblocked-byに設定してください。')
            if paired['state'] != 'CLOSED' or paired['stateReason'] != 'COMPLETED':
                errors.append('ペアの要件定義Issueがcompletedでcloseされていません。')
    return errors, warnings


def inspect(policy, number, mode, early_reason=None):
    issue = fetch_issue(policy, number)
    fields = project_fields(issue, policy)
    requirement = None
    if fields.get('Work type') == 'Implementation':
        pattern = r'https://github\.com/' + re.escape(policy['repository']) + r'/issues/([1-9][0-9]*)'
        match = re.fullmatch(pattern, fields.get('Paired issue') or '')
        if match:
            paired = fetch_issue(policy, int(match[1]))
            requirement = paired, project_fields(paired, policy)
    today = datetime.now(ZoneInfo(policy['timezone'])).date()
    errors, warnings = assess(issue, fields, mode, today, requirement, early_reason)
    return {'issue': number, 'mode': mode, 'fields': fields,
            'errors': errors, 'warnings': warnings, 'ready': not errors}


def context_path(root):
    path = Path(run(['git', 'rev-parse', '--git-path', 'axvern-project-task.json'], cwd=root))
    return path if path.is_absolute() else root / path


def branch_issue(branch):
    match = re.search(r'(?:^|/)(?:issue|task)[/-]([1-9][0-9]*)(?=$|[-/])', branch)
    return int(match[1]) if match else None


def resolve(args, root, branch):
    if args.issue is not None:
        return args.issue, args.mode or 'develop', args.early_start_reason
    if os.environ.get('AXVERN_ISSUE'):
        try:
            number = int(os.environ['AXVERN_ISSUE'])
        except ValueError as exc:
            raise GateError('AXVERN_ISSUEは正の整数で指定してください。') from exc
        return number, args.mode or 'develop', args.early_start_reason
    number = branch_issue(branch)
    path = context_path(root)
    if path.exists():
        try:
            context = json.loads(path.read_text())
            if context['branch'] == branch:
                return context['issue'], args.mode or context['mode'], args.early_start_reason or context.get('early_start_reason')
        except (KeyError, TypeError, json.JSONDecodeError) as exc:
            raise GateError('作業選択情報が不正です。selectで選び直してください。') from exc
    if number:
        return number, args.mode or 'develop', args.early_start_reason
    raise GateError('対象Issueを指定してください: python3 qualities/scripts/project_gate.py select N')


def save_context(root, branch, number, mode, reason):
    path = context_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
            temporary = Path(stream.name)
            json.dump({'branch': branch, 'issue': number, 'mode': mode,
                       'early_start_reason': reason}, stream)
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def install_hooks(root):
    result = subprocess.run(['git', 'config', '--get', 'core.hooksPath'], cwd=root,
                            text=True, capture_output=True, timeout=20)
    if result.returncode not in (0, 1):
        raise GateError('既存のhooksPathを確認できません。')
    current = result.stdout.strip()
    if current and current != '.githooks':
        raise GateError('既存のcore.hooksPathを保持します。既存Hookとの統合が必要です。')
    legacy = Path(run(['git', 'rev-parse', '--git-path', 'hooks/pre-commit'], cwd=root))
    if not legacy.is_absolute():
        legacy = root / legacy
    if not current and legacy.exists():
        raise GateError('既存のpre-commitを保持します。既存Hookとの統合が必要です。')
    hook = root / '.githooks/pre-commit'
    if not hook.is_file() or not os.access(hook, os.X_OK):
        raise GateError('.githooks/pre-commitに実行権限がありません。')
    run(['git', 'config', '--local', 'core.hooksPath', '.githooks'], cwd=root)
    print('core.hooksPath=.githooks をこのリポジトリで有効化しました。')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('install-hooks')
    for command in ('check', 'inspect', 'select'):
        child = sub.add_parser(command)
        if command == 'select':
            child.add_argument('issue', type=int)
        else:
            child.add_argument('--issue', type=int)
        child.add_argument('--mode', choices=('develop', 'plan'))
        child.add_argument('--early-start-reason')
    args = parser.parse_args()
    try:
        root = Path(run(['git', 'rev-parse', '--show-toplevel']))
        if args.command == 'install-hooks':
            install_hooks(root)
            return 0
        branch = run(['git', 'symbolic-ref', '--quiet', '--short', 'HEAD'], cwd=root)
        number, mode, reason = resolve(args, root, branch)
        if not isinstance(number, int) or isinstance(number, bool) or number < 1:
            raise GateError('対象Issueは正の整数で指定してください。')
        policy = json.loads((root / 'qualities/project-policy.json').read_text())
        result = inspect(policy, number, mode, reason)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        if args.command == 'select' and result['ready']:
            save_context(root, branch, number, mode, reason)
        return 0 if result['ready'] or args.command == 'inspect' else 1
    except (GateError, OSError, ValueError, KeyError, TypeError) as exc:
        print(f'Project gate: {exc}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())

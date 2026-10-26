# axvern の開発運用

このリポジトリの作業管理は [GitHub Project axvern](https://github.com/users/UtakataKyosui/projects/30)
を正本とする。開発・要件整理・受入作業では
[project-workflow Skill](.agents/skills/project-workflow/SKILL.md) を読む。
Claude Code / Codexのセットアップと検証の選択は
[開発ハーネス](qualities/agent-harness.md)を参照する。

- 編集前に対象Issueと受入条件を確認し、`project_gate.py check` を通す。
  調査・レビューのみなら `inspect` を使い、未着手の作業は変更しない。
- 実装は `Implementation`、追加検証・保守は `Follow-up`、要件整理は
  `--mode plan` を使う。要件定義Issueのcloseと実装完了を区別する。
- 実装Issueには、同じProjectの `Requirements` Issueをペアとして指定し、
  GitHubのblocked-byにも設定する。要件と他のBlockerの完了を確認する。
- Phase、Start date、Target dateを尊重する。予定日前の着手はユーザーの
  明示した前倒し指示を理由付きで記録する。予定日超過は報告し再計画する。
- 進捗・検証証拠・残課題をIssueに対応付ける。ProjectやIssueの更新は
  その操作が依頼に含まれる場合に行い、権限を自動的に拡張しない。
- 必要な範囲を検証する。commit・push・PR作成・公開はユーザーの依頼時のみ行う。
- `.claude/worktrees` など他の作業・ローカル状態を変更やcommitの対象にしない。

Git Hooksの導入: `python3 qualities/scripts/project_gate.py install-hooks`。
対象Issueの選択: `python3 qualities/scripts/project_gate.py select 108`。
Hookはcommit前の検査、Skillは編集前から完了報告までの手順を担当する。
ユーザーの明示指示はこの運用より優先し、例外を使った場合は理由と範囲を報告する。

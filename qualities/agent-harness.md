# Claude Code / Codex 開発ハーネス

共通の指示は [AGENTS.md](../AGENTS.md)、作業選択は
[project-workflow](../.agents/skills/project-workflow/SKILL.md)、
着手判定は [project_gate.py](scripts/project_gate.py) を正本とする。
Claude Codeは [CLAUDE.md](../CLAUDE.md) のimportと
`.claude/skills/project-workflow` の相対symlinkを通じて同じ内容を使う。
Codexは `AGENTS.md` と `.agents/skills` を読み込む。

## 初回セットアップ

リポジトリルートで実行する。Python 3.9以降、Git、認証済み `gh` が必要。
Claude Code / Codex本体と開発ツールは各開発環境に導入する。

```sh
gh auth status
python3 qualities/scripts/project_gate.py install-hooks
python3 qualities/scripts/test_project_gate.py
python3 qualities/scripts/test_agent_hook.py
# どちらかを起動
claude
codex
```

GitHub認証には対象リポジトリとProjectの閲覧権限が必要。
資格情報はリポジトリに保存しない。モデル・個人MCP・APIキーは個人設定で管理する。
Codexの `.codex/config.toml` はworkspace-write / on-requestを指定する。
リポジトリ設定の読み込みにはCodex側でプロジェクトを信頼する必要があり、
管理設定・起動引数はリポジトリ設定より優先される。
Claude Codeでは新しいセッションの `/hooks` で登録を確認する。
この設定は通常のツール承認を自動許可しない。

## 作業開始から引き継ぎまで

1. Issue本文・確定要件・AC・関連PRを読む。コードの存在だけで受入済みと判断しない。
2. `python3 qualities/scripts/project_gate.py inspect --issue N` で状態を確認し、
   編集する場合は `python3 qualities/scripts/project_gate.py select N` を通す。
   選択内容はworktree・ブランチに紐づく。切替後は選び直す。
3. 対象ディレクトリの指示を確認して変更し、ACに対応する最小の検証を行う。
4. 最終報告に対象Issue、変更内容、実行した検証と結果、未検証・残課題を記載する。
   別Agentへ引き継ぐ場合も同じ情報と作業ブランチを渡す。
   同じ作業ツリーを2つのAgentで同時編集しない。
5. commit・push・PR・Issue/Project更新はそれぞれ依頼された場合に行う。

## 検証の選び方

| 変更対象 | 検証 |
| --- | --- |
| ハーネス・Project gate | `moon run qualities:harness-test` |
| Rust | `moon run qualities:fmt qualities:check qualities:clippy` と変更に対応するテスト |
| CLI統合・契約 | `moon run qualities:test`（build依存、Mojo契約検証を含む） |
| 公開サイト | 配下のAGENTS.mdに従い `moon run site:check site:build qualities:links` |
| 全体受入 | `devbox run test`。必要な外部CLI・Mojo準備は [品質ガイド](index.md) を参照 |

CLI統合テストはビルド済みバイナリを `AXVERN_TEST_BIN` に渡すため、Moon経由を基本とする。
外部ツール不足によるskipは受入完了としない。変更範囲に応じて実機検証を追加する。
依存方向は `tools/cli → logics/core → logics/models`、統合テストは `logics/tests`。

## Hookの範囲

- Claude Code: SessionStartで着手手順を通知し、Edit / Write / NotebookEditの前に
  最新Projectを検査する。失敗・通信障害・タイムアウト時は編集を拒否する。
- Codex: AGENTS.mdとSkillに従って編集前に同じ検査を実行する。
- Git: 両者共通のpre-commitが最新Projectを再検査する。

ClaudeのBash・MCP経由の書き込み、Codexの編集、jjの変更記録はこのClaude Hookや
Git pre-commitで一律に遮断されない。これらも編集前に明示的に `check` を行う。
Hookはセキュリティ境界ではない。検査を通すためにBashへ迂回しない。
明示指示による例外は理由・範囲を報告し、恒久的な検査無効化にしない。
予定日の前倒しはSkillにある `--early-start-reason` を使う。

## 今回の導入記録

2026-10-27: ユーザーの「Claude CodeとCodexで開発する前提のハーネスセットアップ」
依頼に基づくローカル整備。対応する既存Issueがなかったため、Issue選択の例外として
この設定・ガイド・検証のみを追加した。コマンド実装Phaseの着手は含まない。
PR化にあたり [Follow-up #110](https://github.com/UtakataKyosui/axvern/issues/110)
をProjectへ登録した。以後は本Issueを選択し、コミット時もProject gateを通す。

仕様参照: [Codex設定](https://learn.chatgpt.com/docs/config-file/config-basic)、
[Codex Skills](https://learn.chatgpt.com/docs/build-skills)、
[Claude memory](https://code.claude.com/docs/en/memory)、
[Claude Skills](https://code.claude.com/docs/en/skills)、
[Claude Hooks](https://code.claude.com/docs/en/hooks)。

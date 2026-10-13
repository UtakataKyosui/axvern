# CLI仕様・設定

> 出典: 2026-10-13時点の統合版 `REQUIREMENTS.md`。要件ID・受入条件IDは維持。

## 導入フェーズ別コマンド一覧（計画）

| フェーズ | 主なコマンド | 裏側の既存CLI |
| --- | --- | --- |
| v0.1 | `run`, `status`, `wait`, `history`, `policy check`, `doctor` | pueue |
| v0.2 | `fs list/find/search/read/analyze`, `code search/stats` | eza, fd, rg, bat, tokei, ast-grep |
| v0.2 任意 | `code diff`, `system disk-usage`, `system processes` | git + delta, dust, procs |
| v0.3 | `query`, `recall`, `logs`（仮） | Pueueログと必要に応じたaxvernインデックス |
| v0.4+ | `bench run`, `bench compare`, `profile`（仮） | hyperfine, samply, flamegraph等 |

下記の従来型「コマンド一覧」は全フェーズを通じた**予定構文**であり、v0.1ですべてが利用できるという意味ではない。

## 5. CLIインターフェース（提案）

| コマンド | 概要 |
| --- | --- |
| `axvern run -- <program> [args...]` | ポリシー評価後、Pueueへバックグラウンド登録 |
| `axvern status <execution-id>` | 状態照会 |
| `axvern wait <execution-id> [--timeout 30s]` | 終了待機（時間切れでもタスクは継続） |
| `axvern history [--limit 20]` | axvern経由の登録履歴 |
| `axvern query <execution-id> --match "error"` | キーワード検索 |
| `axvern query <execution-id> --errors` | エラー候補抽出 |
| `axvern query <execution-id> --tail 50` | 末尾50行 |
| `axvern query <execution-id> --from-line 120 --lines 30` | 行範囲抽出 |
| `axvern recall <execution-id>` | 保持されている完全ログを表示 |
| `axvern policy check -- <program> [args...]` | 登録せず実行可否判定 |
| `axvern doctor [--format json]` | 診断 |
| `axvern fs list [path] [--all]` | ezaで一覧を非同期取得 |
| `axvern fs find <pattern> [path] [--hidden] [--no-ignore]` | fdでファイル探索を非同期取得 |
| `axvern fs search <pattern> [path] [--hidden] [--no-ignore]` | rgでテキスト検索を非同期取得 |
| `axvern fs read <file> --from-line 1 --lines 80` | batで部分閲覧（v0.2） |
| `axvern code search --pattern <ast-pattern> [path]` | ast-grepで構造検索（v0.2） |
| `axvern fs analyze <path> [--max-depth 3] [--format json]` | tokei + eza + fdの統合分析（v0.2） |
| `axvern code stats [path]` | tokeiで統計を取得（v0.2任意） |
| `axvern system disk-usage [path]` | dustで容量上位項目を取得（v0.2任意） |
| `axvern system processes [--match <text>]` | procsでプロセスを検索（v0.2任意） |
| `axvern code diff [--staged]` | git + deltaで差分を取得（v0.2任意） |
| `axvern text replace <pattern> <replacement> <path>` | sdによる変更操作（将来候補、明示承認要） |

原則として`--`以降をaxvernの引数から分離する。ただしPueueへの引数渡しの互換性はSpike完了まで確定としない。`query`にはデフォルトの返却サイズ上限を設けるが、`recall`は明示的な全量取得操作であるため、大量出力に関する警告やファイル出力手段を設計する。

### 5.1 出力例（仕様イメージ）

```text
$ axvern run -- cargo test
execution_id: axv_01...
pueue_task_id: 42
status: queued

$ axvern query axv_01... --errors
execution_id: axv_01...
log_state: available
matches:
  18: error[E0308]: mismatched types
  19:   --> src/lib.rs:22:5
hint: axvern query axv_01... --from-line 15 --lines 15
```


## 6. 設定要件

例（詳細スキーマは実装設計で確定）：

```yaml
version: 1
runtime:
  backend: pueue
policy:
  default: ask
  rules:
    - id: read-git-status
      effect: allow
      command: git
      args_prefix: [status]
    - id: forbid-hard-reset
      effect: deny
      command: git
      args_prefix: [reset, --hard]
modern_cli:
  auto_replace: false
  # 対応した安全な引数形式だけを、個別の置換ルールで有効化する
  rules: []
query:
  default_max_lines: 100
  default_max_bytes: 65536
```

- 設定の存在しない場合の振る舞い、設定探索順序、ユーザー別の保存領域は実装前に確定する。
- Pueueデーモンを共有する際、axvern管理のタスクを他のPueue操作から隔離できるとは仮定しない。
- Pueueの設定バージョンやCLI JSON形式の差異は互換性判定の対象にする。



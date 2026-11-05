# #93 doctor 受入対応表

2026-11-05 JST。要件 #40 / 共通 #34。PR #62/#66/#75を再利用。

| 条件 | 実装・検証 |
| --- | --- |
| SPEC-40-01 / AC-13：daemon停止はFail12・remedy | `doctor::diagnose_scope`、`pueue_live::public_cli_execute_doctor_and_history_contracts`（JSON/human、タスク変更なし） |
| SPEC-40-02：任意不足Warning0、必須不足Fail | `cli::doctor_tool_and_store_failures_are_structured`（core/discovery/performance/all、required/optional・availability） |
| SPEC-40-03：不正設定もcheck Fail | 同CLIテスト、`config::validate`。parserの生診断・秘密値を出さない |
| SPEC-40-04：private scratch作成・削除 | 公開CLI liveでprobe残存なし・既存pending不変・Pueue一覧不変。`store::diagnose_until`がread/write/fsync/cleanup |
| AC-22：不足/非対応CLIと修復案 | 同CLIテストでfd欠落、tokei JSON capability欠落、各remedy・version range |
| 全体予算・store lock deadline | `diagnose_scope_with_timeout`は最大30s、各probeはremaining以内かつ最大5s。`contract_v2::doctor_deadline_bounds_waiting_for_another_clients_store_lock`（500ms指定、5s未満でTimeout診断） |
| opt-in置換診断・scope対象 | `rewrite::doctor_reports_rules_and_their_dependencies`、CLI scopeテスト |

modelsのCheckLevel/DoctorReport、coreの診断とbounded probe、tools/cliのscope/JSON/human/Fail終了12を対応させた。
version応答成功は各Adapterの実処理互換の証明にしない。対応済みtoolはcapabilityも確認し、将来toolは未提供を明示する。
自動install/sudo/daemon起動/修復は行わず、既存実行記録を削除しない。

macOS arm64 / Pueue 4.0.4。CLI11件、contract_v2 12件を再実行し、隔離Pueue7件の診断・scratch証拠を参照。
30秒実時間の全scope飽和試験を新たに行ったという主張ではなく、短縮予算のlock実測とコードのdeadline伝播を区別する。
公開仕様: `sites/axvern-document-site/docs/en/cli.md`。既存両OS証拠: #108 / `34-40-macos.md`。Linux新revisionはPR Qualityで確認し、未確認の段階でcloseしない。

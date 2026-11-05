# #89 status 受入対応表

2026-11-05 JST。要件 #36、共通 #34。既存PR #61/#62/#66を再利用。

| 条件 | 実装 | 検証 |
| --- | --- | --- |
| SPEC-36-01：対象失敗でも照会0、exit23 | CLI status、`services::target_snapshot` | `pueue_live::public_cli_execute_doctor_and_history_contracts` |
| SPEC-36-02：未知ID7 | `store::find_record`、CLI | 同liveテスト、`cli::unknown_execution_uses_the_same_exit_code_for_each_query_format` |
| SPEC-36-03：削除/ID再利用はexpired8 | Pueue所有identity照合 | `isolated_registration_wait_cwd_argv_expiry_and_receipt_recovery`（再利用IDを別タスクとして参照しない） |
| SPEC-36-04：親partial・子状態 | `parents::status`、CLI | 公開CLI live、`contract_v2::parent_state_aggregation_does_not_invent_child_exit_codes` |
| AC-03/14：queued/running、null、未知状態拒否 | models ChildState / Pueue `parse_status` | `backend_store::fixtures_normalize_all_results_and_reject_unknown_schema` を強化（各安定状態・exit/終了時刻null・未知variant拒否） |
| daemon停止とbackend失敗の区別 | Pueue adapter、services | `backend_store::status_preserves_start_and_response_failures`、live offline adapter |
| 結果可用性と状態の分離 | `results::inspect`、CLI | `contract_v2` のpending/保存失敗/TTL/eviction、公開CLI live |

登録時queue/task ID/created_atを照合する。source/observed_atはCLIが観測値として出力する。
状態照会で保存結果本文を取得せず、新規登録は行わない。親・子の終了コードを捏造しない。
公開仕様: `sites/axvern-document-site/docs/en/cli.md`、従来の両OS実行参照: #108 / `34-40-macos.md`。

macOS arm64 / Pueue 4.0.4。今回のintegration全202件と、強化したbackend fixtureを実行。
Linuxの新revisionはQuality workflowで確認する。共通差分 #116 / result差分 #117の受入と区別し、CI未確認の段階でIssueをcloseしない。

# #107 共通CLI契約の受入対応

2026-11-05 JST。要件 #34、result固有契約 #65 / 実装 #112。

| 契約 | 実装 | 検証 |
| --- | --- | --- |
| schema2、JSON既定、human、null、対象終了値とCLI終了値 | models `Response`、CLI `error_exit` / `emit` | `cli.rs`、`pueue_live::public_cli_execute_doctor_and_history_contracts` |
| config優先順位・未知key・schema・private権限 | `core/config.rs`、`store.rs` | `cli.rs`、`contract_v2::settings_validate_limits_without_changing_config_schema` |
| Policy→所有Pueue→実行ID→保存結果 | `core/services`、`results.rs` | `pueue_live.rs` の6隔離テスト |
| 未知ID7・backend障害の分類維持 | CLI `result_response` | `cli::unknown_execution_uses_the_same_exit_code_for_each_query_format` |
| 保存失敗と対象exitの分離 (#67) | `results::record_persistence_failure` | `contract_v2::save_failure_after_target_exit_keeps_target_status_and_leaves_no_artifact` |
| worker境界・scratch回復・保守コスト (#68) | PR #113/#114、`parents.rs` | `contract_v2::parent_save_recovers_its_own_abandoned_scratch_and_refuses_unsafe_scratch`、`34-40-macos.md` |

`result_response` がmanifestなしの照会エラーを一律11に変えていたため、共通終了コード表へ統一した。
未知IDは既存互換の `task_not_found` / 7。応答schema2とconfig/store schema1は別契約。

ローカルmacOSで `AXVERN_TEST_BIN=$PWD/target/debug/axvern cargo test -p axvern-tests --locked --test cli --test contract_v2 --test backend_store --test pueue_live` 成功（39 passed、skipなし）。
隔離Pueueにはsandbox外のUnix socket権限が必要。通常sandbox内のdaemon起動失敗は成功証拠に含めない。
LinuxはPRのQuality workflowで検証する。巨大structured itemの差分は #112、未集約子保護は #98、runner強制終了後の安全回復は #68 の責務として残す。
各関連Issueが未完了のまま共通契約全体を受入済みとはしない。

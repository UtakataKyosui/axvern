# #88 run 受入対応表

2026-11-05 JST。要件 #35 / 共通 #34 / result #65。
#108の「検証のみ」の判断に従い、PR #60〜#62/#66/#115の実装を再利用した。

| 受入条件 | 実装 | 検証（logics/tests/tests配下） |
| --- | --- | --- |
| SPEC-35-01、AC-01/02：ID・Pueue所有・対象成功 | `services::run_captured`、CLI `run_response` | `pueue_live::public_cli_execute_doctor_and_history_contracts` |
| SPEC-35-02、RUN-WAIT-001〜004：既定待機・0s・期限後継続 | `run_response`、`observe_wait` | 同liveテスト、`schema_two_keyed_run_replays_after_policy_change_and_backend_log_deletion`（完了済み再送も0sでavailable） |
| SPEC-35-03/04、RUN-IDEMP-001〜003：同時再送・入力競合 | `store::register_request` | `contract_v2::simultaneous_request_replay_conflict_and_unknown_never_register_twice`（8並列・登録1件、承認再実行なし） |
| SPEC-35-05、RUN-IDEMP-004/005：登録不明・永続記録 | `store.rs`、`services` | 上記テスト、`process_crash_after_durable_intent_is_never_resubmitted`、`backend_store::registration_unknown_never_retries_and_bounds_client_wait` |
| SPEC-35-06、AC-05/06：Deny・非対話Askは登録0件 | `policy.rs`、`services` | liveテストの登録前後一覧一致、`terminal_ask_requires_an_explicit_yes` |
| AC-07：argv/cwd、空引数・改行・引用符・Unicode・メタ文字 | models `ExecutionPlan`、Pueue adapter | `isolated_registration_wait_cwd_argv_expiry_and_receipt_recovery`、公開CLI liveテスト（注入marker不在） |
| 対象exit23とrun終了0・保存結果 | CLI `run_response`、`results` | 公開CLI live、keyed live、`contract_v2::worker_captures_before_completion_and_pages_unicode_with_exact_wire_budget` |
| 同時容量予約・公開API統一 | PR #115、`ExecutionService::run` | `concurrent_service_runs_without_request_id_register_one_task_within_capacity` |

型はmodels、Policy/予約/捕捉はcore、構文/JSON/human/終了値はtools/cli。
argv上限・NUL・不正UTF-8はmodelsの検証、wait/request-idの入力拒否は `cli::schema_two_options_policy_cwd_matching_rules_and_invalid_boundaries` で確認する。
保存失敗 #67 とworker回復 #68を重複実装しない。公開仕様は `sites/axvern-document-site/docs/en/cli.md`。

実行: macOS arm64、Pueue 4.0.4、rustc 1.99.1。
`AXVERN_TEST_BIN=$PWD/target/debug/axvern cargo test -p axvern-tests --locked` は202件成功。
うちPueue live 6件を専用socket/queue/storeで実行し、実ユーザーのキューは使用しない。
Linuxの新revision受入はPRのQuality workflowに委ねる。既存両OS証拠は #108 と `34-40-macos.md`、共通差分PR #116、result差分PR #117を参照。
#68の強制終了runner回復、Linuxの新revision検証が未完了の状態で、Issue全体をclose済みとは扱わない。

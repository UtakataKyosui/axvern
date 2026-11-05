# #92 policy check 受入対応表

2026-11-05 JST。要件 #39 / 共通 #34。PR #60/#66/#69の実装を再利用。

| 条件 | 実装・検証 |
| --- | --- |
| SPEC-39-01：未一致Ask/0、承認UI・実行なし | `planner::evaluate`、`cli::syntax_config_errors_and_target_flags`、`policy_check_reports_all_decisions_without_executing_target` |
| SPEC-39-02：Deny > Ask > Allow、全一致rule順 | `policy.rs`、`cli::schema_two_options_policy_cwd_matching_rules_and_invalid_boundaries`（a/d全件、主d） |
| SPEC-39-03：不正policy3、secret非掲載 | `config::validate`、`cli::syntax_config_errors_and_target_flags`、`policy_output_golden_and_refusal_before_backend_setup` |
| SPEC-39-04：cwd検証共通化 | CLI `planned` / `plan`、`cli::schema_two_options_policy_cwd_matching_rules_and_invalid_boundaries`（存在するcwdと不存在cwd2） |
| AC-05/06：runでDeny/非対話Ask登録なし | `pueue_live::public_cli_execute_doctor_and_history_contracts` の登録前後一覧一致 |
| 元/置換後の厳しいPolicyを採用 | `planner.rs`、`cli::policy_check_reports_rewrite_and_evaluates_both_commands`、`rewrite::both_plans_are_evaluated_and_registration_happens_once` |
| opt-in以外で置換しない・shell既定拒否 | `rewrite::rewriting_is_off_by_default`、`policy.rs`、公開CLI live |

policy checkの分岐はExecutionService初期化より前に返り、Pueue接続・対象起動・store登録を呼ばない。
CLIテストはPueue設定なしでもAllow/Ask/Denyの全判定が成功し、touch対象のmarkerを作らず、stderrに承認promptを出さないことを検証する。
modelsのPolicyDecision、coreの検証/Policy/planner、tools/cliの表示・終了値へ分担する。
Ask/Denyを判定した操作自体は成功0。runの拒否9と区別する。

macOS arm64、CLI11件・rewrite10件を実行。既存integration202件、隔離Pueueの実測を再利用。
公開仕様は `sites/axvern-document-site/docs/en/cli.md`。既存両OS参照は #108 / `34-40-macos.md`、新revisionのLinux受入はQuality workflow。
共通 #116 / result #117 の残条件と、この読み取り専用Policy判定の受入を分ける。CI未確認の時点でIssueをcloseしない。

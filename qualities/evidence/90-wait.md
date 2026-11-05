# #90 wait 受入対応表

2026-11-05 JST。要件 #37 / 共通 #34。既存PR #62/#66を再利用。

| 受入条件 | 実装・検証 |
| --- | --- |
| SPEC-37-01：done0 | `services::observe_wait` / CLI、`pueue_live::public_cli_execute_doctor_and_history_contracts` |
| SPEC-37-02：対象exit23、CLI10 | 同公開CLI live（target exit_codeをJSONに保持） |
| SPEC-37-03 / AC-04：timeout124、対象継続 | 同liveで1ms予算超過後、同じIDをreleaseしてdone。adapter deadlineは `backend_store::inherited_backend_pipes_do_not_extend_probe_deadline` |
| SPEC-37-04：0・負数拒否2 | 公開CLI liveの入力境界。整数ms/s/m・1ms〜24hはCLI duration parser |
| SPEC-37-05：Ctrl-C相当 | 新規 `interrupting_wait_keeps_the_same_target_running_without_registration`。待機CLIへSIGINT、signal2終了、同一対象継続、登録件数1を維持、release後にdone |
| 親partial11・null | `contract_v2::parent_state_aggregation_does_not_invent_child_exit_codes`、公開CLI live |
| 保存失敗と対象終了の分離 | #67 / `contract_v2::save_failure_after_target_exit_keeps_target_status_and_leaves_no_artifact` |

poll間隔250ms、各backend呼出しは残りdeadline以内。timeoutは対象停止・再登録を呼ばない。
SIGINT時の完全JSONは保証しない。シェルではsignal2を130相当として観測する。
公開仕様は `sites/axvern-document-site/docs/en/cli.md`、既存両OS証拠は #108 / `34-40-macos.md`。

macOS arm64 / Pueue 4.0.4。新規SIGINTテストとPueue live全件を専用queue/socketで実行。
Linuxの新revisionはPR Qualityで確認する。共通PR #116、result PR #117、#68の残条件を別途保持し、未確認のCIを受入済みとしない。

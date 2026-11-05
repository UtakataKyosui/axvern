# #91 history 受入対応表

2026-11-05 JST。要件 #38 / 共通 #34。PR #61/#62/#66を再利用。

| 条件 | 実装・検証 |
| --- | --- |
| SPEC-38-01：空storeはdaemon不在でも0件/0 | 新規 `cli::offline_history_is_ordered_bounded_and_does_not_disclose_rewritten_argv` |
| SPEC-38-02：offline時metadata・partial11 | 同テスト、`pueue_live::public_cli_execute_doctor_and_history_contracts`（daemon停止後に3行保持） |
| SPEC-38-03：同時刻ID降順 | 同テストで3件同時刻、limit 1/2/3と保存記録不変 |
| SPEC-38-04：limit0拒否2 | 公開CLI live（0/1001）、CLI parser（既定20、1〜1000） |
| AC-15：他client除外・親子・失効mapping | 公開CLI live、`isolated_registration_wait_cwd_argv_expiry_and_receipt_recovery` |
| private権限・破損は黙って落とさない | `backend_store::recovery_permissions_corruption_and_write_failure` |
| result TTLと履歴/request記録の独立 | `contract_v2` のTTL/tombstone、request再送・prune拒否、`pueue_live` のclean後再送 |

コード確認で、opt-in置換を使った実行のrecord.rewriteに元/変換後argvが含まれ、そのままhistoryへ出る差分を検出した。
history応答からrewriteを除外し、private storeには保持する。新規テストは秘密文字列非掲載とstore不変を同時に検証する。
`services::history`、`store::history`、CLIの親子merge/sort/limitが担当。カーソル検索は追加しない。

macOS arm64 / Pueue 4.0.4。CLI suiteと既存integration202件の結果を利用。
公開仕様は `sites/axvern-document-site/docs/en/cli.md`。Linux新revisionはQuality、既存両OS参照は #108 / `34-40-macos.md`。
metadataの自動pruneは行わず、容量不足はfail closed。新規CI未確認の時点でcloseしない。

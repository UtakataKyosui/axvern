# #112 result確定契約の差分受入

2026-11-05 JST。正本は要件 #65。共通終了コードは #107 / PR #116。

| AC | 実装・自動検証 |
| --- | --- |
| RESULT-001〜006 / SPEC-65-01〜06 | `results.rs` / `contract_v2.rs` のpreview、cursor、TTL、eviction、`pueue_live::schema_two_keyed_run_replays_after_policy_change_and_backend_log_deletion` |
| RESULT-007 / SPEC-65-11,13 | `results::maintain`、`contract_v2::preview_truncation_capacity_eviction_and_active_spool_reservations`、容量予約テスト。未集約子保護は #98 のPhase 2受入 |
| SPEC-65-07,08 | `worker_captures_before_completion_and_pages_unicode_with_exact_wire_budget`、`malformed_cursors_are_rejected_even_when_parsable`、新規structuredテスト |
| SPEC-65-09 | `results::structured_fragments`、`structured_text_fragments_reconstruct_and_replay_with_wire_budget`、`structured_record_limit_and_oversized_summary_are_explicit` |
| SPEC-65-10 | #67既存修正 / `save_failure_after_target_exit_keeps_target_status_and_leaves_no_artifact` |
| SPEC-65-12 | `pueue_live` のPueue clean後保存結果再取得 |
| SPEC-65-14 | #68。強制終了後の予約は保護するが、安全な自動回復入口は未提供。再登録・直接file削除を案内しない |
| SPEC-65-15 | PR #116の未知ID7、既存pending/空結果テスト。空structured artifactは新規summary境界テストでも検証 |
| SPEC-65-16 | マージ済みPR #115。公開APIを捕捉・予約経路へ統一、`pueue_live::concurrent_service_runs_without_request_id_register_one_task_within_capacity` |

今回、保存済みJSON行を変更せず、巨大な構造化stringフィールドをUTF-8境界で分割して返す。
item_idは保存行の位置で安定し、text_fieldとフィールド内fragment_indexで再構成する。
非text属性は最初のfragmentだけに含む。cursorには構造化fragment位置を記録し、旧行境界cursorも読める。
100/101件、1024 bytes（JSON外枠とLF含む）、複数field、再送、別ID、Unicode、巨大summary、分割不能な巨大非textを検証する。
巨大非textは取得不能の明示応答とし、存在しないダウンロード参照を返さない。

macOS arm64 / Pueue 4.0.4 / rustc 1.99.1でcoreテスト6件、check・clippy・リンク42件を通過。
既存契約の独立した実行証拠は `34-40-macos.md`。LinuxはPRのQuality workflowで確認する。
runner回復 #68 と未集約子保護 #98 の未完了を、本PRで完了へ読み替えない。

追加回帰: `AXVERN_TEST_BIN=$PWD/target/debug/axvern cargo test -p axvern-tests --locked` は全202テスト成功。任意tool内部の条件分岐をすべて実行した証明とはしない。

## PR #117 レビュー修正

長いpathを分割した際、128 bytes以下の既存textがfragment本文に上書きされていた。
既存textも独立したfield fragmentへ含め、空文字にも終端fragmentを返すよう修正した。
`fragmented_match_path_preserves_short_and_empty_matching_text` は実際のMatchItemを使い、短文・128 bytesのUnicode・空文字と長いpathを1024 bytes/1件のページで完全再構成する。
修正前の短文欠落による失敗を確認し、修正後core7件・clippy・buildを通過した。

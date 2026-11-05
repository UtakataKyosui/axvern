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
| SPEC-65-14 | 2026-11-06の追加回復入口・検証は下記。終了証拠のない予約は引き続き保護する |
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

## #112 追加: 中断された保存公開の明示回復（2026-11-06）

`axvern result recover EXECUTION_ID` はstore lock下で実行メタデータ、Pueueのqueue/task/creation identityと終端状態、worker lease、capture完了証拠を照合する。
workerは対象終了・capture reader終了・spool fsync後にのみ完了証拠を永続化する。
leaseはworkerが保持し、強制終了時にOSが解放する。PID再利用を生存判断に使わない。
確証後に対象終了値・完了時刻を保ったunavailableを公開し、spool/outputを後始末して予約を解放する。
後始末前の中断では予約を引き続き容量へ算入し、同じ回復入口から再試行できる。
回復でPueue add、対象再実行、TTL延長、request-id削除を行わない。通常result読取も回復を実行しない。

追加自動検証:

- `results::tests::recovery_requires_completion_inactive_lease_and_matching_terminal_backend`: 完了証拠なし・live lease・ID再利用・非終端状態を拒否。rename後の中断から対象exit23/完了時刻123を保持し、予約解放と再送を確認。fake Pueueはstatus以外を拒否する。
- `results::tests::killed_worker_releases_lease_but_preserves_completion_proof`: 別プロセスを強制終了し、OS lease解放・pending予約と完了証拠の永続化を確認。
- `cli::result_recovery_rejects_pagination_and_extra_operands_before_backend_access`: ページ引数・余分なoperand・waitをexit2で拒否。

macOS 27 arm64 / rustc 1.99.1 / Pueue 4.0.4:
`cargo test -p axvern-core --locked` 10件成功、`cargo check --workspace --locked`、
`cargo clippy --workspace --all-targets --locked -- -D warnings`、fmt、build成功。
`AXVERN_TEST_BIN=$PWD/target/debug/axvern cargo test -p axvern-tests --locked` は既存全統合テスト成功。
追加CLIテストは別途1件成功。sandbox内のdaemon起動失敗後、隔離Pueueをsandbox外で再検証した。
site check、ローカルリンク42件成功。LinuxとCIの受入証拠はPRのQuality workflowで別途確認する。

制約: 完了証拠を残す前に停止したworker、旧版worker、Pueue消失/identity不一致は回復の確証不足として予約を保持する。
元処理の再登録や直接file削除を案内しない。追加の任意completion metadataは旧binaryで拒否されるため、保守には更新版を使う。
この記録はLinuxで新規障害試験を行った証拠ではなく、#98の未集約子保護の完了も意味しない。

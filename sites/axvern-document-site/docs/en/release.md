# Release gates and evidence

This source branch is a release candidate implementation, not a released artifact. Distribution currently means building/installing from this repository checkout. License selection, binary release destination, registry publication, signing and hosting require separate decisions/authorization. No license grant or published binary is implied.

Run from the repository root after installing the pinned toolchain, Node 24.22.0, pnpm 12.11.2, Moon 2.5.6, Python 3, and Pueue 4.0.4:

```sh
pnpm --dir sites/axvern-document-site install --frozen-lockfile
devbox run test
```

Moon runs Rust fmt/check/Clippy/build/tests, site check/build/local-link validation, the Pueue spike, and an isolated release-build/install smoke test. The install gate verifies the public executable name, version, and help through a temporary Cargo install root; it does not change the user's installed tools.

GitHub Actions runs the same gates on Ubuntu and macOS and uploads `quality-<os>` artifacts: test/release evidence and the Pueue spike JSON. Both matrix jobs must pass before a v0.1 release can be approved. No workflow deploys or publishes packages.

| Requirement | Executable evidence |
| --- | --- |
| AC-01/02 | Public CLI returns before completion; persisted ID resolves to the same backend task |
| AC-03/14 | Public status/wait normalize states, child exit and timestamps; JSON parses without diagnostic text |
| AC-04 | Timeout followed by successful completion of the same task |
| AC-05/06 | Deny/default Ask/shell refusal creates no tasks |
| AC-07 | Empty/quoted/newline/metacharacter/Unicode/flag-like arguments and cwd survive unchanged |
| AC-13 | Stopped isolated daemon produces doctor Fail, recovery instruction and exit 12 in JSON/human |
| AC-15 | Direct Pueue registration is absent from axvern history; history limit and expiry verified |

Tests create a private daemon/config/socket/store and never attach to the user's queue. Store tests include thread/process concurrency, interrupted writers, schema/permission failures and receipt recovery. Public output golden fixtures cover stdout/stderr and failure exits. Doctor is checked before/after against task snapshots and pending writer data.

Local macOS execution is recorded under `qualities/evidence/14-16-macos.md`. Linux results are supplied by the Ubuntu CI job after this branch is committed/pushed; until that evidence exists, the cross-platform release gate remains pending. Do not describe Linux acceptance as completed merely because its workflow is configured.

## Schema 2 additions (#34–#40)

The current source adds JSON-default schema 2, bounded run waiting, durable request IDs, saved run previews/result pagination, logical-parent state aggregation, policy cwd/matching-rule reporting and scoped doctor checks. Local evidence is recorded in `qualities/evidence/34-40-macos.md`: Rust contract/integration tests, Mojo validator/process probes, strict Clippy/fmt, site build/links and isolated install smoke pass on macOS. Linux evidence remains pending CI. That historical core-only validation did not cover discovery producers or aggregate artifacts; the v0.2 gate below adds their acceptance environment. Remote Issues are not automatically closed by these checks.

## v0.2 Discover acceptance (#2 / #22)

The discovery commands and limited replacement catalog are implemented. This PR adds the final public usage/limitations documentation and a required live-test environment; it does not declare Epic #2 complete or publish a release. The scope and known limitations are in the [discovery guide](/discovery).

CI installs fd 10.5.0, ripgrep 15.2.0, bat 0.26.1, ast-grep 0.51.0 and tokei 15.0.0, in addition to Pueue 4.0.4. `AXVERN_REQUIRE_DISCOVERY_TOOLS=1` makes a missing dependency fail instead of returning early from discovery live tests. The preflight verifies pinned versions and tokei JSON serialization and writes `target/discovery-tools.json` (uploaded with the matrix artifacts). Optional eza/git/delta/dust/procs are not installed as gate dependencies. Permission checks still explain skips when a privileged user can read mode-000 directories; CI should run as an unprivileged user.

After installing tools and the pinned Mojo compiler, run the full gate locally:

```sh
python3 qualities/scripts/discovery_tools.py
AXVERN_REQUIRE_DISCOVERY_TOOLS=1 devbox run test
```

Local tests without the strict environment may skip absent tools; that is useful during development but is not full v0.2 acceptance evidence. A green fixture-only run does not certify real CLI compatibility. New Linux/macOS live results must be read from the PR's completed CI jobs; this document does not assert success before they run.

| Requirement / boundary | Executable coverage in `logics/tests/tests` |
| --- | --- |
| AC-16–19, AC-25/26 | `rewrite.rs`, `cli.rs`: default-off, catalog forms, original/transformed Policy, missing-tool fallback, no automatic writes/repeaters |
| AC-20/21 | `discovery_fixtures.rs`, `discovery_live.rs`: argv, sorted entries, hidden/ignore behavior, regex/glob/case, root scope; fd replaces eza for structured lists |
| AC-22 | `discovery_cli.rs`, `discovery_fixtures.rs`, `code_fixtures.rs`: missing/unsupported tools, JSON capability, refusal before registration |
| AC-23 | `discovery_live.rs`: ranges/EOF, BOM/CRLF, binary/invalid encoding and escaped controls |
| AC-24 | `code_fixtures.rs`, `code_live.rs`: supported languages, malformed patterns, byte positions and best-effort parse errors |
| AC-SPEC-41-01–05 | `discovery_live.rs` (`list_applies_depth_hidden_ignore_symlink_and_git_rules`, `list_truncates_at_the_item_limit_and_confirms_an_exact_fit`, `unreadable_subdirectory_is_a_partial_result_not_a_failure`), `discovery_cli.rs` (`invalid_input_is_refused_with_exit_2_before_registration`): empty directory, depth 0/33, permission-denied partial, limit truncation, root symlink |
| AC-SPEC-42-01–05 | `discovery_live.rs` (`find_maps_regex_glob_extension_type_case_and_full_path`), `discovery_fixtures.rs` (`fd_records_are_sorted_typed_and_non_utf8_names_are_reported_by_parent`, `fd_failures_are_typed`): zero match, invalid pattern, glob+extension AND, newline names, partial vs truncated |
| AC-SPEC-43-01–05 | `discovery_live.rs` (`search_maps_ranges_context_globs_ignore_and_not_found`), `discovery_fixtures.rs` (`rg_json_becomes_sorted_items_with_ranges_and_context`, `rg_exit_codes_distinguish_no_match_partial_and_invalid_pattern`, `rg_binary_files_non_utf8_names_and_lines_and_the_match_limit`): no match, invalid pattern, multi-range line, binary/non-UTF-8, long-line flag |
| AC-SPEC-44-01–05 | `discovery_live.rs` (`read_returns_the_requested_range_with_eof_and_rejects_unreadable_targets`, `long_lines_are_cut_on_a_character_boundary_and_flagged`), `discovery_cli.rs` (`reading_a_fifo_is_rejected_without_waiting_for_a_writer`, `read_line_range_accepts_the_last_valid_line_and_rejects_only_real_overflow`), `discovery_fixtures.rs` (`bat_rejects_binary_and_non_utf8_and_flags_long_lines`): EOF, lines 0/overflow, directory/FIFO, binary, long line |
| AC-FSLIST-001–019 | `discovery_live.rs`, `discovery_fixtures.rs` (`fd_uses_only_gitignore_with_nested_negations_and_explicit_roots`, `live_fd_does_not_apply_personal_excludes`): cwd default, depth, hidden/ignore, `.git`, symlinks, sorting, errors, limits, `.gitignore` scope |
| AC-27 | `analyze_fixtures.rs` (`a_directory_has_three_children_with_separate_display_and_scan_depths`, `a_file_is_analyzed_alone_and_is_a_manifest_only_by_name`), `analyze_live.rs` (`a_file_is_analyzed_alone`): directory sections vs single-file languages child |
| AC-28 | `discovery_cli.rs` (`analyze_is_validated_and_refused_before_any_child_is_registered`): one denied or noninteractive-Ask child registers no parent/child |
| AC-29 | `analyze_fixtures.rs` (`a_failed_child_gives_a_partial_result_with_its_error`, `a_child_lost_before_storing_its_result_fails_only_its_section`, `registration_stops_at_the_first_failure_and_keeps_registered_children`), `analyze_live.rs` (`a_failed_child_leaves_a_partial_result_and_the_others_run_once`) |
| AC-30 | `analyze_fixtures.rs` (separate display/scan depths), `analyze_live.rs` (`unreadable_directories_and_bounds_are_reported`, `.git`/symlink boundaries): tree depth N, scan depth 32, truncated vs partial |
| AC-32 | `analyze_fixtures.rs` (`three_finished_children_are_published_once_and_reread_without_the_backend`, `parent_time_precedes_child_registration_even_across_a_second_boundary`), `analyze_live.rs` (reread after `pueue clean`): parent publishes once, retains child outcomes, paging reruns nothing |
| AC-SPEC-46-01–04 | `code_live.rs` (`search_distinguishes_zero_matches_bad_patterns_and_bad_input`, `search_finds_positions_and_distrusts_files_that_do_not_parse`), `code_fixtures.rs` (`zero_matches_invalid_pattern_and_failures_are_different_results`, `matches_use_byte_columns_and_files_with_syntax_errors_are_not_trusted`), `discovery_cli.rs` (`code_input_is_checked_before_registration`): zero match, missing/unsupported lang before registration, parse_error partial |
| AC-SPEC-47-01–04 | `code_fixtures.rs` (`an_empty_target_has_zero_totals`, `language_stats_keep_embedded_languages_out_of_the_totals`, `a_tokei_without_json_support_is_not_capable`), `code_live.rs` (`stats_report_unreadable_directories_as_partial`, `stats_count_languages_and_keep_embedded_ones_apart`), `discovery_cli.rs` (`code_tools_need_policy_and_the_right_build`): empty target, host/embedded split, partial scope, JSON-less tokei refused |
| AC-SPEC-49-01–04 | `disk_usage.rs`, `discovery_cli.rs`: unreadable directory → partial + `lower_bound`, hard link counted once, top/depth validation before registration; dust is not used, so no `unsupported_tool_capability` path exists (AC-SPEC-49-03 is satisfied by always measuring natively in the requested mode) |
| AC-SPEC-48-01–04 | `code_diff.rs`, `discovery_cli.rs` (`code_diff_and_processes_input_is_refused_with_exit_2_before_registration`): no differences → empty `done` result and git state unchanged; outside a worktree → `not_a_repository`, exit 2, nothing registered; binary file → metadata only; delta is never used, so output is always plain (AC-SPEC-48-04 holds trivially); a configured filter/fsmonitor program never starts and `info/attributes` filters are refused |
| AC-SPEC-50-01–04 | `processes.rs`, `discovery_cli.rs`: unfiltered snapshot with no process operation, zero matches → empty `done`, `--limit 0/1001` → `invalid_input` exit 2 before registration; permission-hidden processes yield `partial` by construction (`permission_denied` error) but cannot be provoked portably, so that path is not exercised by a test |
| AC-SPEC-45-01–05 | `analyze_fixtures.rs`, `analyze_live.rs`, `discovery_cli.rs`: sections/file input, partial child failure, Policy-before-registration, saved paging and no rerun |
| Bounds, symlinks, permissions | discovery/code/analyze fixture and live suites; oversized header regression, lost child and registration interruption cases |
| Core schema/CLI | Mojo evaluator in `qualities/cli_contract`, existing core Rust/isolated Pueue suites |

AC-27–30/32 are defined in Issue #45 (2026-10-18) and mapped above; AC-31 is excluded from the v0.2 gate; saved `result` artifacts do not imply future query/recall guarantees. Requirement Issues still name eza for structured listings and stronger scope/depth behavior than some tools can provide; the fd decision and documented tool limitations need to be reconciled in those Issues before declaring the Epic complete. Text/code search and statistics now share the depth-32 and `.gitignore` contract, verified by `scope_live.rs` on Linux/macOS. The native ast-grep syntax-error detector remains best-effort, and tokei still cannot report a reliable count of skipped unknown/binary files.

Historical macOS measurements are in `qualities/evidence/18-macos.md`, `19-macos.md`, `20-macos.md`, and the optional-adapter spike `23-macos.md`. They describe the environment measured at that time; the new strict CI transcript is the evidence for this revision. Schema-1 additive fields may be rejected by older binaries, so downgrade compatibility is not promised. Release numbering, distribution and publication remain separate decisions.

## v0.3 Retrieve acceptance (#3 / #24–#27, #51–#53)

`query` and `recall` are implemented over the Pueue log that still exists; `logs` is deliberately not a command. The behaviour, limits and retention are in the [retrieve guide](/retrieve). Measurements and the spike are in `qualities/evidence/24-27-retrieve.md`; the macOS run (peak RSS about 8.8 MiB on a 95 MB log) is recorded there; the Mojo contract extension is still owed, so the Mojo gate is not claimed. Remote Issues are not closed by these checks.

| Requirement / boundary | Executable coverage in `logics/tests/tests` |
| --- | --- |
| AC-08 / FR-QUERY-002–006 | `logs_fixtures.rs` (`match_is_literal_case_sensitive_and_numbers_original_lines`, `error_candidates_use_independent_words_only`, `tail_returns_the_last_lines_with_exact_numbers`, `range_is_one_based_and_beyond_the_end_is_empty`), `logs_live.rs`: matching lines carry line number, execution ID and condition |
| AC-09 / FR-QUERY-007 | `logs_live.rs` (`query_and_recall_read_the_log_without_running_the_command_again`): repeated selectors, one counter line, one history entry |
| AC-10 / FR-QUERY-008, 013 | `logs_live.rs` (`removed_or_deleted_logs_are_explicit_and_consistent_with_history`), `logs_fixtures.rs` (`the_reader_distinguishes_available_partial_missing_and_unreadable`): `expired` / `log_missing`, no invented result |
| AC-11 / FR-QUERY-010 | `logs_live.rs` (`a_running_task_gives_a_partial_snapshot_and_keeps_running`), `logs_fixtures.rs` (`a_running_log_is_a_fixed_snapshot_that_may_keep_growing`) |
| AC-12 / FR-QUERY-009 | `logs_live.rs` (`huge_logs_and_long_lines_stay_within_bounded_memory`), `logs_fixtures.rs` (`limits_truncate_and_count_what_was_omitted`, `a_huge_single_line_is_capped_without_cutting_a_character`) |
| FR-QUERY-011 | stream is always `combined` (`logs_fixtures.rs`, `logs_live.rs`) |
| AC-SPEC-51-01 | `logs_cli.rs` (`malformed_query_and_recall_invocations_exit_2`), `logs_fixtures.rs` (`invalid_specs_are_rejected`): two selectors → exit 2, no backend access |
| AC-SPEC-51-02 | `logs_fixtures.rs` (`range_is_one_based_and_beyond_the_end_is_empty`, `match_is_literal_case_sensitive_and_numbers_original_lines`): zero lines → empty `items`, exit 0 |
| AC-SPEC-51-03 | `logs_live.rs` (`a_running_task_gives_a_partial_snapshot_and_keeps_running`): `partial_log=true`, task continues |
| AC-SPEC-51-04 | `logs_live.rs` (`removed_or_deleted_logs_are_explicit_and_consistent_with_history`): `log_missing`, exit 11 |
| AC-SPEC-51-05 | `logs_fixtures.rs` (`scan_budget_stops_work_and_never_invents_tail_line_numbers`), `logs_live.rs` (`huge_logs_and_long_lines_stay_within_bounded_memory`): `scan_complete=false`, exit 11, ~9.5 MiB peak RSS |
| AC-SPEC-52-01 | `logs_fixtures.rs` (`recall_returns_small_logs_inline_and_refuses_to_pretend_for_large_ones`): complete text, exit 0 |
| AC-SPEC-52-02 | `logs_live.rs` (`invalid_encoding_and_inline_limits_have_distinct_results`): `output_too_large`, exit 11, no truncated text |
| AC-SPEC-52-03 | `logs_fixtures.rs` (`recall_output_never_overwrites_follows_links_or_leaves_partial_files`): `output_exists`, exit 15, nothing overwritten |
| AC-SPEC-52-04 | `logs_live.rs` (`removed_or_deleted_logs_are_explicit_and_consistent_with_history`): `log_missing`, no file published |
| AC-SPEC-52-05 | `logs_live.rs` (`invalid_encoding_and_inline_limits_have_distinct_results`), `logs_fixtures.rs` (`recall_output_streams_bytes_to_a_new_private_file`): non-UTF-8 saved byte-identically with `encoding=bytes` metadata |
| AC-SPEC-53-01 | `logs_cli.rs`: `logs` is an unknown subcommand, exit 2, nothing registered; `usage_documents_the_retrieve_commands` shows it is absent from help |
| AC-SPEC-53-02/03 | Documented mapping (`query --tail`, `recall`) in the retrieve guide; both paths covered by the rows above |
| AC-31 | Retention and loss are stated in the retrieve guide; saved `result` artifacts and raw Pueue logs have separate lifetimes (`removed_or_deleted_logs_are_explicit_and_consistent_with_history`) |

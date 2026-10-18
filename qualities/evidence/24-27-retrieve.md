# Issues #24–#27 implementation evidence

Date: 2026-10-18. Platform: Linux x86_64 (container, uid 0). Pueue 4.0.4 built from crates.io (`cargo install pueue --version =4.0.4 --locked`). The macOS run is still owed (see "Not verified").

## Spike: how to read a retained Pueue log (#24)

| Option | Finding | Decision |
| --- | --- | --- |
| `pueue log --json [--lines N] ID` | Returns the whole log as one JSON string; with `--lines` it returns only the tail, with no byte offsets. A big log is held in the Pueue client, the pipe and axvern. No way to stream or resume. | Kept only for the existing bounded `output_owned` probe. |
| Read `<pueue_directory>/task_logs/<id>.log` | Plain combined stdout+stderr file; confirmed by the live tests (tasks 0..n, file appears when the task starts, removed by `pueue clean`). Streamable, snapshot length is the file size at open. | **Chosen.** `pueue_directory` is read from `execution.pueue_config` (`shared.pueue_directory`, Pueue's default data directory when unset). |
| `bstr` | Not needed: non-UTF-8 logs are rejected with `invalid_encoding`, and `recall --output` copies raw bytes with `std::io`. | Not adopted. |
| `regex` / SQLite / full-text index | `--match` is a literal `str::contains`; `--errors` is a hand-written word scan. No index: the Pueue log is the only store. | Not adopted. |
| Own copy / index of raw logs | Not needed: `result` already keeps bounded structured previews; copying raw logs would add TTL, capacity, permission and secret-retention duties for no new capability. | Rejected. |

Constraint carried into the docs: the file layout is a Pueue 4.0.4 implementation detail. Another Pueue version must be re-verified (the adapter is pinned to 4.0.4 already).

Identity: the task is looked up (queue + task ID + creation time) before opening and again after; an ID reused or a task cleaned in between is `expired`. The file is opened with `lstat` + `open` + inode comparison, so a symlink or swapped file is refused. Completion of a finished log is re-verified (size and mtime unchanged, path still the same inode) after reading.

## Measurements (live, `logics/tests/tests/logs_live.rs`)

Log: 94,877,522 bytes (1.2 M ordinary lines, one 8 MiB line, final `error:` line). Peak RSS is the kernel `VmHWM` of the CLI process.

| Call | Peak RSS | Result |
| --- | --- | --- |
| `query --match "line 1199999"` | 9.5 MiB | `scan_complete=false`, exit 11, `omitted_count=null` (64 MiB scan budget) |
| `query --errors` | 9.5 MiB | same |
| `query --tail 3` | 9.5 MiB | same, no items, no invented line numbers |
| `query --from-line 10 --lines 5` | 9.5 MiB | exit 0 (range ends inside the budget) |
| `recall --output FILE` | 9.5 MiB | exit 0, byte-identical, mode 0600 |

Other states exercised against the real daemon: running (partial snapshot while the task continues, then complete), `pueue clean` (`expired`, matching `history`), log file deleted (`log_missing`), non-UTF-8/NUL (`invalid_encoding`; raw bytes saved with `--output`), 200 KB log over the inline limit (`output_too_large`), 300 KB single line (cut at 64 KiB, `line_truncated`), existing output (`output_exists`, 15). The access-denied case (`permission_denied`, 4) is skipped when the tests run as root (mode bits are ignored); the fixture test has the same guard.

## Re-run guarantee (AC-08 to AC-12, AC-31)

`query_and_recall_read_the_log_without_running_the_command_again` registers a Python program that appends one line to a counter file, then runs 3 × (match, errors, tail, range, empty match, recall) and a `recall --output`. The counter stays at one line and `history` lists one execution.

## Test map

- `logs_fixtures.rs` (18 tests, no daemon): selector semantics, CRLF/Unicode/no final newline, every `--errors` boundary, limits and omitted counts, giant single line, non-UTF-8/NUL, scan budget (bytes and time), spec validation, Reader states (available/partial/queued/missing/unreadable/symlink), identity failures, growing snapshot, change detection, recall inline/large/escape-inflated/invalid UTF-8, atomic file publication, no overwrite (existing, dangling symlink, race), no leftovers on size limit / shrink / deadline, Pueue config parsing.
- `logs_cli.rs`: 36 malformed invocations exit 2 before any backend access; `logs` is an unknown subcommand.
- `logs_live.rs` (6 tests): real daemon, see above.
- `sha256` unit tests: FIPS 180-4 vectors.

## Not verified here

- macOS (the pinned reference platform of the other evidence files): `VmHWM` is Linux-only, so the memory table needs `/usr/bin/time -l` there. The `task_logs` layout should be confirmed against the macOS Pueue 4.0.4 data directory.
- The Mojo CLI contract cases were not extended or run (Mojo is not installed).
- Parent (analysis) IDs: the `invalid_target` response is implemented; it is not covered by an automated test because registering a parent needs fd and tokei.

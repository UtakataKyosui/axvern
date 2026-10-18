# Retrieve: query and recall

`query` and `recall` read the log that Pueue still holds for an execution. They never run the original command again, never register a task and never start the daemon. Reading the same log repeatedly, with different conditions, leaves the target's side effects unchanged.

```text
axvern query EXECUTION_ID (--match TEXT | --errors | --tail N | --from-line N --lines M) [--limit 100] [--max-bytes 65536]
axvern recall EXECUTION_ID [--output FILE]
```

There is no separate `logs` command: use `query --tail N` for recent lines and `recall` for the whole log. `result` (see the [CLI contract](/cli)) is independent: it returns the structured result axvern saved, which can outlive the Pueue log.

## Where the log comes from

Pueue 4.0.4 writes one combined stream per task to `<pueue_directory>/task_logs/<task id>.log`; `pueue_directory` is read from the Pueue configuration named by `execution.pueue_config` (Pueue's default data directory when unset). `pueue log --json` returns the whole log as one JSON string, so a large log would always be held in memory twice; axvern therefore streams the file. Before and after opening, the task is looked up again with its stored queue and creation identity, so a recycled Pueue ID can never expose another execution's output. A symbolic link in place of the log is not followed.

- **stdout and stderr cannot be separated.** The stream is always reported as `combined`.
- **Encoding.** Text must be valid UTF-8 without NUL. Otherwise `invalid_encoding` (exit 2) is returned and nothing is replaced with U+FFFD. `recall --output` saves the raw bytes.
- **Snapshot.** The size at open time is fixed. A running task is never followed; text it appends later is not read.

## Availability

| Situation | Result | Exit |
| --- | --- | --- |
| Finished, log present | complete text, `partial_log=false` | 0 |
| Running (or paused) | snapshot, `partial_log=true`, `complete=false`; the task continues | 0 |
| Queued, no log yet | empty partial snapshot | 0 |
| Task removed (`pueue clean`/`reset`) or ID reused | `expired`, as `history` reports it | 8 |
| Task listed but log file gone | `log_missing` | 11 |
| Log not readable | `permission_denied` | 4 |
| Log shrank or changed while read | `log_changed` | 11 |
| Unknown execution ID | `task_not_found` | 7 |
| A parent analysis ID | `invalid_target`, listing its children (a parent keeps no log of its own) | 2 |

A lost log is never reconstructed by running the command again.

## query

Exactly one selector is required; `--from-line` and `--lines` go together. Lines are numbered from 1 in the original log; `\n` and `\r\n` both end a line and a final line without a terminator is counted.

- `--match TEXT` — non-empty (up to 1024 bytes), case-sensitive, **literal** substring. It is never a regular expression.
- `--errors` — lines containing an independent word `error`, `failed`, `fatal` or `panic`, ASCII case-insensitive (`errors`, `terror`, `error_code` do not match). This is rule version 1, a heuristic list of candidates; it does not say whether the program succeeded.
- `--tail N` (1–1000) — the last N lines. Exact numbers need the whole log, so if the scan budget runs out no line numbers are invented: the call returns `partial_result` (11) with no items.
- `--from-line N --lines M` — M lines (1–1000) starting at N. Past the end the list is empty.

`--limit` (default 100, 1–1000) caps the number of items and `--max-bytes` (default 65536, 1024–65536) caps the text bytes. A longer line is cut at `--max-bytes` on a character boundary and marked `line_truncated`. Match, errors and range keep original order and drop the later lines; tail keeps the newest lines. `truncated` and `omitted_count` describe what was left out; `omitted_count` is `null` when it could not be counted.

The scan itself is bounded to 64 MiB and 10 seconds. When it stops early, `scan_complete=false`, the items found so far are returned with `partial_result` and exit 11.

```json
{"schema_version":2,"command":"query","execution_id":"ex_…","state":"done","exit_code":0,"output":{"kind":"query","source_execution_id":"ex_…","selector":{"match":"error"},"observed_at":"2026-10-15T00:00:00Z","stream":"combined","log_availability":"available","partial_log":false,"log_location":"…/task_logs/3.log","snapshot_bytes":1234,"encoding":"utf-8","limit":100,"max_bytes":65536,"items":[{"line":12,"text":"error: failed"}],"truncated":false,"omitted_count":0,"scan_complete":true,"scanned_bytes":1234},"error":null}
```

## recall

Without `--output`, a complete log is returned inline as `text` only if the whole JSON response fits 64 KiB. Anything larger is `output_too_large` (11); the text is never silently cut. Human output escapes control characters.

`--output FILE` streams the exact bytes into a new file (`encoding=bytes`, binary allowed) and prints only metadata: path, `bytes` and `sha256`. The path must not exist (including a dangling symbolic link) and its parent directory must exist; otherwise `output_exists` (15) or `invalid_target` (2), and nothing is overwritten. The file is written privately (0600) next to its destination and published atomically; a failure, timeout or change of the log leaves no final file. The size limit defaults to 1 GiB and the deadline to 5 minutes (see [configuration](/configuration)); exceeding either is an error and the task is unaffected. The `recall` limits are separate from `query`'s.

## Retention, deletion and what is not guaranteed

axvern keeps no copy of raw logs. Their lifetime is Pueue's: they exist until `pueue clean`, `pueue reset`, `pueue remove` or a manual deletion of the task log. `results.retention_seconds` applies only to the structured results axvern saves, not to the Pueue log, and neither side deletes the other. History keeps the record of a removed task and reports it as `expired`, which is also what `query` and `recall` report.

A complete log is **not** guaranteed when: the task is still running; Pueue or the user removed the log; the process that wrote it was killed before flushing; or the program printed secrets, which are then in the log in clear text. Treat Pueue's data directory as sensitive (it is owned by the user running the daemon) and delete a log with `pueue clean` or `pueue remove ID`. axvern does not redact content and never quotes log text in error messages.

## Measured behaviour

On a 95 MB log with a single 8 MiB line, the CLI's peak resident memory was about 9.5 MiB for `--match`, `--errors`, `--tail`, a line range, and `recall --output`, because only 64 KiB read buffers, one capped line and at most 1000 selected lines are held. Searches beyond the 64 MiB scan budget end with `scan_complete=false` and exit 11. See the evidence file `qualities/evidence/24-27-retrieve.md`.

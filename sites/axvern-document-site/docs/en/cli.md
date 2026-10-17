# CLI contract (schema 2)

The default output is JSON. This is an explicit change from schema 1's human default; scripts must check `schema_version=2`. Use `--format human` for readable output. Configuration and metadata files keep their independent schema version 1. Global `--config PATH` and `--format json|human` can precede or follow a command, before target `--`. Unknown/duplicate options are errors. No pager or color is used. Human data escapes control characters.

```text
axvern run [--cwd PATH] [--wait 1s] [--request-id KEY] -- PROGRAM [ARGS...]
axvern status EXECUTION_ID
axvern wait EXECUTION_ID [--timeout 30s]
axvern result EXECUTION_ID [--cursor TOKEN] [--limit 100] [--max-bytes 65536]
axvern history [--limit 20]
axvern doctor [--scope core|discovery|logs|performance|all]
axvern policy check [--cwd PATH] -- PROGRAM [ARGS...]
```

See [v0.2 discovery](/discovery) for `fs list/find/search/read/analyze`, `code search/stats`, backend versions and scope limitations.

## Registration and waiting

Target argv after `--` preserves empty strings, spaces, newlines, quotes, Unicode and flag-like arguments. Limits remain 256 arguments, 4096 program bytes, 65536 bytes per argument; UTF-8 without NUL is required. cwd resolves to an existing absolute directory. Policy evaluates the original program/argv and any opt-in rewritten plan before the internal result runner is registered with Pueue; the stricter decision wins. Ask requires both stdin and stderr terminals and explicit `y`, `Y` or `yes`; the prompt is on stderr. Deny and noninteractive Ask never register a task.

`run --wait` defaults to 1s, accepts integer `ms`, `s`, `m`, from zero through 24h. The budget starts after registration. Zero skips observation and leaves state null. At the deadline, return the ID and last observed state; the task continues. A successfully registered/responded-to run exits 0 even if its target fails. A result-store failure is separate from target failure.

`--request-id` accepts 1–128 ASCII alphanumeric characters plus `. _ : -`. Within one store, the same key plus the same program, argv and resolved cwd returns the existing execution ID. Wait/format, inherited environment, executable contents and later Policy changes do not change that identity. An existing reservation does not request new approval. Use a new key for an intentional rerun. Different input is `request_conflict` (15). Reservations are fsynced before `add`, serialized across clients, and never automatically deleted. An ambiguous registration returns `registration_unknown` (14) and its execution ID/queue label; a retry never calls `add` again. Reconcile manually with the dedicated Pueue queue. This is conservative at-most-once submission, not a distributed exactly-once guarantee.

`wait --timeout` defaults to 30s, accepts integer `ms`, `s`, `m`, 1ms–24h. Zero, negative, fractional, bare and overflowing values are rejected before lookup. Polling is 250ms plus backend latency; each request is capped at 5s or the remaining budget. Timeout returns 124 with the latest observed state, if any. Target failure returns 10 and preserves its numeric exit code in JSON. Ctrl-C interrupts only the client (Unix signal semantics, usually 130); a complete JSON line is not promised after a signal.

## JSON, status and history

JSON stdout is one UTF-8 object plus LF. Diagnostics are in `error`/result data; only approval prompts use stderr. Every response has `schema_version`, `command`, `execution_id`, `state`, `exit_code`, `output`, `error`. Unavailable values are null. Target exit code is distinct from CLI exit status.

```json
{"schema_version":2,"command":"run","execution_id":"ex_550e8400-e29b-41d4-a716-446655440000","state":"running","exit_code":null,"output":{"result_availability":"pending","result":null},"error":null}
```

IDs are `ex_` plus lowercase UUIDv4; integer Pueue IDs are not accepted. `status` verifies queue namespace, task ID and creation identity. Deleted/reused mappings return `expired` (8). Queued/Stashed/Locked become `queued`; Running/Paused become `running`; successful completion is `done`; Failed/Killed/Errored/DependencyFailed/FailedToSpawn become `failed`. Unknown backend states fail closed. Missing numerical child exits and unobserved times remain null. A successfully inspected failed target gives CLI exit 0. `status`/`wait.output` includes `kind`, `record`, `task`, `source=backend`, `observed_at` (Unix seconds) and `result_availability`; it does not include the result body. Persisted logical parents (created through the core API) aggregate their concrete children: all queued → queued; any unfinished → running; all successful → done; mixed success/failure → partial; no successes → failed. Parent wait returns 11 for partial, 10 for failed, without inventing a numeric aggregate exit code. Status/history return child IDs and observations. `fs analyze` creates discovery parents and publishes their aggregate artifact lazily; see the [analysis contract](/discovery).

`history` accepts 1–1000 rows (default 20), ordered by registration second descending, then ID descending. It returns local records only, without argv, environment or logs. Each row includes its record/cwd, task/error, command and result availability. Removed backend tasks remain as expired rows. Offline nonempty history returns metadata/per-row errors and `partial_result` (11); an empty store succeeds offline. Corrupt records fail rather than disappearing. Result expiry does not delete history or request reservations.

## Saved run results

The internal runner captures a bounded combined stdout/stderr preview and publishes an immutable result before completing its Pueue task. It also forwards the combined log to Pueue. Keep the installed axvern executable available at the same path until queued tasks finish. `result` reads saved bytes without executing the target or consuming/refreshing the result; it works after Pueue cleans the task/log. Legacy registrations have no captured result.

An immediately completed run and later `result` use the same result structure: `kind=run`, `summary` (target exit, stream, completion second), `items`, `completeness`, `errors`, `warnings`, `page`. Text fragments carry `item_id`, sequential `fragment_index`, and `continued`. Unicode characters are never split across returned fragments; invalid UTF-8 is reported explicitly. stdout/stderr are labelled `combined`, not falsely separated.

A page returns at most 100 fragments (default 100, range 1–100) and 64KiB including its JSON envelope and LF (`--max-bytes` range 1024–65536). Escaped JSON size is counted. Cursors belong to one immutable execution artifact. Replay returns the same page while retained. `has_more`/`next_cursor` describe saved pages; `completeness.truncated` separately describes the preview's capture limit. An undersized response is `output_limit_too_small` (11).

Availability is `pending`, `available`, `expired`, `evicted` or `unavailable`. Pending results are null. Expiry and capacity eviction return distinct typed errors (11), never rerun the target, and retain tombstones. A successfully fetched result exits 0 even when the target exited 23. A killed/crashed runner can leave an active reservation; automatic repair/reexecution is deliberately unavailable.

## Doctor

Default scope `core` checks configuration/Policy, private store, Pueue 4.0.4, daemon and a bounded owned-log probe. `logs` includes the same log checks. `discovery` also probes fd/rg/bat/ast-grep/tokei and reports eza as optional; `performance` probes hyperfine; `all` probes both and reports optional integrations unavailable. git/delta/dust/procs and performance integrations are reported as unprovided; the implemented discovery adapters report supported version ranges and required features. A successful version probe does not certify every later release. Required missing/failed probes are Fail, optional integrations/project tools are Warning. Fail dominates Warning dominates Pass; Fail exits 12, others 0. Invalid config is a check failure, with further feasible checks continued.

Each external probe has a 5s limit, capped by the overall 30s probe deadline. Devbox/Moon/Cargo/pnpm are checked only for corresponding project markers. Store scratch tests read/write/fsync and clean up. Private directories/lock files may be created; existing task/record data is not changed. Log contents are not returned. Each failure/warning includes a remedy. Doctor never installs tools, starts daemons, uses sudo or repairs data.

## CLI exit codes

| Exit | Meaning |
| --- | --- |
| 0 | Successful registration/inspection/result retrieval; target may have failed |
| 1 | Backend/protocol failure |
| 2 | Invalid input, encoding or cursor |
| 3 | Invalid/missing selected configuration |
| 4 | Permissions unsafe/denied |
| 5 | Required executable missing |
| 6 | Daemon unavailable |
| 7 | Unknown local execution ID |
| 8 | Backend mapping expired/reused/foreign |
| 9 | Policy/approval refused |
| 10 | Explicit wait observed target failure |
| 11 | Partial history, missing/expired/evicted result, output limit too small |
| 12 | Doctor Fail |
| 13 | Store I/O/schema/capacity failure |
| 14 | Registration outcome unknown; never blindly resubmit |
| 15 | Request ID conflict |
| 16 | Unsupported capability |
| 124 | Explicit wait/backend timeout; task is not stopped |

The error code is stable; its message is display text and does not quote secret argv/config values. Registration persistence errors retain a receipt where available. Preserve it privately and repair metadata without registering again.

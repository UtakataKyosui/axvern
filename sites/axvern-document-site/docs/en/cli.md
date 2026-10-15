# CLI contract (schema 1)

Global options `--config PATH` and `--format human|json` can precede or follow the subcommand, before the target `--`. Default format: human. Duplicate or unknown options are errors. No pager or ANSI color is emitted. `--help` and `--version` are standalone informational flags.

```text
axvern run [--cwd PATH] -- <program> [args...]
axvern status <execution-id>
axvern wait <execution-id> [--timeout 30s]
axvern history [--limit 20]
axvern doctor
axvern policy check -- <program> [args...]
```

`run` requires `--`; everything after it is native target argv, including empty strings and axvern-looking options. cwd defaults to the invoking directory; a supplied path is resolved to an existing absolute directory. UTF-8 without NUL is required at the backend/JSON boundary. Limits: 256 arguments, 4096 program bytes, 65536 bytes per argument; OS command-size limits still apply.

Ask prompts on stderr only when both stdin and stderr are terminals. Only `y`, `Y`, or `yes` approves; other answers/EOF refuse. JSON mode does not grant approval. Deny always refuses. `policy check` reports a decision without registering anything and exits 0 even for Deny.

`wait` defaults to 30 seconds. Accepted units: integer `ms`, `s`, `m`; range 1ms–24h inclusive. Zero, negative, fractional, bare numbers, and overflow are errors. Polling is at most once per 20ms plus backend latency. Each backend request has a 5s cap, shortened to the remaining wait budget. Timeout and Ctrl-C interrupt the client only; they do not stop the daemon task. Ctrl-C follows normal Unix signal termination; no JSON response is promised after a signal.

`history` defaults to 20, accepts 1–1000, and rejects zero. Order: registered UNIX second descending, then execution ID descending for ties. It contains only local axvern records, with current backend snapshots where available. Removed/reused task IDs have `error.code=expired`; daemon failure preserves metadata with per-entry errors and a top-level `partial_result` (exit 11). An empty store returns an empty array, even offline. No automatic TTL or capacity eviction occurs.

## Output and exit status

JSON stdout is exactly one response plus newline; diagnostics are fields, without stderr logging. Human results go to stdout and errors to stderr. Doctor's check explanations/remedies are result data on stdout. Child stdout/stderr are never forwarded by these commands. Common fields are always present; unavailable values are `null`:

```json
{"schema_version":1,"execution_id":"ex_550e8400-e29b-41d4-a716-446655440000","state":null,"exit_code":null,"output":null,"error":{"code":"task_not_found","message":"unknown axvern execution"}}
```

`run.output.record` contains `execution_id`, `backend` (`queue`, `task_id`, `created_at`), `registered_at` (UNIX seconds), and `cwd`. Registration leaves top-level state/exit_code null because state may change immediately. `status`/`wait.output` contains `record`, `task`, `error`; task has identity, state, child exit_code, started_at, finished_at. `history.output.executions` is an array of these views.

States: Queued/Stashed/Locked → `queued`; Running/Paused → `running`; Done Success → `done`, exit 0; Failed/Killed/Errored/DependencyFailed/FailedToSpawn → `failed`. Child exit code is null before completion and when no numeric exit exists. Start/end timestamps retain Pueue's timestamp strings; absent timestamps are null. `partial` is reserved for future aggregate results, not a child state. v0.2 parent analysis IDs/results are outside this v0.1 contract.

| CLI exit | Meaning |
| --- | --- |
| 0 | Registered, inspected, completed successfully, history available (including expired rows), doctor Pass/Warning, or policy evaluated |
| 1 | Backend/protocol/unsupported Pueue version failure |
| 2 | Invalid input/encoding |
| 3 | Invalid/missing selected configuration |
| 4 | Permissions unsafe/denied |
| 5 | Pueue executable missing |
| 6 | Daemon state unavailable |
| 7 | Unknown local execution ID |
| 8 | Known mapping expired (deleted/reused/other queue) |
| 9 | Policy/approval refused |
| 10 | `wait` observed child failure; numeric child exit remains in JSON |
| 11 | Partial history/output unavailable |
| 12 | Doctor has one or more Fail checks |
| 13 | Store I/O/schema failure |
| 14 | Registration outcome unknown; never blindly retry |
| 124 | Wait/backend timeout; task not stopped |

`status` exits 0 for a successfully inspected failed child; `wait` exits 10. A child exit of 23 is `exit_code:23`, not CLI exit 23. Error messages are diagnostic text, not a stable enum; error codes and schema fields are the machine contract.

If registration succeeds but persistence fails, the error response retains `output.registration_receipt` and the execution ID. Preserve it privately; repair the store and save that receipt using the core store API. No public automatic recovery command exists yet. On `registration_unknown`, use the nonce label in the error with Pueue to reconcile manually. Retrying `run` is a fresh registration, not an idempotency mechanism.

## Doctor

`doctor.output` contains `level` and `checks`; each check has `name`, `level`, `message`, `remedy` (nullable). Levels are `pass`, `warning`, `fail`; Fail dominates Warning dominates Pass. A missing CLI, wrong Pueue version, invalid config/policy, unsafe store, or unavailable daemon is Fail. Unverifiable owned logs and unavailable project-only tools are Warning. Runtime dependency: Pueue 4.0.4; future phase tools are not required.

Daemon calls have a 5s timeout each. Log probing uses at most one owned task and never returns its content. Store diagnosis validates records and read/write/fsync through a dedicated scratch file, then removes it. It may create private store directories and a lock file, but never changes execution records, pending writer files, or Pueue tasks. Devbox/Moon/Cargo/pnpm are probed only when the current directory contains their corresponding `devbox.json`/`.moon`/`Cargo.toml`/`package.json` marker. Additional diagnostics can extend the structured check list.

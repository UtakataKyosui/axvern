# Configuration and policy

Selection: CLI `--config` > `AXVERN_CONFIG` > the OS user configuration path for `axvern/config.toml` (via `directories::ProjectDirs`). No project config discovery or merging occurs. Only an absent implicit file selects defaults. Empty environment variables, explicit missing files, unknown keys, malformed TOML, and unsupported schema fail closed. Selected files must be regular, UTF-8, at most 1MiB, mode 0600 or 0400, without symlink ancestors or unsafe writable ancestors. Parser errors do not echo configuration values.

```toml
schema_version = 1
[execution]
pueue_config = "/absolute/path/to/pueue.yml"
# Optional: default is the OS user local data directory / executions.
store = "/absolute/path/to/private/executions"

[policy]
default = "ask"

[[policy.rules]]
id = "cargo-test"
effect = "allow"
command = "cargo"
args_prefix = ["test"]
```

Rules match the program string exactly and an argv prefix, with Deny > Ask > Allow. No match means Ask; `policy.default` must be Ask. An Allow rule permits execution; review interpreters and wrapper programs as carefully as shells. Policy is not an operating-system sandbox. Shell commands are conservatively denied unless a matching rule explicitly sets `allow_shell=true` and a nonempty `args_prefix`. A generic Allow does not permit shell evaluation. Replacement remains disabled and is not implemented.

`execution.pueue_config` is required for backend commands, and must be an absolute existing file. No implicit Pueue project config is used. This trusted file must configure the POSIX shell described in [setup](/getting-started). Its canonical path names the queue namespace; changing its daemon/socket target in place is unsupported. Use a new path/store when changing queues.

Store directories are created with 0700 and files with 0600. Symlink/unsafe ancestors are refused. Metadata, durable request reservations and result manifests use independent schema-1 documents and OS locks/atomic publication/fsync. Old metadata remains readable; no past logs are captured retroactively. JSON response schema 2 does not change the config schema.

```toml
[results]
retention_seconds = 604800
capacity_bytes = 1073741824
max_artifact_bytes = 67108864

[discovery]
max_items = 10000
max_entries = 100000
timeout_ms = 60000
```

Result retention accepts 1h–365 days; capacity 1MiB–1TiB; artifact limit 1KiB–1GiB and no greater than capacity. Set both capacity and artifact limit when lowering capacity. Completion time starts retention; reads never extend it. Expired results are removed first, then oldest completed results (completion second/ID order) to obtain capacity. Active spool reservations count toward capacity and are not evicted. Preview capture stops at its reserved byte budget and records truncation while the target continues. Metadata and tombstones count toward used bytes and are not automatically removed; filesystem allocation overhead is not a quota guarantee. Storage exhaustion refuses new work when detected before registration and otherwise records a result failure separately from target failure.

Discovery limits are validated/stored for future integrations: max_items 1–100000, max_entries at least max_items and at most 1000000, timeout_ms 1000–600000. They do not enable discovery commands or limit arbitrary run durations.

History, request IDs and tombstones have no automatic TTL. The legacy `prune_before` metadata API refuses to run once request reservations exist; no deletion CLI is exposed. Stop clients/workers and back up the private store before manual maintenance. Do not remove request records to recover space and then resend old keys: that would defeat duplicate prevention. Active manifests left by killed runners require explicit manual investigation; they are never silently evicted/reexecuted. Pueue's own clean/reset/retention is independent of captured run results.

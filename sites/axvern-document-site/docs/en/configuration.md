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

Store directories are created with 0700 and files with 0600. Symlink/unsafe ancestors are refused. Records are atomic JSON documents (`schema_version=1`) guarded by an OS lock. Unknown store schemas fail without overwriting them. There is no automatic migration or retention policy yet; stop axvern clients and back up the private store before any manual maintenance. The core API `prune_before(UNIX_SECONDS)` removes metadata only, not daemon tasks or logs; CLI cleanup is not yet exposed. Removing metadata loses execution-ID lookup. Pueue's own `clean`/`reset`/retention can remove logs independently.

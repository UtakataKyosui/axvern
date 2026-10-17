# Build, install, and execute

Targets: Linux and macOS (Unix filesystems). Rust 1.98.0 is pinned in `rust-toolchain.toml`; Pueue 4.0.4 is the only supported backend version. Windows is not supported. See [release evidence](/release) for the tested platforms.

From the repository root:

```sh
cargo build --release --locked --bin axvern
cargo install --locked --path tools/cli
axvern --version
cargo install --locked pueue --version 4.0.4
pueue --version
```

The executable is named `axvern`, although its Cargo package is `axvern-cli`. The install smoke gate uses an isolated install root and does not overwrite your installed executable.

Create/use your own Pueue configuration, following the [Pueue documentation](https://github.com/Nukesor/pueue/wiki). Set its `daemon.shell_command` to `["sh", "-c", "{{ pueue_command_string }}"]`; axvern's argv escaping is tested with this POSIX shell. Start your daemon explicitly:

```sh
pueued --config /absolute/path/to/pueue.yml
```

axvern never starts, stops, resets, or cleans your daemon. Use a dedicated Pueue configuration when isolating workloads. Keep its daemon/socket identity stable; use a new configuration path and store when switching daemons.

Create a private axvern config (replace both paths):

```toml
schema_version = 1
[execution]
pueue_config = "/absolute/path/to/pueue.yml"
store = "/absolute/path/to/axvern-executions"

[[policy.rules]]
id = "allow-sleep"
command = "sleep"
effect = "allow"
```

```sh
chmod 600 /absolute/path/to/axvern.toml
axvern --config /absolute/path/to/axvern.toml doctor
axvern --config /absolute/path/to/axvern.toml run -- sleep 5
# Use the returned ex_… ID:
axvern --config /absolute/path/to/axvern.toml status <execution-id>
axvern --config /absolute/path/to/axvern.toml wait <execution-id> --timeout 30s
axvern --config /absolute/path/to/axvern.toml result <execution-id>
axvern --config /absolute/path/to/axvern.toml history --limit 20
```

The default output is JSON schema 2; use `--format human` for readable text. `run` waits up to 1s after registration, returning an ID and either saved results or the current state. Use `--wait 0s` for immediate return, and `--request-id KEY` for retryable submission. Registration success does not mean the child succeeded. To inspect output, use the returned Pueue ID and the same configuration:

```sh
pueue --config /absolute/path/to/pueue.yml log <pueue-task-id>
```

Commands without an Allow rule default to Ask. With redirected input, they are refused without registration (exit 9):

```sh
axvern --config /absolute/path/to/axvern.toml run -- true </dev/null
```

If `doctor` reports a daemon failure, start `pueued --config` with that same file and check socket permissions. `doctor` displays a remedy; it never installs or repairs tools automatically.

## Install the pinned discovery tools

For the full v0.2 acceptance environment, install these in addition to Pueue:

```sh
cargo install --locked fd-find --version 10.5.0
cargo install --locked ripgrep --version 15.2.0
cargo install --locked bat --version 0.26.1
cargo install --locked tokei --version 15.0.0
npm install --global @ast-grep/cli@0.51.0
python3 qualities/scripts/discovery_tools.py
```

The tokei version probe must list JSON serialization support. eza is required only if you enable `ls-to-eza`, not for structured filesystem discovery. Add the [backend Policy rules](/discovery) before noninteractive use, then run:

```sh
axvern --config /absolute/path/to/axvern.toml doctor --scope discovery
axvern --config /absolute/path/to/axvern.toml fs list . --depth 2
axvern --config /absolute/path/to/axvern.toml code stats .
axvern --config /absolute/path/to/axvern.toml fs analyze . --wait 0s
# Use the returned parent ID; retrieval never re-registers its children:
axvern --config /absolute/path/to/axvern.toml result <execution-id>
```

The source checkout remains pre-release; these installation commands do not imply a registry publication of axvern.

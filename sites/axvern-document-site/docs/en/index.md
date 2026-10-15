# axvern

axvern is a Rust CLI for running commands asynchronously through Pueue with an explicit execution policy and private execution IDs.

The current source implements `run`, `status`, `wait`, `result`, `history`, `doctor`, and `policy check`. This is pre-release software; no binary distribution or hosted site has been published by this work.

- [Install and run a command](/getting-started)
- [CLI and JSON contract](/cli)
- [Configuration and policy](/configuration)
- [Architecture and limits](/architecture)
- [Release gates and evidence](/release)

Filesystem/code inspection is planned for v0.2. `query` and `recall` are planned for v0.3 and are unavailable in this version. Saved run previews are available through `result`; full retained logs use Pueue directly.

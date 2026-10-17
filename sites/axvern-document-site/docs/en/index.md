# axvern

axvern is a Rust CLI for running commands asynchronously through Pueue with an explicit execution policy and private execution IDs.

The current source implements `run`, `status`, `wait`, `result`, `history`, `doctor`, `policy check`, `fs list/find/search/read/analyze`, `code search/stats`, and limited opt-in command rewriting. This is pre-release software; no binary distribution or hosted site has been published by this work.

- [Install and run a command](/getting-started)
- [CLI and JSON contract](/cli)
- [Discover a codebase](/discovery)
- [Configuration and policy](/configuration)
- [Architecture and limits](/architecture)
- [Release gates and evidence](/release)

Filesystem/code inspection is implemented; see the tool versions and limits in the discovery guide. `query` and `recall` are planned for v0.3 and are unavailable in this version. Saved run previews are available through `result`; full retained logs use Pueue directly.

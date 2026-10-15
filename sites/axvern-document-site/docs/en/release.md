# v0.1 release gates

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

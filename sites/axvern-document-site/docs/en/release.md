# Release gates and evidence

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

## Schema 2 additions (#34–#40)

The current source adds JSON-default schema 2, bounded run waiting, durable request IDs, saved run previews/result pagination, logical-parent state aggregation, policy cwd/matching-rule reporting and scoped doctor checks. Local evidence is recorded in `qualities/evidence/34-40-macos.md`: Rust contract/integration tests, Mojo validator/process probes, strict Clippy/fmt, site build/links and isolated install smoke pass on macOS. Linux evidence remains pending CI. That historical core-only validation did not cover discovery producers or aggregate artifacts; the v0.2 gate below adds their acceptance environment. Remote Issues are not automatically closed by these checks.

## v0.2 Discover acceptance (#2 / #22)

The discovery commands and limited replacement catalog are implemented. This PR adds the final public usage/limitations documentation and a required live-test environment; it does not declare Epic #2 complete or publish a release. The scope and known limitations are in the [discovery guide](/discovery).

CI installs fd 10.5.0, ripgrep 15.2.0, bat 0.26.1, ast-grep 0.51.0 and tokei 15.0.0, in addition to Pueue 4.0.4. `AXVERN_REQUIRE_DISCOVERY_TOOLS=1` makes a missing dependency fail instead of returning early from discovery live tests. The preflight verifies pinned versions and tokei JSON serialization and writes `target/discovery-tools.json` (uploaded with the matrix artifacts). Optional eza/git/delta/dust/procs are not installed as gate dependencies. Permission checks still explain skips when a privileged user can read mode-000 directories; CI should run as an unprivileged user.

After installing tools and the pinned Mojo compiler, run the full gate locally:

```sh
python3 qualities/scripts/discovery_tools.py
AXVERN_REQUIRE_DISCOVERY_TOOLS=1 devbox run test
```

Local tests without the strict environment may skip absent tools; that is useful during development but is not full v0.2 acceptance evidence. A green fixture-only run does not certify real CLI compatibility. New Linux/macOS live results must be read from the PR's completed CI jobs; this document does not assert success before they run.

| Requirement / boundary | Executable coverage in `logics/tests/tests` |
| --- | --- |
| AC-16–19, AC-25/26 | `rewrite.rs`, `cli.rs`: default-off, catalog forms, original/transformed Policy, missing-tool fallback, no automatic writes/repeaters |
| AC-20/21 | `discovery_fixtures.rs`, `discovery_live.rs`: argv, sorted entries, hidden/ignore behavior, regex/glob/case, root scope; fd replaces eza for structured lists |
| AC-22 | `discovery_cli.rs`, `discovery_fixtures.rs`, `code_fixtures.rs`: missing/unsupported tools, JSON capability, refusal before registration |
| AC-23 | `discovery_live.rs`: ranges/EOF, BOM/CRLF, binary/invalid encoding and escaped controls |
| AC-24 | `code_fixtures.rs`, `code_live.rs`: supported languages, malformed patterns, byte positions and best-effort parse errors |
| AC-SPEC-45-01–05 | `analyze_fixtures.rs`, `analyze_live.rs`, `discovery_cli.rs`: sections/file input, partial child failure, Policy-before-registration, saved paging and no rerun |
| Bounds, symlinks, permissions | discovery/code/analyze fixture and live suites; oversized header regression, lost child and registration interruption cases |
| Core schema/CLI | Mojo evaluator in `qualities/cli_contract`, existing core Rust/isolated Pueue suites |

The original AC-27–30/32 definitions remain absent from the Issue acceptance text. They are not invented or marked passed here. AC-31 is excluded from the v0.2 gate; saved `result` artifacts do not imply future query/recall guarantees. Requirement Issues still name eza for structured listings and stronger scope/depth behavior than some tools can provide; the fd decision and documented tool limitations need to be reconciled in those Issues before declaring the Epic complete. Text/code search and statistics now share the depth-32 and `.gitignore` contract, verified by `scope_live.rs` on Linux/macOS. The native ast-grep syntax-error detector remains best-effort, and tokei still cannot report a reliable count of skipped unknown/binary files.

Historical macOS measurements are in `qualities/evidence/18-macos.md`, `19-macos.md`, `20-macos.md`, and the optional-adapter spike `23-macos.md`. They describe the environment measured at that time; the new strict CI transcript is the evidence for this revision. Schema-1 additive fields may be rejected by older binaries, so downgrade compatibility is not promised. Release numbering, distribution and publication remain separate decisions.

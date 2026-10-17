# Issue #17: modern CLI compatibility and adapter contract

The compatibility boundary is `adapters::tools::{ToolSpec, DiscoveryAdapter,
Normalizer, TargetExit}`. All discovery adapters return an `ExecutionPlan`
(program, individual argv, cwd), use the same Policy/Pueue worker path and store
bounded immutable results. They do not execute a shell or repeat an operation
when a saved result is read. `eza` is the display-only exception: opt-in
`ls-to-eza` uses the ordinary captured-run path and never parses names from its
rendered text. Structured `fs list` and analysis trees use fd instead.

## Versions and machine-readable output

| CLI | Minimum checked by doctor | Recorded/pinned version | Output contract |
| --- | --- | --- | --- |
| fd | 10.0.0 | 10.5.0 | NUL-delimited names, metadata in normalizer; invalid UTF-8 becomes a scoped error |
| rg | 14.0.0 | 15.2.0 | JSON Lines; exit 1 means no match, exit 2 is failure/partial scope |
| bat | 0.24.0 | 0.26.1 | Plain bytes with color/pager disabled; normalized numbered UTF-8 lines |
| ast-grep | 0.51.0 | 0.51.0 | Validated scan JSON; malformed records and parse-error findings stay explicit |
| tokei | 15.0.0 | 15.0.0 | Validated streaming JSON; version must advertise JSON serialization support |
| eza (optional) | 0.23.5 | 0.23.5 | Captured display text only; no filename round trip or structured-entry guarantee |

A minimum version is a compatibility policy, not evidence that every later
release was measured. Required discovery versions are pinned by the #22 CI gate.
Both doctor and discovery preflight consume the same `ToolSpec` and detector.
Missing executables fail with `missing_tool`; unsupported, malformed, nonzero or
unavailable/timed-out version probes fail before task registration with
`unsupported_tool_capability`. A supported tokei version without JSON support
also fails. Doctor marks missing/incompatible eza as a warning because no required
discovery command depends on it. The optional run-replacement catalog retains its
existing availability-based behavior; it is not a structured discovery adapter.

## Executable acceptance mapping

| Contract | Evidence |
| --- | --- |
| Missing/old/unrecognized/failed version probes; every required adapter refused before registration; doctor parity; tokei feature | `discovery_cli.rs`, `cli.rs`, `code_fixtures.rs`; bounded timeout regression in `discovery_cli.rs` |
| No shell interpolation, exact argv/cwd, Policy before registration, no registration on refusal | `discovery_fixtures.rs`, `code_fixtures.rs`, `discovery_cli.rs`, `rewrite.rs`, `pueue_live.rs` |
| fd regex/glob/case/newline/hidden/ignore, NUL and non-UTF-8 names | `discovery_fixtures.rs`, `discovery_live.rs`; fixture bytes cover filesystems that reject non-UTF-8 names |
| rg empty results vs failed targets, JSON/binary/context/encoding | `discovery_fixtures.rs`, `discovery_live.rs` |
| bat range/EOF/BOM/CRLF/binary/control bytes, ast-grep/tokei JSON validation | `discovery_fixtures.rs`, `discovery_live.rs`, `code_fixtures.rs`, `code_live.rs` |
| Time/item/byte/entry limits, symlinks, partial child failures, saved-result paging | discovery/code/analyze fixture and live suites, `contract_v2.rs` |
| Real noninteractive eza through Policy/Pueue, display-only saved text, no rerun | `tools_live.rs` (optional; install eza 0.23.5 to execute) |

The required live suites run on Linux/macOS in the strict #22 CI environment.
Their version preflight writes `target/discovery-tools.json`, and the full test
transcript is uploaded as `quality-<os>/quality.log`. The optional eza spike must
be reported separately; its absence must not be called a successful measurement.
CI installs pinned eza in a separate step after the required gate has completed
without it, then runs `tools_live.rs` with `AXVERN_REQUIRE_EZA_SPIKE=1`. This
dedicated step fails rather than skips if eza is absent.
Existing macOS measurements and tool limitations are recorded in
[18-macos.md](18-macos.md) and [19-macos.md](19-macos.md).

The original Issue's eza-backed structured-list assumption is superseded by the
measured escaping limitation: display bytes are not a safe path protocol. This
implements #17's display-text contract without making eza a required dependency.
No new analysis engine, editing operation, publication, or interactive TUI is
introduced. The documented tokei scope/depth and ast-grep parser limitations
remain separate Epic reconciliation work.

## Local verification (2026-10-17, Linux x86_64)

Rust 1.98.0 build, fmt, strict workspace/all-target Clippy, Python quality-plan
and dependency-preflight tests, YAML parsing and local documentation links pass.
The five fixture/CLI suites report 57 passing tests, including the 10-second
version-timeout refusal; the embedded optional fd live case skipped locally.
Local incremental compilation produced a zero-length object in this runtime;
disabling incremental compilation completed the build and checks.

eza 0.23.5 and Pueue 4.0.4 were built locally. This runtime rejects Unix-socket
creation (`Operation not permitted`), so its Pueue-backed eza spike cannot run.
The Linux/macOS CI jobs, including the separate mandatory display-spike step,
are the live acceptance evidence for this revision; local results do not claim
successful Pueue execution.

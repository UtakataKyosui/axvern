# Issue #14–#16 local acceptance evidence

Date: 2026-10-15 (JST). Platform: macOS / arm64. Base: `2e8ffa3`.
Branch: `feat/resolve-issues-14-16`. This describes the uncommitted implementation on that branch; it is not evidence of a published release.

Executed `devbox run test` successfully: 11 Moon tasks completed, exit 0. Rust 1.98.0, Pueue 4.0.4. Rust integration tests: 27 passed (backend_store 8, cli 6, contracts 10, pueue_live 3). No ignored tests.

Verified Rust fmt/check/Clippy (`-D warnings`)/build/tests; Rspress/Biome check and site build; 19 local documentation links; isolated Pueue spike; release binary build and `cargo install --locked --path tools/cli --root <temporary-root>`. Installed binary reports `axvern 0.1.0`; help includes the six public commands; noninteractive Ask returns JSON policy_denied and exit 9 before backend initialization.

Public CLI integration (`public_cli_execute_doctor_and_history_contracts`) verifies:

- AC-01/02: return while the job remains queued/running, execution ID resolves to its registered Pueue identity.
- AC-03/14: queued/running/done/failed normalized in parseable JSON, child exit 23 retained with wait wrapper exit 10, start/end timestamps present after completion.
- AC-04: 1ms wait expires (exit 124), same job subsequently finishes without cancellation.
- AC-05/06: Deny/default Ask/default shell denial make no registrations; refusal occurs even without configured backend.
- AC-07: empty, spaces, quote, newline, shell metacharacter, Unicode and option-like argv plus cwd preserved; injection marker absent.
- AC-13: stopped daemon gives doctor Fail and explicit `start pueued --config` remedy in JSON/human, exit 12.
- AC-15: direct Pueue registrations do not enter axvern history; limit bounds, unknown ID, deleted mapping expiry and offline metadata are checked.
- Doctor leaves task snapshots and existing pending writer data unchanged, removes its scratch probe, reports Warning for unverifiable logs and Pass after owned logs are available.

`terminal_ask_requires_an_explicit_yes` uses a private PTY, checks refusal and explicit approval, and verifies task counts. Golden fixtures lock policy JSON stdout/human stdout; refusal and unknown-ID outputs are also asserted. `doctor_tool_and_store_failures_are_structured` verifies missing/wrong Pueue versions, unsafe store permissions, project-only tool warnings and absence of unrelated tool probes. Existing adapter/store tests retain interrupted/concurrent writes, malformed/ambiguous registration without retry, and receipt re-save without a second registration.

Machine-readable runtime evidence is generated under ignored `target/release-evidence.json` and `target/pueue-spike.json`; full local console output is retained outside the tracked source. CI uploads equivalent evidence plus `quality.log` separately for Ubuntu and macOS.

Linux acceptance remains pending. CI configuration is not a substitute for a successful Linux run. License, binary destination, registry, signing and hosting are undecided. No commit, push, PR, Issue edit, deployment or publication is part of this implementation task.

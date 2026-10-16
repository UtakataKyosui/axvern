# Issue #20 implementation evidence

Date: 2026-10-16. Platform: macOS arm64. fd 10.5.0, tokei 15.0.0 (serialization support: json, cbor, yaml), Pueue 4.0.4.

## Tool behaviour `fs analyze` depends on

- `fd --glob -- '{Cargo.toml,package.json,…}'` matches the brace alternatives against the file name at any depth up to `--max-depth`; `Cargo.toml.bak` is not matched.
- Neither `fd` nor `tokei --streaming json` follows a symbolic link below the root: a link to a directory outside the root holding `Cargo.toml` and `.rs` files adds nothing to the manifests or statistics, and the tree lists the link itself as `symlink`.
- `tokei -- FILE` counts only that file. A file target therefore needs no directory scan: its tree item and manifest membership are taken from the validated target.
- tokei has no depth option, so the languages section walks the whole root; `summary.scope` reports `max_depth: null` for it and 32 for the manifest search.
- Pueue passes the client's environment (including `PATH`) to the task it registers. A `pueue clean` right after a worker published its result can still see that task running, because the worker publishes before its process exits.

## Verification

- `logics/tests/tests/analyze_fixtures.rs`: child plans and depths, file input, AC-SPEC-45-01/02/04/05 with the real discovery worker and fake tools, lost children, registration stopped partway, parent save failure, paging over 100 items and byte-budget truncation (no external CLI).
- `logics/tests/tests/discovery_cli.rs`: input validation, AC-SPEC-45-03 (one child asked or denied registers nothing and creates no store) and a missing tool, before registration.
- `logics/tests/tests/analyze_live.rs`: real fd/tokei through an isolated Pueue daemon: three sections, display depth against scan depth, `.git`/ignore/symlink boundaries, unread scripts, re-reading after `pueue clean`, file input, a failing child (partial, `wait` exit 11), unreadable directories, paging and `max_items`. Skipped when pueue, fd or tokei is missing.
- The Mojo contract cases were extended but not executed here (Mojo is not installed); all 50 cases were replayed against the built binary with a script.

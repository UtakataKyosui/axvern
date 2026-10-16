# Issue #19 implementation evidence

Date: 2026-10-16. Platform: macOS arm64. ast-grep 0.51.0, tokei 15.0.0 (serialization support: json, cbor, yaml), Pueue 4.0.4.

## Tool behaviour the adapters depend on

- `ast-grep run --pattern` accepts a malformed pattern and matches its error node. `ast-grep scan --inline-rules` rejects it with exit 8, so `code search` uses `scan` with a `pattern` rule; the rule text is built with `serde_json`, and a pattern containing quotes or `---` stays inside its own string.
- `scan` exits 0 whether or not anything matched or could be read. Unreadable paths appear only as `ERROR: <path>: <reason>` on stderr.
- A second inline rule `kind: ERROR` reports files that do not parse. Verified for Rust, JavaScript, TypeScript and Go. Not detected: a missing token (`let x = 1` without `;`) and the Python input `def f(:`. `code search` therefore reports parse errors on a best-effort basis only.
- `range.start.column` and `range.end.column` are 0-based character counts, not bytes (column 12 for a match that starts at byte 14). The adapter converts them from the `lines` text.
- `--config /dev/null` keeps a project's `sgconfig.yml` (`languageGlobs` remapped `*.rs` to Python in the spike) from changing which files belong to a language. Symlinks are not followed by default.
- `tokei --streaming json` prints one record per file in no fixed order, with nested languages under `blobs`. `--output json` prints nothing until the end. `tokei --version` ends with the compiled-in formats; a build without `json` is detected from that line.
- tokei per-language counts exclude embedded languages (an 8-line HTML file with 4 script lines is HTML 4 + JavaScript 4); tokei's own `Total` adds both. `code stats` totals are the sum over host languages and list embedded languages per host.
- tokei reports read errors only on stderr (two lines for a file) and exits 0; unknown-language and binary files are dropped without a report, so `skipped_files` is `null`.
- Neither tool applies `.gitignore` outside a Git worktree (`--no-require-git` has no equivalent), so such runs carry an `ignore_rules_not_applied` warning. tokei cannot skip `.git/info/exclude` or global ignore files.

## Verification

- `logics/tests/tests/code_fixtures.rs`: argv/no-shell, pattern injection, byte columns, parse-error handling, exit codes, limits, embedded languages, overflow, JSON-capability detection, serde round trips against recorded output in `logics/tests/fixtures/code/` (no external CLI).
- `logics/tests/tests/discovery_cli.rs`: input, language, policy, missing-tool and unsupported-build refusals before registration.
- `logics/tests/tests/code_live.rs`: real ast-grep/tokei through an isolated Pueue daemon; skipped when pueue, ast-grep or tokei is missing.
- The Mojo contract evaluator cases and error-code table were updated but not executed here (Mojo is not installed); the new cases were replayed against the built binary with a script.

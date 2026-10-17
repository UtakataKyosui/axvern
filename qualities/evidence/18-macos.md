# Issue #18 implementation evidence

Date: 2026-10-16. Platform: macOS arm64. fd 10.5.0, ripgrep 15.2.0, bat 0.26.1, eza 0.23.5, Pueue 4.0.4.

## Tool behaviour the adapters depend on

- `fd` reports unreadable directories only with `--show-errors` and still exits 0. It cannot disable `.git/info/exclude`, `.ignore` or `.fdignore`; `--full-path` matches the canonical absolute path, so root-relative full-path matching is done by the normalizer. Extension filters are case-insensitive.
- `rg` exits 1 for "no match" and 2 when any path was unreadable. A positive `--glob` overrides ignore rules, so positive globs are applied by the normalizer; only `!.git` is passed to rg. Binary files are counted through `--binary` and their matches are discarded.
- Ignore files above the Git root are applied by default by both tools; outside a repository `--no-require-git --no-ignore-parent` limits them to the root and below.
- `bat` passes bytes through (BOM, CRLF, escape sequences) and exits 0 past EOF; one line beyond the request is read to detect EOF.
- `eza` escapes names without escaping backslashes and cannot stop at `.git`, so `fs list` reads entries through `fd`; `eza` remains an optional `doctor` entry.
- APFS rejects non-UTF-8 file names; the non-UTF-8 path contract is covered with recorded fd/rg output instead.

## Verification

- `logics/tests/tests/discovery_fixtures.rs`: argv/no-shell, sorting, newline and non-UTF-8 names, limits, rg exit codes, binary, context and ranges, bat EOF/BOM/CRLF/binary (no external CLI).
- `logics/tests/tests/discovery_cli.rs`: input, target, policy and missing-tool refusals before registration.
- `logics/tests/tests/discovery_live.rs`: real fd/rg/bat through an isolated Pueue daemon; skipped when a tool is missing.
- The Mojo contract evaluator cases and error-code table were updated but not executed here (Mojo is not installed).

## Review follow-up: isolate ignore sources

`fd` now runs with `--no-ignore`; the normalizer applies only `.gitignore` using
`ignore::gitignore`. Repository-root ancestor rules and nested negations are
preserved; global ignore, `.git/info/exclude`, `.ignore` and `.fdignore` cannot
hide entries. Outside a repository, only rules at/below the explicit root apply.
Explicit ignored roots remain usable; descendant symlinks are not followed.
Unreadable or malformed ignore files produce a partial result, not silent omission.

This filters fd candidates rather than pruning its traversal. Candidates under
ignored directories therefore also consume `max_entries`; fd's depth limit,
worker timeout, item and artifact limits remain in force. Permission errors
inside ignored directories do not make the visible scope partial.
Regression fixtures cover personal excludes, parent scope, nested negation,
excluded parent directories, explicit ignored roots and `--include-ignored`.

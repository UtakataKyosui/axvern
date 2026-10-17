# Discover a codebase (v0.2)

These commands are implemented in the source checkout. They validate inputs and Policy before registering an internal worker with Pueue, wait up to `--wait` (default 1s), then return an execution ID and any available saved result. Use `--wait 0s` to return immediately and `result` to retrieve the immutable result later. Query/recall is not required. This is pre-release software, not a published v0.2 release.

## Tools and approval

| Command | Backend | Minimum supported version | Pinned acceptance version |
| --- | --- | --- | --- |
| `fs list`, `fs find` | fd | 10.0.0 | 10.5.0 |
| `fs search` | ripgrep (`rg`) | 14.0.0 | 15.2.0 |
| `fs read` | bat | 0.24.0 | 0.26.1 |
| `code search` | ast-grep | 0.51.0 | 0.51.0 |
| `code stats` | tokei with JSON serialization | 15.0.0 | 15.0.0 |
| `fs analyze` | fd + tokei | as above | as above |

Pueue/client and daemon must be 4.0.4. `doctor --scope discovery` reports missing, unsupported and incapable builds. A version range is not evidence that every later release was tested. eza is optional: its display text cannot safely represent structured entries, so `fs list` uses fd. git/delta/dust/procs are optional, unprovided integrations and do not block the discovery gate.

Add exact backend program rules to your [private configuration](/configuration) when noninteractive discovery is intended:

```toml
[[policy.rules]]
id = "discover-files"
command = "fd"
effect = "allow"

[[policy.rules]]
id = "search-text"
command = "rg"
effect = "allow"

[[policy.rules]]
id = "read-files"
command = "bat"
effect = "allow"

[[policy.rules]]
id = "search-code"
command = "ast-grep"
effect = "allow"

[[policy.rules]]
id = "count-code"
command = "tokei"
effect = "allow"
```

No match defaults to Ask; noninteractive Ask is refused. Each child of `fs analyze` must pass Policy and tool preflight before any child is registered. A denial registers none. These rules permit the corresponding backend, rather than imposing an OS sandbox or a filesystem access boundary.

## Commands

```text
axvern fs list [PATH] [--depth 1] [--include-ignored] [--wait 1s]
axvern fs find PATTERN [PATH] [--glob] [--full-path] [--extension EXT] [--type file|directory|symlink|any] [--depth 32] [--include-ignored] [--ignore-case] [--wait 1s]
axvern fs search PATTERN [PATH] [--fixed-strings] [--glob GLOB] [--context 0] [--ignore-case] [--include-ignored] [--wait 1s]
axvern fs read FILE [--from-line 1] [--lines 80] [--wait 1s]
axvern code search --pattern PATTERN --lang rust|javascript|typescript|python|go [PATH] [--include-ignored] [--wait 1s]
axvern code stats [PATH] [--include-ignored] [--wait 1s]
axvern fs analyze PATH [--depth 3] [--include-ignored] [--wait 1s]
```

Global `--config PATH` and `--format json|human` apply. Unlisted backend flags are not forwarded. Depth accepts 1–32; context 0–20; read line count 1–10000 and starting line at least 1. Repeat `--extension` or `--glob` to add filters. Omitted PATH uses cwd, except `fs read` and `fs analyze`, which require a target. `fs list`/`find` require a directory; search/stats accept a file or directory.

| Result kind | Item contents |
| --- | --- |
| `fs.list`, `fs.find` | root-relative `path`, `type` (`file`, `directory`, `symlink`, `other`) |
| `fs.search` | `path`, 1-based `line`, `text`, byte ranges, context lines, truncation flag |
| `fs.read` | 1-based `line`, `text`, truncation flag; summary reports EOF |
| `code.search` | `path`, `lang`, `start`/exclusive `end` (1-based line/UTF-8 byte column), `snippet`, truncation flag |
| `code.stats` | language, files/code/comments/blanks, embedded-language counts |
| `fs.analyze` | the corresponding entry/stat item plus its `section` |

`fs find` uses regex by default, glob with `--glob`, and case-sensitive matching unless `--ignore-case`. `--full-path` matches root-relative paths, not the canonical absolute root; fd's extension filters remain case-insensitive. `fs search` uses regex by default or literal `--fixed-strings`; positive globs narrow saved matches without overriding ignore rules. Zero text matches (rg exit 1) is a normal completed result. Binary matches are dropped and counted in the summary. `fs read` rejects binary/invalid UTF-8 data, normalizes BOM/CRLF, and preserves text as data; human rendering escapes terminal controls. Long lines/snippets are shortened on UTF-8 boundaries and flagged.

Code search uses `ast-grep scan --inline-rules`, not a project `sgconfig.yml`. A pattern is data, not a new rule. `tsx` is not one of the supported language selectors. Invalid patterns, unsupported languages and zero matches are distinct. Files with a detected `ERROR` node contribute parse errors rather than trusted matches; missing-token errors and some Python syntax errors are not detected. This is best-effort syntax-error reporting, not a full validity guarantee.

Code stats totals sum host-language counts. Embedded languages are separate and not added to those totals: HTML 4 + embedded JavaScript 4 contributes 4 to the host total. Unknown-language/binary files are not reported by tokei, so `skipped_files` is null, not a fabricated zero.

## Scope and limits

Explicit root symlinks resolve to their target. Descendant symlinks are not followed. `.git` may be an entry, but its internals are not traversed; direct targets inside `.git` are rejected. Hidden entries are included. Listings sort the collected subset by UTF-8 path bytes; a truncated subset is not promised to be the global lexicographic prefix. Non-UTF-8 paths and unreadable entries produce explicit partial results.

| Backend | Ignore behavior without `--include-ignored` |
| --- | --- |
| fd | axvern applies only `.gitignore`, from the repository root through nested target rules; outside a repository only at/below the selected root. Global ignore, `.git/info/exclude`, `.ignore` and `.fdignore` do not hide entries. |
| rg | Git ignore rules within repositories; outside repositories only at/below the selected root. Global, exclude and dot-ignore sources are disabled. |
| ast-grep | `.gitignore` within a Git worktree; outside it the result warns `ignore_rules_not_applied`. Global/exclude/dot-ignore sources are disabled. |
| tokei | `.gitignore` within a Git worktree; outside it the result warns `ignore_rules_not_applied`. Global ignore and `.git/info/exclude` cannot be disabled independently. |

`--include-ignored` disables ignore exclusion, but never permits `.git` internals or following descendant links. fd enumerates candidates with ignore disabled and filters them in axvern. Consequently even excluded candidates consume `max_entries`; its traversal is bounded by depth and the worker timeout. An explicitly selected ignored directory remains usable. Rules inside an excluded directory cannot re-include descendants. Ignore files are cached during a run, not treated as a filesystem snapshot.

[Discovery configuration](/configuration) defaults to 10000 saved items, 100000 reported entries, 60s per child. Artifact size also constrains retained items. Tool streaming records determine what can be counted; tokei does not enumerate unknown-language files. A limit produces `completeness.truncated` and reasons (`max_items`, `max_entries`, `timeout`, `artifact_bytes`), separately from `partial` errors. A configured display depth is the requested scope, not truncation. No depth limit is imposed on code search/stats because those tools do not provide one.

## Parent analysis and saved results

`fs analyze` creates a logical parent, with languages (tokei), manifests (fd find) and tree (fd list) children. Tree depth defaults to 3; manifest scan depth is 32; languages has no depth limit. `summary.scope` records each section's tool/depth. Manifests are names only (`Cargo.toml`, `package.json`, `pnpm-workspace.yaml`, `devbox.json`, `pyproject.toml`, `go.mod`); their contents/scripts are never executed. A file target creates only the languages child; tree and manifest membership come from that file's metadata without scanning siblings.

Every response page retains `children`, including section, execution ID, state, exit code, result availability, completeness and error. All usable sections produce `done`, some usable produce `partial`, none usable produce `failed`. Errors/warnings identify their section. Successful sections are retained if another child fails, without rerunning them. Partial child scope can also be reported through completeness even when its process state is done.

Aggregation is lazy: the first observation after children finish publishes the parent artifact once under the store lock. `status`, `wait`, `history` or `result` can trigger that publication; these reads never register new work. An unobserved parent can lose a section if the child result expires first. Once published, the parent uses its own saved artifact. `summary.snapshot=false` and acquisition timestamps make clear that sections were collected at different times.

Saved results use the same [pagination and retention contract](/cli) as run results: `output.result` contains kind, summary, items, completeness, errors, warnings and page. Fetching retained pages does not resume a scan or replay a child. A successful `result` retrieval exits 0 even for a partial/failed target; explicit parent `wait` exits 11 for partial or 10 for failed. Registration can fail after some children were accepted; preserve the returned parent/child IDs and receipt rather than blindly retrying.

The configured budgets apply to each child; the aggregate also limits its own items/artifact. Item order is languages → manifests → tree, so byte truncation removes tree entries first. Oversized headers fail with `output_limit_too_small`. This retained-artifact behavior does not claim the future AC-31 query/recall guarantee.

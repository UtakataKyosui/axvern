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
| `system disk-usage` | built in (no external CLI) | – | – |
| `code diff` | git, run by axvern with a fixed isolated argv | 2.40.0 | 2.43.0 |
| `system processes` | built in (`/proc` on Linux, `ps` elsewhere) | – | – |
| `fs analyze` | fd + tokei | as above | as above |

Pueue/client and daemon must be 4.0.4. `doctor --scope discovery` reports missing, unsupported and incapable builds. A version range is not evidence that every later release was tested. eza is optional: its display text cannot safely represent structured entries, so `fs list` uses fd. delta/dust/procs are optional, unprovided integrations and do not block the discovery gate. git is required only by `code diff`; `doctor` reports it as a warning when absent.

Every discovery adapter checks its version before registration, using the same capability detector as doctor. An unsupported version, failed/unrecognized version probe, or tokei build without JSON support is refused with `unsupported_tool_capability`; a probe timeout does not authorize execution. Optional eza display support is diagnosed from 0.23.5 (the measured version); missing or incompatible eza produces a warning and does not block structured discovery.

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
axvern system disk-usage [PATH] [--depth 3] [--top 20] [--size allocated|apparent] [--wait 1s]
axvern code diff [--staged] [--wait 1s]
axvern system processes [--match TEXT] [--limit 100] [--wait 1s]
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
| fd / rg / ast-grep / tokei | axvern applies only `.gitignore`, from the repository root through nested target rules; outside a repository only at/below the selected root. Global ignore, `.git/info/exclude`, `.ignore`, `.fdignore` and `.tokeignore` do not hide results. Explicit ignored file targets remain readable/searchable. |

`--include-ignored` disables ignore exclusion, but never permits `.git` internals or following descendant links. fd enumerates candidates with ignore disabled and filters them in axvern. Consequently even excluded candidates consume `max_entries`; its traversal is bounded by depth and the worker timeout. An explicitly selected ignored directory remains usable. Rules inside an excluded directory cannot re-include descendants. Ignore files are cached during a run, not treated as a filesystem snapshot.

[Discovery configuration](/configuration) defaults to 10000 saved items, 100000 reported entries, 60s per child. Artifact size also constrains retained items. Tool streaming records determine what can be counted; tokei does not enumerate unknown-language files. A limit produces `completeness.truncated` and reasons (`max_items`, `max_entries`, `timeout`, `artifact_bytes`), separately from `partial` errors. A configured display depth is the requested scope, not truncation. Text/code search and statistics have a scan depth of 32, enforced by native traversal flags or anchored backend exclusion globs and checked again during normalization. Their summaries record `scan_depth: 32`; older saved artifacts without the field retain an unknown depth. Ignored candidates can still consume enumeration/time budgets because filtering occurs after backend enumeration.

## Parent analysis and saved results

`fs analyze` creates a logical parent, with languages (tokei), manifests (fd find) and tree (fd list) children. Tree depth defaults to 3; manifest scan depth is 32; language statistics also use depth 32. `summary.scope` records each section's tool/depth, using the saved language child's depth so older unbounded results are not relabeled. Manifests are names only (`Cargo.toml`, `package.json`, `pnpm-workspace.yaml`, `devbox.json`, `pyproject.toml`, `go.mod`); their contents/scripts are never executed. A file target creates only the languages child; tree and manifest membership come from that file's metadata without scanning siblings.

Every response page retains `children`, including section, execution ID, state, exit code, result availability, completeness and error. All usable sections produce `done`, some usable produce `partial`, none usable produce `failed`. Errors/warnings identify their section. Successful sections are retained if another child fails, without rerunning them. Partial child scope can also be reported through completeness even when its process state is done.

Aggregation is lazy: the first observation after children finish publishes the parent artifact once under the store lock. `status`, `wait`, `history` or `result` can trigger that publication; these reads never register new work. An unobserved parent can lose a section if the child result expires first. Once published, the parent uses its own saved artifact. `summary.snapshot=false` and acquisition timestamps make clear that sections were collected at different times.

Saved results use the same [pagination and retention contract](/cli) as run results: `output.result` contains kind, summary, items, completeness, errors, warnings and page. Fetching retained pages does not resume a scan or replay a child. A successful `result` retrieval exits 0 even for a partial/failed target; explicit parent `wait` exits 11 for partial or 10 for failed. Registration can fail after some children were accepted; preserve the returned parent/child IDs and receipt rather than blindly retrying.

The configured budgets apply to each child; the aggregate also limits its own items/artifact. Item order is languages → manifests → tree, so byte truncation removes tree entries first. Oversized headers fail with `output_limit_too_small`. This retained-artifact behavior does not claim the future AC-31 query/recall guarantee.

## Disk usage

`system disk-usage` is measured by axvern itself, not by dust: dust's JSON has no partial marker, counts hard links twice in apparent mode and gives no node type (spike #23). It starts like the other commands (Policy, Pueue, `--wait`, `result`) but its worker walks the tree in-process, so no external CLI is required. For Policy it is the pseudo-program `axvern-disk-usage`; add an exact rule when noninteractive use is intended:

```toml
[[policy.rules]]
id = "disk-usage"
command = "axvern-disk-usage"
effect = "allow"
```

`--size allocated` (default) reports the blocks the filesystem allocated; `--size apparent` reports logical length. The mode is always returned and never substituted. Units are integer bytes. A directory's own entry is counted and a parent's size contains its children, so the listed rows must not be added up: use `summary.total_bytes`. Hidden entries, ignored files and `.git` contents are measured (a capacity question, unlike code search). Symbolic links count as links and are never followed, including a link given explicitly as the target (the link itself is measured, not what it points to); `.git` and its subdirectories may be targeted directly; a directory on another filesystem is not entered (`other_filesystem_skipped` warning). A file with several hard links is counted once and attributed to the first path in byte order of its path components; the other links show 0.

`--depth` (default 3, 1–32) only selects which rows are listed; the whole tree is measured within the usual budgets. `--top` (default 20, 1–1000) keeps the largest rows, ties ordered by path bytes, and is a request, not truncation. An unreadable path makes the result `partial` with `summary.lower_bound=true` (sizes are at least these values); reaching `max_entries` or the timeout is `truncated` with the same lower bound. A directory is read only as far as the remaining entry budget needs and the timeout allows, so a truncated walk measures a subset that is not promised to be the global path-order prefix. Non-UTF-8 names are counted in the totals, reported as an `invalid_encoding` error and not listed. `summary.entries` counts measured entries below the root. Sizes are not a point-in-time snapshot; entries that vanish during the walk produce a warning.

## Code diff

`code diff` compares the Git worktree that contains the current directory (the whole worktree, whatever subdirectory you are in). By default it compares index → worktree; `--staged` compares HEAD → index, and before the first commit the empty tree stands in for HEAD. Untracked files are not compared; `summary.untracked_included` is always `false`. Outside a worktree (including inside `.git`) it fails before registration with `not_a_repository`, exit 2. No arbitrary revisions, paths or git options are accepted.

Git is the source of truth, but it is never run the way a user would run it. A plain `git diff --no-ext-diff --no-textconv` still starts the clean/process filter program that repository configuration attaches to a path (`qualities/scripts/git_filter_spike.py` reproduces it), so a "read" command could execute repository-chosen programs. axvern instead runs one fixed argv (`git diff --no-ext-diff --no-textconv --ignore-submodules=all --patch-with-raw -z -M50% [--cached]`) with a cleared environment, no user or system configuration, no pager, `core.fsmonitor=false`, no optional index locks, and `GIT_ATTR_SOURCE` set to the empty tree so no `.gitattributes` is read. `GIT_ATTR_SOURCE` does not cover `.git/info/attributes`; a repository whose `info/attributes` mentions `filter` or defines a macro is refused (`unsupported_capability`) both before registration and again before the run. Hooks, pagers and external diff drivers are never started, git's own state is not modified, and delta is not used: output is always plain text.

Items are `old_path`, `new_path`, `change` (`added`, `deleted`, `modified`, `renamed`, `type_changed`), `binary`, `hunks` and `patch_truncated`. `old_path` is null for an added file and `new_path` for a deleted one. Renames use a fixed 50 % similarity; below it the change is an add and a delete. A binary file carries only this metadata and no content. A hunk holds its line range and the patch text (lossily decoded as UTF-8); a hunk is cut at 8 KiB and a file at 32 KiB, always on whole lines, and the item says `patch_truncated`. A cut patch is for reading and is not guaranteed to apply. Zero differences is a normal `done` result with no items. Paths that are not valid UTF-8 are reported as `invalid_encoding` errors (partial) and not listed. For Policy the pseudo-program is `axvern-code-diff`:

```toml
[[policy.rules]]
id = "code-diff"
command = "axvern-code-diff"
effect = "allow"
```

## System processes

`system processes` takes one snapshot of the process table; it is not a monitor and never sends signals, changes priority or elevates privileges. It is read by axvern itself, not procs, whose JSON has no executable-name column (the command is the whole argv), a start time without a zone and no marker for processes hidden by permissions (spike #23). For Policy the pseudo-program is `axvern-system-processes`.

Items are `pid`, `ppid`, `uid`, `name`, `state`, `rss_bytes` and `started_at`, in ascending PID order; `null` means not obtained. `summary.observed_at` is when the table was read by the worker, not when the task was registered, and PIDs are reused, so a PID identifies a process only together with that time. `uid` is the real numeric owner (no name lookup). `state` is `running`, `sleeping`, `uninterruptible`, `stopped`, `zombie`, `idle`, `dead` or `other`. `rss_bytes` is resident memory in bytes. `started_at` is UTC and derived from the kernel boot time on Linux; on other systems it is null. The command line, environment and network information are never read.

`name` is the basename of the executable when its link can be read, otherwise the kernel command name (at most 15 bytes; it can differ from the executable and for other users' processes may be the only name available). `--match TEXT` keeps processes whose `name` contains TEXT as a case-sensitive literal (non-empty, at most 256 bytes, no `/` or NUL); it is never matched against arguments. `--limit` is 1–1000 (default 100); a result longer than the limit is `truncated` with `max_items`, and `summary.scanned` then counts only what was examined. A process that ends while being read is a `disappeared` warning. A process that cannot be inspected for lack of permission is a `permission_denied` error and makes the result `partial` (still retrieved with exit 0); a process hidden entirely by the system (for example `hidepid`) cannot be detected. Zero matches is a normal `done` result.

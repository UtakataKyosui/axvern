# Issue #23 optional adapter spike: git + delta, dust, procs

Date: 2026-10-16. Platform: macOS arm64, non-interactive stdout/stderr (no TTY). git 2.55.0, delta 0.20.1, dust 1.2.6, procs 0.14.12. Measurements ran against throw-away fixtures under `/tmp`; no user queue, repository or process was changed. Linux was not measured.

## Verdicts

| Tool | Verdict | Role |
| --- | --- | --- |
| git | Not adopted for worktree diffs: fixed argv still executes configured filters | candidate diff source only after verified isolation |
| delta | Adoptable for human display only, via staged execution | optional formatter |
| dust | Not adopted for `system disk-usage` (#49) | none until the conditions below hold |
| procs | Not adopted for `system processes` (#50) | none until the conditions below hold |

No public command is added. `doctor --scope all` reports git/delta/dust/procs as optional warnings ("integration is not provided; installed version is not certified"), so absent tools never fail the required gate. Core and `discovery` scopes do not probe them.

## git + delta

- Candidate argv for the structured result, **not approved for hostile configuration**: `git --no-pager -c color.ui=never diff [--cached] --no-ext-diff --no-textconv --no-color -M --raw -z` (entries with `R100`/`M`/`A` status, NUL-separated, newline and space in names preserved) plus `--numstat -z` (binary shows `-\t-`) and the unified patch for hunks. Binary files print `Binary files ... differ` only.
- User/repo configuration is hostile by default and must be neutralized:
  - `diff.external` set: plain `git diff` exits 128; `--no-ext-diff` restores exit 0.
  - `color.ui=always` leaks ANSI into captured output; `--no-color` / `-c color.ui=never` removes it.
  - A `diff.<driver>.textconv` configured through `.gitattributes` executes during plain `git diff` (observed side effect); with `--no-textconv` it does not.
  - **Review correction:** `.gitattributes` assigning `filter=fixture` still causes `filter.fixture.clean` or `filter.fixture.process` to execute for a worktree diff with the candidate argv. Neither `--no-ext-diff` nor `--no-textconv` disables content filters. `python3 qualities/scripts/git_filter_spike.py` reproduces this on git 2.55.0 in an isolated temporary repository. The fixture clean program writes a marker and passes stdin through; raw, numstat and patch all write the marker and exit 0. The process fixture writes a marker then deliberately exits before its protocol handshake; all three forms launch it and exit 128 with `filter.fixture.required=true`. This verifies program execution, not successful process-protocol conversion. These measurements concern worktree diffs, not `--cached`.
  - `core.pager` is not used with non-TTY stdout, but `--no-pager` keeps this independent of that.
- Exit codes: success 0 with or without differences; `--exit-code`/`--quiet` return 1 on differences and must not be used (collides with error handling). Outside a repository `git diff` exits 129 (it falls back to `--no-index` usage); `git rev-parse --show-toplevel` exits 128 and is the repository check to use. Unknown revision exits 128.
- Unborn repository: `git diff --cached` works against the empty tree (status `A`, exit 0). Untracked files are never listed.
- Rename: `-M` gives `R100` for pure renames; behaviour at partial similarity thresholds (`-M50%` with a content change) was not stressed, so keep the fixed default and report unsure cases as add/delete as #48 requires.
- delta consumes a patch on stdin only. It exits 0 for invalid or empty input and for failed upstream, so it cannot signal that the patch was bad. It emits ANSI even when piped and ignores `NO_COLOR`. It reads the user's gitconfig `[delta]` section unless `--no-gitconfig` is given. `--paging=never` is required to avoid a pager.
- Piping `git diff | delta` loses git's exit status without `pipefail` and gives no way to capture the structured result once. Missing delta exits 127 from the shell.
- Adoption gate for #48: worktree diff remains unsupported until configuration/attributes are isolated or every applicable clean/process filter is disabled with verified coverage. An acceptable implementation must prove that clean and process marker fixtures are never launched for raw, numstat and patch output, including filters from repository/user configuration and nested attributes. No neutralization scheme was validated in this spike; the candidate argv alone is insufficient.
- Once a safe git source is established, run it first and capture its status and stdout, then feed the captured patch to `delta --no-gitconfig --paging=never` as a second fixed step. A delta failure or absence keeps the git result and adds a warning plus plain text. Do not pass a shell pipeline to Pueue. Pueue registration of a two-step fixed wrapper is a #48 implementation matter; this spike did not register Pueue tasks.

## dust

- `dust -j -P -o b -d N [-n N] [-s] PATH` writes JSON to stdout (despite the help text saying "current directory") with exit 0: `{"size":"139264B","name":"du","children":[...]}` where `size` is a string with unit suffix (bytes only with `-o b`), `name` is the path as given, and there is no file/directory marker, depth marker or truncation flag.
- Allocated mode (default) counts a hard link once. Apparent mode (`-s`) lists both links at 100000 bytes each and sums them, contradicting #49's single-count rule.
- A symlink is reported as 0B (allocated) or its link length (apparent, 4B); it is not followed by default.
- Unreadable directories: exit 0, the entry is listed as `0B`, and the only signal is a stderr line ("Did not have permissions for all directories"). The JSON carries no partial marker.
- Nonexistent path exits 1 with a text message on stderr. A file argument returns a single node.
- Hidden files are included by default. The plain output is a bar graph with box drawing characters and is not parseable.
- Verdict: not adopted. Required for adoption: axvern must own the size accounting (hard-link dedup in apparent mode, explicit partial/lower_bound from permission errors, node type), which dust's JSON does not provide, so either a pinned-version adapter that derives these from `--print-errors` stderr and a second pass, or a native traversal (#49 explicitly leaves this to "unsupported_tool_capability" when machine-readable input cannot be verified). Linux behaviour and `-x` filesystem boundary were not measured.

## procs

- `procs --json --no-header` writes a JSON array with exit 0; keys are display column titles. Columns are chosen only through a config file (`--load-config`; `--only` accepts one column). A config with `Pid, Ppid, Uid, User, State, VmRss, VmSize, StartTime, ElapsedTime, Command` yields `PID`, `Parent PID`, `UID`, `State` (`"R"`/`"S"`), `VmRSS` (bytes), `Start` (`"YYYY/MM/DD HH:MM"`, local time, no zone, minute precision), `Elapsed` (human string like `"1.2days"`), `Command`.
- `Command` is the full argv. There is no executable-name column, and paths with spaces (`.../Browser Helper.app/.../Browser Helper --type=...`) make the executable/argument boundary ambiguous, so #50's "basename only, no argv" cannot be satisfied without exposing or guessing from argv.
- Keyword filter matches any column including argv and user, is case-sensitive and literal by default; a non-match returns `[]` with exit 0. `--regex`, `--and/--or` exist. It cannot restrict matching to the basename.
- Without privileges only the caller's UID is listed (390 of about 554 processes in the run, all UID 501); there is no partial/warning signal.
- One process reported a start time in 1976 (kernel/boot-time quirk), so `Start` is not trustworthy.
- `--color always --pager always` with `--json` still produced plain JSON. Unknown flag exits 2.
- Verdict: not adopted. Required for adoption: a verified field set without argv (name only), a start time with timezone semantics, a documented partial indicator for unreadable processes, and Linux measurements for each field/unit. Until then `system processes` would be a thin reinterpretation of display output and #50's privacy and partial contracts cannot be met.

## Verification

- Review follow-up: `python3 qualities/scripts/git_filter_spike.py` verifies six filter invocations (clean/process × raw/numstat/patch) with the candidate argv. No filter-safe git adapter or public command is introduced.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `AXVERN_TEST_BIN=<debug binary> cargo test --workspace --locked` (45 tests) pass. The doctor test asserts that `git`, `delta`, `dust` and `procs` are optional warnings in the `all` scope.
- The Mojo CLI contract evaluator and Moon were not run (not installed here).
- delta was installed from the upstream release archive because the Homebrew formula (`git-delta`) could not be installed in this environment; dust and procs came from Homebrew.

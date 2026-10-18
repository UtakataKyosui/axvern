//! `code diff`: Git is the source of truth, run with a fixed argv inside the worker.
//!
//! `git diff --no-ext-diff --no-textconv` still runs the clean/process filter configured for a
//! path (spike #23), which would execute a program named by repository configuration. The diff
//! therefore runs with:
//! - `GIT_ATTR_SOURCE` set to the empty tree, so no `.gitattributes` (and no `filter=`) is read;
//! - `.git/info/attributes`, which that variable does not cover, required to be free of
//!   `filter` and macro definitions (checked before registration and again before running);
//! - user/system configuration, the pager, an external diff driver, textconv, fsmonitor,
//!   submodule traversal and optional index locks all switched off.
//!
//! One invocation prints `--raw` records (NUL separated) followed by the patch, so paths come
//! from the raw part and are never parsed out of `diff --git` headers.
use super::*;
use std::{ffi::OsString, fs, time::Instant};

pub const PROGRAM: &str = "axvern-code-diff";
pub const NATIVE: ToolSpec = ToolSpec {
    name: PROGRAM,
    purpose: "code diff (runs git with a fixed, isolated argv)",
    min_version: [0, 0, 0],
    tested_version: "built in",
    capable: any_build,
    feature: "",
};
/// Rename similarity, fixed so that the same change always yields the same classification.
const RENAME_THRESHOLD: &str = "-M50%";
const MAX_GIT_OUTPUT: usize = 16 * 1024 * 1024;
const MAX_HUNK_BYTES: usize = 8 * 1024;
const MAX_FILE_PATCH_BYTES: usize = 32 * 1024;

pub struct CodeDiff(pub CodeDiffJob);
impl DiscoveryAdapter for CodeDiff {
    fn tool(&self) -> &'static ToolSpec {
        &NATIVE
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let args: Vec<OsString> = if self.0.staged {
            vec!["--staged".into()]
        } else {
            Vec::new()
        };
        Ok(ExecutionPlan {
            program: PROGRAM.into(),
            args,
            cwd: PathBuf::from(&self.0.root),
        })
    }
    fn normalizer(&self, _byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        Err(Error::new(
            ErrorCode::BackendFailure,
            "code diff runs in the worker, not through a registered child process",
        ))
    }
    fn is_native(&self) -> bool {
        true
    }
    fn preflight(&self) -> Result<(), Error> {
        require_supported(&GIT)?;
        guard(Path::new(&self.0.root))
    }
    fn run_native(&self, deadline: Instant) -> Option<DiscoveryOutcome> {
        Some(run(&self.0, deadline))
    }
}

fn isolated(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .args(["--no-pager", "--no-optional-locks", "-C"])
        .arg(root)
        .args([
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotepath=false",
            "-c",
            "color.ui=never",
        ]);
    command
}
fn git_text(root: &Path, args: &[&str], deadline: Instant) -> Result<Option<String>, Error> {
    let mut command = isolated(root);
    command.args(args);
    let out = capture(&mut command, deadline, 64 * 1024).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::new(ErrorCode::MissingTool, "git is not installed")
        } else {
            Error::new(ErrorCode::BackendFailure, "git could not be started")
        }
    })?;
    if out.timed_out {
        return Err(Error::new(ErrorCode::Timeout, "git did not finish in time"));
    }
    Ok((out.code == Some(0))
        .then(|| String::from_utf8(out.stdout).ok())
        .flatten()
        .map(|s| s.trim_end_matches('\n').to_owned()))
}
/// The worktree containing `dir`, as a resolved UTF-8 path.
pub fn worktree_root(dir: &Path) -> Result<PathBuf, Error> {
    let deadline = Instant::now() + Duration::from_secs(10);
    match git_text(dir, &["rev-parse", "--show-toplevel"], deadline)? {
        Some(top) if !top.is_empty() => Ok(PathBuf::from(top)),
        _ => Err(Error::new(
            ErrorCode::NotARepository,
            "the directory is not inside a Git worktree",
        )),
    }
}
/// Refuses a repository whose private attributes could still name a filter.
pub fn guard(root: &Path) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let unsupported = |message: &str| Error::new(ErrorCode::UnsupportedCapability, message);
    let Some(path) = git_text(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/attributes",
        ],
        deadline,
    )?
    else {
        return Err(Error::new(
            ErrorCode::NotARepository,
            "the directory is not inside a Git worktree",
        ));
    };
    match fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("filter") || text.contains("[attr]") {
                return Err(unsupported(
                    "info/attributes may select a filter driver, which a read-only diff cannot run safely",
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(unsupported("info/attributes could not be inspected")),
    }
}

fn run(job: &CodeDiffJob, deadline: Instant) -> DiscoveryOutcome {
    let root = Path::new(&job.root);
    let comparison = if job.staged {
        DiffComparison::HeadToIndex
    } else {
        DiffComparison::IndexToWorktree
    };
    let summary = DiscoverySummary::CodeDiff {
        comparison,
        untracked_included: false,
        root: job.root.clone(),
    };
    let failed = |code: ErrorCode, message: &str| DiscoveryOutcome {
        state: ChildState::Failed,
        exit_code: Some(1),
        document: DiscoveryDocument {
            summary: summary.clone(),
            items: Vec::new(),
            completeness: Completeness::new(Vec::new(), true),
            errors: vec![ResultIssue::new(code, message)],
            warnings: Vec::new(),
        },
    };
    if let Err(e) = require_supported(&GIT).and_then(|()| guard(root)) {
        return failed(e.code, &e.message);
    }
    // The empty tree stands in for HEAD before the first commit.
    let empty_tree = git_text(root, &["hash-object", "-t", "tree", "--stdin"], deadline)
        .ok()
        .flatten()
        .filter(|h| !h.is_empty());
    let mut command = isolated(root);
    command
        .env("GIT_ATTR_SOURCE", empty_tree.as_deref().unwrap_or(""))
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--ignore-submodules=all",
            "--no-abbrev",
            "--patch-with-raw",
            "-z",
            RENAME_THRESHOLD,
        ]);
    if job.staged {
        let has_head = git_text(root, &["rev-parse", "--verify", "-q", "HEAD"], deadline)
            .ok()
            .flatten()
            .is_some();
        command.arg("--cached");
        if !has_head && let Some(tree) = &empty_tree {
            command.arg(tree);
        }
    }
    command.arg("--");
    if empty_tree.is_none() {
        return failed(
            ErrorCode::BackendFailure,
            "git could not report the empty tree",
        );
    }
    let out = match capture(&mut command, deadline, MAX_GIT_OUTPUT) {
        Ok(out) => out,
        Err(_) => return failed(ErrorCode::BackendFailure, "git could not be started"),
    };
    if out.timed_out {
        return failed(ErrorCode::Timeout, "git did not finish in time");
    }
    if !out.capped && out.code != Some(0) {
        return failed(ErrorCode::BackendFailure, "git diff failed");
    }
    let (entries, patch_start) = parse_raw(&out.stdout);
    let chunks = split_patch(&out.stdout[patch_start..]);
    let mut warnings = Vec::new();
    let mut reasons = Vec::new();
    let aligned = chunks.len() == entries.len() || (out.capped && chunks.len() <= entries.len());
    if !aligned {
        warnings.push(ResultWarning {
            code: WarningCode::SourceChanged,
            message: "the patch did not match the file list; hunks were left out".into(),
            path: None,
            section: None,
        });
    }
    if out.capped {
        reasons.push(TruncationReason::BodyBytes);
    }
    let mut items = Vec::new();
    let mut partial = false;
    let mut errors = Vec::new();
    let last = chunks.len().saturating_sub(1);
    for (i, entry) in entries.iter().enumerate() {
        let name = |p: &Option<Vec<u8>>| match p {
            None => Ok(None),
            Some(bytes) => std::str::from_utf8(bytes).map(|s| Some(s.to_owned())),
        };
        let (Ok(old_path), Ok(new_path)) = (name(&entry.old), name(&entry.new)) else {
            partial = true;
            errors.push(ResultIssue::new(
                ErrorCode::InvalidEncoding,
                "a path is not valid UTF-8; the change is not listed",
            ));
            continue;
        };
        let chunk = aligned.then(|| chunks.get(i)).flatten();
        let (binary, hunks, cut) = match chunk {
            Some(text) => read_chunk(text, out.capped && i == last),
            None => (false, Vec::new(), true),
        };
        items.push(DiscoveryItem::Diff(DiffItem {
            old_path,
            new_path,
            change: entry.change,
            binary,
            hunks,
            patch_truncated: cut,
        }));
    }
    if items.len() as u64 > job.limits.max_items {
        items.truncate(job.limits.max_items as usize);
        reasons.push(TruncationReason::MaxItems);
    }
    DiscoveryOutcome {
        state: ChildState::Done,
        exit_code: Some(0),
        document: DiscoveryDocument {
            summary,
            items,
            completeness: Completeness::new(reasons, partial),
            errors,
            warnings,
        },
    }
}

struct Entry {
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    change: DiffChange,
}
/// Parses the `--raw -z` records at the start of the output and returns the entries with the
/// offset where the patch begins. A record cut by the output cap is dropped.
fn parse_raw(data: &[u8]) -> (Vec<Entry>, usize) {
    fn field<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let rest = data.get(*pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        *pos += end + 1;
        Some(&rest[..end])
    }
    let mut entries = Vec::new();
    let mut pos = 0;
    while data.get(pos) == Some(&b':') {
        let start = pos;
        let Some(meta) = field(data, &mut pos) else {
            return (entries, start);
        };
        // `:old-mode new-mode old-sha new-sha STATUS`; the status letter may carry a score.
        let kind = meta
            .rsplit(|&b| b == b' ')
            .next()
            .and_then(|word| word.first())
            .copied()
            .unwrap_or(b'M');
        let Some(first) = field(data, &mut pos).map(<[u8]>::to_vec) else {
            return (entries, start);
        };
        let second = if matches!(kind, b'R' | b'C') {
            match field(data, &mut pos) {
                Some(path) => Some(path.to_vec()),
                None => return (entries, start),
            }
        } else {
            None
        };
        let same = |change| Entry {
            old: Some(first.clone()),
            new: Some(first.clone()),
            change,
        };
        entries.push(match kind {
            b'A' => Entry {
                old: None,
                new: Some(first),
                change: DiffChange::Added,
            },
            b'C' => Entry {
                old: None,
                new: second,
                change: DiffChange::Added,
            },
            b'D' => Entry {
                old: Some(first),
                new: None,
                change: DiffChange::Deleted,
            },
            b'R' => Entry {
                old: Some(first),
                new: second,
                change: DiffChange::Renamed,
            },
            b'T' => same(DiffChange::TypeChanged),
            _ => same(DiffChange::Modified),
        });
    }
    // An empty record separates the file list from the patch.
    if data.get(pos) == Some(&0) {
        pos += 1;
    }
    (entries, pos)
}
/// Splits the patch text into one chunk per file, each starting at its `diff` header line.
fn split_patch(patch: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut line = 0;
    while line < patch.len() {
        let rest = &patch[line..];
        if rest.starts_with(b"diff --git ")
            || rest.starts_with(b"diff --cc ")
            || rest.starts_with(b"diff --combined ")
        {
            starts.push(line);
        }
        match rest.iter().position(|&b| b == b'\n') {
            Some(n) => line += n + 1,
            None => break,
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| &patch[s..starts.get(i + 1).copied().unwrap_or(patch.len())])
        .collect()
}
/// `(binary, hunks, cut)` for one file's patch. `cut` also covers a patch the output cap ended.
/// A hunk is kept as a prefix of whole lines: once one line does not fit, nothing after it is
/// added, so the text never skips lines in the middle.
fn read_chunk(chunk: &[u8], output_cut: bool) -> (bool, Vec<DiffHunk>, bool) {
    let text = String::from_utf8_lossy(chunk);
    let mut cut = output_cut;
    let mut binary = false;
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut open = false;
    let mut used = 0usize;
    for line in text.split_inclusive('\n') {
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            binary = true;
        }
        if line.starts_with("@@ ") {
            match parse_header(line) {
                Some(hunk) if used + line.len() <= MAX_FILE_PATCH_BYTES => {
                    used += line.len();
                    hunks.push(hunk);
                    open = true;
                }
                _ => {
                    // Over the per-file bound, or a header this reader does not understand.
                    cut = true;
                    break;
                }
            }
        } else if open && let Some(hunk) = hunks.last_mut() {
            if hunk.patch.len() + line.len() > MAX_HUNK_BYTES
                || used + line.len() > MAX_FILE_PATCH_BYTES
            {
                cut = true;
                open = false;
            } else {
                hunk.patch.push_str(line);
                used += line.len();
            }
        }
    }
    if binary {
        hunks.clear();
    }
    (binary, hunks, cut)
}
/// `@@ -a[,b] +c[,d] @@` → a hunk holding just its header line.
fn parse_header(line: &str) -> Option<DiffHunk> {
    let mut words = line.split_ascii_whitespace();
    words.next()?;
    let range = |word: &str, sign: char| -> Option<(u64, u64)> {
        let body = word.strip_prefix(sign)?;
        let (start, count) = body.split_once(',').unwrap_or((body, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    let (old_start, old_lines) = range(words.next()?, '-')?;
    let (new_start, new_lines) = range(words.next()?, '+')?;
    Some(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        patch: line.to_owned(),
    })
}

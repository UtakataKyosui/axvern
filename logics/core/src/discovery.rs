//! Read-only investigation use cases: validate a request before anything is registered, then
//! run the registered job in the background worker and store its normalized result.
use crate::{
    adapters::tools::{
        self, Flow, Normalizer, Stop, TargetExit, ToolSpec, ast_grep, fd::PathFilter,
        rg::GlobFilter,
    },
    config::Results,
    results,
    store::ExecutionStore,
};
use axvern_models::*;
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{RecvTimeoutError, sync_channel},
    thread,
    time::{Duration, Instant},
};

/// Never block on a FIFO/device swapped in between path validation and opening.
fn open_regular(path: &Path) -> Result<File, Error> {
    if !fs::metadata(path)
        .map_err(|e| target_error(&e, path))?
        .is_file()
    {
        return Err(invalid(
            ErrorCode::InvalidTarget,
            "path is not a regular file",
        ));
    }
    let handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| target_error(&e, path))?;
    if !handle
        .metadata()
        .map_err(|e| target_error(&e, path))?
        .is_file()
    {
        return Err(invalid(
            ErrorCode::InvalidTarget,
            "path is not a regular file",
        ));
    }
    Ok(handle)
}

/// A validated request: the job the worker needs and the argv Policy must judge.
#[derive(Debug)]
pub struct Prepared {
    pub job: DiscoveryJob,
    pub plan: ExecutionPlan,
    pub tool: &'static ToolSpec,
}

fn invalid(code: ErrorCode, message: &str) -> Error {
    Error::new(code, message)
}
pub(crate) fn target_error(e: &std::io::Error, requested: &Path) -> Error {
    match e.kind() {
        std::io::ErrorKind::NotFound if requested.symlink_metadata().is_ok() => {
            invalid(ErrorCode::InvalidTarget, "path is a broken symbolic link")
        }
        std::io::ErrorKind::NotFound => invalid(ErrorCode::TargetNotFound, "path does not exist"),
        std::io::ErrorKind::PermissionDenied => {
            invalid(ErrorCode::PermissionDenied, "path is not readable")
        }
        // A symbolic link cycle, or a component that is not a directory.
        _ => invalid(ErrorCode::InvalidTarget, "path cannot be resolved"),
    }
}
/// An explicit root may be a symbolic link or ignored, but never Git's own storage.
pub(crate) fn resolve(path: Option<&Path>, cwd: &Path) -> Result<PathBuf, Error> {
    let requested = path.map_or_else(|| cwd.to_path_buf(), |p| cwd.join(p));
    let resolved = requested
        .canonicalize()
        .map_err(|e| target_error(&e, &requested))?;
    if resolved.components().any(|c| c.as_os_str() == ".git") {
        return Err(invalid(
            ErrorCode::InvalidTarget,
            "Git's internal storage is not a valid target",
        ));
    }
    if resolved.to_str().is_none() {
        return Err(invalid(
            ErrorCode::InvalidEncoding,
            "path is not valid UTF-8",
        ));
    }
    Ok(resolved)
}
fn utf8(path: &Path) -> String {
    path.to_str().unwrap_or_default().to_owned()
}
fn in_repo(dir: &Path) -> bool {
    dir.ancestors().any(|a| a.join(".git").exists())
}
fn directory(path: Option<&Path>, cwd: &Path) -> Result<PathBuf, Error> {
    let root = resolve(path, cwd)?;
    if !root.is_dir() {
        return Err(invalid(ErrorCode::InvalidTarget, "path is not a directory"));
    }
    fs::read_dir(&root).map_err(|e| target_error(&e, &root))?;
    Ok(root)
}
/// A directory is searched as the root; a regular file as its parent plus its basename.
fn directory_or_file(path: Option<&Path>, cwd: &Path) -> Result<(PathBuf, Option<String>), Error> {
    let target = resolve(path, cwd)?;
    if target.is_dir() {
        fs::read_dir(&target).map_err(|e| target_error(&e, &target))?;
        Ok((target, None))
    } else if target.is_file() {
        open_regular(&target)?;
        let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        Ok((parent, Some(name)))
    } else {
        Err(invalid(
            ErrorCode::InvalidTarget,
            "path is neither a directory nor a regular file",
        ))
    }
}
/// Target of `system disk-usage`. Unlike code search it measures capacity, so `.git` is a valid
/// target, and a symbolic link given explicitly is measured as the link itself, not its target.
fn disk_target(path: Option<&Path>, cwd: &Path) -> Result<(PathBuf, Option<String>), Error> {
    let requested = path.map_or_else(|| cwd.to_path_buf(), |p| cwd.join(p));
    let meta = fs::symlink_metadata(&requested).map_err(|e| target_error(&e, &requested))?;
    let utf8_name = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .ok_or_else(|| invalid(ErrorCode::InvalidEncoding, "path is not valid UTF-8"))
    };
    if meta.file_type().is_symlink()
        && let Some(parent) = requested.parent()
    {
        let name = utf8_name(&requested)?;
        let parent = parent
            .canonicalize()
            .map_err(|e| target_error(&e, parent))?;
        if parent.to_str().is_none() {
            return Err(invalid(
                ErrorCode::InvalidEncoding,
                "path is not valid UTF-8",
            ));
        }
        return Ok((parent, Some(name)));
    }
    let target = requested
        .canonicalize()
        .map_err(|e| target_error(&e, &requested))?;
    if target.to_str().is_none() {
        return Err(invalid(
            ErrorCode::InvalidEncoding,
            "path is not valid UTF-8",
        ));
    }
    if target.is_dir() {
        fs::read_dir(&target).map_err(|e| target_error(&e, &target))?;
        Ok((target, None))
    } else if target.is_file() {
        let name = utf8_name(&target)?;
        let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok((parent, Some(name)))
    } else {
        Err(invalid(
            ErrorCode::InvalidTarget,
            "path is neither a directory nor a regular file",
        ))
    }
}
fn depth(depth: u32) -> Result<u32, Error> {
    if (1..=MAX_DEPTH).contains(&depth) {
        Ok(depth)
    } else {
        Err(invalid(ErrorCode::InvalidInput, "depth must be 1..32"))
    }
}
fn pattern_text(pattern: &str) -> Result<(), Error> {
    if pattern.is_empty() || pattern.contains('\0') {
        return Err(invalid(
            ErrorCode::InvalidPattern,
            "pattern must not be empty",
        ));
    }
    Ok(())
}

pub fn prepare(
    request: &DiscoveryRequest,
    cwd: &Path,
    limits: &DiscoveryLimits,
) -> Result<Prepared, Error> {
    let job = match request {
        DiscoveryRequest::List {
            path,
            depth: d,
            include_ignored,
        } => {
            let d = depth(*d)?;
            let root = directory(path.as_deref(), cwd)?;
            DiscoveryJob::List(ListJob {
                in_repo: in_repo(&root),
                root: utf8(&root),
                depth: d,
                include_ignored: *include_ignored,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::Find {
            pattern,
            path,
            syntax,
            full_path,
            extensions,
            entry_type,
            depth: d,
            include_ignored,
            ignore_case,
        } => {
            let d = depth(*d)?;
            pattern_text(pattern)?;
            PathFilter::compile(*syntax, pattern, *ignore_case)?;
            if extensions
                .iter()
                .any(|e| e.is_empty() || e.starts_with('.') || e.contains(['/', '\0']))
            {
                return Err(invalid(
                    ErrorCode::InvalidInput,
                    "extension must be non-empty, without a leading dot",
                ));
            }
            if *entry_type == EntryFilter::Directory && !extensions.is_empty() {
                return Err(invalid(
                    ErrorCode::InvalidInput,
                    "--extension cannot be combined with --type directory",
                ));
            }
            let root = directory(path.as_deref(), cwd)?;
            DiscoveryJob::Find(FindJob {
                in_repo: in_repo(&root),
                root: utf8(&root),
                pattern: pattern.clone(),
                syntax: *syntax,
                full_path: *full_path,
                extensions: extensions.clone(),
                entry_type: *entry_type,
                depth: d,
                include_ignored: *include_ignored,
                ignore_case: *ignore_case,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::Search {
            pattern,
            path,
            fixed_strings,
            globs,
            context,
            ignore_case,
            include_ignored,
        } => {
            pattern_text(pattern)?;
            if pattern.contains(['\n', '\r']) {
                return Err(invalid(
                    ErrorCode::InvalidPattern,
                    "only single-line patterns are supported",
                ));
            }
            if !fixed_strings {
                regex::Regex::new(pattern).map_err(|_| {
                    invalid(ErrorCode::InvalidPattern, "invalid regular expression")
                })?;
            }
            GlobFilter::compile(globs)?;
            if *context > MAX_CONTEXT {
                return Err(invalid(ErrorCode::InvalidInput, "context must be 0..20"));
            }
            let (root, file) = directory_or_file(path.as_deref(), cwd)?;
            DiscoveryJob::Search(SearchJob {
                in_repo: in_repo(&root),
                root: utf8(&root),
                file,
                pattern: pattern.clone(),
                fixed_strings: *fixed_strings,
                globs: globs.clone(),
                context: *context,
                ignore_case: *ignore_case,
                include_ignored: *include_ignored,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::Read {
            file,
            from_line,
            lines,
        } => {
            if !(1..=i64::MAX as u64).contains(from_line)
                || !(1..=MAX_READ_LINES).contains(lines)
                || from_line
                    .checked_add(*lines as u64 - 1)
                    .is_none_or(|e| e > i64::MAX as u64)
            {
                return Err(invalid(
                    ErrorCode::InvalidInput,
                    "invalid line range: from-line 1.., lines 1..10000, no overflow",
                ));
            }
            let path = resolve(Some(file), cwd)?;
            let mut handle = open_regular(&path)?;
            let mut head = [0; 8192];
            let n = handle
                .read(&mut head)
                .map_err(|e| target_error(&e, &path))?;
            if head[..n].contains(&0) {
                return Err(invalid(
                    ErrorCode::UnsupportedEncoding,
                    "binary files cannot be read as text",
                ));
            }
            DiscoveryJob::Read(ReadJob {
                path: utf8(&path),
                from_line: *from_line,
                lines: *lines,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::CodeSearch {
            pattern,
            lang,
            path,
            include_ignored,
        } => {
            if pattern.is_empty() || lang.is_empty() {
                return Err(invalid(
                    ErrorCode::InvalidInput,
                    "pattern and language must not be empty",
                ));
            }
            if pattern.contains('\0') {
                return Err(invalid(
                    ErrorCode::InvalidPattern,
                    "pattern must not contain NUL",
                ));
            }
            let language = ast_grep::language(lang).ok_or_else(|| {
                invalid(
                    ErrorCode::UnsupportedLanguage,
                    "unsupported language; supported: rust, javascript, typescript, python, go",
                )
            })?;
            let (root, file) = directory_or_file(path.as_deref(), cwd)?;
            if file.as_deref().is_some_and(|f| !language.owns(f)) {
                return Err(invalid(
                    ErrorCode::InvalidInput,
                    "the file's extension does not belong to the language",
                ));
            }
            DiscoveryJob::CodeSearch(CodeSearchJob {
                in_repo: in_repo(&root),
                root: utf8(&root),
                file,
                pattern: pattern.clone(),
                lang: language.name.to_owned(),
                include_ignored: *include_ignored,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::CodeStats {
            path,
            include_ignored,
        } => {
            let (root, file) = directory_or_file(path.as_deref(), cwd)?;
            DiscoveryJob::CodeStats(CodeStatsJob {
                in_repo: in_repo(&root),
                root: utf8(&root),
                file,
                include_ignored: *include_ignored,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::DiskUsage {
            path,
            depth: d,
            top,
            size_mode,
        } => {
            let d = depth(*d)?;
            if !(1..=MAX_DISK_TOP).contains(top) {
                return Err(invalid(ErrorCode::InvalidInput, "top must be 1..1000"));
            }
            let (root, file) = disk_target(path.as_deref(), cwd)?;
            DiscoveryJob::DiskUsage(DiskUsageJob {
                root: utf8(&root),
                file,
                depth: d,
                top: *top,
                size_mode: *size_mode,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::CodeDiff { staged } => {
            tools::require_supported(&tools::GIT)?;
            let root = tools::code_diff::worktree_root(cwd)?;
            if root.to_str().is_none() {
                return Err(invalid(
                    ErrorCode::InvalidEncoding,
                    "path is not valid UTF-8",
                ));
            }
            DiscoveryJob::CodeDiff(CodeDiffJob {
                root: utf8(&root),
                staged: *staged,
                limits: limits.clone(),
            })
        }
        DiscoveryRequest::Processes { matching, limit } => {
            if !(1..=MAX_PROCESS_LIMIT).contains(limit) {
                return Err(invalid(ErrorCode::InvalidInput, "limit must be 1..1000"));
            }
            if let Some(text) = matching
                && (text.is_empty() || text.len() > MAX_PROCESS_MATCH || text.contains(['\0', '/']))
            {
                return Err(invalid(
                    ErrorCode::InvalidInput,
                    "match must be a non-empty name of at most 256 bytes, without '/'",
                ));
            }
            DiscoveryJob::Processes(ProcessesJob {
                matching: matching.clone(),
                limit: *limit,
                limits: limits.clone(),
            })
        }
    };
    let adapter = tools::adapter(&job);
    let plan = adapter.plan()?;
    JsonExecutionPlan::try_from(&plan)?;
    Ok(Prepared {
        tool: adapter.tool(),
        job,
        plan,
    })
}
/// The tool must be installed, and capable of what the job needs, before a task is
/// registered for it.
pub fn ensure_tool(job: &DiscoveryJob, plan: &ExecutionPlan) -> Result<(), Error> {
    if tools::adapter(job).is_native() {
        return tools::adapter(job).preflight();
    }
    tools::locate(&plan.program).ok_or_else(|| {
        Error::new(
            ErrorCode::MissingTool,
            format!("{} is not installed", plan.program.to_string_lossy()),
        )
    })?;
    tools::adapter(job).preflight()
}

/// Room kept for the header when truncation adds a reason to it.
const HEADER_SLACK: u64 = 512;
const STDERR_LIMIT: usize = 64 * 1024;

fn execute(
    plan: &ExecutionPlan,
    normalizer: &mut dyn Normalizer,
    deadline: Instant,
) -> (TargetExit, Vec<u8>) {
    let spawned = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return (
            TargetExit {
                spawn_failed: true,
                ..Default::default()
            },
            Vec::new(),
        );
    };
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let (tx, rx) = sync_channel::<Vec<u8>>(8);
    let out = thread::spawn(move || {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(n) = stdout.read(&mut buffer) {
            if n == 0 || tx.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let err = thread::spawn(move || {
        let (mut kept, mut buffer) = (Vec::new(), [0; 8192]);
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let room = STDERR_LIMIT.saturating_sub(kept.len()).min(n);
            kept.extend_from_slice(&buffer[..room]);
        }
        kept
    });
    let mut stopped = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            stopped = Some(Stop::Truncated(TruncationReason::Timeout));
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(chunk) => {
                if let Flow::Stop(stop) = normalizer.feed(&chunk) {
                    stopped = Some(stop);
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                stopped = Some(Stop::Truncated(TruncationReason::Timeout));
                break;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if stopped.is_some() {
        let _ = child.kill();
    }
    // A blocked reader must not outlive the target.
    drop(rx);
    let status = child.wait();
    let _ = out.join();
    let stderr = err.join().unwrap_or_default();
    (
        TargetExit {
            code: status
                .ok()
                .and_then(|s| s.code())
                .filter(|_| stopped.is_none()),
            spawn_failed: false,
            stopped,
        },
        stderr,
    )
}
/// Write header and as many items as the reservation allows; returns the artifact size.
fn write_artifact(
    file: &mut File,
    document: &mut DiscoveryDocument,
    budget: u64,
) -> Result<u64, Error> {
    let (mut header, items) = results::encode_document(document)?;
    let mut total = header.len() as u64 + HEADER_SLACK;
    let mut keep = items.len();
    for (i, line) in items.iter().enumerate() {
        if total + line.len() as u64 > budget {
            keep = i;
            break;
        }
        total += line.len() as u64;
    }
    if keep < items.len() {
        document.items.truncate(keep);
        let mut reasons = document.completeness.truncation_reasons.clone();
        if !reasons.contains(&TruncationReason::ArtifactBytes) {
            reasons.push(TruncationReason::ArtifactBytes);
        }
        document.completeness = Completeness::new(reasons, document.completeness.partial);
        if let DiscoverySummary::CodeStats { scope_complete, .. } = &mut document.summary {
            *scope_complete = false;
        }
        header = results::encode_document(document)?.0;
    }
    if header.len() as u64
        + items[..keep]
            .iter()
            .map(|line| line.len() as u64)
            .sum::<u64>()
        > budget
    {
        return Err(Error::new(
            ErrorCode::OutputLimitTooSmall,
            "result header exceeds the artifact reservation",
        ));
    }
    let failure = |_| Error::new(ErrorCode::StoreFailure, "result store unavailable");
    file.write_all(&header).map_err(failure)?;
    for line in &items[..keep] {
        file.write_all(line).map_err(failure)?;
    }
    file.sync_all().map_err(failure)?;
    Ok(file.metadata().map_err(failure)?.len())
}

/// Called only by the internal runner registered with Pueue: run the target, normalize its
/// output and publish the stored result. The returned status is the Pueue task's own.
pub fn run_worker(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
    job: &DiscoveryJob,
    plan: &ExecutionPlan,
) -> Result<i32, Error> {
    let (mut m, mut file) = results::reserve_or_record(store, settings, id)?;
    let budget = m.reserved();
    let adapter = tools::adapter(job);
    if adapter.is_native() {
        let deadline = Instant::now() + Duration::from_millis(job.limits().timeout_ms);
        let outcome = adapter
            .run_native(deadline)
            .unwrap_or_else(|| unreachable!());
        return finish_worker(store, id, job, &mut m, &mut file, budget, outcome);
    }
    let mut normalizer = match adapter.normalizer(budget.saturating_sub(HEADER_SLACK * 2)) {
        Ok(n) => n,
        Err(error) => {
            m.complete(job.kind(), ChildState::Failed, None);
            results::record_persistence_failure(store, id, &mut m, error);
            return Ok(1);
        }
    };
    let deadline = Instant::now() + Duration::from_millis(job.limits().timeout_ms);
    let (exit, stderr) = execute(plan, normalizer.as_mut(), deadline);
    let outcome = normalizer.finish(&exit, &stderr);
    finish_worker(store, id, job, &mut m, &mut file, budget, outcome)
}
fn finish_worker(
    store: &ExecutionStore,
    id: &ExecutionId,
    job: &DiscoveryJob,
    m: &mut results::Manifest,
    file: &mut File,
    budget: u64,
    outcome: DiscoveryOutcome,
) -> Result<i32, Error> {
    let (state, exit_code) = (outcome.state, outcome.exit_code);
    m.complete(job.kind(), state, exit_code);
    let mut document = outcome.document;
    let saved = write_artifact(file, &mut document, budget).and_then(|bytes| {
        results::publish_available(store, id, m, bytes, document.completeness.truncated)
    });
    if let Err(error) = saved {
        results::record_persistence_failure(store, id, m, error);
    }
    Ok(match state {
        ChildState::Done => 0,
        _ => exit_code.filter(|c| *c != 0).unwrap_or(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_truncation_updates_the_statistics_scope_flag() {
        let path = std::env::temp_dir().join(format!("axvern-stats-header-{}", std::process::id()));
        let mut file = File::create(&path).unwrap();
        let mut document = DiscoveryDocument {
            summary: DiscoverySummary::CodeStats {
                root: "/r".into(),
                scan_depth: Some(MAX_DEPTH),
                files: 1,
                code: 1,
                comments: 0,
                blanks: 0,
                scope_complete: true,
                skipped_files: None,
            },
            items: vec![DiscoveryItem::Stat(LanguageStatItem {
                language: "x".repeat(4096),
                files: 1,
                code: 1,
                comments: 0,
                blanks: 0,
                embedded: vec![],
            })],
            completeness: Completeness::new(vec![], false),
            errors: vec![],
            warnings: vec![],
        };
        write_artifact(&mut file, &mut document, 2048).unwrap();
        assert!(document.items.is_empty());
        assert!(document.completeness.truncated);
        assert!(matches!(
            document.summary,
            DiscoverySummary::CodeStats {
                scope_complete: false,
                ..
            }
        ));
        drop(file);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn oversized_header_is_rejected_before_writing() {
        let path = std::env::temp_dir().join(format!("axvern-header-{}", std::process::id()));
        let mut file = File::create(&path).unwrap();
        let mut document = DiscoveryDocument {
            summary: DiscoverySummary::Search {
                root: "/r".into(),
                scan_depth: Some(MAX_DEPTH),
                pattern: "x".repeat(4096),
                binary_files_skipped: 0,
            },
            items: Vec::new(),
            completeness: Completeness::new(Vec::new(), false),
            errors: Vec::new(),
            warnings: Vec::new(),
        };
        assert_eq!(
            write_artifact(&mut file, &mut document, 1024)
                .unwrap_err()
                .code,
            ErrorCode::OutputLimitTooSmall
        );
        assert_eq!(file.metadata().unwrap().len(), 0);
        drop(file);
        fs::remove_file(path).unwrap();
    }
}

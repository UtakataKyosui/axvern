//! Thin adapters from a [`DiscoveryJob`] to one external CLI and back.
//!
//! An adapter never builds a shell string: it returns an [`ExecutionPlan`] whose argv is
//! handed to the Policy and Pueue path unchanged. The [`Normalizer`] turns the target's
//! stdout into the stored [`DiscoveryDocument`]; it runs inside the background worker, so
//! reading a stored result never invokes the tool again. New tools (for example `tokei`
//! or `ast-grep`) add a [`DiscoveryJob`] variant, a [`ToolSpec`] and a [`DiscoveryAdapter`].
//!
//! Spike facts the adapters rely on (macOS, fd 10.5.0, ripgrep 15.2.0, bat 0.26.1):
//! - `fd` exits 0 on unreadable directories and only reports them with `--show-errors`;
//!   it cannot disable `.git/info/exclude`, `.ignore` or `.fdignore`; `--full-path` matches
//!   the absolute path, so root-relative full-path matching is done by the normalizer.
//! - `rg` exits 1 for "no match" and 2 when any file could not be read, even if it matched.
//! - `bat` passes file bytes through unchanged (BOM, CRLF, escapes) and succeeds past EOF.
//! - `eza` escapes names lossily (a literal backslash is not escaped) and cannot skip
//!   descending into `.git`, so it is not used to produce structured entries.
use crate::adapters::pueue::probe;
use axvern_models::*;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub mod bat;
pub mod fd;
pub mod rg;

/// Detection contract consumed by `doctor`.
#[derive(Clone, Copy, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub purpose: &'static str,
    /// Oldest release providing every flag the adapter uses.
    pub min_version: [u32; 3],
    /// Release the adapter's fixtures were recorded with.
    pub tested_version: &'static str,
}
pub const FD: ToolSpec = ToolSpec {
    name: "fd",
    purpose: "file discovery",
    min_version: [10, 0, 0],
    tested_version: "10.5.0",
};
pub const RG: ToolSpec = ToolSpec {
    name: "rg",
    purpose: "text search",
    min_version: [14, 0, 0],
    tested_version: "15.2.0",
};
pub const BAT: ToolSpec = ToolSpec {
    name: "bat",
    purpose: "file rendering",
    min_version: [0, 24, 0],
    tested_version: "0.26.1",
};
impl ToolSpec {
    pub fn supported_range(&self) -> String {
        let [a, b, c] = self.min_version;
        format!(">={a}.{b}.{c} (tested {})", self.tested_version)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Missing,
    /// The probe timed out or could not run.
    Unknown,
    Unsupported(String),
    Supported(String),
}
/// First `x.y.z` token of a `--version` output.
pub fn version_token(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()?
        .split_ascii_whitespace()
        .find_map(|word| {
            let version = word.strip_prefix('v').unwrap_or(word).split('-').next()?;
            let parts: Vec<_> = version.split('.').collect();
            (parts.len() == 3
                && parts.iter().all(|p| {
                    !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_digit())
                }))
            .then(|| version.to_owned())
        })
}
pub fn classify_version(spec: &ToolSpec, version: &str) -> ToolStatus {
    let parsed: Vec<u32> = version.split('.').filter_map(|p| p.parse().ok()).collect();
    match parsed.as_slice() {
        [a, b, c] if [*a, *b, *c] >= spec.min_version => ToolStatus::Supported(version.into()),
        _ => ToolStatus::Unsupported(version.into()),
    }
}
pub fn detect(spec: &ToolSpec, timeout: Duration) -> ToolStatus {
    match probe(Command::new(spec.name).arg("--version"), timeout) {
        Ok((true, bytes)) => match version_token(&bytes) {
            Some(version) => classify_version(spec, &version),
            None => ToolStatus::Unsupported("unknown".into()),
        },
        Ok((false, _)) => ToolStatus::Unsupported("unknown".into()),
        Err(e) if e.code == ErrorCode::MissingTool => ToolStatus::Missing,
        Err(_) => ToolStatus::Unknown,
    }
}
/// Resolve `program` on `PATH` without executing it.
pub fn locate(program: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if Path::new(program).components().count() > 1 {
        return executable(Path::new(program)).then(|| program.into());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| executable(p))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// A configured bound was reached; the stored result is flagged truncated.
    Truncated(TruncationReason),
    /// The requested data was fully read; the tool was stopped early and nothing is missing.
    Satisfied,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop(Stop),
}
/// How the target ended. `stopped` is set when the worker ended it on request of the normalizer
/// or on timeout; the exit status of a stopped tool is meaningless.
#[derive(Clone, Copy, Debug, Default)]
pub struct TargetExit {
    pub code: Option<i32>,
    pub spawn_failed: bool,
    pub stopped: Option<Stop>,
}
/// Stream parser for one target. `feed` sees stdout in arbitrary chunks; `finish` sees the
/// exit status and a bounded copy of stderr and returns the document to store.
pub trait Normalizer {
    fn feed(&mut self, chunk: &[u8]) -> Flow;
    fn finish(self: Box<Self>, exit: &TargetExit, stderr: &[u8]) -> DiscoveryOutcome;
}
pub trait DiscoveryAdapter {
    fn tool(&self) -> &'static ToolSpec;
    /// The exact argv to register; nothing here is interpreted by a shell.
    fn plan(&self) -> Result<ExecutionPlan, Error>;
    /// `byte_budget` bounds the serialized size of the stored items.
    fn normalizer(&self, byte_budget: u64) -> Result<Box<dyn Normalizer>, Error>;
}
pub fn adapter(job: &DiscoveryJob) -> Box<dyn DiscoveryAdapter> {
    match job {
        DiscoveryJob::List(j) => Box::new(fd::Fd::list(j.clone())),
        DiscoveryJob::Find(j) => Box::new(fd::Fd::find(j.clone())),
        DiscoveryJob::Search(j) => Box::new(rg::Rg(j.clone())),
        DiscoveryJob::Read(j) => Box::new(bat::Bat(j.clone())),
    }
}

/// Error and warning lists that stay small enough to be repeated on every result page.
#[derive(Default)]
pub(crate) struct Issues {
    pub errors: Vec<ResultIssue>,
    pub warnings: Vec<ResultWarning>,
}
impl Issues {
    pub fn error(&mut self, issue: ResultIssue) {
        if self.errors.len() + 1 < MAX_ISSUES {
            self.errors.push(issue);
        } else if self.errors.len() + 1 == MAX_ISSUES {
            self.errors.push(ResultIssue::new(
                ErrorCode::PartialResult,
                "further errors were omitted",
            ));
        }
    }
    pub fn warning(&mut self, code: WarningCode, message: &str, path: Option<String>) {
        if self.warnings.len() < MAX_ISSUES {
            self.warnings.push(ResultWarning {
                code,
                message: message.into(),
                path,
            });
        }
    }
}
/// Shortest valid-UTF-8 ancestor of a path whose own name is not valid UTF-8.
pub(crate) fn identifiable_parent(path: &[u8]) -> String {
    let valid = match std::str::from_utf8(path) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&path[..e.valid_up_to()]).unwrap_or(""),
    };
    match valid.rfind('/') {
        Some(i) if i > 0 => valid[..i].to_owned(),
        _ => ".".into(),
    }
}
/// Longest prefix of at most `max` bytes that ends on a character boundary.
pub(crate) fn cut_utf8(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}
/// Messages are fixed text: tool stderr is classified, never echoed.
pub(crate) fn classify_io_message(message: &str) -> ErrorCode {
    if message.contains("Permission denied") {
        ErrorCode::PermissionDenied
    } else if message.contains("No such file or directory") {
        ErrorCode::TargetNotFound
    } else {
        ErrorCode::BackendFailure
    }
}
pub(crate) fn missing_tool(spec: &ToolSpec) -> ResultIssue {
    ResultIssue::new(
        ErrorCode::MissingTool,
        format!("{} is not installed or not executable", spec.name),
    )
}

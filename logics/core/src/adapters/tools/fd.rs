//! `fd` backs both `fs list` (every entry below the root) and `fs find` (name filter).
use super::*;
use std::{ffi::OsString, fs, path::PathBuf};

/// Matcher used where fd cannot express the requested semantics (root-relative full path)
/// and by input validation; it uses the same engines as fd (`regex`, `globset`).
pub(crate) enum PathFilter {
    Regex(regex::Regex),
    Glob(globset::GlobMatcher),
}
impl PathFilter {
    pub(crate) fn compile(
        syntax: PatternSyntax,
        pattern: &str,
        ignore_case: bool,
    ) -> Result<Self, Error> {
        fn invalid<E>(_: E) -> Error {
            Error::new(ErrorCode::InvalidPattern, "invalid pattern")
        }
        Ok(match syntax {
            PatternSyntax::Regex => Self::Regex(
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(ignore_case)
                    .build()
                    .map_err(invalid)?,
            ),
            PatternSyntax::Glob => Self::Glob(
                globset::GlobBuilder::new(pattern)
                    .case_insensitive(ignore_case)
                    .literal_separator(true)
                    .build()
                    .map_err(invalid)?
                    .compile_matcher(),
            ),
        })
    }
    fn is_match(&self, relative: &str) -> bool {
        match self {
            Self::Regex(r) => r.is_match(relative),
            Self::Glob(g) => g.is_match(relative),
        }
    }
}

struct Scope {
    root: String,
    in_repo: bool,
    depth: u32,
    include_ignored: bool,
    limits: DiscoveryLimits,
}
pub struct Fd {
    scope: Scope,
    find: Option<FindJob>,
    summary: DiscoverySummary,
}
impl Fd {
    pub fn list(job: ListJob) -> Self {
        Self {
            summary: DiscoverySummary::List {
                root: job.root.clone(),
                depth: job.depth,
                include_ignored: job.include_ignored,
            },
            scope: Scope {
                root: job.root,
                in_repo: job.in_repo,
                depth: job.depth,
                include_ignored: job.include_ignored,
                limits: job.limits,
            },
            find: None,
        }
    }
    pub fn find(job: FindJob) -> Self {
        Self {
            summary: DiscoverySummary::Find {
                root: job.root.clone(),
                pattern: job.pattern.clone(),
                syntax: job.syntax,
                depth: job.depth,
                include_ignored: job.include_ignored,
            },
            scope: Scope {
                root: job.root.clone(),
                in_repo: job.in_repo,
                depth: job.depth,
                include_ignored: job.include_ignored,
                limits: job.limits.clone(),
            },
            find: Some(job),
        }
    }
}
impl DiscoveryAdapter for Fd {
    fn tool(&self) -> &'static ToolSpec {
        &FD
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let mut args: Vec<OsString> = [
            "--hidden",
            "--no-global-ignore-file",
            "--color",
            "never",
            "--show-errors",
            "--print0",
            "--strip-cwd-prefix=always",
            // A `.git` entry is reported, its contents are not.
            "--exclude",
            "**/.git/**",
            "--max-depth",
        ]
        .map(OsString::from)
        .into();
        args.push(self.scope.depth.to_string().into());
        if self.scope.include_ignored {
            args.push("--no-ignore".into());
        } else if !self.scope.in_repo {
            // Outside a repository only ignore files below the root apply.
            args.extend(["--no-require-git", "--no-ignore-parent"].map(OsString::from));
        }
        let pattern = match &self.find {
            None => "^".to_owned(),
            Some(f) => {
                if f.syntax == PatternSyntax::Glob && !f.full_path {
                    args.push("--glob".into());
                }
                args.push(
                    if f.ignore_case {
                        "--ignore-case"
                    } else {
                        "--case-sensitive"
                    }
                    .into(),
                );
                for extension in &f.extensions {
                    args.extend(["--extension".into(), extension.into()]);
                }
                match f.entry_type {
                    EntryFilter::Any => (),
                    EntryFilter::File => args.extend(["--type", "f"].map(OsString::from)),
                    EntryFilter::Directory => args.extend(["--type", "d"].map(OsString::from)),
                    EntryFilter::Symlink => args.extend(["--type", "l"].map(OsString::from)),
                }
                // fd would match the absolute path; the normalizer filters full-path matches.
                if f.full_path {
                    "^".to_owned()
                } else {
                    f.pattern.clone()
                }
            }
        };
        args.extend(["--".into(), pattern.into()]);
        Ok(ExecutionPlan {
            program: FD.name.into(),
            args,
            cwd: PathBuf::from(&self.scope.root),
        })
    }
    fn normalizer(&self, byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        let filter = match &self.find {
            Some(f) if f.full_path => {
                Some(PathFilter::compile(f.syntax, &f.pattern, f.ignore_case)?)
            }
            _ => None,
        };
        Ok(Box::new(FdNormalizer {
            root: PathBuf::from(&self.scope.root),
            summary: self.summary.clone(),
            limits: self.scope.limits.clone(),
            byte_budget,
            filter,
            pending: Vec::new(),
            skipping: false,
            entries: 0,
            bytes: 0,
            items: Vec::new(),
            issues: Issues::default(),
        }))
    }
}

/// A single path is never longer than this; a longer unterminated record is corrupt.
const RECORD_LIMIT: usize = 64 * 1024;
struct FdNormalizer {
    root: PathBuf,
    summary: DiscoverySummary,
    limits: DiscoveryLimits,
    byte_budget: u64,
    filter: Option<PathFilter>,
    pending: Vec<u8>,
    skipping: bool,
    entries: u64,
    bytes: u64,
    items: Vec<EntryItem>,
    issues: Issues,
}
pub(crate) fn entry_type(meta: &fs::Metadata) -> EntryType {
    let kind = meta.file_type();
    if kind.is_symlink() {
        EntryType::Symlink
    } else if kind.is_dir() {
        EntryType::Directory
    } else if kind.is_file() {
        EntryType::File
    } else {
        EntryType::Other
    }
}
impl FdNormalizer {
    fn record(&mut self, raw: &[u8]) -> Flow {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Flow::Stop(Stop::Truncated(TruncationReason::MaxEntries));
        }
        // fd marks directories with a trailing separator.
        let raw = raw.strip_suffix(b"/").unwrap_or(raw);
        if raw.is_empty() {
            return Flow::Continue;
        }
        let Ok(relative) = std::str::from_utf8(raw) else {
            self.issues.error(
                ResultIssue::new(
                    ErrorCode::InvalidEncoding,
                    "an entry name below this path is not valid UTF-8",
                )
                .at(identifiable_parent(raw)),
            );
            return Flow::Continue;
        };
        if self.filter.as_ref().is_some_and(|f| !f.is_match(relative)) {
            return Flow::Continue;
        }
        if self.items.len() as u64 >= self.limits.max_items {
            return Flow::Stop(Stop::Truncated(TruncationReason::MaxItems));
        }
        let size = relative.len() as u64 + 48;
        if self.bytes + size > self.byte_budget {
            return Flow::Stop(Stop::Truncated(TruncationReason::ArtifactBytes));
        }
        let entry_type = match fs::symlink_metadata(self.root.join(relative)) {
            Ok(meta) => entry_type(&meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.issues.warning(
                    WarningCode::EntryVanished,
                    "the entry disappeared during the scan",
                    Some(relative.to_owned()),
                );
                return Flow::Continue;
            }
            Err(e) => {
                let code = if e.kind() == std::io::ErrorKind::PermissionDenied {
                    ErrorCode::PermissionDenied
                } else {
                    ErrorCode::BackendFailure
                };
                self.issues
                    .error(ResultIssue::new(code, "cannot inspect the entry").at(relative));
                return Flow::Continue;
            }
        };
        self.bytes += size;
        self.items.push(EntryItem {
            path: relative.to_owned(),
            entry_type,
        });
        Flow::Continue
    }
    fn stderr(&mut self, stderr: &[u8]) {
        for line in String::from_utf8_lossy(stderr).lines() {
            let Some(rest) = line.strip_prefix("[fd error]: ") else {
                continue;
            };
            let (path, message) = rest.rsplit_once(": ").unwrap_or(("", rest));
            let path = path.strip_prefix("./").unwrap_or(path);
            match classify_io_message(message) {
                ErrorCode::PermissionDenied => self.issues.error(
                    ResultIssue::new(ErrorCode::PermissionDenied, "directory is not readable")
                        .at(if path.is_empty() { "." } else { path }),
                ),
                ErrorCode::TargetNotFound => self.issues.warning(
                    WarningCode::EntryVanished,
                    "the entry disappeared during the scan",
                    Some(path.to_owned()),
                ),
                _ => (),
            }
        }
    }
}
impl Normalizer for FdNormalizer {
    fn feed(&mut self, chunk: &[u8]) -> Flow {
        for byte in chunk {
            if *byte == 0 {
                let record = std::mem::take(&mut self.pending);
                if std::mem::take(&mut self.skipping) {
                    continue;
                }
                if let Flow::Stop(stop) = self.record(&record) {
                    return Flow::Stop(stop);
                }
            } else if !self.skipping {
                self.pending.push(*byte);
                if self.pending.len() > RECORD_LIMIT {
                    self.pending.clear();
                    self.skipping = true;
                    self.issues.error(ResultIssue::new(
                        ErrorCode::BackendFailure,
                        "an overlong path was skipped",
                    ));
                }
            }
        }
        Flow::Continue
    }
    fn finish(mut self: Box<Self>, exit: &TargetExit, stderr: &[u8]) -> DiscoveryOutcome {
        self.stderr(stderr);
        let mut state = ChildState::Done;
        let mut reasons = Vec::new();
        if exit.spawn_failed {
            state = ChildState::Failed;
            self.issues.error(missing_tool(&FD));
        } else if let Some(Stop::Truncated(reason)) = exit.stopped {
            reasons.push(reason);
        } else if exit.stopped.is_none() && exit.code != Some(0) {
            state = ChildState::Failed;
            let text = String::from_utf8_lossy(stderr);
            self.issues.error(
                if ["regex parse error", "invalid glob", "error: invalid value"]
                    .iter()
                    .any(|m| text.contains(m))
                {
                    ResultIssue::new(ErrorCode::InvalidPattern, "fd rejected the pattern")
                } else {
                    ResultIssue::new(ErrorCode::BackendFailure, "fd failed")
                },
            );
        }
        self.items.sort_by(|a, b| a.path.cmp(&b.path));
        let partial = !self.issues.errors.is_empty();
        DiscoveryOutcome {
            state,
            exit_code: exit.code,
            document: DiscoveryDocument {
                summary: self.summary,
                items: self.items.into_iter().map(DiscoveryItem::Entry).collect(),
                completeness: Completeness::new(reasons, partial),
                errors: self.issues.errors,
                warnings: self.issues.warnings,
            },
        }
    }
}

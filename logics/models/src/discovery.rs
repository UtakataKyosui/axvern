//! Discovery contracts: requests, resolved jobs, result items and the stored result document.
//! A job is the serializable input of one background investigation; it is never a shell string.
use crate::{ChildState, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_DEPTH: u32 = 32;
pub const MAX_CONTEXT: u32 = 20;
pub const DEFAULT_READ_LINES: u32 = 80;
pub const MAX_READ_LINES: u32 = 10_000;
/// Largest stored text of one source line.
pub const LINE_BYTES: usize = 16 * 1024;
/// Context lines are shortened more strongly so one search item always fits a result page.
pub const CONTEXT_LINE_BYTES: usize = 512;
/// Largest stored body of `fs read`.
pub const READ_BODY_BYTES: u64 = 1024 * 1024;
/// Bounds on the issue lists stored with a result, so that every page carries them.
pub const MAX_ISSUES: usize = 50;
/// Largest stored text of one `code search` match.
pub const SNIPPET_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryLimits {
    pub max_items: u64,
    pub max_entries: u64,
    pub timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternSyntax {
    Regex,
    Glob,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryFilter {
    Any,
    File,
    Directory,
    Symlink,
}

/// User intent before any path is resolved. The core turns it into a [`DiscoveryJob`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryRequest {
    List {
        path: Option<PathBuf>,
        depth: u32,
        include_ignored: bool,
    },
    Find {
        pattern: String,
        path: Option<PathBuf>,
        syntax: PatternSyntax,
        full_path: bool,
        extensions: Vec<String>,
        entry_type: EntryFilter,
        depth: u32,
        include_ignored: bool,
        ignore_case: bool,
    },
    Search {
        pattern: String,
        path: Option<PathBuf>,
        fixed_strings: bool,
        globs: Vec<String>,
        context: u32,
        ignore_case: bool,
        include_ignored: bool,
    },
    Read {
        file: PathBuf,
        from_line: u64,
        lines: u32,
    },
    CodeSearch {
        pattern: String,
        lang: String,
        path: Option<PathBuf>,
        include_ignored: bool,
    },
    CodeStats {
        path: Option<PathBuf>,
        include_ignored: bool,
    },
}

/// `root` is a resolved absolute UTF-8 directory. `in_repo` records whether a Git
/// worktree contains it, which selects how ignore files above the root are treated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListJob {
    pub root: String,
    pub in_repo: bool,
    pub depth: u32,
    pub include_ignored: bool,
    pub limits: DiscoveryLimits,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindJob {
    pub root: String,
    pub in_repo: bool,
    pub pattern: String,
    pub syntax: PatternSyntax,
    pub full_path: bool,
    pub extensions: Vec<String>,
    pub entry_type: EntryFilter,
    pub depth: u32,
    pub include_ignored: bool,
    pub ignore_case: bool,
    pub limits: DiscoveryLimits,
}
/// `file` is the basename inside `root` when the searched path is a single file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchJob {
    pub root: String,
    pub file: Option<String>,
    pub in_repo: bool,
    pub pattern: String,
    pub fixed_strings: bool,
    pub globs: Vec<String>,
    pub context: u32,
    pub ignore_case: bool,
    pub include_ignored: bool,
    pub limits: DiscoveryLimits,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadJob {
    pub path: String,
    pub from_line: u64,
    pub lines: u32,
    pub limits: DiscoveryLimits,
}
/// `lang` is a name from the adapter's language table. `file` is the basename inside `root`
/// when the searched path is a single file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeSearchJob {
    pub root: String,
    pub file: Option<String>,
    pub in_repo: bool,
    pub pattern: String,
    pub lang: String,
    pub include_ignored: bool,
    pub limits: DiscoveryLimits,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeStatsJob {
    pub root: String,
    pub file: Option<String>,
    pub in_repo: bool,
    pub include_ignored: bool,
    pub limits: DiscoveryLimits,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum DiscoveryJob {
    List(ListJob),
    Find(FindJob),
    Search(SearchJob),
    Read(ReadJob),
    CodeSearch(CodeSearchJob),
    CodeStats(CodeStatsJob),
}
impl DiscoveryJob {
    /// Public command name, as reported in the response envelope.
    pub fn command(&self) -> &'static str {
        match self {
            Self::List(_) => "fs list",
            Self::Find(_) => "fs find",
            Self::Search(_) => "fs search",
            Self::Read(_) => "fs read",
            Self::CodeSearch(_) => "code search",
            Self::CodeStats(_) => "code stats",
        }
    }
    /// Result kind, as reported in the stored result.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::List(_) => "fs.list",
            Self::Find(_) => "fs.find",
            Self::Search(_) => "fs.search",
            Self::Read(_) => "fs.read",
            Self::CodeSearch(_) => "code.search",
            Self::CodeStats(_) => "code.stats",
        }
    }
    pub fn limits(&self) -> &DiscoveryLimits {
        match self {
            Self::List(j) => &j.limits,
            Self::Find(j) => &j.limits,
            Self::Search(j) => &j.limits,
            Self::Read(j) => &j.limits,
            Self::CodeSearch(j) => &j.limits,
            Self::CodeStats(j) => &j.limits,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryItem {
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: EntryType,
}
/// 1-based UTF-8 byte columns; `end_byte` is exclusive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start_byte: u64,
    pub end_byte: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLine {
    pub line: u64,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchItem {
    pub path: String,
    pub line: u64,
    pub text: String,
    pub ranges: Vec<ByteRange>,
    pub context_before: Vec<ContextLine>,
    pub context_after: Vec<ContextLine>,
    pub line_truncated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineItem {
    pub line: u64,
    pub text: String,
    pub line_truncated: bool,
}
/// 1-based line and 1-based UTF-8 byte column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CodePosition {
    pub line: u64,
    pub byte_column: u64,
}
/// `end` is exclusive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeMatchItem {
    pub path: String,
    pub lang: String,
    pub start: CodePosition,
    pub end: CodePosition,
    pub snippet: String,
    pub snippet_truncated: bool,
}
/// Counts of one language embedded in a host language (for example JavaScript in HTML).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedStat {
    pub language: String,
    pub files: u64,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
}
/// Lines of the files whose own language is `language`; embedded lines are listed separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageStatItem {
    pub language: String,
    pub files: u64,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
    pub embedded: Vec<EmbeddedStat>,
}
/// Variants are told apart by their required fields, so the order does not matter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiscoveryItem {
    Match(MatchItem),
    Line(LineItem),
    Entry(EntryItem),
    Code(CodeMatchItem),
    Stat(LanguageStatItem),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiscoverySummary {
    Find {
        root: String,
        pattern: String,
        syntax: PatternSyntax,
        depth: u32,
        include_ignored: bool,
    },
    Search {
        root: String,
        pattern: String,
        binary_files_skipped: u64,
    },
    Read {
        path: String,
        from_line: u64,
        requested_lines: u32,
        eof: bool,
    },
    List {
        root: String,
        depth: u32,
        include_ignored: bool,
    },
    CodeSearch {
        root: String,
        lang: String,
        pattern: String,
    },
    CodeStats {
        root: String,
        files: u64,
        code: u64,
        comments: u64,
        blanks: u64,
        scope_complete: bool,
        /// Null: tokei does not report the files it ignored (unknown language, binary).
        skipped_files: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationReason {
    MaxItems,
    MaxEntries,
    Timeout,
    BodyBytes,
    ArtifactBytes,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completeness {
    /// True only when the whole requested depth/filter scope was examined.
    pub scope_complete: bool,
    pub truncated: bool,
    pub truncation_reasons: Vec<TruncationReason>,
    /// Null when the amount left out is unknown.
    pub omitted_count: Option<u64>,
    /// Some part of the scope could not be read; see `errors`.
    pub partial: bool,
}
impl Completeness {
    pub fn new(reasons: Vec<TruncationReason>, partial: bool) -> Self {
        let truncated = !reasons.is_empty();
        Self {
            scope_complete: !truncated && !partial,
            truncated,
            omitted_count: if truncated { None } else { Some(0) },
            truncation_reasons: reasons,
            partial,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultIssue {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}
impl ResultIssue {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
            line: None,
        }
    }
    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    SourceChanged,
    EntryVanished,
    BinaryFilesSkipped,
    /// The tool cannot apply ignore files outside a Git worktree.
    IgnoreRulesNotApplied,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultWarning {
    pub code: WarningCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
/// Everything a stored discovery result carries. Items are ordered and bounded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryDocument {
    pub summary: DiscoverySummary,
    pub items: Vec<DiscoveryItem>,
    pub completeness: Completeness,
    pub errors: Vec<ResultIssue>,
    pub warnings: Vec<ResultWarning>,
}
/// Target outcome after normalization. `exit_code` is the target's own exit status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryOutcome {
    pub state: ChildState,
    pub exit_code: Option<i32>,
    pub document: DiscoveryDocument,
}

//! Contracts for reading logs that Pueue still holds. No I/O implementation lives here.
use crate::Error;
use serde::Serialize;
use std::io::Read;

/// `query` input limits (FR-QUERY-009). The scan budget bounds work on a huge log.
pub const QUERY_DEFAULT_LIMIT: usize = 100;
pub const QUERY_MAX_LIMIT: usize = 1000;
pub const QUERY_DEFAULT_MAX_BYTES: usize = 65536;
pub const QUERY_MIN_MAX_BYTES: usize = 1024;
pub const QUERY_MAX_LINES: u64 = 1000;
pub const QUERY_MATCH_MAX_BYTES: usize = 1024;
pub const QUERY_SCAN_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const QUERY_SCAN_SECONDS: u64 = 10;
/// Version of the deterministic `--errors` word list.
pub const ERRORS_RULE_VERSION: u32 = 1;
/// `recall` inline payload and `--output` limits.
pub const RECALL_INLINE_BYTES: usize = 65536;
pub const RECALL_DEFAULT_OUTPUT_BYTES: u64 = 1 << 30;
pub const RECALL_MAX_OUTPUT_BYTES: u64 = 10 << 30;
pub const RECALL_DEFAULT_DEADLINE_SECONDS: u64 = 300;

/// What the Reader could tell about the retained log of one execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogAvailability {
    Available,
    /// The task still runs; the snapshot may be shorter than the final log.
    Partial,
    Missing,
    Unreadable,
}
/// Pueue merges stdout and stderr into one file; they cannot be told apart afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Combined,
}

/// A readable view of one log, fixed at open time. `Read` yields at most `snapshot_len` bytes.
pub struct LogHandle {
    pub location: String,
    pub snapshot_len: u64,
    pub reader: Box<dyn LogCursor>,
}
pub trait LogCursor: Read {
    /// Fails with `LogChanged` when the log shrank, or (when `exact`) changed at all, since open.
    fn verify_unchanged(&mut self, exact: bool) -> Result<(), Error>;
}
/// Opens the retained log of a Pueue task. Never registers or re-runs a command.
pub trait LogSource {
    fn open(&self, task_id: u64) -> Result<LogHandle, Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuerySelector {
    Match(String),
    Errors,
    Tail(u64),
    Range { from_line: u64, lines: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuerySpec {
    pub selector: QuerySelector,
    pub limit: usize,
    pub max_bytes: usize,
}
impl QuerySpec {
    pub fn validate(&self) -> Result<(), Error> {
        use crate::ErrorCode::InvalidInput;
        let bad = |m: &str| Err(Error::new(InvalidInput, m));
        if !(1..=QUERY_MAX_LIMIT).contains(&self.limit) {
            return bad("limit must be 1..1000");
        }
        if !(QUERY_MIN_MAX_BYTES..=QUERY_DEFAULT_MAX_BYTES).contains(&self.max_bytes) {
            return bad("max-bytes must be 1024..65536");
        }
        match &self.selector {
            QuerySelector::Match(text) if text.is_empty() || text.contains('\0') => {
                bad("match text must be non-empty")
            }
            QuerySelector::Match(text) if text.len() > QUERY_MATCH_MAX_BYTES => {
                bad("match text is too long")
            }
            QuerySelector::Tail(n) if !(1..=QUERY_MAX_LINES).contains(n) => {
                bad("tail must be 1..1000")
            }
            QuerySelector::Range { from_line, lines }
                if *from_line < 1 || !(1..=QUERY_MAX_LINES).contains(lines) =>
            {
                bad("from-line must be >= 1 and lines 1..1000")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueryItem {
    /// 1-based line number in the original log.
    pub line: u64,
    pub text: String,
    /// The line exceeded the per-line byte cap; `text` is its prefix.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub line_truncated: bool,
}

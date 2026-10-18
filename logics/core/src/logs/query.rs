//! Deterministic, bounded extraction from a log snapshot (FR-QUERY-002..006, 009, 013).
//!
//! The log is streamed once. Memory is bounded by the per-line cap, the item limits and the
//! output byte budget, never by the log size.
use axvern_models::*;
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct ScanBudget {
    pub max_bytes: u64,
    pub timeout: Duration,
}
impl Default for ScanBudget {
    fn default() -> Self {
        Self {
            max_bytes: QUERY_SCAN_MAX_BYTES,
            timeout: Duration::from_secs(QUERY_SCAN_SECONDS),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct QueryScan {
    pub items: Vec<QueryItem>,
    /// Some selected lines are not in `items`, or the scan did not finish.
    pub truncated: bool,
    /// `None` when the scan stopped before the remaining count could be known.
    pub omitted_count: Option<u64>,
    pub scan_complete: bool,
    pub scanned_bytes: u64,
}

struct Line {
    number: u64,
    text: String,
    truncated: bool,
}
enum Next {
    Line(Line),
    End,
    /// Scan budget (bytes or time) exhausted before the next line was complete.
    Stopped,
}
struct Lines<R: Read> {
    reader: BufReader<R>,
    cap: usize,
    number: u64,
    scanned: u64,
    started: Instant,
    budget: ScanBudget,
}
fn io_failure(error: &std::io::Error) -> Error {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            Error::new(ErrorCode::PermissionDenied, "the log is not readable")
        }
        _ => Error::new(ErrorCode::BackendFailure, "cannot read the log"),
    }
}
impl<R: Read> Lines<R> {
    fn new(reader: R, cap: usize, budget: ScanBudget) -> Self {
        Self {
            reader: BufReader::with_capacity(64 * 1024, reader),
            cap,
            number: 0,
            scanned: 0,
            started: Instant::now(),
            budget,
        }
    }
    /// `\n` and `\r\n` end a line; a last line without a terminator is still a line.
    fn next(&mut self) -> Result<Next, Error> {
        let mut kept: Vec<u8> = Vec::new();
        let mut dropped = false;
        let mut any = false;
        loop {
            if self.scanned >= self.budget.max_bytes
                || self.started.elapsed() >= self.budget.timeout
            {
                return Ok(Next::Stopped);
            }
            let chunk = self.reader.fill_buf().map_err(|e| io_failure(&e))?;
            if chunk.is_empty() {
                if !any {
                    return Ok(Next::End);
                }
                break;
            }
            any = true;
            let room = usize::try_from(self.budget.max_bytes - self.scanned).unwrap_or(usize::MAX);
            let window = &chunk[..chunk.len().min(room)];
            let (segment, ended) = match window.iter().position(|b| *b == b'\n') {
                Some(at) => (&window[..at], Some(at + 1)),
                None => (window, None),
            };
            let space = self.cap.saturating_sub(kept.len());
            kept.extend_from_slice(&segment[..segment.len().min(space)]);
            dropped |= segment.len() > space;
            let consumed = ended.unwrap_or(window.len());
            self.scanned += consumed as u64;
            self.reader.consume(consumed);
            if ended.is_some() {
                break;
            }
        }
        self.number += 1;
        if !dropped && kept.last() == Some(&b'\r') {
            kept.pop();
        }
        let text = decode(kept, dropped, self.number)?;
        Ok(Next::Line(Line {
            number: self.number,
            text,
            truncated: dropped,
        }))
    }
}
/// Valid UTF-8 text only. A cut that splits a code point is repaired; anything else is an error.
fn decode(mut bytes: Vec<u8>, cut: bool, number: u64) -> Result<String, Error> {
    let invalid = || {
        Error::new(
            ErrorCode::InvalidEncoding,
            format!(
                "the log is not valid UTF-8 text (line {number}); use recall --output to save the raw bytes"
            ),
        )
    };
    if bytes.contains(&0) {
        return Err(invalid());
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) => {
            let valid = error.utf8_error().valid_up_to();
            let incomplete_tail = error.utf8_error().error_len().is_none();
            bytes = error.into_bytes();
            if cut && incomplete_tail {
                bytes.truncate(valid);
                String::from_utf8(bytes).map_err(|_| invalid())
            } else {
                Err(invalid())
            }
        }
    }
}
/// Independent word `error`, `failed`, `fatal` or `panic`, ASCII case-insensitive (rule version 1).
pub fn is_error_candidate(line: &str) -> bool {
    const WORDS: [&str; 4] = ["error", "failed", "fatal", "panic"];
    let bytes = line.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        if word(bytes[i]) {
            let start = i;
            while i < bytes.len() && word(bytes[i]) {
                i += 1;
            }
            if WORDS
                .iter()
                .any(|w| bytes[start..i].eq_ignore_ascii_case(w.as_bytes()))
            {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}
struct Collector {
    limit: usize,
    max_bytes: usize,
    items: Vec<QueryItem>,
    bytes: usize,
    full: bool,
    omitted: u64,
}
impl Collector {
    fn new(spec: &QuerySpec) -> Self {
        Self {
            limit: spec.limit,
            max_bytes: spec.max_bytes,
            items: Vec::new(),
            bytes: 0,
            full: false,
            omitted: 0,
        }
    }
    /// Once one selected line did not fit, later ones are counted, never reordered in.
    fn offer(&mut self, line: Line) {
        if self.full
            || self.items.len() >= self.limit
            || self.bytes + line.text.len() > self.max_bytes
        {
            self.full = true;
            self.omitted += 1;
            return;
        }
        self.bytes += line.text.len();
        self.items.push(QueryItem {
            line: line.number,
            text: line.text,
            line_truncated: line.truncated,
        });
    }
}
pub fn scan(reader: impl Read, spec: &QuerySpec, budget: &ScanBudget) -> Result<QueryScan, Error> {
    spec.validate()?;
    let mut lines = Lines::new(reader, spec.max_bytes, *budget);
    match &spec.selector {
        QuerySelector::Tail(n) => scan_tail(&mut lines, spec, *n),
        selector => {
            let mut collector = Collector::new(spec);
            let complete = loop {
                match lines.next()? {
                    Next::End => break true,
                    Next::Stopped => break false,
                    Next::Line(line) => {
                        let hit = match selector {
                            QuerySelector::Match(needle) => line.text.contains(needle.as_str()),
                            QuerySelector::Errors => is_error_candidate(&line.text),
                            QuerySelector::Range { from_line, lines } => {
                                line.number >= *from_line && line.number - from_line < *lines
                            }
                            QuerySelector::Tail(_) => unreachable!("handled above"),
                        };
                        if hit {
                            collector.offer(line);
                        } else if let QuerySelector::Range { from_line, lines } = selector
                            && line.number >= from_line + lines
                        {
                            break true;
                        }
                    }
                }
            };
            Ok(QueryScan {
                truncated: collector.omitted > 0 || !complete,
                // After an early stop of a range scan the remaining count is exactly known.
                omitted_count: complete.then_some(collector.omitted),
                items: collector.items,
                scan_complete: complete,
                scanned_bytes: lines.scanned,
            })
        }
    }
}
/// The last `n` lines. Exact line numbers need the whole log, so an unfinished scan yields none.
fn scan_tail<R: Read>(lines: &mut Lines<R>, spec: &QuerySpec, n: u64) -> Result<QueryScan, Error> {
    let window = usize::try_from(n).unwrap_or(usize::MAX);
    let mut ring: VecDeque<(u64, Option<(String, bool)>)> = VecDeque::new();
    // Entries whose text was dropped always form a prefix of the ring.
    let (mut blank, mut kept, mut bytes) = (0usize, 0usize, 0usize);
    let complete = loop {
        match lines.next()? {
            Next::End => break true,
            Next::Stopped => break false,
            Next::Line(line) => {
                bytes += line.text.len();
                kept += 1;
                ring.push_back((line.number, Some((line.text, line.truncated))));
                if ring.len() > window {
                    match ring.pop_front() {
                        Some((_, Some((text, _)))) => {
                            kept -= 1;
                            bytes -= text.len();
                        }
                        Some((_, None)) => blank -= 1,
                        None => {}
                    }
                }
                while kept > spec.limit || bytes > spec.max_bytes {
                    if let Some((_, slot)) = ring.get_mut(blank)
                        && let Some((text, _)) = slot.take()
                    {
                        kept -= 1;
                        bytes -= text.len();
                        blank += 1;
                    } else {
                        break;
                    }
                }
            }
        }
    };
    if !complete {
        return Ok(QueryScan {
            items: Vec::new(),
            truncated: true,
            omitted_count: None,
            scan_complete: false,
            scanned_bytes: lines.scanned,
        });
    }
    let items: Vec<_> = ring
        .iter()
        .filter_map(|(number, slot)| {
            slot.as_ref().map(|(text, truncated)| QueryItem {
                line: *number,
                text: text.clone(),
                line_truncated: *truncated,
            })
        })
        .collect();
    let omitted = (ring.len() - items.len()) as u64;
    Ok(QueryScan {
        items,
        truncated: omitted > 0,
        omitted_count: Some(omitted),
        scan_complete: true,
        scanned_bytes: lines.scanned,
    })
}

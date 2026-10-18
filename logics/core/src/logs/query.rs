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
    /// Whether the selector's predicate held anywhere in the whole line, kept or not.
    hit: bool,
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
    probe: Probe,
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
    fn new(reader: R, cap: usize, budget: ScanBudget, probe: Probe) -> Self {
        Self {
            reader: BufReader::with_capacity(64 * 1024, reader),
            cap,
            number: 0,
            scanned: 0,
            started: Instant::now(),
            budget,
            probe,
        }
    }
    /// `\n` and `\r\n` end a line; a last line without a terminator is still a line.
    fn next(&mut self) -> Result<Next, Error> {
        let mut kept: Vec<u8> = Vec::new();
        let mut dropped = false;
        let mut any = false;
        let mut probe = self.probe.fresh();
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
            // The predicate sees every byte of the line; only the output copy is capped.
            probe.feed(segment);
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
            hit: probe.finish(),
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
    let mut probe = Probe::Errors(ErrorWords::default());
    probe.feed(line.as_bytes());
    probe.finish()
}
#[derive(Clone, Default)]
pub struct ErrorWords {
    word: [u8; 7],
    len: usize,
    hit: bool,
}
impl ErrorWords {
    fn close(&mut self) {
        const WORDS: [&str; 4] = ["error", "failed", "fatal", "panic"];
        if self.len > 0
            && self.len <= 6
            && WORDS
                .iter()
                .any(|w| self.word[..self.len].eq_ignore_ascii_case(w.as_bytes()))
        {
            self.hit = true;
        }
        self.len = 0;
    }
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b.is_ascii_alphanumeric() || b == b'_' {
                if self.len < self.word.len() {
                    self.word[self.len] = b;
                }
                self.len = self.len.saturating_add(1);
            } else {
                self.close();
            }
        }
    }
}
/// Streaming per-line predicate, so a match beyond the retained prefix is still found.
#[derive(Clone)]
pub enum Probe {
    Off,
    Literal {
        needle: Vec<u8>,
        carry: Vec<u8>,
        hit: bool,
    },
    Errors(ErrorWords),
}
impl Probe {
    fn for_selector(selector: &QuerySelector) -> Self {
        match selector {
            QuerySelector::Match(text) => Probe::Literal {
                needle: text.as_bytes().to_vec(),
                carry: Vec::new(),
                hit: false,
            },
            QuerySelector::Errors => Probe::Errors(ErrorWords::default()),
            _ => Probe::Off,
        }
    }
    fn fresh(&self) -> Self {
        match self {
            Probe::Off => Probe::Off,
            Probe::Literal { needle, .. } => Probe::Literal {
                needle: needle.clone(),
                carry: Vec::new(),
                hit: false,
            },
            Probe::Errors(_) => Probe::Errors(ErrorWords::default()),
        }
    }
    fn feed(&mut self, bytes: &[u8]) {
        match self {
            Probe::Off => {}
            Probe::Errors(words) => words.feed(bytes),
            Probe::Literal { needle, carry, hit } => {
                if *hit {
                    return;
                }
                let mut buffer = std::mem::take(carry);
                buffer.extend_from_slice(bytes);
                let first = needle[0];
                let mut at = 0;
                while let Some(offset) = buffer[at..].iter().position(|b| *b == first) {
                    at += offset;
                    if buffer[at..].starts_with(needle) {
                        *hit = true;
                        return;
                    }
                    at += 1;
                }
                // Keep only what could still start a match that spans into the next chunk.
                let keep = needle.len() - 1;
                *carry = buffer[buffer.len().saturating_sub(keep)..].to_vec();
            }
        }
    }
    fn finish(mut self) -> bool {
        match &mut self {
            Probe::Off => false,
            Probe::Literal { hit, .. } => *hit,
            Probe::Errors(words) => {
                words.close();
                words.hit
            }
        }
    }
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
    let mut lines = Lines::new(
        reader,
        spec.max_bytes,
        *budget,
        Probe::for_selector(&spec.selector),
    );
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
                            QuerySelector::Match(_) | QuerySelector::Errors => line.hit,
                            QuerySelector::Range { from_line, lines } => {
                                line.number >= *from_line && line.number - from_line < *lines
                            }
                            QuerySelector::Tail(_) => unreachable!("handled above"),
                        };
                        // The last requested line ends the scan: the next line is never read.
                        let last = matches!(selector, QuerySelector::Range { from_line, lines }
                            if line.number >= *from_line && line.number - *from_line + 1 >= *lines);
                        if hit {
                            collector.offer(line);
                        }
                        if last {
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

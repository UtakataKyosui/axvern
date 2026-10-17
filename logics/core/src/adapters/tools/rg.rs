//! `rg --json` backs `fs search`. Positive `--glob` filters are applied here, not by rg:
//! an rg glob overrides ignore rules and would search ignored files.
use super::scope::FileScope;
use super::*;
use serde_json::Value;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

/// Gitignore-style globs as rg reads them: positives are ORed, negatives always exclude.
pub(crate) struct GlobFilter {
    include: Vec<globset::GlobMatcher>,
    exclude: Vec<globset::GlobMatcher>,
}
impl GlobFilter {
    pub(crate) fn compile(globs: &[String]) -> Result<Self, Error> {
        let (mut include, mut exclude) = (Vec::new(), Vec::new());
        for glob in globs {
            let (negated, text) = match glob.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, glob.as_str()),
            };
            let text = text.strip_suffix('/').unwrap_or(text);
            let anchored = text.starts_with('/') || text.contains('/');
            let text = text.strip_prefix('/').unwrap_or(text);
            let pattern = if anchored {
                text.to_owned()
            } else {
                format!("**/{text}")
            };
            if text.is_empty() {
                return Err(Error::new(ErrorCode::InvalidPattern, "empty glob"));
            }
            let matcher = globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|_| Error::new(ErrorCode::InvalidPattern, "invalid glob"))?
                .compile_matcher();
            if negated {
                exclude.push(matcher);
            } else {
                include.push(matcher);
            }
        }
        Ok(Self { include, exclude })
    }
    /// A glob that matches a directory covers everything below it.
    fn allows(&self, relative: &str) -> bool {
        let covers = |set: &[globset::GlobMatcher]| {
            set.iter().any(|g| {
                relative
                    .match_indices('/')
                    .map(|(i, _)| &relative[..i])
                    .chain([relative])
                    .any(|prefix| g.is_match(prefix))
            })
        };
        (self.include.is_empty() || covers(&self.include)) && !covers(&self.exclude)
    }
}

pub struct Rg(pub SearchJob);
impl DiscoveryAdapter for Rg {
    fn tool(&self) -> &'static ToolSpec {
        &RG
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let j = &self.0;
        let mut args: Vec<OsString> = [
            "--json",
            "--no-config",
            "--color",
            "never",
            "--hidden",
            "--no-ignore",
            "--max-depth",
            "32",
            "--no-ignore-global",
            "--no-ignore-exclude",
            "--no-ignore-dot",
            "--no-mmap",
            // Searched in full so that binary files can be counted, then dropped by `end`.
            "--binary",
            "--glob",
            "!.git",
        ]
        .map(OsString::from)
        .into();
        args.push(
            if j.ignore_case {
                "--ignore-case"
            } else {
                "--case-sensitive"
            }
            .into(),
        );
        if j.fixed_strings {
            args.push("--fixed-strings".into());
        }
        if j.context > 0 {
            args.extend(["--context".into(), j.context.to_string().into()]);
        }
        args.extend(["--regexp".into(), j.pattern.clone().into(), "--".into()]);
        args.push(j.file.clone().unwrap_or_else(|| ".".into()).into());
        Ok(ExecutionPlan {
            program: RG.name.into(),
            args,
            cwd: PathBuf::from(&j.root),
        })
    }
    fn normalizer(&self, byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        let j = &self.0;
        Ok(Box::new(RgNormalizer {
            summary_root: j.root.clone(),
            pattern: j.pattern.clone(),
            context: j.context as u64,
            limits: j.limits.clone(),
            byte_budget,
            filter: GlobFilter::compile(&j.globs)?,
            scope: FileScope::new(&j.root, j.in_repo, j.include_ignored, j.file.is_some()),
            pending: Vec::new(),
            skipping: false,
            file: None,
            entries: 0,
            matches: 0,
            bytes: 0,
            binary: 0,
            items: Vec::new(),
            issues: Issues::default(),
            stop: None,
        }))
    }
}

/// Larger JSON records (a single enormous line) are skipped rather than buffered.
const RECORD_LIMIT: usize = 4 * 1024 * 1024;
struct PendingMatch {
    line: u64,
    text: String,
    ranges: Vec<ByteRange>,
    truncated: bool,
}
struct OpenFile {
    path: String,
    /// Every line rg printed for this file (matches and context), shortened for context use.
    lines: BTreeMap<u64, String>,
    matches: Vec<PendingMatch>,
    issues: Vec<ResultIssue>,
    skipped: bool,
    overflow: bool,
}
struct RgNormalizer {
    summary_root: String,
    pattern: String,
    context: u64,
    limits: DiscoveryLimits,
    byte_budget: u64,
    filter: GlobFilter,
    scope: FileScope,
    pending: Vec<u8>,
    skipping: bool,
    file: Option<OpenFile>,
    entries: u64,
    matches: u64,
    bytes: u64,
    binary: u64,
    items: Vec<MatchItem>,
    issues: Issues,
    stop: Option<Stop>,
}
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().take_while(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}
/// rg encodes text as `{"text": ..}` and non-UTF-8 as `{"bytes": base64}`.
enum Data {
    Text(String),
    Bytes(Vec<u8>),
}
fn data(value: &Value) -> Option<Data> {
    if let Some(text) = value["text"].as_str() {
        Some(Data::Text(text.to_owned()))
    } else {
        value["bytes"].as_str().and_then(base64).map(Data::Bytes)
    }
}
impl RgNormalizer {
    fn stop(&mut self, stop: Stop) -> Flow {
        self.stop = Some(stop);
        Flow::Stop(stop)
    }
    fn record(&mut self, raw: &[u8]) -> Flow {
        let Ok(event) = serde_json::from_slice::<Value>(raw) else {
            self.issues.error(ResultIssue::new(
                ErrorCode::BackendFailure,
                "unreadable rg record",
            ));
            return Flow::Continue;
        };
        let body = &event["data"];
        match event["type"].as_str() {
            Some("begin") => self.begin(body),
            Some("match") => self.found(body),
            Some("context") => {
                self.context_line(body);
                Flow::Continue
            }
            Some("end") => {
                if body["binary_offset"].is_u64() {
                    if let Some(file) = self.file.take() {
                        self.matches -= file.matches.len() as u64;
                        if !file.skipped {
                            self.binary += 1;
                        }
                    }
                    return Flow::Continue;
                }
                self.flush()
            }
            _ => Flow::Continue,
        }
    }
    fn begin(&mut self, body: &Value) -> Flow {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return self.stop(Stop::Truncated(TruncationReason::MaxEntries));
        }
        let (path, skipped) = match data(&body["path"]) {
            Some(Data::Text(text)) => {
                let path = text.strip_prefix("./").unwrap_or(&text).to_owned();
                let skipped = !self.filter.allows(&path)
                    || !self.scope.allows(&path, false, &mut self.issues);
                (path, skipped)
            }
            other => {
                let parent = match other {
                    Some(Data::Bytes(bytes)) => identifiable_parent(&bytes),
                    _ => ".".into(),
                };
                self.issues.error(
                    ResultIssue::new(
                        ErrorCode::InvalidEncoding,
                        "a file name below this path is not valid UTF-8",
                    )
                    .at(parent),
                );
                (String::new(), true)
            }
        };
        self.file = Some(OpenFile {
            path,
            lines: BTreeMap::new(),
            matches: Vec::new(),
            issues: Vec::new(),
            skipped,
            overflow: false,
        });
        Flow::Continue
    }
    fn context_line(&mut self, body: &Value) {
        let Some(file) = self.file.as_mut().filter(|f| !f.skipped && !f.overflow) else {
            return;
        };
        let (Some(Data::Text(text)), Some(number)) =
            (data(&body["lines"]), body["line_number"].as_u64())
        else {
            return;
        };
        let text = text.trim_end_matches(['\n', '\r']);
        file.lines
            .insert(number, cut_utf8(text, CONTEXT_LINE_BYTES).0.to_owned());
    }
    fn found(&mut self, body: &Value) -> Flow {
        let Some(file) = self.file.as_mut().filter(|f| !f.skipped) else {
            return Flow::Continue;
        };
        let Some(number) = body["line_number"].as_u64() else {
            return Flow::Continue;
        };
        let text = match data(&body["lines"]) {
            Some(Data::Text(text)) => text,
            _ => {
                let mut issue = ResultIssue::new(
                    ErrorCode::InvalidEncoding,
                    "the matching line is not valid UTF-8",
                )
                .at(file.path.clone());
                issue.line = Some(number);
                file.issues.push(issue);
                return Flow::Continue;
            }
        };
        if self.matches >= self.limits.max_items {
            // Keep reading until `end` classifies this file as text or binary.
            // Only one overflow bit is needed; additional matches need no buffering.
            file.overflow = true;
            return Flow::Continue;
        }
        self.matches += 1;
        let text = text.trim_end_matches(['\n', '\r']);
        let (kept, truncated) = cut_utf8(text, LINE_BYTES);
        let limit = kept.len() as u64;
        // rg offsets are 0-based byte offsets; the contract is 1-based with an exclusive end.
        let ranges = body["submatches"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter_map(|m| Some((m["start"].as_u64()?, m["end"].as_u64()?)))
                    .filter(|(start, _)| *start < limit || (*start == 0 && limit == 0))
                    .map(|(start, end)| ByteRange {
                        start_byte: start + 1,
                        end_byte: end.min(limit) + 1,
                    })
                    .collect()
            })
            .unwrap_or_default();
        file.lines
            .insert(number, cut_utf8(kept, CONTEXT_LINE_BYTES).0.to_owned());
        file.matches.push(PendingMatch {
            line: number,
            text: kept.to_owned(),
            ranges,
            truncated,
        });
        Flow::Continue
    }
    fn flush(&mut self) -> Flow {
        let Some(file) = self.file.take() else {
            return Flow::Continue;
        };
        for issue in file.issues {
            self.issues.error(issue);
        }
        for m in file.matches {
            let window = |from: u64, to: u64| -> Vec<ContextLine> {
                if from > to {
                    return Vec::new();
                }
                file.lines
                    .range(from..=to)
                    .map(|(line, text)| ContextLine {
                        line: *line,
                        text: text.clone(),
                    })
                    .collect()
            };
            let item = MatchItem {
                path: file.path.clone(),
                line: m.line,
                context_before: window(m.line.saturating_sub(self.context).max(1), m.line - 1),
                context_after: window(m.line + 1, m.line + self.context),
                text: m.text,
                ranges: m.ranges,
                line_truncated: m.truncated,
            };
            let size = serde_json::to_vec(&item)
                .map(|v| v.len() as u64 + 1)
                .unwrap_or(0);
            if self.bytes + size > self.byte_budget {
                return self.stop(Stop::Truncated(TruncationReason::ArtifactBytes));
            }
            self.bytes += size;
            self.items.push(item);
        }
        if file.overflow {
            return self.stop(Stop::Truncated(TruncationReason::MaxItems));
        }
        Flow::Continue
    }
    fn stderr(&mut self, stderr: &[u8]) -> bool {
        let mut invalid_pattern = false;
        for line in String::from_utf8_lossy(stderr).lines() {
            let Some(rest) = line.strip_prefix("rg: ") else {
                continue;
            };
            if rest.starts_with("regex parse error") || rest.contains("error parsing") {
                invalid_pattern = true;
                continue;
            }
            let Some((path, message)) = rest.rsplit_once(": ") else {
                continue;
            };
            let path = path.strip_prefix("./").unwrap_or(path);
            if !self.scope.allows(path, true, &mut self.issues) {
                continue;
            }
            match classify_io_message(message) {
                ErrorCode::PermissionDenied => self.issues.error(
                    ResultIssue::new(ErrorCode::PermissionDenied, "path is not readable").at(path),
                ),
                ErrorCode::TargetNotFound => self.issues.warning(
                    WarningCode::EntryVanished,
                    "the entry disappeared during the scan",
                    Some(path.to_owned()),
                ),
                _ => (),
            }
        }
        invalid_pattern
    }
}
impl Normalizer for RgNormalizer {
    fn feed(&mut self, chunk: &[u8]) -> Flow {
        if let Some(stop) = self.stop {
            return Flow::Stop(stop);
        }
        for byte in chunk {
            if *byte == b'\n' {
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
                        ErrorCode::PartialResult,
                        "a match record was too large and was skipped",
                    ));
                }
            }
        }
        Flow::Continue
    }
    fn finish(mut self: Box<Self>, exit: &TargetExit, stderr: &[u8]) -> DiscoveryOutcome {
        // Without `end`, binary classification is unknown. Never publish that file.
        self.file = None;
        let stop = self.stop.or(exit.stopped);
        let invalid_pattern = self.stderr(stderr);
        let mut state = ChildState::Done;
        let mut reasons = Vec::new();
        if exit.spawn_failed {
            state = ChildState::Failed;
            self.issues.error(missing_tool(&RG));
        } else if let Some(Stop::Truncated(reason)) = stop {
            reasons.push(reason);
        } else if stop.is_none() {
            match exit.code {
                // 1: nothing matched. That is a result, not a failure.
                Some(0 | 1) => (),
                Some(2) if invalid_pattern => {
                    state = ChildState::Failed;
                    self.issues.error(ResultIssue::new(
                        ErrorCode::InvalidPattern,
                        "rg rejected the pattern",
                    ));
                }
                Some(2) => {
                    if self.issues.errors.is_empty() {
                        self.issues.error(ResultIssue::new(
                            ErrorCode::PartialResult,
                            "rg could not search some paths",
                        ));
                    }
                }
                _ => {
                    state = ChildState::Failed;
                    self.issues
                        .error(ResultIssue::new(ErrorCode::BackendFailure, "rg failed"));
                }
            }
        }
        if self.binary > 0 {
            self.issues.warning(
                WarningCode::BinaryFilesSkipped,
                "binary files were skipped",
                None,
            );
        }
        self.items
            .sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        let partial = !self.issues.errors.is_empty();
        DiscoveryOutcome {
            state,
            exit_code: exit.code,
            document: DiscoveryDocument {
                summary: DiscoverySummary::Search {
                    root: self.summary_root,
                    scan_depth: Some(MAX_DEPTH),
                    pattern: self.pattern,
                    binary_files_skipped: self.binary,
                },
                items: self.items.into_iter().map(DiscoveryItem::Match).collect(),
                completeness: Completeness::new(reasons, partial),
                errors: self.issues.errors,
                warnings: self.issues.warnings,
            },
        }
    }
}

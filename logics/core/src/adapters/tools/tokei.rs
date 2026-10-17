//! `tokei --streaming json` backs `code stats`. Records arrive one per file in no fixed order,
//! so a stop keeps what was counted and the aggregate is deterministic.
//!
//! Counts are those tokei reports for the file's own language. Languages embedded in a file
//! (JavaScript in HTML) are listed per host as `embedded` and are not part of the host's
//! counts or of the summary totals, so no line is counted twice.
//! [`parse_record`] and [`Tally`] do not depend on the worker; other commands may reuse them.
use super::*;
use serde_json::Value;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

/// Whether `tokei --version` reports a build that can print JSON.
pub fn has_json(version: &[u8]) -> bool {
    std::str::from_utf8(version).is_ok_and(|text| {
        text.split("serialization support:")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .is_some_and(|formats| formats.split(',').any(|f| f.trim() == "json"))
    })
}

pub struct Tokei(pub CodeStatsJob);
impl DiscoveryAdapter for Tokei {
    fn tool(&self) -> &'static ToolSpec {
        &TOKEI
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let j = &self.0;
        let mut args: Vec<OsString> = [
            "--streaming",
            "json",
            "--hidden",
            "--exclude",
            ".git",
            "--no-ignore-dot",
        ]
        .map(OsString::from)
        .into();
        if j.include_ignored {
            args.push("--no-ignore".into());
        } else if !j.in_repo {
            args.push("--no-ignore-parent".into());
        }
        args.push("--".into());
        args.push(j.file.clone().unwrap_or_else(|| ".".into()).into());
        Ok(ExecutionPlan {
            program: TOKEI.name.into(),
            args,
            cwd: PathBuf::from(&j.root),
        })
    }
    fn normalizer(&self, _byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        Ok(Box::new(TokeiNormalizer {
            job: self.0.clone(),
            pending: Vec::new(),
            skipping: false,
            records: 0,
            tally: Tally::default(),
            issues: Issues::default(),
            stop: None,
        }))
    }
}

/// One file of a `--streaming json` stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub language: String,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
    /// Every language nested in the file, flattened: `(language, code, comments, blanks)`.
    pub embedded: Vec<(String, u64, u64, u64)>,
}
fn counts(stats: &Value) -> Option<(u64, u64, u64)> {
    Some((
        stats["code"].as_u64()?,
        stats["comments"].as_u64()?,
        stats["blanks"].as_u64()?,
    ))
}
fn collect_blobs(blobs: &Value, out: &mut Vec<(String, u64, u64, u64)>) -> Option<()> {
    for (language, stats) in blobs.as_object()? {
        let (code, comments, blanks) = counts(stats)?;
        out.push((language.clone(), code, comments, blanks));
        collect_blobs(&stats["blobs"], out)?;
    }
    Some(())
}
/// None when the line is not a record of the expected shape or a count is not a `u64`.
pub fn parse_record(raw: &[u8]) -> Option<FileRecord> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let file = &value["stats"];
    let stats = &file["stats"];
    let (code, comments, blanks) = counts(stats)?;
    let mut embedded = Vec::new();
    if !stats["blobs"].is_null() {
        collect_blobs(&stats["blobs"], &mut embedded)?;
    }
    let path = file["name"].as_str()?;
    Some(FileRecord {
        path: path.strip_prefix("./").unwrap_or(path).to_owned(),
        language: value["language"].as_str()?.to_owned(),
        code,
        comments,
        blanks,
        embedded,
    })
}

#[derive(Default)]
struct Counts {
    files: u64,
    code: u64,
    comments: u64,
    blanks: u64,
}
impl Counts {
    fn add(&mut self, code: u64, comments: u64, blanks: u64) {
        self.files = self.files.saturating_add(1);
        self.code = self.code.saturating_add(code);
        self.comments = self.comments.saturating_add(comments);
        self.blanks = self.blanks.saturating_add(blanks);
    }
}
#[derive(Default)]
struct Host {
    own: Counts,
    embedded: BTreeMap<String, Counts>,
}
/// Aggregate of file records by host language.
#[derive(Default)]
pub struct Tally {
    hosts: BTreeMap<String, Host>,
}
impl Tally {
    pub fn add(&mut self, record: &FileRecord) {
        let host = self.hosts.entry(record.language.clone()).or_default();
        host.own.add(record.code, record.comments, record.blanks);
        // A language that occurs twice in one file is still one file of that language.
        let mut per_language: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
        for (language, code, comments, blanks) in &record.embedded {
            let sum = per_language.entry(language).or_default();
            *sum = (
                sum.0.saturating_add(*code),
                sum.1.saturating_add(*comments),
                sum.2.saturating_add(*blanks),
            );
        }
        for (language, (code, comments, blanks)) in per_language {
            host.embedded
                .entry(language.to_owned())
                .or_default()
                .add(code, comments, blanks);
        }
    }
    /// Host languages in name order.
    pub fn items(&self) -> Vec<LanguageStatItem> {
        self.hosts
            .iter()
            .map(|(language, host)| LanguageStatItem {
                language: language.clone(),
                files: host.own.files,
                code: host.own.code,
                comments: host.own.comments,
                blanks: host.own.blanks,
                embedded: host
                    .embedded
                    .iter()
                    .map(|(language, c)| EmbeddedStat {
                        language: language.clone(),
                        files: c.files,
                        code: c.code,
                        comments: c.comments,
                        blanks: c.blanks,
                    })
                    .collect(),
            })
            .collect()
    }
    /// Sum over host languages: `(files, code, comments, blanks)`.
    pub fn totals(&self) -> (u64, u64, u64, u64) {
        self.hosts.values().fold((0, 0, 0, 0), |t, h| {
            (
                t.0.saturating_add(h.own.files),
                t.1.saturating_add(h.own.code),
                t.2.saturating_add(h.own.comments),
                t.3.saturating_add(h.own.blanks),
            )
        })
    }
}

/// Larger JSON records are skipped rather than buffered.
const RECORD_LIMIT: usize = 4 * 1024 * 1024;
struct TokeiNormalizer {
    job: CodeStatsJob,
    pending: Vec<u8>,
    skipping: bool,
    records: u64,
    tally: Tally,
    issues: Issues,
    stop: Option<Stop>,
}
impl TokeiNormalizer {
    fn record(&mut self, raw: &[u8]) -> Flow {
        if self.records >= self.job.limits.max_entries {
            let stop = Stop::Truncated(TruncationReason::MaxEntries);
            self.stop = Some(stop);
            return Flow::Stop(stop);
        }
        self.records += 1;
        match parse_record(raw) {
            Some(record) => self.tally.add(&record),
            None => self.issues.error(ResultIssue::new(
                ErrorCode::BackendFailure,
                "unreadable tokei record",
            )),
        }
        Flow::Continue
    }
    /// tokei logs `[<time> ERROR tokei::..] <reason> reading <path>` or, for a file,
    /// `Error reading <path>:` followed by the indented reason.
    fn stderr(&mut self, stderr: &[u8]) {
        let text = String::from_utf8_lossy(stderr);
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some((_, message)) = line
                .split_once(" ERROR ")
                .and_then(|(_, rest)| rest.split_once("] "))
            else {
                continue;
            };
            let located = match message.strip_prefix("Error reading ") {
                Some(path) => path
                    .strip_suffix(':')
                    .zip(lines.get(i + 1).map(|l| l.trim())),
                None => message
                    .rsplit_once(" reading ")
                    .map(|(reason, path)| (path, reason)),
            };
            let Some((path, reason)) = located else {
                self.issues.error(ResultIssue::new(
                    ErrorCode::PartialResult,
                    "tokei could not read some paths",
                ));
                continue;
            };
            let path = path.strip_prefix("./").unwrap_or(path);
            match classify_io_message(reason) {
                ErrorCode::TargetNotFound => self.issues.warning(
                    WarningCode::EntryVanished,
                    "the entry disappeared during the scan",
                    Some(path.to_owned()),
                ),
                ErrorCode::PermissionDenied => self.issues.error(
                    ResultIssue::new(ErrorCode::PermissionDenied, "path is not readable").at(path),
                ),
                _ => self.issues.error(
                    ResultIssue::new(ErrorCode::PartialResult, "tokei could not read this path")
                        .at(path),
                ),
            }
        }
    }
}
impl Normalizer for TokeiNormalizer {
    fn feed(&mut self, chunk: &[u8]) -> Flow {
        if let Some(stop) = self.stop {
            return Flow::Stop(stop);
        }
        for byte in chunk {
            if *byte == b'\n' {
                let record = std::mem::take(&mut self.pending);
                if std::mem::take(&mut self.skipping) || record.is_empty() {
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
                        "a tokei record was too large and was skipped",
                    ));
                }
            }
        }
        Flow::Continue
    }
    fn finish(mut self: Box<Self>, exit: &TargetExit, stderr: &[u8]) -> DiscoveryOutcome {
        let stop = self.stop.or(exit.stopped);
        self.stderr(stderr);
        let mut state = ChildState::Done;
        let mut reasons = Vec::new();
        if exit.spawn_failed {
            state = ChildState::Failed;
            self.issues.error(missing_tool(&TOKEI));
        } else if let Some(Stop::Truncated(reason)) = stop {
            reasons.push(reason);
        } else if stop.is_none() && exit.code != Some(0) {
            state = ChildState::Failed;
            self.issues
                .error(ResultIssue::new(ErrorCode::BackendFailure, "tokei failed"));
        }
        if !self.job.include_ignored && !self.job.in_repo {
            self.issues.warning(
                WarningCode::IgnoreRulesNotApplied,
                "ignore files are applied only inside a Git worktree",
                None,
            );
        }
        let (files, code, comments, blanks) = self.tally.totals();
        let mut items = self.tally.items();
        if items.len() as u64 > self.job.limits.max_items {
            items.truncate(self.job.limits.max_items as usize);
            if !reasons.contains(&TruncationReason::MaxItems) {
                reasons.push(TruncationReason::MaxItems);
            }
        }
        let completeness = Completeness::new(reasons, !self.issues.errors.is_empty());
        DiscoveryOutcome {
            state,
            exit_code: exit.code,
            document: DiscoveryDocument {
                summary: DiscoverySummary::CodeStats {
                    root: self.job.root,
                    files,
                    code,
                    comments,
                    blanks,
                    scope_complete: completeness.scope_complete,
                    skipped_files: None,
                },
                items: items.into_iter().map(DiscoveryItem::Stat).collect(),
                completeness,
                errors: self.issues.errors,
                warnings: self.issues.warnings,
            },
        }
    }
}

//! `ast-grep scan --inline-rules` backs `code search`: a `pattern` rule yields the matches and
//! a `kind: ERROR` rule marks files that did not parse. A root rule also reports files with
//! no matches, so max_entries counts scanned files. The rules are
//! built with `serde_json` only; a pattern can never add a field or a second rule.
use super::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashSet},
    ffi::OsString,
    path::PathBuf,
};

const MATCH_RULE: &str = "axvern-match";
const ERROR_RULE: &str = "axvern-parse-error";
const FILE_RULE: &str = "axvern-file";
/// `ast-grep` exit status for a rule that cannot be loaded, which here means the pattern.
const EXIT_INVALID_RULE: i32 = 8;

/// A language `code search` accepts and the file extensions that belong to it.
#[derive(Clone, Copy, Debug)]
pub struct Language {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}
/// Languages whose match, zero-match and parse-error behavior was verified with ast-grep 0.51.0.
pub const LANGUAGES: [Language; 5] = [
    Language {
        name: "rust",
        extensions: &["rs"],
    },
    Language {
        name: "javascript",
        extensions: &["js", "jsx", "mjs", "cjs"],
    },
    Language {
        name: "typescript",
        extensions: &["ts", "cts", "mts"],
    },
    Language {
        name: "python",
        extensions: &["py", "pyi"],
    },
    Language {
        name: "go",
        extensions: &["go"],
    },
];
pub fn language(name: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.name == name)
}
impl Language {
    pub fn owns(&self, file: &str) -> bool {
        Path::new(file)
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|e| self.extensions.contains(&e))
    }
}

pub struct AstGrep(pub CodeSearchJob);
impl AstGrep {
    /// YAML documents, each written as a JSON object.
    fn rules(&self) -> String {
        let root_kind = match self.0.lang.as_str() {
            "javascript" | "typescript" => "program",
            "python" => "module",
            _ => "source_file",
        };
        [
            json!({"id": MATCH_RULE, "language": self.0.lang, "rule": {"pattern": self.0.pattern}}),
            json!({"id": ERROR_RULE, "language": self.0.lang, "rule": {"kind": "ERROR"}}),
            json!({"id": FILE_RULE, "language": self.0.lang, "rule": {"kind": root_kind}}),
        ]
        .map(|rule| rule.to_string())
        .join("\n---\n")
    }
}
impl DiscoveryAdapter for AstGrep {
    fn tool(&self) -> &'static ToolSpec {
        &AST_GREP
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let j = &self.0;
        let mut args: Vec<OsString> = [
            "scan",
            "--inline-rules",
            &self.rules(),
            "--json=stream",
            "--color",
            "never",
            "--config",
            "/dev/null",
            "--no-ignore",
            "hidden",
            "--no-ignore",
            "dot",
            "--no-ignore",
            "exclude",
            "--no-ignore",
            "global",
            "--globs",
            "!.git",
        ]
        .map(OsString::from)
        .into();
        if j.include_ignored {
            args.extend(["--no-ignore", "parent", "--no-ignore", "vcs"].map(OsString::from));
        } else if !j.in_repo {
            args.extend(["--no-ignore", "parent"].map(OsString::from));
        }
        args.push("--".into());
        args.push(j.file.clone().unwrap_or_else(|| ".".into()).into());
        Ok(ExecutionPlan {
            program: AST_GREP.name.into(),
            args,
            cwd: PathBuf::from(&j.root),
        })
    }
    fn normalizer(&self, byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        let j = &self.0;
        Ok(Box::new(AstGrepNormalizer {
            job: j.clone(),
            byte_budget,
            pending: Vec::new(),
            skipping: false,
            seen: HashSet::new(),
            parse_errors: BTreeSet::new(),
            matches: Vec::new(),
            bytes: 0,
            issues: Issues::default(),
            stop: None,
        }))
    }
}

/// Byte offset of the `chars`-th character of `line`, or its length when it ends there.
fn byte_offset(line: &str, chars: u64) -> Option<usize> {
    let chars = usize::try_from(chars).ok()?;
    match line.char_indices().nth(chars) {
        Some((offset, _)) => Some(offset),
        None => (line.chars().count() == chars).then_some(line.len()),
    }
}
/// One `axvern-match` record as a stored item. ast-grep numbers lines and columns from 0 and
/// counts columns in characters; the contract is 1-based UTF-8 bytes with an exclusive end.
pub(crate) fn code_match(record: &Value, lang: &str) -> Option<CodeMatchItem> {
    let path = record["file"].as_str()?;
    let (text, lines) = (record["text"].as_str()?, record["lines"].as_str()?);
    let range = &record["range"];
    let (start_line, start_col) = (
        range["start"]["line"].as_u64()?,
        range["start"]["column"].as_u64()?,
    );
    let (end_line, end_col) = (
        range["end"]["line"].as_u64()?,
        range["end"]["column"].as_u64()?,
    );
    let rows: Vec<&str> = lines.split('\n').collect();
    let first = rows.first()?;
    let last = rows.get(usize::try_from(end_line.checked_sub(start_line)?).ok()?)?;
    let (snippet, truncated) = cut_utf8(text, SNIPPET_BYTES);
    Some(CodeMatchItem {
        path: path.strip_prefix("./").unwrap_or(path).to_owned(),
        lang: lang.to_owned(),
        start: CodePosition {
            line: start_line + 1,
            byte_column: byte_offset(first, start_col)? as u64 + 1,
        },
        end: CodePosition {
            line: end_line + 1,
            byte_column: byte_offset(last, end_col)? as u64 + 1,
        },
        snippet: snippet.to_owned(),
        snippet_truncated: truncated,
    })
}

/// Larger JSON records are skipped rather than buffered.
const RECORD_LIMIT: usize = 4 * 1024 * 1024;
struct AstGrepNormalizer {
    job: CodeSearchJob,
    byte_budget: u64,
    pending: Vec<u8>,
    skipping: bool,
    seen: HashSet<String>,
    parse_errors: BTreeSet<String>,
    matches: Vec<CodeMatchItem>,
    bytes: u64,
    issues: Issues,
    stop: Option<Stop>,
}
impl AstGrepNormalizer {
    fn stop(&mut self, stop: Stop) -> Flow {
        self.stop = Some(stop);
        Flow::Stop(stop)
    }
    fn record(&mut self, raw: &[u8]) -> Flow {
        let Ok(event) = serde_json::from_slice::<Value>(raw) else {
            self.issues.error(ResultIssue::new(
                ErrorCode::BackendFailure,
                "unreadable ast-grep record",
            ));
            return Flow::Continue;
        };
        let Some(path) = event["file"].as_str() else {
            return Flow::Continue;
        };
        let path = path.strip_prefix("./").unwrap_or(path);
        if !self.seen.contains(path) {
            if self.seen.len() as u64 >= self.job.limits.max_entries {
                return self.stop(Stop::Truncated(TruncationReason::MaxEntries));
            }
            self.seen.insert(path.to_owned());
        }
        match event["ruleId"].as_str() {
            Some(ERROR_RULE) => {
                self.parse_errors.insert(path.to_owned());
            }
            Some(MATCH_RULE) => {
                let Some(item) = code_match(&event, &self.job.lang) else {
                    self.issues.error(ResultIssue::new(
                        ErrorCode::BackendFailure,
                        "unreadable ast-grep match",
                    ));
                    return Flow::Continue;
                };
                if self.matches.len() as u64 >= self.job.limits.max_items {
                    return self.stop(Stop::Truncated(TruncationReason::MaxItems));
                }
                let size = serde_json::to_vec(&item).map_or(0, |v| v.len() as u64 + 1);
                if self.bytes + size > self.byte_budget {
                    return self.stop(Stop::Truncated(TruncationReason::ArtifactBytes));
                }
                self.bytes += size;
                self.matches.push(item);
            }
            _ => (),
        }
        Flow::Continue
    }
    fn stderr(&mut self, stderr: &[u8]) {
        for line in String::from_utf8_lossy(stderr).lines() {
            let Some((path, message)) = line
                .strip_prefix("ERROR: ")
                .and_then(|rest| rest.rsplit_once(": "))
            else {
                continue;
            };
            let path = path.strip_prefix("./").unwrap_or(path);
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
    }
}
impl Normalizer for AstGrepNormalizer {
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
        let stop = self.stop.or(exit.stopped);
        self.stderr(stderr);
        let mut state = ChildState::Done;
        let mut reasons = Vec::new();
        if exit.spawn_failed {
            state = ChildState::Failed;
            self.issues.error(missing_tool(&AST_GREP));
        } else if let Some(Stop::Truncated(reason)) = stop {
            reasons.push(reason);
        } else if stop.is_none() {
            match exit.code {
                Some(0 | 1) => (),
                Some(EXIT_INVALID_RULE) => {
                    state = ChildState::Failed;
                    self.issues.error(ResultIssue::new(
                        ErrorCode::InvalidPattern,
                        "ast-grep rejected the pattern",
                    ));
                }
                _ => {
                    state = ChildState::Failed;
                    self.issues.error(ResultIssue::new(
                        ErrorCode::BackendFailure,
                        "ast-grep failed",
                    ));
                }
            }
        }
        // Matches in a file that did not parse are not a reliable result.
        let broken = std::mem::take(&mut self.parse_errors);
        for path in &broken {
            self.issues.error(
                ResultIssue::new(ErrorCode::ParseError, "the file has a syntax error").at(path),
            );
        }
        let mut items = std::mem::take(&mut self.matches);
        items.retain(|m| !broken.contains(&m.path));
        items.sort_by(|a, b| (&a.path, a.start, a.end).cmp(&(&b.path, b.start, b.end)));
        if !self.job.include_ignored && !self.job.in_repo {
            self.issues.warning(
                WarningCode::IgnoreRulesNotApplied,
                "ignore files are applied only inside a Git worktree",
                None,
            );
        }
        let partial = !self.issues.errors.is_empty();
        DiscoveryOutcome {
            state,
            exit_code: exit.code,
            document: DiscoveryDocument {
                summary: DiscoverySummary::CodeSearch {
                    root: self.job.root,
                    lang: self.job.lang,
                    pattern: self.job.pattern,
                },
                items: items.into_iter().map(DiscoveryItem::Code).collect(),
                completeness: Completeness::new(reasons, partial),
                errors: self.issues.errors,
                warnings: self.issues.warnings,
            },
        }
    }
}

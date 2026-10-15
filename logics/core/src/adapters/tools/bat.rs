//! `bat` backs `fs read`: a bounded line range of one UTF-8 text file, with no decorations.
use super::*;
use std::{ffi::OsString, fs, path::PathBuf, time::SystemTime};

pub struct Bat(pub ReadJob);
impl DiscoveryAdapter for Bat {
    fn tool(&self) -> &'static ToolSpec {
        &BAT
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let j = &self.0;
        // One line beyond the request reveals whether the file continues.
        let end = j
            .from_line
            .checked_add(j.lines as u64)
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or_else(|| Error::new(ErrorCode::InvalidInput, "line range overflows"))?;
        let mut args: Vec<OsString> = [
            "--no-config",
            "--paging=never",
            "--color=never",
            "--style=plain",
            "--decorations=never",
            "--tabs=0",
            "--wrap=never",
        ]
        .map(OsString::from)
        .into();
        args.push(format!("--line-range={}:{end}", j.from_line).into());
        args.extend(["--".into(), j.path.clone().into()]);
        let path = PathBuf::from(&j.path);
        Ok(ExecutionPlan {
            program: BAT.name.into(),
            args,
            cwd: path
                .parent()
                .ok_or_else(|| Error::new(ErrorCode::InvalidTarget, "file has no parent"))?
                .into(),
        })
    }
    fn normalizer(&self, byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        let j = &self.0;
        Ok(Box::new(BatNormalizer {
            path: j.path.clone(),
            from_line: j.from_line,
            wanted: j.lines as u64,
            byte_budget,
            before: fingerprint(&j.path),
            current: Vec::new(),
            overflowed: false,
            seen: 0,
            body: 0,
            bytes: 0,
            items: Vec::new(),
            more: false,
            failure: None,
            stop: None,
        }))
    }
}
fn fingerprint(path: &str) -> Option<(u64, SystemTime)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

struct BatNormalizer {
    path: String,
    from_line: u64,
    wanted: u64,
    byte_budget: u64,
    before: Option<(u64, SystemTime)>,
    current: Vec<u8>,
    overflowed: bool,
    /// Lines completed so far, including the probe line past the request.
    seen: u64,
    body: u64,
    bytes: u64,
    items: Vec<LineItem>,
    more: bool,
    failure: Option<ErrorCode>,
    stop: Option<Stop>,
}
impl BatNormalizer {
    fn stop(&mut self, stop: Stop) -> Flow {
        self.stop = Some(stop);
        Flow::Stop(stop)
    }
    fn fail(&mut self, code: ErrorCode) -> Flow {
        self.failure = Some(code);
        self.stop(Stop::Satisfied)
    }
    fn line(&mut self) -> Flow {
        let mut raw = std::mem::take(&mut self.current);
        let overflowed = std::mem::take(&mut self.overflowed);
        if self.seen == self.wanted {
            self.more = true;
            return self.stop(Stop::Satisfied);
        }
        let number = self.from_line + self.seen;
        self.seen += 1;
        if raw.last() == Some(&b'\r') && !overflowed {
            raw.pop();
        }
        if number == 1 && raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            raw.drain(..3);
        }
        let text = match std::str::from_utf8(&raw) {
            Ok(text) => text,
            // The kept prefix may end inside a character that the cut removed.
            Err(e) if overflowed && e.error_len().is_none() => {
                std::str::from_utf8(&raw[..e.valid_up_to()]).unwrap_or("")
            }
            Err(_) => return self.fail(ErrorCode::UnsupportedEncoding),
        };
        let (text, cut) = cut_utf8(text, LINE_BYTES);
        let size = text.len() as u64;
        if self.body + size > READ_BODY_BYTES {
            return self.stop(Stop::Truncated(TruncationReason::BodyBytes));
        }
        if self.bytes + size + 64 > self.byte_budget {
            return self.stop(Stop::Truncated(TruncationReason::ArtifactBytes));
        }
        self.body += size;
        self.bytes += size + 64;
        self.items.push(LineItem {
            line: number,
            text: text.to_owned(),
            line_truncated: cut || overflowed,
        });
        Flow::Continue
    }
}
impl Normalizer for BatNormalizer {
    fn feed(&mut self, chunk: &[u8]) -> Flow {
        if let Some(stop) = self.stop {
            return Flow::Stop(stop);
        }
        for byte in chunk {
            match *byte {
                0 => return self.fail(ErrorCode::UnsupportedEncoding),
                b'\n' => {
                    if let Flow::Stop(stop) = self.line() {
                        return Flow::Stop(stop);
                    }
                }
                b => {
                    // Keep a little more than one stored line so the cut can be validated.
                    if self.current.len() < LINE_BYTES + 4 {
                        self.current.push(b);
                    } else {
                        self.overflowed = true;
                    }
                }
            }
        }
        Flow::Continue
    }
    fn finish(mut self: Box<Self>, exit: &TargetExit, stderr: &[u8]) -> DiscoveryOutcome {
        let mut issues = Issues::default();
        if self.stop.is_none() && exit.stopped.is_none() && !exit.spawn_failed {
            if !self.current.is_empty() || self.overflowed {
                let _ = self.line();
            }
        }
        let stop = self.stop.or(exit.stopped);
        let mut state = ChildState::Done;
        let mut reasons = Vec::new();
        if exit.spawn_failed {
            state = ChildState::Failed;
            issues.error(missing_tool(&BAT));
        } else if let Some(code) = self.failure {
            state = ChildState::Failed;
            self.items.clear();
            issues.error(ResultIssue::new(
                code,
                "the file is binary or not valid UTF-8 text",
            ));
        } else if let Some(Stop::Truncated(reason)) = stop {
            reasons.push(reason);
        } else if stop.is_none() && exit.code != Some(0) {
            state = ChildState::Failed;
            let code = classify_io_message(&String::from_utf8_lossy(stderr));
            issues
                .error(ResultIssue::new(code, "bat could not read the file").at(self.path.clone()));
        }
        if self.before.is_some() && fingerprint(&self.path) != self.before {
            issues.warning(
                WarningCode::SourceChanged,
                "the file changed while it was being read",
                Some(self.path.clone()),
            );
        }
        let eof = state == ChildState::Done && reasons.is_empty() && !self.more;
        let partial = !issues.errors.is_empty();
        DiscoveryOutcome {
            state,
            exit_code: exit.code,
            document: DiscoveryDocument {
                summary: DiscoverySummary::Read {
                    path: self.path,
                    from_line: self.from_line,
                    requested_lines: self.wanted as u32,
                    eof,
                },
                items: self.items.into_iter().map(DiscoveryItem::Line).collect(),
                completeness: Completeness::new(reasons, partial),
                errors: issues.errors,
                warnings: issues.warnings,
            },
        }
    }
}

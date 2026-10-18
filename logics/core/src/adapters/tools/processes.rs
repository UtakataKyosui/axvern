//! `system processes`: one snapshot of the process table, read by axvern itself.
//!
//! procs cannot back this command (spike #23): its JSON has no executable-name column (the
//! command is the whole argv, ambiguous when a path holds spaces), its start time has no zone,
//! and a process hidden by permissions leaves no partial marker. The worker therefore reads
//! `/proc` on Linux and `ps` elsewhere, and never reads an argv, an environment or a socket.
//!
//! `name` is the basename of the executable link when it can be read and the kernel's command
//! name (at most 15 bytes) otherwise. `--match` is a case-sensitive literal looked for in it.
use super::*;
use std::{collections::BTreeMap, ffi::OsString, time::Instant};

pub const PROGRAM: &str = "axvern-system-processes";
pub const NATIVE: ToolSpec = ToolSpec {
    name: PROGRAM,
    purpose: "process snapshot (built in, no external CLI)",
    min_version: [0, 0, 0],
    tested_version: "built in",
    capable: any_build,
    feature: "",
};

pub struct Processes(pub ProcessesJob);
impl DiscoveryAdapter for Processes {
    fn tool(&self) -> &'static ToolSpec {
        &NATIVE
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let mut args: Vec<OsString> = vec!["--limit".into(), self.0.limit.to_string().into()];
        if self.0.matching.is_some() {
            args.push("--match".into());
            // The text is not echoed into the plan: a process name filter may be sensitive.
            args.push("<text>".into());
        }
        Ok(ExecutionPlan {
            program: PROGRAM.into(),
            args,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        })
    }
    fn normalizer(&self, _byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        Err(Error::new(
            ErrorCode::BackendFailure,
            "system processes runs in the worker, not through a child process",
        ))
    }
    fn is_native(&self) -> bool {
        true
    }
    fn preflight(&self) -> Result<(), Error> {
        Ok(())
    }
    fn run_native(&self, deadline: Instant) -> Option<DiscoveryOutcome> {
        Some(snapshot(&self.0, deadline))
    }
}

/// What was read for one process before filtering.
struct Row {
    item: ProcessItem,
}
struct Scan {
    rows: BTreeMap<u32, Row>,
    scanned: u64,
    issues: Issues,
    reasons: Vec<TruncationReason>,
}

fn summary(job: &ProcessesJob, observed_at: &str, scanned: u64) -> DiscoverySummary {
    DiscoverySummary::Processes {
        observed_at: observed_at.to_owned(),
        limit: job.limit,
        matching: job.matching.clone(),
        scanned,
    }
}
fn snapshot(job: &ProcessesJob, deadline: Instant) -> DiscoveryOutcome {
    let observed_at = rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    );
    let mut scan = Scan {
        rows: BTreeMap::new(),
        scanned: 0,
        issues: Issues::default(),
        reasons: Vec::new(),
    };
    if let Err(error) = read_table(job, deadline, &mut scan) {
        return DiscoveryOutcome {
            state: ChildState::Failed,
            exit_code: Some(1),
            document: DiscoveryDocument {
                summary: summary(job, &observed_at, scan.scanned),
                items: Vec::new(),
                completeness: Completeness::new(Vec::new(), true),
                errors: vec![error],
                warnings: Vec::new(),
            },
        };
    }
    let mut reasons = scan.reasons;
    let cap = (job.limit as u64).min(job.limits.max_items) as usize;
    let mut items: Vec<DiscoveryItem> = scan
        .rows
        .into_values()
        .map(|row| DiscoveryItem::Process(row.item))
        .collect();
    if items.len() > cap {
        items.truncate(cap);
        reasons.push(TruncationReason::MaxItems);
    }
    DiscoveryOutcome {
        state: ChildState::Done,
        exit_code: Some(0),
        document: DiscoveryDocument {
            summary: summary(job, &observed_at, scan.scanned),
            items,
            completeness: Completeness::new(reasons, !scan.issues.errors.is_empty()),
            errors: scan.issues.errors,
            warnings: scan.issues.warnings,
        },
    }
}
/// Keeps the row when it passes the `--match` filter. Returns false once enough rows are held to
/// show the result is longer than the limit.
fn keep(job: &ProcessesJob, scan: &mut Scan, pid: u32, item: ProcessItem) -> bool {
    scan.scanned += 1;
    if job
        .matching
        .as_deref()
        .is_none_or(|text| item.name.contains(text))
    {
        scan.rows.insert(pid, Row { item });
    }
    // The table is read in PID order, so one row beyond the limit is enough to know it is cut.
    (scan.rows.len() as u64) <= (job.limit as u64).min(job.limits.max_items)
}

#[cfg(target_os = "linux")]
fn read_table(job: &ProcessesJob, deadline: Instant, scan: &mut Scan) -> Result<(), ResultIssue> {
    use std::{
        fs,
        io::ErrorKind,
        os::unix::ffi::{OsStrExt, OsStringExt},
    };
    let fail = |e: &std::io::Error| {
        let code = if e.kind() == ErrorKind::PermissionDenied {
            ErrorCode::PermissionDenied
        } else {
            ErrorCode::BackendFailure
        };
        ResultIssue::new(code, "the process table could not be read")
    };
    let mut pids: Vec<u32> = Vec::new();
    for entry in fs::read_dir("/proc").map_err(|e| fail(&e))? {
        let entry = entry.map_err(|e| fail(&e))?;
        if let Some(pid) = std::str::from_utf8(entry.file_name().as_bytes())
            .ok()
            .and_then(|n| {
                n.bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| n.parse().ok())?
            })
        {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    let boot = fs::read_to_string("/proc/stat").ok().and_then(|text| {
        text.lines()
            .find_map(|l| l.strip_prefix("btime ")?.trim().parse::<u64>().ok())
    });
    let gone = |scan: &mut Scan, pid: u32| {
        scan.issues.warning(
            WarningCode::Disappeared,
            &format!("process {pid} ended before it could be read"),
            None,
        );
    };
    for (index, pid) in pids.into_iter().enumerate() {
        if index as u64 >= job.limits.max_entries {
            scan.reasons.push(TruncationReason::MaxEntries);
            return Ok(());
        }
        if index % 64 == 0 && Instant::now() >= deadline {
            scan.reasons.push(TruncationReason::Timeout);
            return Ok(());
        }
        let stat = match fs::read(format!("/proc/{pid}/stat")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound || e.raw_os_error() == Some(3) => {
                gone(scan, pid);
                continue;
            }
            Err(e) => {
                let (code, message) = if e.kind() == ErrorKind::PermissionDenied {
                    (ErrorCode::PermissionDenied, "a process is not inspectable")
                } else {
                    (ErrorCode::PartialResult, "a process could not be read")
                };
                scan.issues
                    .error(ResultIssue::new(code, message).at(format!("pid:{pid}")));
                continue;
            }
        };
        // `pid (comm) state ppid ...`: comm may hold spaces and parentheses, so cut at the last `)`.
        let (Some(open), Some(close)) = (
            stat.iter().position(|&b| b == b'('),
            stat.iter().rposition(|&b| b == b')'),
        ) else {
            continue;
        };
        if open >= close {
            continue;
        }
        let comm = &stat[open + 1..close];
        let rest = String::from_utf8_lossy(&stat[close + 1..]).into_owned();
        let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
        let exe = fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|p| p.into_os_string().into_vec())
            .filter(|b| !b.is_empty());
        let name_bytes = match exe {
            Some(path) => {
                let path = path.strip_suffix(b" (deleted)").unwrap_or(&path).to_vec();
                path.rsplit(|&b| b == b'/').next().unwrap_or(&[]).to_vec()
            }
            None => comm.to_vec(),
        };
        let Ok(name) = String::from_utf8(name_bytes) else {
            scan.issues.error(
                ResultIssue::new(
                    ErrorCode::InvalidEncoding,
                    "a process name is not valid UTF-8; it is not listed",
                )
                .at(format!("pid:{pid}")),
            );
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let matches = job.matching.as_deref().is_none_or(|t| name.contains(t));
        let (mut uid, mut rss_bytes) = (None, None);
        if matches {
            match fs::read_to_string(format!("/proc/{pid}/status")) {
                Ok(status) => {
                    for line in status.lines() {
                        if let Some(v) = line.strip_prefix("Uid:") {
                            uid = v
                                .split_ascii_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok());
                        } else if let Some(v) = line.strip_prefix("VmRSS:") {
                            rss_bytes = v
                                .split_ascii_whitespace()
                                .next()
                                .and_then(|n| n.parse::<u64>().ok())
                                .map(|kib| kib.saturating_mul(1024));
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound || e.raw_os_error() == Some(3) => {
                    gone(scan, pid);
                    continue;
                }
                Err(_) => scan.issues.error(
                    ResultIssue::new(ErrorCode::PartialResult, "a process could not be read")
                        .at(format!("pid:{pid}")),
                ),
            }
        }
        // USER_HZ is 100 on every Linux architecture axvern supports.
        let started_at = fields
            .get(19)
            .and_then(|t| t.parse::<u64>().ok())
            .zip(boot)
            .map(|(ticks, boot)| rfc3339(boot + ticks / 100));
        let item = ProcessItem {
            pid,
            ppid: fields.get(1).and_then(|v| v.parse().ok()),
            uid,
            name,
            state: fields.first().map(|s| state_name(s.as_bytes()[0])),
            rss_bytes,
            started_at,
        };
        if !keep(job, scan, pid, item) {
            return Ok(());
        }
    }
    Ok(())
}
/// Elsewhere the portable `ps` columns are used; the start time is not available without a
/// zone-aware format, so it stays null.
#[cfg(not(target_os = "linux"))]
fn read_table(job: &ProcessesJob, deadline: Instant, scan: &mut Scan) -> Result<(), ResultIssue> {
    let mut command = Command::new("ps");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .args(["-ax", "-o", "pid=,ppid=,uid=,state=,rss=,comm="]);
    let out = capture(&mut command, deadline, 16 * 1024 * 1024).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ResultIssue::new(ErrorCode::MissingTool, "ps is not installed")
        } else {
            ResultIssue::new(ErrorCode::BackendFailure, "ps could not be started")
        }
    })?;
    if out.timed_out {
        return Err(ResultIssue::new(
            ErrorCode::Timeout,
            "ps did not finish in time",
        ));
    }
    if out.code != Some(0) && !out.capped {
        return Err(ResultIssue::new(ErrorCode::BackendFailure, "ps failed"));
    }
    if out.capped {
        scan.reasons.push(TruncationReason::BodyBytes);
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut table: Vec<(u32, ProcessItem)> = Vec::new();
    for line in text.lines() {
        // The command is last and may contain spaces; the five numeric columns are fixed.
        let mut parts = line.trim_start().splitn(6, char::is_whitespace);
        let mut next = || {
            let mut word = parts.next()?;
            while word.is_empty() {
                word = parts.next()?;
            }
            Some(word)
        };
        let (Some(pid), Some(ppid), Some(uid), Some(state), Some(rss)) =
            (next(), next(), next(), next(), next())
        else {
            continue;
        };
        let command = parts.collect::<Vec<_>>().join(" ");
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let name = command
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        if name.is_empty() {
            continue;
        }
        table.push((
            pid,
            ProcessItem {
                pid,
                ppid: ppid.parse().ok(),
                uid: uid.parse().ok(),
                name,
                state: state.bytes().next().map(state_name),
                rss_bytes: rss.parse::<u64>().ok().map(|kib| kib.saturating_mul(1024)),
                started_at: None,
            },
        ));
    }
    table.sort_by_key(|(pid, _)| *pid);
    for (index, (pid, item)) in table.into_iter().enumerate() {
        if index as u64 >= job.limits.max_entries {
            scan.reasons.push(TruncationReason::MaxEntries);
            break;
        }
        if !keep(job, scan, pid, item) {
            break;
        }
    }
    Ok(())
}

fn state_name(code: u8) -> String {
    match code {
        b'R' => "running",
        b'S' => "sleeping",
        b'D' => "uninterruptible",
        b'Z' => "zombie",
        b'T' | b't' => "stopped",
        b'I' => "idle",
        b'X' | b'x' => "dead",
        _ => "other",
    }
    .into()
}
/// `2026-10-15T00:00:00Z` for a Unix time in seconds (proleptic Gregorian, UTC).
pub(crate) fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (H. Hinnant), days counted from 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

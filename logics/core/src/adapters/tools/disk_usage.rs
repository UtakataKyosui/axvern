//! `system disk-usage`, measured by axvern itself.
//!
//! dust cannot back this command (spike #23): its JSON has no partial marker, counts
//! hard links twice in apparent mode and lists no node type. The walk therefore runs in the
//! background worker instead of a child process; the plan only names the pseudo-program
//! `axvern-disk-usage` so that Policy can judge the request like any other.
//!
//! Sizes are `st_blocks * 512` (allocated) or the logical length (apparent). A directory's own
//! entry is counted. Symbolic links are counted as links and never followed; a directory on
//! another filesystem than the root is not entered. Hidden entries, ignored files and `.git`
//! contents are all measured: this is a capacity question, not a code search.
//! Entries are visited in byte order of their names, so a hard-linked file is attributed to the
//! first path in that order and counted once.
use super::*;
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    time::Instant,
};

pub const PROGRAM: &str = "axvern-disk-usage";
pub const NATIVE: ToolSpec = ToolSpec {
    name: PROGRAM,
    purpose: "disk usage (built in, no external CLI)",
    min_version: [0, 0, 0],
    tested_version: "built in",
    capable: any_build,
    feature: "",
};

pub struct DiskUsage(pub DiskUsageJob);
impl DiscoveryAdapter for DiskUsage {
    fn tool(&self) -> &'static ToolSpec {
        &NATIVE
    }
    fn plan(&self) -> Result<ExecutionPlan, Error> {
        let j = &self.0;
        let mode = match j.size_mode {
            SizeMode::Allocated => "allocated",
            SizeMode::Apparent => "apparent",
        };
        let args: Vec<OsString> = [
            "--size".to_owned(),
            mode.to_owned(),
            "--depth".to_owned(),
            j.depth.to_string(),
            "--top".to_owned(),
            j.top.to_string(),
            "--".to_owned(),
            j.file.clone().unwrap_or_else(|| ".".into()),
        ]
        .map(OsString::from)
        .into();
        Ok(ExecutionPlan {
            program: PROGRAM.into(),
            args,
            cwd: PathBuf::from(&j.root),
        })
    }
    fn normalizer(&self, _byte_budget: u64) -> Result<Box<dyn Normalizer>, Error> {
        Err(Error::new(
            ErrorCode::BackendFailure,
            "disk usage runs in the worker, not through a child process",
        ))
    }
    fn is_native(&self) -> bool {
        true
    }
    fn preflight(&self) -> Result<(), Error> {
        Ok(())
    }
    fn run_native(&self, deadline: Instant) -> Option<DiscoveryOutcome> {
        Some(measure(&self.0, deadline))
    }
}

struct Node {
    /// Root-relative path, or None when the name is not valid UTF-8.
    path: Option<String>,
    depth: u32,
    bytes: u64,
}
struct Walk<'a> {
    job: &'a DiskUsageJob,
    deadline: Instant,
    device: u64,
    seen_links: HashSet<(u64, u64)>,
    nodes: Vec<Node>,
    entries: u64,
    issues: Issues,
    stop: Option<TruncationReason>,
    lower_bound: bool,
}
fn size(job: &DiskUsageJob, meta: &fs::Metadata) -> u64 {
    match job.size_mode {
        SizeMode::Allocated => meta.blocks().saturating_mul(512),
        SizeMode::Apparent => meta.len(),
    }
}
fn join(parent: &Option<String>, name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(name).ok()?;
    match parent {
        Some(p) if p.is_empty() => Some(name.to_owned()),
        Some(p) => Some(format!("{p}/{name}")),
        None => None,
    }
}

impl Walk<'_> {
    /// Bytes of the entry at `path` (relative path `rel`), including everything below it.
    /// Iterative: a deeply nested tree must not exhaust the stack.
    fn measure(&mut self, path: PathBuf, rel: Option<String>, meta: fs::Metadata) -> u64 {
        // Each frame: the node index, directory listing still to visit.
        struct Frame {
            index: usize,
            path: PathBuf,
            rel: Option<String>,
            depth: u32,
            pending: std::vec::IntoIter<Vec<u8>>,
            bytes: u64,
        }
        let leaf = |this: &mut Self, rel: &Option<String>, depth: u32, bytes: u64| {
            this.nodes.push(Node {
                path: rel.clone(),
                depth,
                bytes,
            });
        };
        let mut stack: Vec<Frame> = Vec::new();
        let mut result = 0;
        let mut next: Option<(PathBuf, Option<String>, u32, fs::Metadata)> =
            Some((path, rel, 0, meta));
        loop {
            if let Some((path, rel, depth, meta)) = next.take() {
                if Instant::now() >= self.deadline {
                    self.stop.get_or_insert(TruncationReason::Timeout);
                    self.lower_bound = true;
                }
                if self.stop.is_some() {
                    // Everything not yet measured is missing from the totals.
                } else if self.entries >= self.job.limits.max_entries && depth > 0 {
                    self.stop = Some(TruncationReason::MaxEntries);
                    self.lower_bound = true;
                } else {
                    if depth > 0 {
                        self.entries += 1;
                    }
                    let own = if meta.nlink() > 1
                        && !meta.is_dir()
                        && !self.seen_links.insert((meta.dev(), meta.ino()))
                    {
                        0
                    } else {
                        size(self.job, &meta)
                    };
                    if meta.is_dir() {
                        let names = self.list(&path, &rel);
                        let index = self.nodes.len();
                        self.nodes.push(Node {
                            path: rel.clone(),
                            depth,
                            bytes: 0,
                        });
                        stack.push(Frame {
                            index,
                            path,
                            rel,
                            depth,
                            pending: names.into_iter(),
                            bytes: own,
                        });
                    } else {
                        leaf(self, &rel, depth, own);
                        result = own;
                        if let Some(frame) = stack.last_mut() {
                            frame.bytes = frame.bytes.saturating_add(own);
                        }
                    }
                }
            }
            // Advance the innermost directory to its next child, or close it.
            let Some(frame) = stack.last_mut() else {
                break;
            };
            match frame.pending.next() {
                Some(name) if self.stop.is_none() => {
                    let child = frame
                        .path
                        .join(OsString::from(std::ffi::OsStr::from_bytes(&name)));
                    let child_rel = join(&frame.rel, &name);
                    let depth = frame.depth + 1;
                    if child_rel.is_none() {
                        self.issues.error(
                            ResultIssue::new(
                                ErrorCode::InvalidEncoding,
                                "an entry name is not valid UTF-8; it is counted but not listed",
                            )
                            .at(frame.rel.clone().unwrap_or_else(|| ".".into())),
                        );
                    }
                    match fs::symlink_metadata(&child) {
                        Ok(meta) if meta.is_dir() && meta.dev() != self.device => {
                            self.issues.warning(
                                WarningCode::OtherFilesystemSkipped,
                                "a directory on another filesystem was not entered",
                                child_rel.clone(),
                            );
                        }
                        Ok(meta) => next = Some((child, child_rel, depth, meta)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            self.issues.warning(
                                WarningCode::EntryVanished,
                                "the entry disappeared during the scan",
                                child_rel,
                            );
                        }
                        Err(e) => self.unreadable(&e, &child_rel),
                    }
                }
                Some(_) => {
                    // Stopped: the rest of this directory is unmeasured.
                    frame.pending = Vec::new().into_iter();
                }
                None => {
                    let done = stack.pop().expect("open frame");
                    self.nodes[done.index].bytes = done.bytes;
                    result = done.bytes;
                    if let Some(parent) = stack.last_mut() {
                        parent.bytes = parent.bytes.saturating_add(done.bytes);
                    }
                }
            }
        }
        result
    }
    /// Names in byte order. Reading stops at the deadline and after one name more than the
    /// remaining entry budget, so a huge directory can neither outrun the timeout nor be held
    /// in memory whole; the walk then ends on the budget as for any other bound.
    fn list(&mut self, path: &Path, rel: &Option<String>) -> Vec<Vec<u8>> {
        let reader = match fs::read_dir(path) {
            Ok(reader) => reader,
            Err(e) => {
                self.unreadable(&e, rel);
                return Vec::new();
            }
        };
        let room = self.job.limits.max_entries.saturating_sub(self.entries);
        let mut names: Vec<Vec<u8>> = Vec::new();
        for (count, entry) in reader.enumerate() {
            if count % 1024 == 0 && Instant::now() >= self.deadline {
                self.stop.get_or_insert(TruncationReason::Timeout);
                self.lower_bound = true;
                break;
            }
            match entry {
                Ok(entry) => names.push(entry.file_name().as_bytes().to_vec()),
                Err(e) => self.unreadable(&e, rel),
            }
            if names.len() as u64 > room {
                break;
            }
        }
        names.sort();
        names
    }
    fn unreadable(&mut self, e: &std::io::Error, rel: &Option<String>) {
        self.lower_bound = true;
        let path = rel.clone().unwrap_or_else(|| ".".into());
        let (code, message) = if e.kind() == std::io::ErrorKind::PermissionDenied {
            (ErrorCode::PermissionDenied, "path is not readable")
        } else {
            (ErrorCode::PartialResult, "path could not be measured")
        };
        self.issues.error(ResultIssue::new(code, message).at(path));
    }
}

fn measure(job: &DiskUsageJob, deadline: Instant) -> DiscoveryOutcome {
    let root = PathBuf::from(&job.root);
    let target = job.file.as_ref().map_or(root.clone(), |f| root.join(f));
    let failed = |code: ErrorCode, message: &str| DiscoveryOutcome {
        state: ChildState::Failed,
        exit_code: Some(1),
        document: DiscoveryDocument {
            summary: summary(job, 0, true, 0),
            items: Vec::new(),
            completeness: Completeness::new(Vec::new(), true),
            errors: vec![ResultIssue::new(code, message)],
            warnings: Vec::new(),
        },
    };
    let meta = match fs::symlink_metadata(&target) {
        Ok(meta) => meta,
        Err(e) => {
            return failed(classify_io(&e), "the target could not be measured");
        }
    };
    let mut walk = Walk {
        job,
        deadline,
        device: meta.dev(),
        seen_links: HashSet::new(),
        nodes: Vec::new(),
        entries: 0,
        issues: Issues::default(),
        stop: None,
        lower_bound: false,
    };
    let rel = job.file.is_none().then(String::new).or(job.file.clone());
    let total = walk.measure(target, rel, meta);
    // Listing: nodes within the display depth, biggest first, ties by path.
    let mut listed: Vec<(&str, u64)> = walk
        .nodes
        .iter()
        .filter(|n| n.depth >= 1 && n.depth <= job.depth)
        .filter_map(|n| n.path.as_deref().map(|p| (p, n.bytes)))
        .collect();
    if job.file.is_some() {
        listed = walk
            .nodes
            .first()
            .and_then(|n| n.path.as_deref().map(|p| (p, n.bytes)))
            .into_iter()
            .collect();
    }
    listed.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.as_bytes().cmp(b.0.as_bytes()))
    });
    let mut reasons = Vec::new();
    if let Some(reason) = walk.stop {
        reasons.push(reason);
    }
    let mut cut = job.top as usize;
    if listed.len() > cut.min(job.limits.max_items as usize)
        && job.limits.max_items < job.top as u64
    {
        reasons.push(TruncationReason::MaxItems);
        cut = job.limits.max_items as usize;
    }
    let items: Vec<DiscoveryItem> = listed
        .into_iter()
        .take(cut)
        .map(|(path, bytes)| {
            DiscoveryItem::Usage(DiskUsageItem {
                path: path.to_owned(),
                bytes,
            })
        })
        .collect();
    let partial = !walk.issues.errors.is_empty();
    DiscoveryOutcome {
        state: ChildState::Done,
        exit_code: Some(0),
        document: DiscoveryDocument {
            summary: summary(job, total, walk.lower_bound, walk.entries),
            items,
            completeness: Completeness::new(reasons, partial),
            errors: walk.issues.errors,
            warnings: walk.issues.warnings,
        },
    }
}
fn classify_io(e: &std::io::Error) -> ErrorCode {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        std::io::ErrorKind::NotFound => ErrorCode::TargetNotFound,
        _ => ErrorCode::BackendFailure,
    }
}
fn summary(job: &DiskUsageJob, total: u64, lower_bound: bool, entries: u64) -> DiscoverySummary {
    DiscoverySummary::DiskUsage {
        root: job.root.clone(),
        size_mode: job.size_mode,
        unit: "bytes".into(),
        depth: job.depth,
        top: job.top,
        total_bytes: total,
        lower_bound,
        entries,
    }
}

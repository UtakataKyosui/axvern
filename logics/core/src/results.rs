//! Private immutable run previews, captured before the Pueue task completes.
//! Manifest tombstones and request reservations outlive the captured bytes.
use crate::{config::Results, store::ExecutionStore};
use axvern_models::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

fn failure() -> Error {
    Error::new(
        ErrorCode::StoreFailure,
        "result store unavailable; execution was not retried",
    )
}
fn io<T>(r: std::io::Result<T>) -> Result<T, Error> {
    r.map_err(|_| failure())
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    schema_version: u32,
    pub execution_id: ExecutionId,
    pub finished_at: Option<u64>,
    pub state: Option<ChildState>,
    pub exit_code: Option<i32>,
    pub availability: String,
    /// `run` stores raw combined output; other kinds store a `DiscoveryDocument`.
    #[serde(default = "run_kind")]
    pub kind: String,
    reserved_bytes: u64,
    bytes: u64,
    truncated: bool,
    pub error: Option<Error>,
    /// State of a logical parent, which may be `partial`; `state` then only tells whether
    /// any child succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_state: Option<TaskState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion: Option<Completion>,
    #[serde(skip)]
    _lease: Option<Arc<File>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    finished_at: u64,
    state: ChildState,
    exit_code: Option<i32>,
}
fn run_kind() -> String {
    "run".into()
}
fn private(path: &Path, directory: bool) -> Result<(), Error> {
    let m = io(path.symlink_metadata())?;
    if m.file_type().is_symlink()
        || (directory && !m.is_dir())
        || (!directory && !m.is_file())
        || m.permissions().mode() & 0o777 != if directory { 0o700 } else { 0o600 }
    {
        return Err(Error::new(
            ErrorCode::PermissionDenied,
            "result store must be private without symlinks",
        ));
    }
    Ok(())
}
fn directory(store: &ExecutionStore) -> Result<PathBuf, Error> {
    let path = store.root.join("results");
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(failure()),
    }
    private(&path, true)?;
    Ok(path)
}
fn publish(dir: &Path, m: &Manifest) -> Result<(), Error> {
    let scratch = dir.join(format!("{}.scratch", m.execution_id));
    if scratch.symlink_metadata().is_ok() {
        private(&scratch, false)?;
        io(fs::remove_file(&scratch))?;
    }
    let mut f = io(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&scratch))?;
    io(f.write_all(&serde_json::to_vec(m).map_err(|_| failure())?))?;
    io(f.sync_all())?;
    io(fs::rename(
        scratch,
        dir.join(format!("{}.result", m.execution_id)),
    ))?;
    io(io(File::open(dir))?.sync_all())
}
fn read(dir: &Path, id: &ExecutionId) -> Result<Option<Manifest>, Error> {
    let path = dir.join(format!("{id}.result"));
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failure()),
        Ok(_) => (),
    }
    private(&path, false)?;
    let mut bytes = Vec::new();
    io(io(File::open(path))?.take(16385).read_to_end(&mut bytes))?;
    let m: Manifest = serde_json::from_slice(&bytes).map_err(|_| failure())?;
    if m.schema_version != 1
        || m.execution_id != *id
        || m.bytes > 1073741824
        || m.reserved_bytes > 1073741824
    {
        return Err(failure());
    }
    if !matches!(
        m.availability.as_str(),
        "pending" | "available" | "expired" | "evicted" | "unavailable"
    ) || m.finished_at.is_some() != m.state.is_some()
        || (m.availability == "pending" && m.finished_at.is_some())
        || (m.availability == "available" && m.finished_at.is_none())
        || m.completion
            .as_ref()
            .is_some_and(|c| !matches!(c.state, ChildState::Done | ChildState::Failed))
    {
        return Err(failure());
    }
    Ok(Some(m))
}
fn manifests(dir: &Path) -> Result<Vec<Manifest>, Error> {
    let mut out = vec![];
    for entry in io(fs::read_dir(dir))? {
        let path = io(entry)?.path();
        if path.extension().is_some_and(|e| e == "result") {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(failure)?
                .parse()
                .map_err(|_| failure())?;
            out.push(read(dir, &id)?.ok_or_else(failure)?);
        }
    }
    out.sort_by_key(|m| (m.finished_at, m.execution_id.to_string()));
    Ok(out)
}
fn usage(root: &Path) -> Result<u64, Error> {
    let mut n = 0u64;
    for entry in io(fs::read_dir(root))? {
        let path = io(entry)?.path();
        let m = io(path.symlink_metadata())?;
        if m.is_dir() {
            private(&path, true)?;
            n = n.saturating_add(usage(&path)?);
        } else {
            private(&path, false)?;
            if !path.extension().is_some_and(|e| e == "spool") {
                n = n.saturating_add(m.len());
            }
        }
    }
    Ok(n)
}
fn remove_bytes(dir: &Path, m: &mut Manifest, why: &str) -> Result<(), Error> {
    let path = dir.join(format!("{}.output", m.execution_id));
    if m.availability == why && !path.try_exists().map_err(|_| failure())? {
        return Ok(());
    }
    // Publish tombstone first. A crash can leave inaccessible bytes, never a false available result.
    m.availability = why.into();
    publish(dir, m)?;
    match fs::remove_file(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(failure()),
    }
    io(io(File::open(dir))?.sync_all())
}
fn maintain(store: &ExecutionStore, settings: &Results, needed: u64) -> Result<u64, Error> {
    let dir = directory(store)?;
    let mut entries = manifests(&dir)?;
    for m in &mut entries {
        if m.availability == "available"
            && m.finished_at
                .is_some_and(|t| now().saturating_sub(t) >= settings.retention_seconds)
        {
            remove_bytes(&dir, m, "expired")?;
        }
        // Finish deletion after an interrupted eviction.
        if matches!(m.availability.as_str(), "expired" | "evicted") {
            remove_bytes(&dir, m, &m.availability.clone())?;
        }
    }
    let reserved: u64 = entries
        .iter()
        .filter(|m| {
            m.finished_at.is_none() || (m.availability == "unavailable" && m.completion.is_some())
        })
        .map(|m| m.reserved_bytes)
        .sum();
    let mut used = usage(&store.root)?.saturating_add(reserved);
    for m in entries.iter_mut().filter(|m| m.availability == "available") {
        if used.saturating_add(needed) <= settings.capacity_bytes {
            break;
        }
        remove_bytes(&dir, m, "evicted")?;
        used = usage(&store.root)?.saturating_add(reserved);
    }
    Ok(settings.capacity_bytes.saturating_sub(used))
}
/// Caller holds the store lock (also used by durable request registration).
pub fn preflight_registration_locked(
    store: &ExecutionStore,
    settings: &Results,
    plan: &ExecutionPlan,
) -> Result<(), Error> {
    let dto = JsonExecutionPlan::try_from(plan)?;
    // An atomic intent update temporarily needs both the old document and its
    // replacement, in addition to the execution record and result manifest.
    let needed = 2 * serde_json::to_vec(&dto).map_err(|_| failure())?.len() as u64 + 65536;
    if maintain(store, settings, needed)? < needed {
        return Err(Error::new(
            ErrorCode::StorageFull,
            "no capacity for registration metadata",
        ));
    }
    Ok(())
}
/// Reserve bounded spool capacity before launching the target. Never evict active data.
pub fn reserve(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
) -> Result<(Manifest, File), Error> {
    let _lock = store.lock()?;
    let dir = directory(store)?;
    if read(&dir, id)?.is_some() {
        return Err(Error::new(
            ErrorCode::RequestConflict,
            "execution already has a result manifest",
        ));
    }
    let free = maintain(
        store,
        settings,
        settings.max_artifact_bytes.saturating_add(32768),
    )?;
    if free < 33792 {
        return Err(Error::new(
            ErrorCode::StorageFull,
            "no capacity for new result",
        ));
    }
    let budget = settings.max_artifact_bytes.min(free - 32768);
    let lease_path = dir.join(format!("{id}.lease"));
    let lease = io(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(lease_path))?;
    io(lease.lock())?;
    let m = Manifest {
        schema_version: 1,
        execution_id: id.clone(),
        finished_at: None,
        state: None,
        exit_code: None,
        availability: "pending".into(),
        kind: run_kind(),
        reserved_bytes: budget,
        bytes: 0,
        truncated: false,
        error: None,
        aggregate_state: None,
        completion: None,
        _lease: Some(Arc::new(lease)),
    };
    publish(&dir, &m)?;
    let file = io(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(format!("{id}.spool"))))?;
    Ok((m, file))
}
struct Capture {
    file: File,
    kept: u64,
    budget: u64,
    truncated: bool,
    failed: bool,
}
fn drain(mut stream: impl Read, capture: Arc<Mutex<Capture>>) {
    let mut buffer = [0; 8192];
    loop {
        let n = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                if let Ok(mut c) = capture.lock() {
                    c.failed = true;
                }
                break;
            }
        };
        // Forward a combined log to Pueue, independently of private preview persistence.
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(&buffer[..n]);
        if let Ok(mut c) = capture.lock() {
            let keep = n.min(c.budget.saturating_sub(c.kept) as usize);
            if c.file.write_all(&buffer[..keep]).is_err() {
                c.failed = true;
            }
            c.kept += keep as u64;
            c.truncated |= keep < n;
        }
    }
}
/// Reserve capacity for the worker's result. When that fails the task has been submitted but
/// no target process starts, so keep a typed explanation whenever metadata can still be published.
pub(crate) fn reserve_or_record(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
) -> Result<(Manifest, File), Error> {
    match reserve(store, settings, id) {
        Ok(reservation) => Ok(reservation),
        Err(error) => {
            if let Ok(_lock) = store.lock()
                && let Ok(dir) = directory(store)
                && read(&dir, id)?.is_none()
            {
                let unavailable = Manifest {
                    schema_version: 1,
                    execution_id: id.clone(),
                    finished_at: Some(now()),
                    state: Some(ChildState::Failed),
                    exit_code: None,
                    availability: "unavailable".into(),
                    kind: run_kind(),
                    reserved_bytes: 0,
                    bytes: 0,
                    truncated: false,
                    error: Some(error.clone()),
                    aggregate_state: None,
                    completion: None,
                    _lease: None,
                };
                let _ = publish(&dir, &unavailable);
            }
            Err(error)
        }
    }
}
/// Make a finished spool the immutable artifact and publish its manifest.
pub(crate) fn publish_available(
    store: &ExecutionStore,
    id: &ExecutionId,
    m: &mut Manifest,
    bytes: u64,
    truncated: bool,
) -> Result<(), Error> {
    m.bytes = bytes;
    m.truncated = truncated;
    m.availability = "available".into();
    let _lock = store.lock()?;
    let dir = directory(store)?;
    // Persist proof only after the target and capture readers finished and the spool
    // was synced. Keep the pending reservation until immutable publication completes.
    let mut pending = read(&dir, id)?.ok_or_else(failure)?;
    pending.completion = Some(Completion {
        finished_at: m.finished_at.ok_or_else(failure)?,
        state: m.state.ok_or_else(failure)?,
        exit_code: m.exit_code,
    });
    publish(&dir, &pending)?;
    io(fs::rename(
        dir.join(format!("{id}.spool")),
        dir.join(format!("{id}.output")),
    ))?;
    publish(&dir, m)
}
/// Explicit recovery, never a read side effect. Missing proof leaves reservations intact.
/// Pueue identity, completion receipt and worker lease are checked under the store lock.
pub fn recover(
    store: &ExecutionStore,
    adapter: &crate::adapters::pueue::PueueAdapter,
    id: &ExecutionId,
) -> Result<(), Error> {
    let _lock = store.lock()?;
    let record = store.read(id)?;
    let dir = directory(store)?;
    let mut m = read(&dir, id)?.ok_or_else(failure)?;
    if m.availability != "pending"
        && !(m.availability == "unavailable" && m.completion.is_some() && m.reserved_bytes > 0)
    {
        return Ok(());
    }
    let review = || {
        Error::new(
            ErrorCode::RequestConflict,
            "recovery requires confirmed target completion and an inactive worker; reservation retained",
        )
    };
    let receipt = m.completion.clone().ok_or_else(review)?;
    let path = dir.join(format!("{id}.lease"));
    private(&path, false)?;
    let lease = io(OpenOptions::new().read(true).write(true).open(path))?;
    match lease.try_lock() {
        Ok(()) => (),
        Err(std::fs::TryLockError::WouldBlock) => return Err(review()),
        Err(_) => return Err(failure()),
    }
    let task = adapter.inspect_owned(&record.backend)?;
    if !matches!(task.state, ChildState::Done | ChildState::Failed) || task.finished_at.is_none() {
        return Err(review());
    }
    // A completed capture may have died between rename and manifest publication.
    // Do not infer a complete artifact from leftover bytes; retain the target status
    // and explicitly record unavailable before releasing any reserved capacity.
    for extension in ["spool", "output"] {
        let path = dir.join(format!("{id}.{extension}"));
        match path.symlink_metadata() {
            Ok(_) => private(&path, false)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(failure()),
        }
    }
    m.finished_at = Some(receipt.finished_at);
    m.state = Some(receipt.state);
    m.exit_code = receipt.exit_code;
    m.availability = "unavailable".into();
    m.error = Some(Error::new(
        ErrorCode::StoreFailure,
        "worker interrupted during result publication; target was not retried",
    ));
    // Publish a conservative tombstone while still accounting for leftover spool.
    publish(&dir, &m)?;
    for extension in ["spool", "output"] {
        let path = dir.join(format!("{id}.{extension}"));
        match fs::remove_file(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(failure()),
        }
    }
    io(io(File::open(&dir))?.sync_all())?;
    m.reserved_bytes = 0;
    m.bytes = 0;
    publish(&dir, &m)
}
/// Every failure after the target completed must preserve the target's actual status and
/// expose a separate store error.
pub(crate) fn record_persistence_failure(
    store: &ExecutionStore,
    id: &ExecutionId,
    m: &mut Manifest,
    error: Error,
) {
    m.availability = "unavailable".into();
    m.error = Some(error);
    m.reserved_bytes = 0;
    m.bytes = 0;
    if let Ok(_lock) = store.lock()
        && let Ok(dir) = directory(store)
        && publish(&dir, m).is_ok()
    {
        for extension in ["spool", "output"] {
            let path = dir.join(format!("{id}.{extension}"));
            if path.symlink_metadata().is_ok() && private(&path, false).is_ok() {
                let _ = fs::remove_file(path);
            }
        }
        let _ = File::open(&dir).and_then(|f| f.sync_all());
    }
    eprintln!("result persistence failed; target exit status preserved");
}
impl Manifest {
    /// Record the target's final status; `kind` selects how the artifact is read back.
    pub(crate) fn complete(&mut self, kind: &str, state: ChildState, exit_code: Option<i32>) {
        self.kind = kind.into();
        self.finished_at = Some(now());
        self.state = Some(state);
        self.exit_code = exit_code;
    }
    pub(crate) fn reserved(&self) -> u64 {
        self.reserved_bytes
    }
}
/// Called only by the internal runner registered with Pueue. No new Pueue tasks.
pub fn run_worker(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
    plan: &ExecutionPlan,
) -> Result<i32, Error> {
    let (mut m, file) = reserve_or_record(store, settings, id)?;
    let capture = Arc::new(Mutex::new(Capture {
        file,
        kept: 0,
        budget: m.reserved_bytes,
        truncated: false,
        failed: false,
    }));
    let child = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let status = match child {
        Ok(mut child) => {
            let out = child.stdout.take().unwrap();
            let err = child.stderr.take().unwrap();
            let a = capture.clone();
            let b = capture.clone();
            let out = std::thread::spawn(move || drain(out, a));
            let err = std::thread::spawn(move || drain(err, b));
            let status = child.wait();
            let readers_ok = out.join().is_ok() && err.join().is_ok();
            if !readers_ok {
                capture.lock().map_err(|_| failure())?.failed = true;
            }
            status.ok()
        }
        Err(_) => {
            m.error = Some(Error::new(
                ErrorCode::BackendFailure,
                "target could not be started",
            ));
            None
        }
    };
    m.finished_at = Some(now());
    m.exit_code = status.as_ref().and_then(|s| s.code());
    m.state = Some(if status.as_ref().is_some_and(|s| s.success()) {
        ChildState::Done
    } else {
        ChildState::Failed
    });
    let exit = m.exit_code.unwrap_or(1);
    let saved = (|| -> Result<(), Error> {
        let mut c = capture.lock().map_err(|_| failure())?;
        if c.failed {
            return Err(failure());
        }
        if c.truncated && c.kept > 0 {
            let tail_len = c.kept.min(4) as usize;
            io(c.file.seek(SeekFrom::End(-(tail_len as i64))))?;
            let mut tail = vec![0; tail_len];
            io(c.file.read_exact(&mut tail))?;
            if let Some(i) = tail.iter().rposition(|b| b & 0xc0 != 0x80) {
                let expected = match tail[i] {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
                if expected > tail_len - i {
                    c.kept -= (tail_len - i) as u64;
                    io(c.file.set_len(c.kept))?;
                }
            }
        }
        // Include trailing UTF-8 trimming in the durability barrier.
        io(c.file.sync_all())?;
        let (kept, truncated) = (c.kept, c.truncated);
        drop(c);
        publish_available(store, id, &mut m, kept, truncated)
    })();
    if let Err(error) = saved {
        record_persistence_failure(store, id, &mut m, error);
    }
    Ok(exit)
}
pub fn inspect(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
) -> Result<Option<Manifest>, Error> {
    let _lock = store.lock()?;
    maintain(store, settings, 0)?;
    read(&directory(store)?, id)
}
/// Cursor is an immutable-artifact ID plus byte offset encoded as hexadecimal.
fn cursor(id: &ExecutionId, offset: u64, index: u64) -> String {
    format!("{id}:{offset}:{index}")
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn offset(id: &ExecutionId, token: Option<&str>) -> Result<(u64, u64, bool), Error> {
    let Some(token) = token else {
        return Ok((0, 0, false));
    };
    let bad = || Error::new(ErrorCode::InvalidCursor, "invalid cursor");
    if token.len() > 128
        || token.len() % 2 != 0
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad());
    }
    let bytes: Vec<_> = token
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            std::str::from_utf8(c)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(bad)
        })
        .collect::<Result<_, _>>()?;
    let s = String::from_utf8(bytes).map_err(|_| bad())?;
    let (owner_offset, index) = s.rsplit_once(':').ok_or_else(bad)?;
    let (owner, n) = owner_offset.rsplit_once(':').ok_or_else(bad)?;
    let (owner, structured) = owner
        .strip_suffix(":s")
        .map_or((owner, false), |v| (v, true));
    if owner != id.to_string() {
        return Err(bad());
    }
    let position: u64 = n.parse().map_err(|_| bad())?;
    let fragment: u64 = index.parse().map_err(|_| bad())?;
    if position.to_string() != n
        || fragment.to_string() != index
        || (structured && position == 0)
        || (!structured && (fragment > position || (position == 0) != (fragment == 0)))
    {
        return Err(bad());
    }
    Ok((position, fragment, structured))
}
pub fn page(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
    token: Option<&str>,
    limit: usize,
    max_bytes: usize,
) -> Result<(Option<Manifest>, Value), Error> {
    let (start, mut index, structured) = offset(id, token)?;
    let _lock = store.lock()?;
    maintain(store, settings, 0)?;
    let dir = directory(store)?;
    let Some(m) = read(&dir, id)? else {
        return Ok((
            None,
            json!({"result_availability":"unavailable", "result": null}),
        ));
    };
    if m.availability != "available" {
        return Ok((
            Some(m.clone()),
            json!({"result_availability":m.availability,"result":null,"result_error":m.error}),
        ));
    }
    if m.kind != "run" {
        let output = structured_page(
            &dir,
            &m,
            start,
            if structured { index } else { 0 },
            limit,
            max_bytes,
        );
        let output = match output {
            Ok(output) => output,
            Err(error) if error.code == ErrorCode::UnsupportedToolCapability => {
                let mut unavailable = m;
                unavailable.availability = "unavailable".into();
                unavailable.error = Some(error.clone());
                return Ok((
                    Some(unavailable),
                    json!({"result_availability":"unavailable","result":null,"result_error":error}),
                ));
            }
            Err(error) => return Err(error),
        };
        return Ok((Some(m), output));
    }
    if structured {
        return Err(Error::new(ErrorCode::InvalidCursor, "invalid cursor"));
    }
    if start > m.bytes {
        return Err(Error::new(ErrorCode::InvalidCursor, "invalid cursor"));
    }
    let path = dir.join(format!("{id}.output"));
    private(&path, false)?;
    let mut file = io(File::open(path))?;
    io(file.seek(SeekFrom::Start(start)))?;
    if start < m.bytes {
        let mut first = [0];
        io(file.read_exact(&mut first))?;
        if first[0] & 0xc0 == 0x80 {
            return Err(Error::new(
                ErrorCode::InvalidCursor,
                "cursor splits a UTF-8 character",
            ));
        }
        io(file.seek(SeekFrom::Start(start)))?;
    }
    let mut items = Vec::new();
    let mut position = start;
    while position < m.bytes && items.len() < limit {
        let mut bytes = vec![0; 8192.min((m.bytes - position) as usize)];
        io(file.read_exact(&mut bytes))?;
        // A preview is explicitly text. Invalid UTF-8 is never replaced silently.
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(e) if e.error_len().is_none() && position + (bytes.len() as u64) < m.bytes => {
                e.valid_up_to()
            }
            Err(e) if e.error_len().is_none() && m.truncated => e.valid_up_to(),
            Err(_) => {
                return Err(Error::new(
                    ErrorCode::InvalidEncoding,
                    "captured output is not UTF-8 text",
                ));
            }
        };
        if valid < bytes.len() {
            io(file.seek(SeekFrom::Current(-((bytes.len() - valid) as i64))))?;
            bytes.truncate(valid);
        }
        if bytes.is_empty() {
            break;
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| failure())?;
        let mut take = bytes.len();
        loop {
            while take > 0 && !text.is_char_boundary(take) {
                take -= 1;
            }
            if take == 0 {
                break;
            }
            items.push(json!({"item_id":"combined", "fragment_index":index, "continued":position+(take as u64)<m.bytes,"text":&text[..take]}));
            let candidate = page_output(&m, &items, position + take as u64, index + 1);
            if fits_page(&m, &candidate, max_bytes)? {
                break;
            }
            items.pop();
            take /= 2;
        }
        if take == 0 {
            break;
        }
        position += take as u64;
        index += 1;
        io(file.seek(SeekFrom::Start(position)))?;
    }
    if items.is_empty() && start < m.bytes {
        return Err(Error::new(
            ErrorCode::OutputLimitTooSmall,
            "output limit too small",
        ));
    }
    let output = page_output(&m, &items, position, index);
    if !fits_page(&m, &output, max_bytes)? {
        return Err(Error::new(
            ErrorCode::OutputLimitTooSmall,
            "output limit too small",
        ));
    }
    Ok((Some(m), output))
}

fn page_output(m: &Manifest, items: &[Value], position: u64, index: u64) -> Value {
    json!({"result_availability":"available", "result":{"kind":"run", "summary":{"exit_code":m.exit_code,"stream":"combined","finished_at":m.finished_at}, "items":items,
        "completeness":{"scope_complete":!m.truncated,"truncated":m.truncated,"truncation_reasons":if m.truncated { vec!["artifact_limit"] } else { vec![] }},
        "errors":m.error.iter().collect::<Vec<_>>(),"warnings":[],"page":{"has_more":position<m.bytes,"next_cursor":if position<m.bytes { Some(cursor(&m.execution_id,position,index)) } else { None }}}})
}
fn fits_page(m: &Manifest, output: &Value, max_bytes: usize) -> Result<bool, Error> {
    let mut response = Response::success(output);
    response.command = Some("result".into());
    response.execution_id = Some(m.execution_id.clone());
    response.state = m.state.map(Into::into);
    response.exit_code = m.exit_code;
    Ok(serde_json::to_vec(&response).map_err(|_| failure())?.len() < max_bytes)
}

/// Discovery artifacts hold one header line (summary, completeness, issues) followed by one
/// JSON line per item. A cursor is the byte offset of the next item line.
fn structured_page(
    dir: &Path,
    m: &Manifest,
    start: u64,
    mut index: u64,
    limit: usize,
    max_bytes: usize,
) -> Result<Value, Error> {
    let invalid = || Error::new(ErrorCode::InvalidCursor, "invalid cursor");
    let path = dir.join(format!("{}.output", m.execution_id));
    private(&path, false)?;
    let mut reader = BufReader::new(io(File::open(path))?);
    let mut header = Vec::new();
    io(reader.read_until(b'\n', &mut header))?;
    let head: Value = serde_json::from_slice(&header).map_err(|_| failure())?;
    let header_len = header.len() as u64;
    let mut position = header_len;
    if start != 0 {
        if start < header_len || start > m.bytes {
            return Err(invalid());
        }
        let mut previous = [0];
        io(reader.seek(SeekFrom::Start(start - 1)))?;
        io(reader.read_exact(&mut previous))?;
        if previous[0] != b'\n' {
            return Err(invalid());
        }
        position = start;
    }
    io(reader.seek(SeekFrom::Start(position)))?;
    // The envelope carries a longer command name than the one measured here.
    let budget = max_bytes.saturating_sub(16);
    let mut items = Vec::new();
    while position < m.bytes && items.len() < limit {
        let mut line = Vec::new();
        let n = io(reader.read_until(b'\n', &mut line))? as u64;
        if n == 0 {
            return Err(failure());
        }
        let item = serde_json::from_slice::<Value>(&line).map_err(|_| failure())?;
        let fragments = if index == 0
            && fits_page(
                m,
                &structured_output(m, &head, std::slice::from_ref(&item), position + n, 0),
                budget,
            )? {
            vec![item.clone()]
        } else {
            structured_fragments(&item, position)?
        };
        if fragments.len() == 1 && serde_json::to_vec(&item).map_err(|_| failure())?.len() > 65536 {
            return Err(Error::new(
                ErrorCode::UnsupportedToolCapability,
                "structured item cannot be represented without a text fragment",
            ));
        }
        if index as usize >= fragments.len() {
            return Err(invalid());
        }
        let mut advanced = false;
        for fragment in &fragments[index as usize..] {
            if items.len() == limit {
                break;
            }
            let last = index as usize + 1 == fragments.len();
            let next_position = if last { position + n } else { position };
            let next_index = if last { 0 } else { index + 1 };
            items.push(fragment.clone());
            if !fits_page(
                m,
                &structured_output(m, &head, &items, next_position, next_index),
                budget,
            )? {
                items.pop();
                break;
            }
            index = next_index;
            if last {
                position = next_position;
                advanced = true;
                break;
            }
        }
        if !advanced {
            break;
        }
    }
    if items.is_empty() && position < m.bytes {
        return Err(Error::new(
            ErrorCode::OutputLimitTooSmall,
            "output limit too small",
        ));
    }
    let output = structured_output(m, &head, &items, position, index);
    if !fits_page(m, &output, budget)? {
        return Err(Error::new(
            ErrorCode::OutputLimitTooSmall,
            "output limit too small",
        ));
    }
    Ok(output)
}
/// Fragment large string fields without rewriting the stored artifact. Offsets identify
/// the original item; only the first fragment carries its non-text attributes.
fn structured_fragments(item: &Value, position: u64) -> Result<Vec<Value>, Error> {
    if serde_json::to_vec(item).map_err(|_| failure())?.len() <= 256 {
        return Ok(vec![item.clone()]);
    }
    let Some(object) = item.as_object() else {
        return Ok(vec![item.clone()]);
    };
    let fields: Vec<_> = object
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_str()
                // The fragment payload uses `text`, so preserve an original text
                // field as its own fragment even when it is short or empty.
                .filter(|text| text.len() > 128 || key == "text")
                .map(|text| (key, text))
        })
        .collect();
    if fields.is_empty() {
        return Ok(vec![item.clone()]);
    }
    let mut attributes = object.clone();
    for (key, _) in &fields {
        attributes.remove(*key);
    }
    let mut fragments = Vec::new();
    for (field, text) in fields {
        let mut start = 0;
        let mut index = 0;
        while index == 0 || start < text.len() {
            let mut end = (start + 128).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut fragment = if fragments.is_empty() {
                attributes.clone()
            } else {
                serde_json::Map::new()
            };
            fragment.insert("item_id".into(), json!(format!("item-{position}")));
            fragment.insert("text_field".into(), json!(field));
            fragment.insert("fragment_index".into(), json!(index));
            fragment.insert("continued".into(), json!(end < text.len()));
            fragment.insert("text".into(), json!(&text[start..end]));
            fragments.push(Value::Object(fragment));
            index += 1;
            start = end;
        }
    }
    Ok(fragments)
}
fn structured_output(
    m: &Manifest,
    head: &Value,
    items: &[Value],
    position: u64,
    index: u64,
) -> Value {
    let mut output = json!({"result_availability":"available", "result":{"kind":m.kind, "summary":head["summary"], "items":items,
        "completeness":head["completeness"], "errors":head["errors"], "warnings":head["warnings"],
        "page":{"has_more":position<m.bytes,"next_cursor":if position<m.bytes { Some(format!("{}:s:{position}:{index}",m.execution_id).bytes().map(|b| format!("{b:02x}")).collect::<String>()) } else { None }}}});
    // An aggregate repeats its children's outcomes on every page.
    if let Some(children) = head.get("children") {
        output["result"]["children"] = children.clone();
    }
    output
}
/// The stored header of a structured result: summary, completeness, issues and, for an
/// aggregate, its children. None without a manifest; a null header when no artifact remains.
pub fn header(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
) -> Result<Option<(Manifest, Value)>, Error> {
    let _lock = store.lock()?;
    maintain(store, settings, 0)?;
    let dir = directory(store)?;
    let Some(m) = read(&dir, id)? else {
        return Ok(None);
    };
    if m.availability != "available" || m.kind == "run" {
        return Ok(Some((m, Value::Null)));
    }
    let path = dir.join(format!("{id}.output"));
    private(&path, false)?;
    let mut line = Vec::new();
    io(BufReader::new(io(File::open(path))?).read_until(b'\n', &mut line))?;
    let head = serde_json::from_slice(&line).map_err(|_| failure())?;
    Ok(Some((m, head)))
}
/// Holds the store lock while a result is derived from other stored results, so that the
/// inputs cannot be evicted half-read and the derived result is published at most once.
pub(crate) struct Derivation<'a> {
    store: &'a ExecutionStore,
    settings: &'a Results,
    dir: PathBuf,
    _lock: File,
}
impl<'a> Derivation<'a> {
    pub(crate) fn begin(store: &'a ExecutionStore, settings: &'a Results) -> Result<Self, Error> {
        let lock = store.lock()?;
        maintain(store, settings, 0)?;
        Ok(Self {
            dir: directory(store)?,
            store,
            settings,
            _lock: lock,
        })
    }
    pub(crate) fn manifest(&self, id: &ExecutionId) -> Result<Option<Manifest>, Error> {
        read(&self.dir, id)
    }
    /// Header and item lines (each ending in a newline) of an available structured result.
    pub(crate) fn document(&self, m: &Manifest) -> Result<(Value, Vec<Vec<u8>>), Error> {
        if m.availability != "available" || m.kind == "run" {
            return Err(failure());
        }
        let path = self.dir.join(format!("{}.output", m.execution_id));
        private(&path, false)?;
        let mut reader = BufReader::new(io(File::open(path))?.take(m.bytes));
        let mut header = Vec::new();
        io(reader.read_until(b'\n', &mut header))?;
        let head = serde_json::from_slice(&header).map_err(|_| failure())?;
        let mut items = Vec::new();
        loop {
            let mut line = Vec::new();
            if io(reader.read_until(b'\n', &mut line))? == 0 {
                break;
            }
            items.push(line);
        }
        Ok((head, items))
    }
    /// Bytes available for a new artifact of up to `wanted` bytes. Older finished results may
    /// be evicted to make room, as for any new result.
    pub(crate) fn budget(&self, wanted: u64) -> Result<u64, Error> {
        let wanted = wanted.min(self.settings.max_artifact_bytes);
        let free = maintain(self.store, self.settings, wanted.saturating_add(32768))?;
        if free < 33792 {
            return Err(Error::new(
                ErrorCode::StorageFull,
                "no capacity for new result",
            ));
        }
        Ok(wanted.min(free - 32768))
    }
    /// Publish `header` and `items` as the immutable available result of `id`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish(
        &self,
        id: &ExecutionId,
        kind: &str,
        aggregate: TaskState,
        finished_at: u64,
        header: &[u8],
        items: &[Vec<u8>],
        truncated: bool,
    ) -> Result<Manifest, Error> {
        if read(&self.dir, id)?.is_some() {
            return Err(Error::new(
                ErrorCode::RequestConflict,
                "execution already has a result manifest",
            ));
        }
        // Bytes left by an interrupted earlier attempt have no manifest and are replaced.
        for extension in ["spool", "output"] {
            let path = self.dir.join(format!("{id}.{extension}"));
            if path.symlink_metadata().is_ok() {
                private(&path, false)?;
                io(fs::remove_file(path))?;
            }
        }
        let spool = self.dir.join(format!("{id}.spool"));
        let mut file = io(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&spool))?;
        io(file.write_all(header))?;
        for line in items {
            io(file.write_all(line))?;
        }
        io(file.sync_all())?;
        let bytes = io(file.metadata())?.len();
        io(fs::rename(spool, self.dir.join(format!("{id}.output"))))?;
        let m = Manifest {
            schema_version: 1,
            execution_id: id.clone(),
            finished_at: Some(finished_at),
            state: Some(if aggregate == TaskState::Failed {
                ChildState::Failed
            } else {
                ChildState::Done
            }),
            exit_code: None,
            availability: "available".into(),
            kind: kind.into(),
            reserved_bytes: 0,
            bytes,
            truncated,
            error: None,
            aggregate_state: Some(aggregate),
            completion: None,
            _lease: None,
        };
        publish(&self.dir, &m)?;
        Ok(m)
    }
}
/// Serialized form of a discovery document: header line, then one line per item.
pub(crate) fn encode_document(doc: &DiscoveryDocument) -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
    let line = |value: Value| -> Result<Vec<u8>, Error> {
        let mut bytes = serde_json::to_vec(&value).map_err(|_| failure())?;
        bytes.push(b'\n');
        Ok(bytes)
    };
    let header = line(json!({
        "summary": doc.summary, "completeness": doc.completeness,
        "errors": doc.errors, "warnings": doc.warnings,
    }))?;
    let items = doc
        .items
        .iter()
        .map(|item| line(serde_json::to_value(item).map_err(|_| failure())?))
        .collect::<Result<_, _>>()?;
    Ok((header, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_publication_worker() {
        let Some(root) = std::env::var_os("AXVERN_RECOVERY_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let store = ExecutionStore::open(root.join("store")).unwrap();
        let id = fs::read_to_string(root.join("id"))
            .unwrap()
            .parse()
            .unwrap();
        let (mut m, mut file) = reserve(&store, &Results::default(), &id).unwrap();
        file.write_all(b"finished capture").unwrap();
        file.sync_all().unwrap();
        m.completion = Some(Completion {
            finished_at: 123,
            state: ChildState::Failed,
            exit_code: Some(23),
        });
        publish(&directory(&store).unwrap(), &m).unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn killed_worker_releases_lease_but_preserves_completion_proof() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let store = ExecutionStore::open(root.join("store")).unwrap();
        let id = ExecutionStore::allocate_id().unwrap();
        fs::write(root.join("id"), id.to_string()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "results::tests::interrupted_publication_worker"])
            .env("AXVERN_RECOVERY_TEST_ROOT", &root)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.join("ready").exists() {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("worker did not become ready");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let dir = directory(&store).unwrap();
        let lease = File::open(dir.join(format!("{id}.lease"))).unwrap();
        assert!(matches!(
            lease.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        lease.try_lock().unwrap();
        let m = read(&dir, &id).unwrap().unwrap();
        assert_eq!(m.availability, "pending");
        assert_eq!(m.completion.unwrap().exit_code, Some(23));
        assert!(m.reserved_bytes > 0);
    }

    #[test]
    fn recovery_requires_completion_inactive_lease_and_matching_terminal_backend() {
        use crate::adapters::pueue::PueueAdapter;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let store = ExecutionStore::open(root.join("store")).unwrap();
        let id = ExecutionStore::allocate_id().unwrap();
        let record = ExecutionRecord {
            execution_id: id.clone(),
            backend: BackendIdentity {
                queue: "test".into(),
                task_id: 1,
                created_at: "original".into(),
            },
            registered_at: now(),
            cwd: root.to_str().unwrap().into(),
            rewrite: None,
        };
        store.save_record(&record).unwrap();
        let script = root.join("pueue");
        let adapter = PueueAdapter {
            executable: script.clone(),
            config: root.join("config"),
            queue: "test".into(),
            timeout: std::time::Duration::from_secs(1),
        };
        let backend = |created: &str, status: Value| {
            let value = json!({"tasks":{"1":{"id":1,"created_at":created,"status":status}}});
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\n[ \"$5\" = status ] || exit 99\nprintf '%s' '{}'\n",
                    value
                ),
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        };
        let done = json!({"Done":{"result":{"Failed":9},"end":"finished"}});
        backend("original", done.clone());
        let settings = Results::default();
        let (m, mut spool) = reserve(&store, &settings, &id).unwrap();
        spool.write_all(b"partial secret").unwrap();
        let dir = directory(&store).unwrap();
        let original = fs::read(dir.join(format!("{id}.result"))).unwrap();
        assert!(recover(&store, &adapter, &id).is_err());
        assert_eq!(
            fs::read(dir.join(format!("{id}.result"))).unwrap(),
            original
        );
        let mut pending = read(&dir, &id).unwrap().unwrap();
        pending.completion = Some(Completion {
            finished_at: 123,
            state: ChildState::Failed,
            exit_code: Some(23),
        });
        publish(&dir, &pending).unwrap();
        // Even a valid completion receipt cannot override a live worker's lease.
        assert!(recover(&store, &adapter, &id).is_err());
        drop(m);
        drop(spool);
        backend("recycled", done.clone());
        assert!(recover(&store, &adapter, &id).is_err());
        backend("original", json!("Running"));
        assert!(recover(&store, &adapter, &id).is_err());
        assert_eq!(read(&dir, &id).unwrap().unwrap().availability, "pending");
        backend("original", done);
        // Also cover a crash after output rename and before final publication.
        fs::rename(
            dir.join(format!("{id}.spool")),
            dir.join(format!("{id}.output")),
        )
        .unwrap();
        recover(&store, &adapter, &id).unwrap();
        let recovered = read(&dir, &id).unwrap().unwrap();
        assert_eq!(recovered.availability, "unavailable");
        assert_eq!(recovered.exit_code, Some(23));
        assert_eq!(recovered.finished_at, Some(123));
        assert_eq!(recovered.reserved_bytes, 0);
        assert!(!dir.join(format!("{id}.output")).exists());
        assert!(!dir.join(format!("{id}.spool")).exists());
        // No backend call is needed on an idempotent replay (daemon may be gone).
        fs::remove_file(script).unwrap();
        recover(&store, &adapter, &id).unwrap();
        assert_eq!(store.history().unwrap(), vec![record]);
    }

    fn artifact(
        items: &[Value],
        summary: Value,
    ) -> (tempfile::TempDir, ExecutionStore, ExecutionId) {
        let temp = tempfile::tempdir().unwrap();
        let store =
            ExecutionStore::open(temp.path().canonicalize().unwrap().join("store")).unwrap();
        let id = ExecutionStore::allocate_id().unwrap();
        let dir = directory(&store).unwrap();
        let path = dir.join(format!("{id}.output"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        writeln!(file, "{}", json!({"summary":summary,"completeness":{"scope_complete":true,"truncated":false,"truncation_reasons":[]},"errors":[],"warnings":[]})).unwrap();
        for item in items {
            writeln!(file, "{item}").unwrap();
        }
        let m = Manifest {
            schema_version: 1,
            execution_id: id.clone(),
            finished_at: Some(now()),
            state: Some(ChildState::Done),
            exit_code: Some(0),
            availability: "available".into(),
            kind: "fs.read".into(),
            reserved_bytes: 0,
            bytes: file.metadata().unwrap().len(),
            truncated: false,
            error: None,
            aggregate_state: None,
            completion: None,
            _lease: None,
        };
        publish(&dir, &m).unwrap();
        (temp, store, id)
    }

    #[test]
    fn structured_text_fragments_reconstruct_and_replay_with_wire_budget() {
        let text = "あ\n\u{1b}".repeat(1000);
        let other = "別の本文".repeat(100);
        let (_temp, store, id) = artifact(
            &[json!({"path":"example","line":1,"body":text,"snippet":other})],
            json!({}),
        );
        let settings = Results::default();
        let mut token = None;
        let mut reconstructed = std::collections::BTreeMap::<String, String>::new();
        let mut indices = std::collections::BTreeMap::<String, u64>::new();
        let mut count = 0;
        loop {
            let (manifest, output) =
                page(&store, &settings, &id, token.as_deref(), 2, 1024).unwrap();
            assert_eq!(
                page(&store, &settings, &id, token.as_deref(), 2, 1024)
                    .unwrap()
                    .1,
                output
            );
            assert!(fits_page(&manifest.unwrap(), &output, 1024).unwrap());
            for item in output["result"]["items"].as_array().unwrap() {
                let field = item["text_field"].as_str().unwrap().to_owned();
                let index = indices.entry(field.clone()).or_default();
                assert_eq!(item["fragment_index"], *index);
                *index += 1;
                reconstructed
                    .entry(field)
                    .or_default()
                    .push_str(item["text"].as_str().unwrap());
                assert_eq!(item.get("path").is_some(), count == 0);
                count += 1;
            }
            token = output["result"]["page"]["next_cursor"]
                .as_str()
                .map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(token.as_ref().unwrap().len() <= 128);
            let foreign = ExecutionStore::allocate_id().unwrap();
            assert_eq!(
                page(&store, &settings, &foreign, token.as_deref(), 2, 1024)
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidCursor
            );
        }
        assert_eq!(reconstructed["body"], text);
        assert_eq!(reconstructed["snippet"], other);
        assert!(count > 2);
    }

    #[test]
    fn fragmented_match_path_preserves_short_and_empty_matching_text() {
        for text in ["match".to_owned(), "é".repeat(64), String::new()] {
            let path = "segment/".repeat(200);
            let item = MatchItem {
                path: path.clone(),
                line: 7,
                text: text.clone(),
                ranges: vec![],
                context_before: vec![],
                context_after: vec![],
                line_truncated: false,
            };
            let (_temp, store, id) = artifact(&[serde_json::to_value(item).unwrap()], json!({}));
            let mut token = None;
            let mut reconstructed = std::collections::BTreeMap::<String, String>::new();
            let mut indices = std::collections::BTreeMap::<String, u64>::new();
            let mut ended = std::collections::BTreeSet::new();
            let mut count = 0;
            loop {
                let (manifest, output) =
                    page(&store, &Results::default(), &id, token.as_deref(), 1, 1024).unwrap();
                assert!(fits_page(&manifest.unwrap(), &output, 1024).unwrap());
                assert_eq!(
                    page(&store, &Results::default(), &id, token.as_deref(), 1, 1024)
                        .unwrap()
                        .1,
                    output
                );
                for fragment in output["result"]["items"].as_array().unwrap() {
                    let field = fragment["text_field"].as_str().unwrap().to_owned();
                    assert!(!ended.contains(&field));
                    let index = indices.entry(field.clone()).or_default();
                    assert_eq!(fragment["fragment_index"], *index);
                    *index += 1;
                    reconstructed
                        .entry(field.clone())
                        .or_default()
                        .push_str(fragment["text"].as_str().unwrap());
                    if fragment["continued"] == false {
                        ended.insert(field);
                    }
                    assert_eq!(fragment.get("line").is_some(), count == 0);
                    count += 1;
                }
                token = output["result"]["page"]["next_cursor"]
                    .as_str()
                    .map(str::to_owned);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(reconstructed.get("path"), Some(&path));
            assert_eq!(reconstructed.get("text"), Some(&text));
            assert_eq!(ended.len(), 2);
        }
    }

    #[test]
    fn structured_record_limit_and_oversized_summary_are_explicit() {
        let records: Vec<_> = (0..101).map(|i| json!({"line":i})).collect();
        let (_temp, store, id) = artifact(&records, json!({}));
        let settings = Results::default();
        let (_, first) = page(&store, &settings, &id, None, 100, 65536).unwrap();
        assert_eq!(first["result"]["items"].as_array().unwrap().len(), 100);
        let token = first["result"]["page"]["next_cursor"].as_str().unwrap();
        let (_, last) = page(&store, &settings, &id, Some(token), 100, 65536).unwrap();
        assert_eq!(last["result"]["items"], json!([{"line":100}]));
        assert_eq!(last["result"]["page"]["has_more"], false);
        let (_temp, store, id) = artifact(&[], json!({"huge":"a".repeat(65536)}));
        assert_eq!(
            page(&store, &settings, &id, None, 100, 65536)
                .unwrap_err()
                .code,
            ErrorCode::OutputLimitTooSmall
        );
        let (_temp, store, id) = artifact(&[json!({"values":vec![0;70000]})], json!({}));
        let (manifest, output) = page(&store, &settings, &id, None, 100, 65536).unwrap();
        assert_eq!(manifest.unwrap().availability, "unavailable");
        assert!(output["result"].is_null());
        assert_eq!(
            output["result_error"]["code"],
            "unsupported_tool_capability"
        );
    }
}

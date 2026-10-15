//! Private immutable run previews, captured before the Pueue task completes.
//! Manifest tombstones and request reservations outlive the captured bytes.
use crate::{config::Results, store::ExecutionStore};
use axvern_models::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
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
    reserved_bytes: u64,
    bytes: u64,
    truncated: bool,
    pub error: Option<Error>,
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
        .filter(|m| m.finished_at.is_none())
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
    let m = Manifest {
        schema_version: 1,
        execution_id: id.clone(),
        finished_at: None,
        state: None,
        exit_code: None,
        availability: "pending".into(),
        reserved_bytes: budget,
        bytes: 0,
        truncated: false,
        error: None,
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
/// Called only by the internal runner registered with Pueue. No new Pueue tasks.
pub fn run_worker(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
    plan: &ExecutionPlan,
) -> Result<i32, Error> {
    let (mut m, file) = match reserve(store, settings, id) {
        Ok(reservation) => reservation,
        Err(error) => {
            // The task was submitted, but no target process has been started.
            // Keep a typed explanation whenever metadata can still be published.
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
                    reserved_bytes: 0,
                    bytes: 0,
                    truncated: false,
                    error: Some(error.clone()),
                };
                let _ = publish(&dir, &unavailable);
            }
            return Err(error);
        }
    };
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
        m.bytes = c.kept;
        m.truncated = c.truncated;
        m.availability = "available".into();
        let _lock = store.lock()?;
        let dir = directory(store)?;
        io(fs::rename(
            dir.join(format!("{id}.spool")),
            dir.join(format!("{id}.output")),
        ))?;
        publish(&dir, &m)
    })();
    if let Err(error) = saved {
        // Every failure after target completion, including trimming/fsync, must
        // preserve the target's actual status and expose a separate store error.
        m.availability = "unavailable".into();
        m.error = Some(error);
        m.reserved_bytes = 0;
        m.bytes = 0;
        if let Ok(_lock) = store.lock()
            && let Ok(dir) = directory(store)
            && publish(&dir, &m).is_ok()
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
fn offset(id: &ExecutionId, token: Option<&str>) -> Result<(u64, u64), Error> {
    let Some(token) = token else {
        return Ok((0, 0));
    };
    let bad = || Error::new(ErrorCode::InvalidCursor, "invalid cursor");
    if token.len() > 128 || token.len() % 2 != 0 {
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
    if owner != id.to_string() {
        return Err(bad());
    }
    let position: u64 = n.parse().map_err(|_| bad())?;
    let fragment: u64 = index.parse().map_err(|_| bad())?;
    if position.to_string() != n
        || fragment.to_string() != index
        || fragment > position
        || (position == 0) != (fragment == 0)
    {
        return Err(bad());
    }
    Ok((position, fragment))
}
pub fn page(
    store: &ExecutionStore,
    settings: &Results,
    id: &ExecutionId,
    token: Option<&str>,
    limit: usize,
    max_bytes: usize,
) -> Result<(Option<Manifest>, Value), Error> {
    let (start, mut index) = offset(id, token)?;
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

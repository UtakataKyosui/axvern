//! Small local metadata store: one immutable record per execution, OS file lock,
//! same-directory atomic publication and fsync. Never stores command output.
use crate::adapters::pueue::PueueAdapter;
use axvern_models::*;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn fail() -> Error {
    Error::new(
        ErrorCode::StoreFailure,
        "execution store I/O failed; preserve registration receipt and repair store without running add again",
    )
}
fn io<T>(result: std::io::Result<T>) -> Result<T, Error> {
    result.map_err(|_| fail())
}
fn secure(path: &Path, directory: bool) -> Result<(), Error> {
    let meta = io(fs::symlink_metadata(path))?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
        || meta.permissions().mode() & 0o777 != if directory { 0o700 } else { 0o600 }
    {
        return Err(Error::new(
            ErrorCode::PermissionDenied,
            "store requires private regular files (0600) and directory (0700), without symlinks",
        ));
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct ExecutionStore {
    pub(crate) root: PathBuf,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    record: ExecutionRecord,
}
/// Durable intent is published before `add`. An unresolved intent is never retried.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestReservation {
    pub schema_version: u32,
    pub execution_id: ExecutionId,
    pub plan: JsonExecutionPlan,
    pub record: Option<ExecutionRecord>,
}
impl ExecutionStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::open_internal(root.into(), true, None)
    }
    fn open_internal(
        root: PathBuf,
        recover: bool,
        deadline: Option<Instant>,
    ) -> Result<Self, Error> {
        if !root.is_absolute() {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "store path must be absolute",
            ));
        }
        // Require the parent to exist. Reject symlink ancestors, including dangling links.
        for ancestor in root.parent().ok_or_else(fail)?.ancestors() {
            let meta = io(fs::symlink_metadata(ancestor))?;
            if meta.file_type().is_symlink()
                || !meta.is_dir()
                || (meta.permissions().mode() & 0o022 != 0
                    && meta.permissions().mode() & 0o1000 == 0)
            {
                return Err(Error::new(
                    ErrorCode::PermissionDenied,
                    "unsafe store ancestor",
                ));
            }
        }
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(fail()),
        }
        secure(&root, true)?;
        let store = Self { root };
        let _lock = match deadline {
            Some(deadline) => store.lock_until(deadline)?,
            None => store.lock()?,
        };
        // A killed writer can leave only unpublished scratch, never a partial record.
        for entry in io(fs::read_dir(&store.root))? {
            let path = io(entry)?.path();
            if recover && path.extension().is_some_and(|v| v == "pending") {
                secure(&path, false)?;
                io(fs::remove_file(path))?;
            }
        }
        Ok(store)
    }
    /// Dedicated scratch only; no record or abandoned writer scratch is changed.
    pub fn diagnose(root: PathBuf) -> Result<(), Error> {
        Self::diagnose_until(root, Instant::now() + Duration::from_secs(30))
    }
    pub fn diagnose_until(root: PathBuf, deadline: Instant) -> Result<(), Error> {
        let store = Self::open_internal(root, false, Some(deadline))?;
        let _lock = store.lock_until(deadline)?;
        store.records()?;
        let path = store
            .root
            .join(format!(".doctor-{}.probe", Self::allocate_id()?));
        let result = (|| {
            let mut file = io(OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path))?;
            io(file.write_all(b"axvern probe"))?;
            io(file.sync_all())?;
            use std::io::{Seek, SeekFrom};
            io(file.seek(SeekFrom::Start(0)))?;
            let mut bytes = Vec::new();
            io(file.read_to_end(&mut bytes))?;
            if bytes != b"axvern probe" {
                return Err(fail());
            }
            Ok(())
        })();
        // Always attempt cleanup, including failed I/O. Report cleanup failures too.
        let cleanup = io(fs::remove_file(&path));
        result.and(cleanup)?;
        drop(_lock);
        crate::parents::history_until(&store, deadline).map(|_| ())
    }
    pub(crate) fn diagnostic_history(
        root: PathBuf,
        deadline: Instant,
    ) -> Result<Vec<ExecutionRecord>, Error> {
        let store = Self::open_internal(root, false, Some(deadline))?;
        let _lock = store.lock_until(deadline)?;
        store.records()
    }
    fn lock_file(&self) -> Result<File, Error> {
        secure(&self.root, true)?;
        let path = self.root.join("store.lock");
        if path.symlink_metadata().is_ok() {
            secure(&path, false)?;
        }
        let file = io(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path))?;
        secure(&path, false)?;
        Ok(file)
    }
    pub(crate) fn lock(&self) -> Result<File, Error> {
        let file = self.lock_file()?;
        io(file.lock())?;
        Ok(file)
    }
    pub(crate) fn lock_until(&self, deadline: Instant) -> Result<File, Error> {
        let file = self.lock_file()?;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(Error::new(
                            ErrorCode::Timeout,
                            "store lock deadline exceeded",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10).min(remaining));
                }
                Err(_) => return Err(fail()),
            }
        }
    }
    fn path(&self, id: &ExecutionId) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }
    pub fn allocate_id() -> Result<ExecutionId, Error> {
        let mut bytes = [0; 16];
        io(io(File::open("/dev/urandom"))?.read_exact(&mut bytes))?;
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "ex_{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
        .parse()
    }
    pub fn save_record(&self, record: &ExecutionRecord) -> Result<(), Error> {
        let _lock = self.lock()?;
        self.save_record_locked(record)
    }
    fn save_record_locked(&self, record: &ExecutionRecord) -> Result<(), Error> {
        if record.cwd.is_empty()
            || record.backend.queue.is_empty()
            || record.backend.created_at.is_empty()
        {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "incomplete execution metadata",
            ));
        }
        let final_path = self.path(&record.execution_id);
        if final_path.symlink_metadata().is_ok() {
            let existing = self.read(&record.execution_id)?;
            return if existing == *record {
                // The prior rename may have succeeded while its durability
                // barrier failed. Recovery must retry that barrier too.
                io(io(File::open(&self.root))?.sync_all())
            } else {
                Err(Error::new(
                    ErrorCode::StoreFailure,
                    "execution ID already maps to different metadata",
                ))
            };
        }
        let pending = self.root.join(format!("{}.pending", record.execution_id));
        // Recover our own abandoned scratch after a failed publication.
        if pending.symlink_metadata().is_ok() {
            secure(&pending, false)?;
            io(fs::remove_file(&pending))?;
        }
        let mut file = io(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pending))?;
        let bytes = serde_json::to_vec(&Document {
            schema_version: 1,
            record: record.clone(),
        })
        .map_err(|_| fail())?;
        if bytes.len() > 65536 {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "execution metadata too large",
            ));
        }
        io(file.write_all(&bytes))?;
        io(file.sync_all())?;
        io(fs::rename(pending, final_path))?;
        io(io(File::open(&self.root))?.sync_all())
    }
    pub(crate) fn read(&self, id: &ExecutionId) -> Result<ExecutionRecord, Error> {
        let path = self.path(id);
        if !path.try_exists().map_err(|_| fail())? {
            return Err(Error::new(
                ErrorCode::TaskNotFound,
                "unknown axvern execution",
            ));
        }
        secure(&path, false)?;
        let mut bytes = Vec::new();
        io(io(File::open(path))?.take(65537).read_to_end(&mut bytes))?;
        if bytes.len() > 65536 {
            return Err(fail());
        }
        let document: Document = serde_json::from_slice(&bytes).map_err(|_| fail())?;
        if document.schema_version != 1 {
            return Err(Error::new(
                ErrorCode::StoreFailure,
                "unsupported store schema; migration required",
            ));
        }
        let record = document.record;
        if record.execution_id != *id {
            return Err(fail());
        }
        Ok(record)
    }
    pub fn find_record(&self, id: &ExecutionId) -> Result<ExecutionRecord, Error> {
        let _lock = self.lock()?;
        self.read(id)
    }
    /// Contains only local registrations. Backend discovery never imports external tasks.
    pub fn history(&self) -> Result<Vec<ExecutionRecord>, Error> {
        let _lock = self.lock()?;
        self.records()
    }
    fn records(&self) -> Result<Vec<ExecutionRecord>, Error> {
        let mut records = Vec::new();
        for entry in io(fs::read_dir(&self.root))? {
            let path = io(entry)?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let id = path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .ok_or_else(fail)?
                    .parse()?;
                records.push(self.read(&id)?);
            }
        }
        records.sort_by_key(|r| (r.registered_at, r.execution_id.to_string()));
        Ok(records)
    }
    /// Explicit metadata-only cleanup. Does not delete Pueue tasks or logs.
    pub fn prune_before(&self, timestamp: u64) -> Result<usize, Error> {
        let _lock = self.lock()?;
        if self.root.join("requests").exists()
            || self.root.join("registrations").exists()
            || io(fs::read_dir(&self.root))?.any(|entry| {
                entry.is_ok_and(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|e| e == "request" || e == "parent")
                })
            })
        {
            return Err(Error::new(
                ErrorCode::StoreFailure,
                "request-id or parent records prevent metadata pruning",
            ));
        }
        let records = self.records()?;
        let mut removed = 0;
        for record in records.into_iter().filter(|r| r.registered_at < timestamp) {
            io(fs::remove_file(self.path(&record.execution_id)))?;
            removed += 1;
        }
        io(io(File::open(&self.root))?.sync_all())?;
        Ok(removed)
    }
    pub fn resolve(&self, id: &ExecutionId, adapter: &PueueAdapter) -> Result<TaskSnapshot, Error> {
        adapter.inspect_owned(&self.find_record(id)?.backend)
    }
    fn request_path(&self, key: &str) -> Result<PathBuf, Error> {
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(Error::new(ErrorCode::InvalidInput, "invalid request-id"));
        }
        // Split the encoding to stay below filesystem NAME_MAX for 128 byte keys.
        let encoded: String = key.bytes().map(|b| format!("{b:02x}")).collect();
        let dir = self.root.join("requests");
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(fail()),
        }
        secure(&dir, true)?;
        let split = encoded.len().min(16);
        let bucket = dir.join(&encoded[..split]);
        match fs::DirBuilder::new().mode(0o700).create(&bucket) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(fail()),
        }
        secure(&bucket, true)?;
        Ok(bucket.join(format!("{}.request", &encoded[split..])))
    }
    fn read_request(&self, path: &Path) -> Result<Option<RequestReservation>, Error> {
        match path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(fail()),
            Ok(_) => (),
        }
        secure(path, false)?;
        let mut bytes = Vec::new();
        io(io(File::open(path))?
            .take(128 * 1024 * 1024)
            .read_to_end(&mut bytes))?;
        let request: RequestReservation = serde_json::from_slice(&bytes).map_err(|_| fail())?;
        if request.schema_version != 1
            || request.record.as_ref().is_some_and(|r| {
                r.execution_id != request.execution_id || r.cwd != request.plan.cwd
            })
        {
            return Err(fail());
        }
        Ok(Some(request))
    }
    fn publish_request(&self, path: &Path, request: &RequestReservation) -> Result<(), Error> {
        let scratch = path.with_extension("scratch");
        if scratch.symlink_metadata().is_ok() {
            secure(&scratch, false)?;
            io(fs::remove_file(&scratch))?;
        }
        let mut file = io(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&scratch))?;
        io(file.write_all(&serde_json::to_vec(request).map_err(|_| fail())?))?;
        io(file.sync_all())?;
        io(fs::rename(scratch, path))?;
        for parent in path.parent().ok_or_else(fail)?.ancestors() {
            io(io(File::open(parent))?.sync_all())?;
            if parent == self.root {
                return Ok(());
            }
        }
        Err(fail())
    }
    pub fn request(
        &self,
        key: &str,
        plan: &ExecutionPlan,
    ) -> Result<Option<RequestReservation>, Error> {
        let _lock = self.lock()?;
        let request = self.read_request(&self.request_path(key)?)?;
        if request
            .as_ref()
            .is_some_and(|r| JsonExecutionPlan::try_from(plan).is_ok_and(|p| p != r.plan))
        {
            return Err(Error::new(
                ErrorCode::RequestConflict,
                "request-id already refers to different input",
            ));
        }
        Ok(request)
    }
    /// Serialized across processes. Authorize/preflight only for a new reservation.
    pub fn register_request(
        &self,
        key: &str,
        plan: &ExecutionPlan,
        preflight: impl FnOnce() -> Result<(), Error>,
        register: impl FnOnce(&ExecutionId) -> Result<BackendIdentity, Error>,
    ) -> Result<RequestReservation, Error> {
        self.register_request_rewritten(key, plan, None, preflight, register)
    }
    /// `plan` is the request identity; `rewrite` is stored in the execution record.
    pub fn register_request_rewritten(
        &self,
        key: &str,
        plan: &ExecutionPlan,
        rewrite: Option<RewriteRecord>,
        preflight: impl FnOnce() -> Result<(), Error>,
        register: impl FnOnce(&ExecutionId) -> Result<BackendIdentity, Error>,
    ) -> Result<RequestReservation, Error> {
        let dto = JsonExecutionPlan::try_from(plan)?;
        let _lock = self.lock()?;
        let path = self.request_path(key)?;
        self.register_locked(&path, dto, None, rewrite, preflight, register)
    }
    /// Every CLI submission reserves durable metadata capacity, including calls
    /// without a user request key. This namespace cannot collide with user keys.
    pub fn register_intent(
        &self,
        id: &ExecutionId,
        plan: &ExecutionPlan,
        preflight: impl FnOnce() -> Result<(), Error>,
        register: impl FnOnce(&ExecutionId) -> Result<BackendIdentity, Error>,
    ) -> Result<RequestReservation, Error> {
        self.register_intent_rewritten(id, plan, None, preflight, register)
    }
    pub fn register_intent_rewritten(
        &self,
        id: &ExecutionId,
        plan: &ExecutionPlan,
        rewrite: Option<RewriteRecord>,
        preflight: impl FnOnce() -> Result<(), Error>,
        register: impl FnOnce(&ExecutionId) -> Result<BackendIdentity, Error>,
    ) -> Result<RequestReservation, Error> {
        let dto = JsonExecutionPlan::try_from(plan)?;
        let _lock = self.lock()?;
        let directory = self.root.join("registrations");
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(fail()),
        }
        secure(&directory, true)?;
        self.register_locked(
            &directory.join(format!("{id}.intent")),
            dto,
            Some(id.clone()),
            rewrite,
            preflight,
            register,
        )
    }
    pub fn intent(&self, id: &ExecutionId) -> Result<Option<RequestReservation>, Error> {
        let _lock = self.lock()?;
        let directory = self.root.join("registrations");
        if !directory.try_exists().map_err(|_| fail())? {
            return Ok(None);
        }
        secure(&directory, true)?;
        self.read_request(&directory.join(format!("{id}.intent")))
    }
    fn register_locked(
        &self,
        path: &Path,
        dto: JsonExecutionPlan,
        id: Option<ExecutionId>,
        rewrite: Option<RewriteRecord>,
        preflight: impl FnOnce() -> Result<(), Error>,
        register: impl FnOnce(&ExecutionId) -> Result<BackendIdentity, Error>,
    ) -> Result<RequestReservation, Error> {
        if let Some(request) = self.read_request(path)? {
            if request.plan != dto {
                return Err(Error::new(
                    ErrorCode::RequestConflict,
                    "request-id already refers to different input",
                ));
            }
            return Ok(request);
        }
        preflight()?;
        let mut request = RequestReservation {
            schema_version: 1,
            execution_id: match id {
                Some(id) => id,
                None => Self::allocate_id()?,
            },
            plan: dto,
            record: None,
        };
        self.publish_request(path, &request)?;
        let backend = register(&request.execution_id)?;
        let record = ExecutionRecord {
            execution_id: request.execution_id.clone(),
            backend,
            registered_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| fail())?
                .as_secs(),
            cwd: request.plan.cwd.clone(),
            rewrite,
        };
        self.save_record_locked(&record)?;
        request.record = Some(record);
        self.publish_request(path, &request)?;
        Ok(request)
    }
}
use std::os::unix::fs::DirBuilderExt;

/// Receipt survives persistence failures in the caller's error value. Saving it again
/// is idempotent; executing the command again is never part of recovery.
#[derive(Debug)]
pub struct RegistrationFailure {
    pub error: Error,
    pub receipt: Option<Box<ExecutionRecord>>,
    pub execution_id: Option<ExecutionId>,
}
pub fn register_and_save(
    adapter: &mut PueueAdapter,
    store: &ExecutionStore,
    plan: &ExecutionPlan,
) -> Result<ExecutionRecord, RegistrationFailure> {
    let execution_id = ExecutionStore::allocate_id().map_err(|error| RegistrationFailure {
        error,
        receipt: None,
        execution_id: None,
    })?;
    let cwd = JsonExecutionPlan::try_from(plan)
        .map_err(|error| RegistrationFailure {
            error,
            receipt: None,
            execution_id: None,
        })?
        .cwd;
    let registered_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RegistrationFailure {
            error: fail(),
            receipt: None,
            execution_id: None,
        })?
        .as_secs();
    let backend = adapter
        .register_identified(plan)
        .map_err(|error| RegistrationFailure {
            error,
            receipt: None,
            execution_id: None,
        })?;
    let record = ExecutionRecord {
        execution_id,
        backend,
        registered_at,
        cwd,
        rewrite: None,
    };
    store
        .save_record(&record)
        .map_err(|error| RegistrationFailure {
            error,
            receipt: Some(Box::new(record.clone())),
            execution_id: Some(record.execution_id.clone()),
        })?;
    Ok(record)
}

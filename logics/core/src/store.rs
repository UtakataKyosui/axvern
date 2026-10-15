//! Small local metadata store: one immutable record per execution, OS file lock,
//! same-directory atomic publication and fsync. Never stores command output.
use crate::adapters::pueue::PueueAdapter;
use axvern_models::*;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
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
    root: PathBuf,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    record: ExecutionRecord,
}
impl ExecutionStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = root.into();
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
        let _lock = store.lock()?;
        // A killed writer can leave only unpublished scratch, never a partial record.
        for entry in io(fs::read_dir(&store.root))? {
            let path = io(entry)?.path();
            if path.extension().is_some_and(|v| v == "pending") {
                secure(&path, false)?;
                io(fs::remove_file(path))?;
            }
        }
        Ok(store)
    }
    fn lock(&self) -> Result<File, Error> {
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
        io(file.lock())?;
        Ok(file)
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
        if record.cwd.is_empty()
            || record.backend.queue.is_empty()
            || record.backend.created_at.is_empty()
        {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "incomplete execution metadata",
            ));
        }
        let _lock = self.lock()?;
        let final_path = self.path(&record.execution_id);
        if final_path.symlink_metadata().is_ok() {
            let existing = self.read(&record.execution_id)?;
            return if existing == *record {
                Ok(())
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
    fn read(&self, id: &ExecutionId) -> Result<ExecutionRecord, Error> {
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
}
use std::os::unix::fs::DirBuilderExt;

/// Receipt survives persistence failures in the caller's error value. Saving it again
/// is idempotent; executing the command again is never part of recovery.
#[derive(Debug)]
pub struct RegistrationFailure {
    pub error: Error,
    pub receipt: Option<Box<ExecutionRecord>>,
}
pub fn register_and_save(
    adapter: &mut PueueAdapter,
    store: &ExecutionStore,
    plan: &ExecutionPlan,
) -> Result<ExecutionRecord, RegistrationFailure> {
    let execution_id = ExecutionStore::allocate_id().map_err(|error| RegistrationFailure {
        error,
        receipt: None,
    })?;
    let cwd = JsonExecutionPlan::try_from(plan)
        .map_err(|error| RegistrationFailure {
            error,
            receipt: None,
        })?
        .cwd;
    let registered_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RegistrationFailure {
            error: fail(),
            receipt: None,
        })?
        .as_secs();
    let backend = adapter
        .register_identified(plan)
        .map_err(|error| RegistrationFailure {
            error,
            receipt: None,
        })?;
    let record = ExecutionRecord {
        execution_id,
        backend,
        registered_at,
        cwd,
    };
    store
        .save_record(&record)
        .map_err(|error| RegistrationFailure {
            error,
            receipt: Some(Box::new(record.clone())),
        })?;
    Ok(record)
}

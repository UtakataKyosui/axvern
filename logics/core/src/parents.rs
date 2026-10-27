//! Persisted logical parents for discovery producers. Children remain owned Pueue executions.
use crate::{services::ExecutionService, store::ExecutionStore};
use axvern_models::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    time::{Duration, Instant},
};
fn failure() -> Error {
    Error::new(ErrorCode::StoreFailure, "invalid parent execution metadata")
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    record: ParentRecord,
}
fn validate(record: &ParentRecord) -> Result<(), Error> {
    let unique: std::collections::HashSet<_> =
        record.children.iter().map(ToString::to_string).collect();
    if record.children.is_empty()
        || record.children.len() > 32
        || unique.len() != record.children.len()
        || record.children.contains(&record.execution_id)
        || !std::path::Path::new(&record.cwd).is_absolute()
    {
        return Err(failure());
    }
    if let Some(spec) = &record.analysis {
        let registered: Vec<_> = spec
            .sections
            .iter()
            .filter_map(|s| s.execution_id.clone())
            .collect();
        let sections: std::collections::HashSet<_> =
            spec.sections.iter().map(|s| s.section).collect();
        if registered != record.children
            || sections.len() != spec.sections.len()
            || !std::path::Path::new(&spec.path).is_absolute()
            || !std::path::Path::new(&spec.root).is_absolute()
        {
            return Err(failure());
        }
    }
    Ok(())
}
fn read(store: &ExecutionStore, id: &ExecutionId) -> Result<Option<ParentRecord>, Error> {
    let path = store.root.join(format!("{id}.parent"));
    let meta = match path.symlink_metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failure()),
    };
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.permissions().mode() & 0o777 != 0o600
    {
        return Err(Error::new(
            ErrorCode::PermissionDenied,
            "parent metadata must be a private regular file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| failure())?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    let d: Document = serde_json::from_slice(&bytes).map_err(|_| failure())?;
    if d.schema_version != 1 || d.record.execution_id != *id {
        return Err(failure());
    }
    validate(&d.record)?;
    Ok(Some(d.record))
}
pub fn find(store: &ExecutionStore, id: &ExecutionId) -> Result<Option<ParentRecord>, Error> {
    let _lock = store.lock()?;
    read(store, id)
}
/// Children must already exist as concrete registrations. Nested/cyclic parents are rejected.
pub fn save(store: &ExecutionStore, record: &ParentRecord) -> Result<(), Error> {
    validate(record)?;
    let _lock = store.lock()?;
    for id in &record.children {
        store.read(id)?;
    }
    if store
        .root
        .join(format!("{}.json", record.execution_id))
        .exists()
    {
        return Err(failure());
    }
    if let Some(existing) = read(store, &record.execution_id)? {
        return if existing == *record {
            Ok(())
        } else {
            Err(failure())
        };
    }
    let scratch = store
        .root
        .join(format!("{}.parent-scratch", record.execution_id));
    // Recover our own abandoned scratch from a writer that died before publication.
    if let Ok(meta) = scratch.symlink_metadata() {
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.permissions().mode() & 0o777 != 0o600
        {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "parent metadata must be a private regular file",
            ));
        }
        fs::remove_file(&scratch).map_err(|_| failure())?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&scratch)
        .map_err(|_| failure())?;
    let bytes = serde_json::to_vec(&Document {
        schema_version: 1,
        record: record.clone(),
    })
    .map_err(|_| failure())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| failure())?;
    fs::rename(
        scratch,
        store.root.join(format!("{}.parent", record.execution_id)),
    )
    .map_err(|_| failure())?;
    File::open(&store.root)
        .and_then(|f| f.sync_all())
        .map_err(|_| failure())
}
pub fn history(store: &ExecutionStore) -> Result<Vec<ParentRecord>, Error> {
    let _lock = store.lock()?;
    history_locked(store)
}
pub(crate) fn history_until(
    store: &ExecutionStore,
    deadline: Instant,
) -> Result<Vec<ParentRecord>, Error> {
    let _lock = store.lock_until(deadline)?;
    history_locked(store)
}
fn history_locked(store: &ExecutionStore) -> Result<Vec<ParentRecord>, Error> {
    let mut records = vec![];
    for entry in fs::read_dir(&store.root).map_err(|_| failure())? {
        let path = entry.map_err(|_| failure())?.path();
        if path.extension().is_some_and(|e| e == "parent") {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(failure)?
                .parse()
                .map_err(|_| failure())?;
            records.push(read(store, &id)?.ok_or_else(failure)?);
        }
    }
    Ok(records)
}
pub fn aggregate(states: &[ChildState]) -> Option<TaskState> {
    if states.is_empty() {
        return None;
    }
    if states.iter().all(|s| *s == ChildState::Queued) {
        return Some(TaskState::Queued);
    }
    if states
        .iter()
        .any(|s| matches!(s, ChildState::Queued | ChildState::Running))
    {
        return Some(TaskState::Running);
    }
    let successes = states.iter().filter(|s| **s == ChildState::Done).count();
    Some(if successes == states.len() {
        TaskState::Done
    } else if successes > 0 {
        TaskState::Partial
    } else {
        TaskState::Failed
    })
}
pub fn inspect(
    service: &ExecutionService,
    record: &ParentRecord,
    timeout: Duration,
) -> Result<ParentView, Error> {
    let mut adapter = service.adapter.clone();
    adapter.timeout = adapter.timeout.min(timeout);
    let tasks = adapter.list();
    inspect_with_tasks(service, record, &tasks)
}
pub fn inspect_with_tasks(
    service: &ExecutionService,
    record: &ParentRecord,
    tasks: &Result<Vec<TaskSnapshot>, Error>,
) -> Result<ParentView, Error> {
    let mut children = vec![];
    for id in &record.children {
        let child = service.store.find_record(id)?;
        let snapshot = tasks.as_ref().map_err(Clone::clone).and_then(|tasks| {
            tasks
                .iter()
                .find(|t| t.identity == child.backend && t.identity.queue == service.adapter.queue)
                .cloned()
                .ok_or_else(|| {
                    Error::new(ErrorCode::Expired, "parent child backend mapping expired")
                })
        });
        match snapshot {
            Ok(task) => children.push(ExecutionView {
                record: child,
                task: Some(service.target_snapshot(task, id)?),
                error: None,
            }),
            Err(error) => children.push(ExecutionView {
                record: child,
                task: None,
                error: Some(error),
            }),
        }
    }
    let complete = children.iter().all(|c| c.task.is_some());
    let states: Vec<_> = children
        .iter()
        .filter_map(|c| c.task.as_ref().map(|t| t.state))
        .collect();
    let unavailable_code = if children
        .iter()
        .filter_map(|c| c.error.as_ref())
        .all(|e| e.code == ErrorCode::Expired)
    {
        ErrorCode::Expired
    } else {
        ErrorCode::PartialResult
    };
    Ok(ParentView {
        record: record.clone(),
        state: if complete { aggregate(&states) } else { None },
        children,
        error: if complete {
            None
        } else {
            Some(Error::new(
                unavailable_code,
                "parent child states partially unavailable",
            ))
        },
    })
}
pub fn wait(
    service: &ExecutionService,
    record: &ParentRecord,
    timeout: Duration,
) -> Result<(ParentView, bool), Error> {
    let deadline = Instant::now() + timeout;
    let mut last = ParentView {
        record: record.clone(),
        state: None,
        children: vec![],
        error: None,
    };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok((last, true));
        }
        match inspect(service, record, remaining) {
            Ok(view) => {
                if Instant::now() >= deadline
                    && view.children.iter().any(|c| {
                        c.error
                            .as_ref()
                            .is_some_and(|e| e.code == ErrorCode::Timeout)
                    })
                {
                    return Ok((last, true));
                }
                last = view;
                if matches!(
                    last.state,
                    Some(TaskState::Done | TaskState::Failed | TaskState::Partial)
                ) || last.error.is_some()
                {
                    return Ok((last, false));
                }
            }
            Err(e) if e.code == ErrorCode::Timeout && Instant::now() >= deadline => {
                return Ok((last, true));
            }
            Err(e) => return Err(e),
        }
        std::thread::sleep(
            Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

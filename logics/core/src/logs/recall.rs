//! Whole-log retrieval (FR-QUERY-008). Inline output is small and exact; anything larger goes to
//! a file the caller names explicitly, published atomically and never over an existing path.
use super::{LogReport, OpenedLog, payload_base, sha256::Sha256};
use crate::services::ExecutionService;
use axvern_models::*;
use serde_json::json;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct RecallSpec {
    pub output: Option<PathBuf>,
    pub max_output_bytes: u64,
    pub deadline: Duration,
}
impl RecallSpec {
    pub fn from_config(output: Option<PathBuf>, logs: &crate::config::Logs) -> Self {
        Self {
            output,
            max_output_bytes: logs.recall_max_output_bytes,
            deadline: Duration::from_secs(logs.recall_deadline_seconds),
        }
    }
}
fn io_failure(error: &std::io::Error, what: &str) -> Error {
    let code = match error.kind() {
        std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => ErrorCode::OutputExists,
        std::io::ErrorKind::StorageFull => ErrorCode::StorageFull,
        _ => ErrorCode::BackendFailure,
    };
    Error::new(code, what)
}
fn changed() -> Error {
    Error::new(ErrorCode::LogChanged, "the log changed while it was read")
}
pub fn recall(
    service: &ExecutionService,
    id: &ExecutionId,
    spec: &RecallSpec,
) -> Result<LogReport, Error> {
    // Reject an unusable destination before touching the log, and never create anything for a
    // log that turns out to be missing.
    let target = spec.output.as_deref().map(target_path).transpose()?;
    let log = super::open(service, id)?;
    recall_opened(id, log, spec, target.as_deref())
}
/// Resolve `--output` against the working directory without following the final component.
pub fn target_path(path: &Path) -> Result<PathBuf, Error> {
    let invalid = |m: &str| Error::new(ErrorCode::InvalidTarget, m);
    let name = path
        .file_name()
        .ok_or_else(|| invalid("--output must name a file"))?;
    let absolute = std::path::absolute(path).map_err(|_| invalid("cannot resolve --output"))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| invalid("--output needs a parent directory"))?;
    if !fs::metadata(parent).is_ok_and(|m| m.is_dir()) {
        return Err(invalid("the parent directory of --output must exist"));
    }
    let target = parent.join(name);
    // `symlink_metadata` also reports a dangling link, which must not be written through.
    if target.symlink_metadata().is_ok() {
        return Err(Error::new(
            ErrorCode::OutputExists,
            "--output already exists; nothing was overwritten",
        ));
    }
    Ok(target)
}
pub fn recall_opened(
    id: &ExecutionId,
    mut log: OpenedLog,
    spec: &RecallSpec,
    target: Option<&Path>,
) -> Result<LogReport, Error> {
    let mut output = payload_base("recall", id, &log);
    let len = log.handle.snapshot_len;
    output["bytes"] = json!(len);
    output["complete"] = json!(!log.partial);
    match target {
        None => {
            if len > RECALL_INLINE_BYTES as u64 {
                return Err(Error::new(
                    ErrorCode::OutputTooLarge,
                    "the log does not fit the inline output; use --output FILE",
                ));
            }
            let mut bytes = Vec::with_capacity(len as usize);
            (&mut log.handle.reader)
                .take(len)
                .read_to_end(&mut bytes)
                .map_err(|e| io_failure(&e, "cannot read the log"))?;
            if bytes.len() as u64 != len {
                return Err(changed());
            }
            log.handle.reader.verify_unchanged(!log.partial)?;
            let text = String::from_utf8(bytes).map_err(|_| {
                Error::new(
                    ErrorCode::InvalidEncoding,
                    "the log is not valid UTF-8 text; use --output FILE to save the raw bytes",
                )
            })?;
            output["encoding"] = json!("utf-8");
            output["text"] = json!(text);
            if serde_json::to_vec(&output).map_or(true, |v| v.len() > RECALL_INLINE_BYTES) {
                return Err(Error::new(
                    ErrorCode::OutputTooLarge,
                    "the escaped log does not fit the inline output; use --output FILE",
                ));
            }
        }
        Some(target) => {
            if len > spec.max_output_bytes {
                return Err(Error::new(
                    ErrorCode::OutputTooLarge,
                    "the log exceeds the configured recall output limit",
                ));
            }
            let digest = publish(&mut log, target, len, spec.deadline)?;
            output["encoding"] = json!("bytes");
            output["output_file"] = json!(target.to_string_lossy());
            output["sha256"] = json!(digest);
        }
    }
    Ok(LogReport {
        output,
        task: log.task,
        error: None,
    })
}
/// Removes the private scratch file unless it was handed over.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn publish(
    log: &mut OpenedLog,
    target: &Path,
    len: u64,
    deadline: Duration,
) -> Result<String, Error> {
    let started = Instant::now();
    let parent = target.parent().expect("validated parent");
    let mut random = [0u8; 8];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .map_err(|e| io_failure(&e, "cannot create a scratch name"))?;
    let scratch_path = parent.join(format!(
        ".axvern-recall-{}.tmp",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&scratch_path)
        .map_err(|e| io_failure(&e, "cannot create the output file"))?;
    let scratch = Scratch(scratch_path);
    let mut hash = Sha256::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    while copied < len {
        if started.elapsed() >= deadline {
            return Err(Error::new(
                ErrorCode::Timeout,
                "recall deadline exceeded; the task is unaffected and no file was published",
            ));
        }
        let want = buffer.len().min((len - copied) as usize);
        let n = log
            .handle
            .reader
            .read(&mut buffer[..want])
            .map_err(|e| io_failure(&e, "cannot read the log"))?;
        if n == 0 {
            return Err(changed());
        }
        hash.update(&buffer[..n]);
        file.write_all(&buffer[..n])
            .map_err(|e| io_failure(&e, "cannot write the output file"))?;
        copied += n as u64;
    }
    log.handle.reader.verify_unchanged(!log.partial)?;
    file.sync_all()
        .map_err(|e| io_failure(&e, "cannot flush the output file"))?;
    // A hard link fails when the name exists, so a concurrent writer is never overwritten.
    fs::hard_link(&scratch.0, target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            Error::new(
                ErrorCode::OutputExists,
                "--output already exists; nothing was overwritten",
            )
        } else {
            io_failure(&e, "cannot publish the output file")
        }
    })?;
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(hash.finish_hex())
}

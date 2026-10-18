//! Direct reader for the log files of a Pueue 4.0.4 daemon (`<pueue_directory>/task_logs/<id>.log`).
//!
//! `pueue log --json` returns the whole log as one JSON string, so a large log would always be
//! held in memory twice. Reading the file lets query/recall stream a fixed snapshot instead.
use axvern_models::*;
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

fn unreadable(error: &std::io::Error) -> Error {
    match error.kind() {
        std::io::ErrorKind::NotFound => Error::new(
            ErrorCode::LogMissing,
            "the Pueue log file does not exist (cleaned, reset, never started or deleted)",
        ),
        std::io::ErrorKind::PermissionDenied => Error::new(
            ErrorCode::PermissionDenied,
            "the Pueue log file is not readable",
        ),
        _ => Error::new(ErrorCode::BackendFailure, "cannot read the Pueue log file"),
    }
}
/// `shared.pueue_directory` of a Pueue configuration, or Pueue's default data directory.
pub fn pueue_directory(config: &Path) -> Result<PathBuf, Error> {
    let text = fs::read_to_string(config).map_err(|e| unreadable(&e))?;
    for line in text.lines() {
        let Some(value) = line.trim_start().strip_prefix("pueue_directory:") else {
            continue;
        };
        let value = value.split(" #").next().unwrap_or("").trim();
        let parsed = if value.starts_with('"') {
            serde_json::from_str::<String>(value).ok()
        } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Some(inner.replace("''", "'"))
        } else {
            Some(value.to_owned())
        };
        return match parsed.filter(|v| !v.is_empty()) {
            Some(path) => Ok(PathBuf::from(path)),
            None => Err(Error::new(
                ErrorCode::InvalidConfig,
                "cannot parse pueue_directory in the Pueue configuration",
            )),
        };
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_local_dir().join("pueue"))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "cannot locate the Pueue data directory",
            )
        })
}
#[derive(Clone, Debug)]
pub struct PueueLogFiles {
    pub directory: PathBuf,
}
impl PueueLogFiles {
    pub fn path(&self, task_id: u64) -> PathBuf {
        self.directory
            .join("task_logs")
            .join(format!("{task_id}.log"))
    }
}
struct FileCursor {
    file: std::io::Take<File>,
    probe: File,
    path: PathBuf,
    identity: (u64, u64),
    len: u64,
    modified: Option<SystemTime>,
}
impl Read for FileCursor {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}
impl LogCursor for FileCursor {
    fn verify_unchanged(&mut self, exact: bool) -> Result<(), Error> {
        let changed = || Error::new(ErrorCode::LogChanged, "the log changed while it was read");
        let meta = self.probe.metadata().map_err(|e| unreadable(&e))?;
        if meta.len() < self.len {
            return Err(changed());
        }
        if exact && (meta.len() != self.len || meta.modified().ok() != self.modified) {
            return Err(changed());
        }
        // The path must still be the file we read, otherwise it was deleted or replaced.
        match fs::symlink_metadata(&self.path) {
            Ok(now) if (now.dev(), now.ino()) == self.identity => Ok(()),
            _ if exact => Err(changed()),
            _ => Ok(()),
        }
    }
}
impl LogSource for PueueLogFiles {
    fn open(&self, task_id: u64) -> Result<LogHandle, Error> {
        let path = self.path(task_id);
        // A symlink in place of a Pueue log is never followed.
        let link = fs::symlink_metadata(&path).map_err(|e| unreadable(&e))?;
        if !link.is_file() {
            return Err(Error::new(
                ErrorCode::BackendFailure,
                "the Pueue log path is not a regular file",
            ));
        }
        let file = File::open(&path).map_err(|e| unreadable(&e))?;
        let meta = file.metadata().map_err(|e| unreadable(&e))?;
        if (meta.dev(), meta.ino()) != (link.dev(), link.ino()) {
            return Err(Error::new(
                ErrorCode::LogChanged,
                "the log was replaced while it was opened",
            ));
        }
        let probe = file.try_clone().map_err(|e| unreadable(&e))?;
        Ok(LogHandle {
            location: path.to_string_lossy().into_owned(),
            snapshot_len: meta.len(),
            reader: Box::new(FileCursor {
                file: file.take(meta.len()),
                probe,
                path,
                identity: (meta.dev(), meta.ino()),
                len: meta.len(),
                modified: meta.modified().ok(),
            }),
        })
    }
}

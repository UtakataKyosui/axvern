//! Retained-log use cases: open the Pueue log of an execution, then `query` or `recall` it.
//! Nothing here registers, restarts or otherwise re-runs the original command.
pub mod query;
pub mod recall;
pub mod sha256;
pub mod source;

use crate::{adapters::tools::processes::rfc3339, services::ExecutionService};
use axvern_models::*;
use serde_json::{Value, json};
use std::io::Read;

pub struct OpenedLog {
    pub task: TaskSnapshot,
    pub handle: LogHandle,
    /// Unix seconds when the snapshot was fixed.
    pub observed_at: u64,
    /// The task had not finished when the snapshot was taken.
    pub partial: bool,
}
impl OpenedLog {
    pub fn availability(&self) -> LogAvailability {
        if self.partial {
            LogAvailability::Partial
        } else {
            LogAvailability::Available
        }
    }
}
/// What a use case reports. `error` accompanies a payload that is still worth returning.
pub struct LogReport {
    pub output: Value,
    pub task: TaskSnapshot,
    pub error: Option<Error>,
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
struct Empty;
impl Read for Empty {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}
impl LogCursor for Empty {
    fn verify_unchanged(&mut self, _: bool) -> Result<(), Error> {
        Ok(())
    }
}
/// Observe the task, fix a snapshot of its log, then re-observe the task so a recycled Pueue ID
/// can never expose another execution's output. `inspect` must check the stored backend identity.
pub fn open_task(
    inspect: &dyn Fn() -> Result<TaskSnapshot, Error>,
    source: &dyn LogSource,
    location: impl FnOnce() -> String,
) -> Result<OpenedLog, Error> {
    let before = inspect()?;
    let finished = matches!(before.state, ChildState::Done | ChildState::Failed);
    let observed_at = now();
    let handle = match source.open(before.identity.task_id) {
        Ok(handle) => handle,
        // A queued task has produced nothing yet: an empty partial log is the truth.
        Err(e) if e.code == ErrorCode::LogMissing && before.state == ChildState::Queued => {
            LogHandle {
                location: location(),
                snapshot_len: 0,
                reader: Box::new(Empty),
            }
        }
        Err(e) => return Err(e),
    };
    inspect()?;
    Ok(OpenedLog {
        task: before,
        handle,
        observed_at,
        partial: !finished,
    })
}
pub fn open(service: &ExecutionService, id: &ExecutionId) -> Result<OpenedLog, Error> {
    let record = service.store.find_record(id)?;
    let source = service.adapter.log_source()?;
    open_task(
        &|| service.adapter.inspect_owned(&record.backend),
        &source,
        || {
            source
                .path(record.backend.task_id)
                .to_string_lossy()
                .into_owned()
        },
    )
}
/// Fields shared by every retained-log payload.
pub(crate) fn payload_base(kind: &str, id: &ExecutionId, log: &OpenedLog) -> Value {
    json!({
        "kind": kind,
        "source_execution_id": id,
        "observed_at": rfc3339(log.observed_at),
        "stream": LogStream::Combined,
        "log_availability": log.availability(),
        "partial_log": log.partial,
        "log_location": log.handle.location,
        "snapshot_bytes": log.handle.snapshot_len,
    })
}
pub fn query(
    service: &ExecutionService,
    id: &ExecutionId,
    spec: &QuerySpec,
) -> Result<LogReport, Error> {
    spec.validate()?;
    let log = open(service, id)?;
    query_opened(id, log, spec, &query::ScanBudget::default())
}
pub fn query_opened(
    id: &ExecutionId,
    mut log: OpenedLog,
    spec: &QuerySpec,
    budget: &query::ScanBudget,
) -> Result<LogReport, Error> {
    spec.validate()?;
    let scan = query::scan(&mut log.handle.reader, spec, budget)?;
    // A finished log must be unchanged however far the scan got; a running one may only have grown.
    log.handle.reader.verify_unchanged(!log.partial)?;
    let mut output = payload_base("query", id, &log);
    output["selector"] = match &spec.selector {
        QuerySelector::Match(text) => json!({"match": text}),
        QuerySelector::Errors => json!({"errors": {"rule_version": ERRORS_RULE_VERSION}}),
        QuerySelector::Tail(n) => json!({"tail": n}),
        QuerySelector::Range { from_line, lines } => {
            json!({"range": {"from_line": from_line, "lines": lines}})
        }
    };
    output["encoding"] = json!("utf-8");
    output["limit"] = json!(spec.limit);
    output["max_bytes"] = json!(spec.max_bytes);
    output["items"] = serde_json::to_value(&scan.items).expect("items JSON");
    output["truncated"] = json!(scan.truncated);
    output["omitted_count"] = json!(scan.omitted_count);
    output["scan_complete"] = json!(scan.scan_complete);
    output["scanned_bytes"] = json!(scan.scanned_bytes);
    let error = (!scan.scan_complete).then(|| {
        Error::new(
            ErrorCode::PartialResult,
            if matches!(spec.selector, QuerySelector::Tail(_)) {
                "scan budget exhausted; exact line numbers for the tail are unknown"
            } else {
                "scan budget exhausted before the whole log was read"
            },
        )
    });
    Ok(LogReport {
        output,
        task: log.task,
        error,
    })
}
pub use recall::recall;

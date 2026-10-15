//! Pinned Pueue CLI boundary. No daemon lifecycle changes and no retry of `add`.
use axvern_models::*;
use serde_json::Value;
use std::{
    ffi::OsString,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct PueueAdapter {
    pub executable: PathBuf,
    pub config: PathBuf,
    /// Stable namespace for this daemon; persist it with every execution.
    pub queue: String,
    pub timeout: Duration,
}
fn failure(message: &str) -> Error {
    Error::new(ErrorCode::BackendFailure, message)
}
fn capture(mut stream: impl Read) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        // Keep draining so a full pipe cannot deadlock the child.
        if result.len() <= LIMIT {
            result.extend_from_slice(&buffer[..n]);
        }
    }
    Ok(result)
}
impl PueueAdapter {
    fn call(&self, args: &[OsString]) -> Result<Vec<u8>, Error> {
        let mut child = Command::new(&self.executable)
            .arg("--config")
            .arg(&self.config)
            .args(["--color", "never"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::new(
                    if e.kind() == std::io::ErrorKind::NotFound {
                        ErrorCode::MissingTool
                    } else {
                        ErrorCode::BackendFailure
                    },
                    "cannot start Pueue CLI",
                )
            })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let out = thread::spawn(move || capture(stdout));
        let err = thread::spawn(move || capture(stderr));
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(Error::new(
                        ErrorCode::Timeout,
                        "Pueue client timed out; task was not stopped",
                    ));
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(failure("cannot wait for Pueue CLI"));
                }
            }
        };
        let stdout = out
            .join()
            .map_err(|_| failure("Pueue reader failed"))?
            .map_err(|_| failure("cannot read Pueue stdout"))?;
        let stderr = err
            .join()
            .map_err(|_| failure("Pueue reader failed"))?
            .map_err(|_| failure("cannot read Pueue stderr"))?;
        let status = status?;
        if stdout.len() > LIMIT || stderr.len() > LIMIT {
            return Err(failure("Pueue response exceeds limit"));
        }
        if !status.success() {
            if args.first().is_some_and(|arg| arg == "status") {
                return Err(Error::new(
                    ErrorCode::DaemonUnavailable,
                    "cannot obtain Pueue daemon state",
                ));
            }
            return Err(failure("Pueue CLI failed"));
        }
        Ok(stdout)
    }
    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }
    pub fn check(&self) -> Result<(), Error> {
        // Version has no daemon side effects. Global config is accepted by Pueue.
        let version = self.call(&Self::args(&["--version"]))?;
        if version != b"pueue 4.0.4\n" {
            return Err(failure("requires Pueue 4.0.4"));
        }
        self.list().map(|_| ())
    }
    pub fn list(&self) -> Result<Vec<TaskSnapshot>, Error> {
        let raw = self.call(&Self::args(&["status", "--json"]))?;
        Self::parse_status(&self.queue, &raw)
    }
    pub fn parse_status(queue: &str, bytes: &[u8]) -> Result<Vec<TaskSnapshot>, Error> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| failure("invalid Pueue JSON"))?;
        let tasks = value
            .get("tasks")
            .and_then(Value::as_object)
            .ok_or_else(|| failure("missing Pueue tasks"))?;
        tasks
            .iter()
            .map(|(key, task)| {
                let id = task["id"]
                    .as_u64()
                    .ok_or_else(|| failure("invalid task ID"))?;
                if key != &id.to_string() {
                    return Err(failure("inconsistent task ID"));
                }
                let created = task["created_at"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| failure("missing task creation identity"))?;
                let status = &task["status"];
                let (state, exit_code, details) = if let Some(done) = status.get("Done") {
                    match &done["result"] {
                        Value::String(s) if s == "Success" => (ChildState::Done, Some(0), done),
                        Value::String(s)
                            if ["Killed", "Errored", "DependencyFailed"].contains(&s.as_str()) =>
                        {
                            (ChildState::Failed, None, done)
                        }
                        v if v.get("FailedToSpawn").and_then(Value::as_str).is_some() => {
                            (ChildState::Failed, None, done)
                        }
                        v if v.get("Failed").is_some() => {
                            let code = v["Failed"]
                                .as_i64()
                                .and_then(|v| i32::try_from(v).ok())
                                .ok_or_else(|| failure("invalid child exit code"))?;
                            (ChildState::Failed, Some(code), done)
                        }
                        _ => return Err(failure("unknown Pueue result")),
                    }
                } else if let Some(running) = status.get("Running").or_else(|| status.get("Paused"))
                {
                    (ChildState::Running, None, running)
                } else if status.get("Queued").is_some()
                    || status.get("Stashed").is_some()
                    || status.get("Locked").is_some()
                {
                    (ChildState::Queued, None, status)
                } else {
                    return Err(failure("unknown Pueue state"));
                };
                Ok(TaskSnapshot {
                    identity: BackendIdentity {
                        queue: queue.into(),
                        task_id: id,
                        created_at: created.into(),
                    },
                    state,
                    exit_code,
                    started_at: details
                        .get("start")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    finished_at: details
                        .get("end")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
            })
            .collect()
    }
    pub fn inspect(&self, task_id: u64) -> Result<TaskSnapshot, Error> {
        self.list()?
            .into_iter()
            .find(|s| s.identity.task_id == task_id)
            .ok_or_else(|| Error::new(ErrorCode::TaskNotFound, "Pueue task is absent"))
    }
    pub fn inspect_owned(&self, identity: &BackendIdentity) -> Result<TaskSnapshot, Error> {
        if identity.queue != self.queue {
            return Err(Error::new(
                ErrorCode::Expired,
                "execution belongs to another queue",
            ));
        }
        match self.inspect(identity.task_id) {
            Ok(task) if task.identity == *identity => Ok(task),
            Ok(_) => Err(Error::new(ErrorCode::Expired, "Pueue task ID was reused")),
            Err(e) if e.code == ErrorCode::TaskNotFound => {
                Err(Error::new(ErrorCode::Expired, "Pueue task was removed"))
            }
            Err(e) => Err(e),
        }
    }
    pub fn wait(
        &self,
        identity: &BackendIdentity,
        timeout: Duration,
    ) -> Result<TaskSnapshot, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::new(
                    ErrorCode::Timeout,
                    "wait timed out; task continues",
                ));
            }
            let mut client = self.clone();
            client.timeout = client.timeout.min(remaining);
            let task = client.inspect_owned(identity)?;
            if matches!(task.state, ChildState::Done | ChildState::Failed) {
                return Ok(task);
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorCode::Timeout,
                    "wait timed out; task continues",
                ));
            }
            thread::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    pub fn register_identified(&mut self, plan: &ExecutionPlan) -> Result<BackendIdentity, Error> {
        if self.queue.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "queue namespace is empty",
            ));
        }
        JsonExecutionPlan::try_from(plan)?;
        if plan.program.is_empty() || !plan.cwd.is_absolute() || !plan.cwd.is_dir() {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "program and absolute existing cwd required",
            ));
        }
        self.check()?;
        let label = crate::store::ExecutionStore::allocate_id()?.to_string();
        let mut args = Self::args(&[
            "add",
            "--print-task-id",
            "--escape",
            "--label",
            &label,
            "--working-directory",
        ]);
        args.push(plan.cwd.as_os_str().into());
        args.push("--".into());
        args.push(plan.program.clone());
        args.extend(plan.args.clone());
        let unknown = || {
            Error::new(
                ErrorCode::RegistrationUnknown,
                format!(
                    "registration outcome unknown; do not retry add; inspect queue for label {label} and reconcile manually"
                ),
            )
        };
        let bytes = self.call(&args).map_err(|e| {
            if e.code == ErrorCode::MissingTool {
                e
            } else {
                unknown()
            }
        })?;
        let raw = std::str::from_utf8(&bytes).map_err(|_| unknown())?.trim();
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(unknown());
        }
        let id: u64 = raw.parse().map_err(|_| unknown())?;
        let raw = self
            .call(&Self::args(&["status", "--json"]))
            .map_err(|_| unknown())?;
        let value: Value = serde_json::from_slice(&raw).map_err(|_| unknown())?;
        if value["tasks"][id.to_string()]["label"].as_str() != Some(&label) {
            return Err(unknown());
        }
        Self::parse_status(&self.queue, &raw)
            .map_err(|_| unknown())?
            .into_iter()
            .find(|s| s.identity.task_id == id)
            .map(|s| s.identity)
            .ok_or_else(unknown)
    }
    /// Validate the task embedded in the same response as the log, so ID reuse
    /// between status and log requests cannot expose another execution's output.
    pub fn output_owned(
        &self,
        identity: &BackendIdentity,
        max_bytes: usize,
    ) -> Result<Vec<u8>, Error> {
        if identity.queue != self.queue
            || identity.queue.is_empty()
            || identity.created_at.is_empty()
        {
            return Err(Error::new(
                ErrorCode::Expired,
                "execution belongs to another queue or has no identity",
            ));
        }
        let id = identity.task_id.to_string();
        let raw = self.call(&Self::args(&["log", "--json", "--lines", "101", &id]))?;
        let value: Value = serde_json::from_slice(&raw).map_err(|_| failure("invalid log JSON"))?;
        let task = &value[&id]["task"];
        if task["id"].as_u64() != Some(identity.task_id)
            || task["created_at"].as_str() != Some(&identity.created_at)
        {
            return Err(Error::new(
                ErrorCode::Expired,
                "Pueue log task identity changed or is absent",
            ));
        }
        let output = value[&id]["output"]
            .as_str()
            .ok_or_else(|| Error::new(ErrorCode::OutputMissing, "Pueue output missing"))?;
        if output.lines().count() > 100 || output.len() > max_bytes {
            return Err(Error::new(
                ErrorCode::PartialResult,
                "log exceeds line or requested byte limit",
            ));
        }
        Ok(output.as_bytes().to_vec())
    }
}
impl Adapter for PueueAdapter {
    fn register(&mut self, plan: &ExecutionPlan) -> Result<u64, Error> {
        self.register_identified(plan).map(|i| i.task_id)
    }
    fn state(&mut self, task_id: u64) -> Result<ChildState, Error> {
        self.inspect(task_id).map(|s| s.state)
    }
    fn output(&mut self, task_id: u64, max_bytes: usize) -> Result<Vec<u8>, Error> {
        let identity = self.inspect(task_id)?.identity;
        self.output_owned(&identity, max_bytes)
    }
}

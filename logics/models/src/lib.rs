//! Shared contracts. Native argv stays native until an explicitly checked JSON boundary.
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, fmt, path::PathBuf, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExecutionId(String);
impl FromStr for ExecutionId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let b = s.as_bytes();
        if b.len() != 39 || !s.starts_with("ex_") {
            return Err(Error::new(ErrorCode::InvalidInput, "invalid execution ID"));
        }
        for (i, c) in b[3..].iter().enumerate() {
            if [8, 13, 18, 23].contains(&i) {
                if *c != b'-' {
                    return Err(Error::new(ErrorCode::InvalidInput, "invalid execution ID"));
                }
            } else if !c.is_ascii_digit() && !(b'a'..=b'f').contains(c) {
                return Err(Error::new(ErrorCode::InvalidInput, "invalid execution ID"));
            }
        }
        if b[17] != b'4' || !b"89ab".contains(&b[22]) {
            return Err(Error::new(ErrorCode::InvalidInput, "expected UUIDv4"));
        }
        Ok(Self(s.into()))
    }
}
impl TryFrom<String> for ExecutionId {
    type Error = Error;
    fn try_from(s: String) -> Result<Self, Error> {
        s.parse()
    }
}
impl From<ExecutionId> for String {
    fn from(id: ExecutionId) -> Self {
        id.0
    }
}
impl fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonExecutionPlan {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}
impl TryFrom<&ExecutionPlan> for JsonExecutionPlan {
    type Error = Error;
    fn try_from(p: &ExecutionPlan) -> Result<Self, Error> {
        fn utf8(s: &std::ffi::OsStr) -> Result<String, Error> {
            s.to_str()
                .filter(|s| !s.contains('\0'))
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidEncoding,
                        "argv/path must be UTF-8 without NUL",
                    )
                })
        }
        Ok(Self {
            program: utf8(&p.program)?,
            args: p.args.iter().map(|s| utf8(s)).collect::<Result<_, _>>()?,
            cwd: utf8(p.cwd.as_os_str())?,
        })
    }
}
impl TryFrom<JsonExecutionPlan> for ExecutionPlan {
    type Error = Error;
    fn try_from(p: JsonExecutionPlan) -> Result<Self, Error> {
        let plan = Self {
            program: p.program.into(),
            args: p.args.into_iter().map(OsString::from).collect(),
            cwd: p.cwd.into(),
        };
        JsonExecutionPlan::try_from(&plan)?;
        Ok(plan)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRef {
    pub execution_id: ExecutionId,
    pub pueue_task_id: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Done,
    Partial,
    Failed,
}
/// A concrete backend child cannot report aggregate-only `partial`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildState {
    Queued,
    Running,
    Done,
    Failed,
}
impl From<ChildState> for TaskState {
    fn from(state: ChildState) -> Self {
        match state {
            ChildState::Queued => Self::Queued,
            ChildState::Running => Self::Running,
            ChildState::Done => Self::Done,
            ChildState::Failed => Self::Failed,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Ask,
    Deny,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub effect: PolicyEffect,
    pub reason: String,
    pub rule_id: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    NotProvided,
    Approved,
    Rejected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    RegistrationUnknown,
    StoreFailure,
    Expired,
    InvalidInput,
    InvalidEncoding,
    InvalidConfig,
    PermissionDenied,
    MissingTool,
    DaemonUnavailable,
    PolicyDenied,
    TaskNotFound,
    OutputMissing,
    PartialResult,
    Timeout,
    BackendFailure,
}

/// Backend identity includes queue identity and creation time: integer IDs are recycled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendIdentity {
    pub queue: String,
    pub task_id: u64,
    pub created_at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub identity: BackendIdentity,
    pub state: ChildState,
    pub exit_code: Option<i32>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub execution_id: ExecutionId,
    pub backend: BackendIdentity,
    pub registered_at: u64,
    pub cwd: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionView {
    pub record: ExecutionRecord,
    pub task: Option<TaskSnapshot>,
    /// Backend lookup failure retains local metadata, including an expired mapping.
    pub error: Option<Error>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckLevel {
    Pass,
    Warning,
    Fail,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub name: String,
    pub level: CheckLevel,
    pub message: String,
    pub remedy: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub level: CheckLevel,
    pub checks: Vec<Diagnostic>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
#[derive(Debug, Serialize)]
pub struct Response<T> {
    pub schema_version: u32,
    pub execution_id: Option<ExecutionId>,
    pub state: Option<TaskState>,
    pub exit_code: Option<i32>,
    pub output: Option<T>,
    pub error: Option<Error>,
}
impl<T> Response<T> {
    pub fn success(output: T) -> Self {
        Self {
            schema_version: 1,
            execution_id: None,
            state: None,
            exit_code: None,
            output: Some(output),
            error: None,
        }
    }
    pub fn failure(error: Error) -> Self {
        Self {
            schema_version: 1,
            execution_id: None,
            state: None,
            exit_code: None,
            output: None,
            error: Some(error),
        }
    }
}
/// Adapter methods must preserve argv and return typed failures; no shell-string API.
pub trait Adapter {
    fn register(&mut self, plan: &ExecutionPlan) -> Result<u64, Error>;
    fn state(&mut self, task_id: u64) -> Result<ChildState, Error>;
    fn output(&mut self, task_id: u64, max_bytes: usize) -> Result<Vec<u8>, Error>;
}
pub trait Store {
    fn save(&mut self, execution: &ExecutionRef) -> Result<(), Error>;
    fn find(&self, id: &ExecutionId) -> Result<ExecutionRef, Error>;
}

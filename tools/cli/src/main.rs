use axvern_core::{
    doctor, policy,
    services::{self, ExecutionService},
};
use axvern_models::*;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::Duration,
};

const USAGE: &str = "usage: axvern [--config PATH] [--format human|json] run [--cwd PATH] [--wait 1s] [--request-id KEY] -- <program> [args...] | result <execution-id> [--cursor TOKEN] [--limit 100] [--max-bytes 65536] | status <execution-id> | wait <execution-id> [--timeout 30s] | history [--limit 20] | doctor [--scope core|discovery|logs|performance|all] | policy check [--cwd PATH] -- <program> [args...]";
struct Input {
    command: String,
    config: Option<PathBuf>,
    cwd: Option<PathBuf>,
    timeout: Duration,
    wait: Duration,
    request_id: Option<String>,
    scope: String,
    cursor: Option<String>,
    max_bytes: usize,
    limit: usize,
    id: Option<ExecutionId>,
    target: Vec<OsString>,
    json: bool,
}
fn invalid() -> Error {
    Error::new(ErrorCode::InvalidInput, USAGE)
}
fn duration(s: &str, zero: bool) -> Result<Duration, Error> {
    let (digits, factor) = if let Some(s) = s.strip_suffix("ms") {
        (s, 1)
    } else if let Some(s) = s.strip_suffix('s') {
        (s, 1000)
    } else if let Some(s) = s.strip_suffix('m') {
        (s, 60000)
    } else {
        return Err(invalid());
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let ms = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .filter(|n| (if zero { 0 } else { 1 }..=86_400_000).contains(n))
        .ok_or_else(invalid)?;
    Ok(Duration::from_millis(ms))
}
fn parse(args: &[OsString]) -> Result<Input, Error> {
    let boundary = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut input = Input {
        command: String::new(),
        config: None,
        cwd: None,
        timeout: Duration::from_secs(30),
        wait: Duration::from_secs(1),
        request_id: None,
        scope: "core".into(),
        cursor: None,
        max_bytes: 65536,
        limit: 20,
        id: None,
        target: args.get(boundary + 1..).unwrap_or_default().to_vec(),
        json: true,
    };
    let mut positional = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while i < boundary {
        let arg = args[i].to_str().ok_or_else(invalid)?;
        if arg.starts_with('-') {
            if !seen.insert(arg) || i + 1 >= boundary {
                return Err(invalid());
            }
            let value = &args[i + 1];
            match arg {
                "--config" if !value.is_empty() => input.config = Some(value.into()),
                "--cwd" if !value.is_empty() => input.cwd = Some(value.into()),
                "--format" if value == "json" || value == "human" => input.json = value == "json",
                "--timeout" => {
                    input.timeout = duration(value.to_str().ok_or_else(invalid)?, false)?
                }
                "--wait" => input.wait = duration(value.to_str().ok_or_else(invalid)?, true)?,
                "--request-id" => {
                    let key = value.to_str().ok_or_else(invalid)?;
                    if key.is_empty()
                        || key.len() > 128
                        || !key
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
                    {
                        return Err(invalid());
                    }
                    input.request_id = Some(key.into());
                }
                "--scope"
                    if matches!(
                        value.to_str(),
                        Some("core" | "discovery" | "logs" | "performance" | "all")
                    ) =>
                {
                    input.scope = value.to_str().unwrap().into()
                }
                "--cursor" => input.cursor = Some(value.to_str().ok_or_else(invalid)?.into()),
                "--max-bytes" => {
                    let number = value.to_str().ok_or_else(invalid)?;
                    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(invalid());
                    }
                    input.max_bytes = number
                        .parse::<usize>()
                        .ok()
                        .filter(|v| (1024..=65536).contains(v))
                        .ok_or_else(invalid)?
                }
                "--limit" => {
                    let s = value.to_str().ok_or_else(invalid)?;
                    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
                        return Err(invalid());
                    }
                    input.limit = s
                        .parse()
                        .ok()
                        .filter(|n| (1..=1000).contains(n))
                        .ok_or_else(invalid)?;
                }
                _ => return Err(invalid()),
            }
            i += 2;
        } else {
            positional.push(arg);
            i += 1;
        }
    }
    input.command = positional.first().ok_or_else(invalid)?.to_string();
    match positional.as_slice() {
        ["run"] if boundary < args.len() && !input.target.is_empty() => (),
        ["policy", "check"] if boundary < args.len() && !input.target.is_empty() => {
            input.command = "policy check".into()
        }
        ["status" | "wait" | "result", id] if boundary == args.len() => {
            input.id = Some(id.parse()?)
        }
        ["history" | "doctor"] if boundary == args.len() => (),
        _ => return Err(invalid()),
    }
    if input.command == "result" {
        if !seen.contains("--limit") {
            input.limit = 100;
        }
        if input.limit > 100 {
            return Err(invalid());
        }
    }
    if (seen.contains("--cwd") && !matches!(input.command.as_str(), "run" | "policy check"))
        || (seen.contains("--timeout") && input.command != "wait")
        || (seen.contains("--scope") && input.command != "doctor")
        || ((seen.contains("--wait") || seen.contains("--request-id")) && input.command != "run")
        || (seen.contains("--limit") && !matches!(input.command.as_str(), "history" | "result"))
        || ((seen.contains("--cursor") || seen.contains("--max-bytes"))
            && input.command != "result")
    {
        return Err(invalid());
    }
    Ok(input)
}
fn error_exit(error: &Error) -> i32 {
    match error.code {
        ErrorCode::RequestConflict => 15,
        ErrorCode::UnsupportedCapability => 16,
        ErrorCode::InvalidInput | ErrorCode::InvalidEncoding | ErrorCode::InvalidCursor => 2,
        ErrorCode::InvalidConfig => 3,
        ErrorCode::PermissionDenied => 4,
        ErrorCode::MissingTool => 5,
        ErrorCode::DaemonUnavailable => 6,
        ErrorCode::TaskNotFound => 7,
        ErrorCode::Expired => 8,
        ErrorCode::PolicyDenied => 9,
        ErrorCode::PartialResult
        | ErrorCode::OutputMissing
        | ErrorCode::ResultExpired
        | ErrorCode::ResultEvicted
        | ErrorCode::OutputLimitTooSmall => 11,
        ErrorCode::StoreFailure | ErrorCode::StorageFull => 13,
        ErrorCode::RegistrationUnknown => 14,
        ErrorCode::Timeout => 124,
        ErrorCode::BackendFailure => 1,
    }
}
fn failure(error: Error) -> (Response<Value>, i32) {
    let exit = error_exit(&error);
    (Response::failure(error), exit)
}
fn plan(input: &Input) -> Result<ExecutionPlan, Error> {
    let cwd = match &input.cwd {
        Some(path) => path.canonicalize(),
        None => std::env::current_dir(),
    }
    .map_err(|_| Error::new(ErrorCode::InvalidInput, "cannot resolve working directory"))?;
    if !cwd.is_dir() {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            "cwd must be a directory",
        ));
    }
    Ok(ExecutionPlan {
        program: input.target[0].clone(),
        args: input.target[1..].to_vec(),
        cwd,
    })
}
fn approve() -> Approval {
    // Prompt contains no command arguments or config values; both terminals required.
    eprint!("Policy asks for approval. Register this command? [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes") {
        Approval::Approved
    } else {
        Approval::Rejected
    }
}
fn execute(input: &Input) -> (Response<Value>, i32) {
    let config = services::load_config(input.config.as_deref());
    if input.command == "doctor" {
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(_) => {
                return failure(Error::new(
                    ErrorCode::InvalidInput,
                    "cannot resolve working directory",
                ));
            }
        };
        let report = doctor::diagnose_scope(config, &cwd, &input.scope);
        let code = if report.level == CheckLevel::Fail {
            12
        } else {
            0
        };
        return (
            Response::success({
                let mut v = serde_json::to_value(report).expect("report JSON");
                v["kind"] = json!("doctor");
                if let Some(checks) = v["checks"].as_array_mut() {
                    for check in checks {
                        check["availability"] = json!(check["level"] == "pass");
                    }
                }
                v
            }),
            code,
        );
    }
    let config = match config {
        Ok(config) => config,
        Err(error) => return failure(error),
    };
    if input.command == "policy check" {
        return match plan(input).and_then(|p| policy::evaluate(&config, &p)) {
            Ok(decision) => (
                Response::success({
                    let mut v = serde_json::to_value(decision).expect("decision JSON");
                    v["kind"] = json!("policy");
                    v
                }),
                0,
            ),
            Err(error) => failure(error),
        };
    }
    if input.command == "run" && input.request_id.is_some() {
        let p = match plan(input) {
            Ok(p) => p,
            Err(e) => return failure(e),
        };
        // Validate encoding and limits even for a replay; evaluation never prompts.
        if let Err(e) = policy::evaluate(&config, &p) {
            return failure(e);
        }
        let mut service = match ExecutionService::new(config.clone()) {
            Ok(s) => s,
            Err(e) => return failure(e),
        };
        let key = input.request_id.as_deref().unwrap();
        let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
        let check_adapter = service.adapter.clone();
        let runner = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => {
                return failure(Error::new(
                    ErrorCode::BackendFailure,
                    "cannot locate internal runner",
                ));
            }
        };
        let worker_store = services::store_path(&config).expect("validated store");
        let settings = config.results.clone();
        // Ask the user before the store lock is taken: a pending prompt must not stall
        // other clients or workers. A replay or conflicting input never prompts.
        let mut approval = Approval::NotProvided;
        match service.store.request(key, &p) {
            Ok(Some(_)) => (),
            Ok(None) => {
                let decision = match policy::evaluate(&config, &p) {
                    Ok(d) => d,
                    Err(e) => return failure(e),
                };
                if decision.effect == PolicyEffect::Ask && interactive {
                    approval = approve();
                }
            }
            Err(e) => return failure(e),
        }
        let registration = service.store.register_request(
            key,
            &p,
            || {
                services::authorize(&config, &p, interactive, approval)?;
                axvern_core::results::preflight_registration_locked(&service.store, &settings, &p)?;
                check_adapter.check()
            },
            |id| {
                let worker = worker_plan(&runner, &worker_store, &settings, &p, id);
                service.adapter.register_with_label(&worker, id)
            },
        );
        return match registration {
            Ok(request) if request.record.is_some() => {
                run_response(&service, request.record.unwrap(), input.wait)
            }
            Ok(request) => unknown_request(request.execution_id),
            Err(error) => match service.store.request(key, &p) {
                Ok(Some(request)) => unknown_request(request.execution_id),
                _ => failure(error),
            },
        };
    }
    // Validate policy before any backend request or local store initialization.
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let mut approval = Approval::NotProvided;
    let run_plan = if input.command == "run" {
        let p = match plan(input) {
            Ok(p) => p,
            Err(e) => return failure(e),
        };
        let decision = match policy::evaluate(&config, &p) {
            Ok(d) => d,
            Err(e) => return failure(e),
        };
        if decision.effect == PolicyEffect::Ask && interactive {
            approval = approve();
        }
        if let Err(e) = services::authorize(&config, &p, interactive, approval) {
            return failure(e);
        }
        Some(p)
    } else {
        None
    };
    let mut service = match ExecutionService::new(config) {
        Ok(s) => s,
        Err(e) => return failure(e),
    };
    if matches!(input.command.as_str(), "status" | "wait" | "result") {
        let id = input.id.as_ref().expect("parsed ID");
        match axvern_core::parents::find(&service.store, id) {
            Ok(Some(parent)) => return parent_response(&service, &parent, input),
            Ok(None) => (),
            Err(e) => return failure(e),
        }
    }
    match input.command.as_str() {
        "run" => {
            let p = run_plan.as_ref().expect("run plan");
            match std::env::current_exe()
                .map_err(|_| axvern_core::store::RegistrationFailure {
                    error: Error::new(ErrorCode::BackendFailure, "cannot locate internal runner"),
                    receipt: None,
                    execution_id: None,
                })
                .and_then(|runner| service.run_captured(&runner, p, interactive, approval))
            {
                Ok(record) => run_response(&service, record, input.wait),
                Err(f) => {
                    let (mut response, code) = failure(f.error);
                    response.execution_id = f.execution_id;
                    if let Some(receipt) = f.receipt {
                        response.execution_id = Some(receipt.execution_id.clone());
                        response.output = Some(json!({"registration_receipt": receipt}));
                    }
                    (response, code)
                }
            }
        }
        "result" => {
            let id = input.id.as_ref().expect("parsed ID");
            if let Err(e) = service.store.find_record(id) {
                return failure(e);
            }
            result_response(
                &service,
                id,
                input.cursor.as_deref(),
                input.limit,
                input.max_bytes,
            )
        }
        "status" | "wait" => {
            let id = input.id.as_ref().expect("parsed ID");
            let result = if input.command == "wait" {
                service.observe_wait(id, input.timeout)
            } else {
                service.status(id).map(|v| (v, false))
            };
            match result {
                Ok((view, timed_out)) => {
                    let task = view.task.as_ref();
                    let code = if timed_out {
                        124
                    } else if input.command == "wait"
                        && task.is_some_and(|t| t.state == ChildState::Failed)
                    {
                        10
                    } else {
                        0
                    };
                    let mut response =
                        Response::success(serde_json::to_value(&view).expect("view JSON"));
                    response.execution_id = Some(id.clone());
                    response.state = task.map(|t| t.state.into());
                    response.exit_code = task.and_then(|t| t.exit_code);
                    if let Some(output) = response.output.as_mut() {
                        output["kind"] = json!(input.command);
                        output["source"] = json!("backend");
                        output["observed_at"] = json!(now());
                        output["result_availability"] =
                            json!(match availability(&service, id, response.state) {
                                Ok(a) => a,
                                Err(e) => return failure(e),
                            });
                    }
                    if timed_out {
                        response.error = Some(Error::new(
                            ErrorCode::Timeout,
                            "wait timed out; task continues",
                        ));
                    }
                    (response, code)
                }
                Err(e) => {
                    let (mut response, code) = failure(e);
                    response.execution_id = Some(id.clone());
                    (response, code)
                }
            }
        }
        "history" => match service.history(input.limit) {
            Ok(views) => {
                let mut partial;
                let executions = views
                    .into_iter()
                    .map(|v| {
                        let a = availability(
                            &service,
                            &v.record.execution_id,
                            v.task.as_ref().map(|t| t.state.into()),
                        )?;
                        let mut value = serde_json::to_value(v).expect("view JSON");
                        value["result_availability"] = json!(a);
                        value["command"] = json!("run");
                        Ok(value)
                    })
                    .collect::<Result<Vec<_>, Error>>();
                let mut executions = match executions {
                    Ok(v) => v,
                    Err(e) => return failure(e),
                };
                let parents = match axvern_core::parents::history(&service.store) {
                    Ok(v) => v,
                    Err(e) => return failure(e),
                };
                for parent in parents {
                    executions.push(json!({"record":parent,"__parent":true}));
                }
                executions.sort_by(|a, b| {
                    b["record"]["registered_at"]
                        .as_u64()
                        .cmp(&a["record"]["registered_at"].as_u64())
                        .then_with(|| {
                            b["record"]["execution_id"]
                                .as_str()
                                .cmp(&a["record"]["execution_id"].as_str())
                        })
                });
                executions.truncate(input.limit);
                partial = executions
                    .iter()
                    .any(|v| v["error"].is_object() && v["error"]["code"] != "expired");
                let tasks = if executions.iter().any(|v| v["__parent"] == true) {
                    Some(service.adapter.list())
                } else {
                    None
                };
                for value in &mut executions {
                    if value["__parent"] == true {
                        let parent: ParentRecord =
                            serde_json::from_value(value["record"].clone()).expect("parent record");
                        let view = match axvern_core::parents::inspect_with_tasks(
                            &service,
                            &parent,
                            tasks.as_ref().unwrap(),
                        ) {
                            Ok(v) => v,
                            Err(e) => return failure(e),
                        };
                        partial |= view
                            .error
                            .as_ref()
                            .is_some_and(|e| e.code != ErrorCode::Expired);
                        *value = parent_output(&view);
                    }
                }
                let mut response =
                    Response::success(json!({"kind": "history", "executions": executions}));
                if partial {
                    response.error = Some(Error::new(
                        ErrorCode::PartialResult,
                        "history metadata available; backend state unavailable",
                    ));
                }
                (response, if partial { 11 } else { 0 })
            }
            Err(e) => failure(e),
        },
        _ => unreachable!(),
    }
}
fn parent_output(view: &ParentView) -> Value {
    json!({"kind":"fs.analyze", "command":"fs analyze", "record":view.record,"task":{"state":view.state,"exit_code":null,"started_at":null,"finished_at":null},"children":view.children,"error":view.error,"source":"backend","observed_at":now(),"result_availability":if matches!(view.state,Some(TaskState::Queued|TaskState::Running)) { "pending" } else { "unavailable" }})
}
fn parent_response(
    service: &ExecutionService,
    parent: &ParentRecord,
    input: &Input,
) -> (Response<Value>, i32) {
    let result = if input.command == "wait" {
        axvern_core::parents::wait(service, parent, input.timeout)
    } else {
        axvern_core::parents::inspect(service, parent, Duration::from_secs(5)).map(|v| (v, false))
    };
    match result {
        Ok((view, timed_out)) => {
            let mut response = Response::success(parent_output(&view));
            response.execution_id = Some(parent.execution_id.clone());
            response.state = view.state;
            response.error = view.error;
            let code = if timed_out {
                response.error = Some(Error::new(
                    ErrorCode::Timeout,
                    "wait timed out; children continue",
                ));
                124
            } else if let Some(error) = &response.error {
                error_exit(error)
            } else if input.command == "result" {
                response.output = Some(json!({"result_availability":"unavailable","result":null}));
                response.error = Some(Error::new(
                    ErrorCode::OutputMissing,
                    "no aggregate result artifact available",
                ));
                11
            } else if input.command == "wait" && view.state == Some(TaskState::Partial) {
                11
            } else if input.command == "wait" && view.state == Some(TaskState::Failed) {
                10
            } else {
                0
            };
            (response, code)
        }
        Err(e) => {
            let (mut response, code) = failure(e);
            response.execution_id = Some(parent.execution_id.clone());
            (response, code)
        }
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn unknown_request(id: ExecutionId) -> (Response<Value>, i32) {
    let (mut response, code) = failure(Error::new(
        ErrorCode::RegistrationUnknown,
        "registration reserved; inspect the queue label matching execution_id; do not retry add",
    ));
    response.output = Some(json!({"registration_label": id}));
    response.execution_id = Some(id);
    (response, code)
}
fn availability(
    service: &ExecutionService,
    id: &ExecutionId,
    state: Option<TaskState>,
) -> Result<String, Error> {
    match axvern_core::results::inspect(&service.store, service.result_settings(), id) {
        Ok(Some(m)) => Ok(m.availability),
        Ok(None) if matches!(state, None | Some(TaskState::Queued | TaskState::Running)) => {
            Ok("pending".into())
        }
        Ok(None) => Ok("unavailable".into()),
        Err(e) => Err(e),
    }
}
fn result_response(
    service: &ExecutionService,
    id: &ExecutionId,
    token: Option<&str>,
    limit: usize,
    max_bytes: usize,
) -> (Response<Value>, i32) {
    match axvern_core::results::page(
        &service.store,
        service.result_settings(),
        id,
        token,
        limit,
        max_bytes,
    ) {
        Ok((manifest, output)) => {
            let mut response = Response::success(output);
            response.execution_id = Some(id.clone());
            if let Some(m) = manifest {
                response.state = m.state.map(Into::into);
                response.exit_code = m.exit_code;
                if matches!(
                    m.availability.as_str(),
                    "expired" | "evicted" | "unavailable"
                ) {
                    response.error = Some(Error::new(
                        match m.availability.as_str() {
                            "expired" => ErrorCode::ResultExpired,
                            "evicted" => ErrorCode::ResultEvicted,
                            _ => ErrorCode::OutputMissing,
                        },
                        format!("result_{}", m.availability),
                    ));
                    return (response, 11);
                }
            } else {
                match service.status(id) {
                    Ok(v) => {
                        response.state = v.task.as_ref().map(|t| t.state.into());
                        response.exit_code = v.task.as_ref().and_then(|t| t.exit_code);
                        if matches!(response.state, Some(TaskState::Queued | TaskState::Running)) {
                            response.output.as_mut().unwrap()["result_availability"] =
                                json!("pending");
                        } else {
                            response.error = Some(Error::new(
                                ErrorCode::OutputMissing,
                                "no saved result for this execution",
                            ));
                            return (response, 11);
                        }
                    }
                    Err(e) => {
                        response.error = Some(e);
                        return (response, 11);
                    }
                }
            }
            (response, 0)
        }
        Err(e) => {
            let (mut response, code) = failure(e);
            response.execution_id = Some(id.clone());
            (response, code)
        }
    }
}
fn run_response(
    service: &ExecutionService,
    record: ExecutionRecord,
    wait: Duration,
) -> (Response<Value>, i32) {
    let deadline = std::time::Instant::now() + wait;
    let mut response = Response::success(
        json!({"record": record, "result_availability": "pending", "result": null}),
    );
    response.execution_id = Some(record.execution_id.clone());
    match axvern_core::results::inspect(
        &service.store,
        service.result_settings(),
        &record.execution_id,
    ) {
        Ok(Some(m)) if m.finished_at.is_some() => return (saved_run_response(service, &record), 0),
        Err(error) => {
            response.error = Some(error);
            response.output.as_mut().unwrap()["result_availability"] = json!("unavailable");
            return (response, 0);
        }
        _ => (),
    }
    if !wait.is_zero() {
        match service.observe_wait(
            &record.execution_id,
            deadline.saturating_duration_since(std::time::Instant::now()),
        ) {
            Ok((view, _)) => {
                response.state = view.task.as_ref().map(|t| t.state.into());
                response.exit_code = view.task.as_ref().and_then(|t| t.exit_code);
                if matches!(response.state, Some(TaskState::Done | TaskState::Failed)) {
                    let result = saved_run_response(service, &record);
                    response.output = result.output;
                    // Result storage failure is separate from the target's completion.
                    response.error = result.error;
                }
            }
            Err(error) => {
                response.error = Some(error);
            }
        }
    }
    (response, 0)
}
fn saved_run_response(service: &ExecutionService, record: &ExecutionRecord) -> Response<Value> {
    let (mut response, _) = result_response(service, &record.execution_id, None, 100, 65536);
    let output = response
        .output
        .get_or_insert_with(|| json!({"result_availability":"unavailable", "result":null}));
    output["record"] = json!(record);
    response
}
fn worker_plan(
    runner: &std::path::Path,
    store: &std::path::Path,
    settings: &axvern_core::config::Results,
    plan: &ExecutionPlan,
    id: &ExecutionId,
) -> ExecutionPlan {
    let mut args = vec![
        "__axvern_worker".into(),
        store.as_os_str().into(),
        id.to_string().into(),
        settings.retention_seconds.to_string().into(),
        settings.capacity_bytes.to_string().into(),
        settings.max_artifact_bytes.to_string().into(),
        "--".into(),
        plan.program.clone(),
    ];
    args.extend(plan.args.clone());
    ExecutionPlan {
        program: runner.as_os_str().into(),
        args,
        cwd: plan.cwd.clone(),
    }
}
fn internal_worker(args: &[OsString]) -> Result<i32, Error> {
    if args.len() < 8 || args[6] != "--" {
        return Err(invalid());
    }
    let number = |i: usize| {
        args[i]
            .to_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(invalid)
    };
    let settings = axvern_core::config::Results {
        retention_seconds: number(3)?,
        capacity_bytes: number(4)?,
        max_artifact_bytes: number(5)?,
    };
    let config = axvern_core::config::Config {
        results: settings.clone(),
        ..Default::default()
    };
    config.validate()?;
    let id = args[2].to_str().ok_or_else(invalid)?.parse()?;
    let store = axvern_core::store::ExecutionStore::open(PathBuf::from(&args[1]))?;
    let plan = ExecutionPlan {
        program: args[7].clone(),
        args: args[8..].to_vec(),
        cwd: std::env::current_dir().map_err(|_| invalid())?,
    };
    axvern_core::results::run_worker(&store, &settings, &id, &plan)
}
fn visible(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
fn human(response: &Response<Value>) {
    if let Some(output) = &response.output {
        if let Some(effect) = output.get("effect").and_then(Value::as_str) {
            let effect = match effect {
                "ask" => "Ask",
                "allow" => "Allow",
                _ => "Deny",
            };
            println!(
                "decision: {effect}\nreason: {}\nrule_id: {}",
                visible(output["reason"].as_str().unwrap_or("")),
                output["rule_id"].as_str().unwrap_or("none")
            );
        } else if let Some(checks) = output.get("checks").and_then(Value::as_array) {
            println!("doctor: {}", output["level"].as_str().unwrap_or("fail"));
            for c in checks {
                println!(
                    "{}: {} — {}",
                    c["name"].as_str().unwrap_or(""),
                    c["level"].as_str().unwrap_or(""),
                    visible(c["message"].as_str().unwrap_or(""))
                );
                if let Some(remedy) = c["remedy"].as_str() {
                    println!("  remedy: {}", visible(remedy));
                }
            }
        } else if let Some(views) = output.get("executions").and_then(Value::as_array) {
            for view in views {
                println!(
                    "{} pueue={} {}",
                    view["record"]["execution_id"].as_str().unwrap_or(""),
                    view["record"]["backend"]["task_id"],
                    view["task"]["state"]
                        .as_str()
                        .or_else(|| view["error"]["code"].as_str())
                        .unwrap_or("unknown")
                );
            }
        } else {
            println!(
                "execution_id: {}",
                response
                    .execution_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            );
            if let Some(record) = output
                .get("record")
                .or_else(|| output.get("registration_receipt"))
            {
                println!("pueue_task_id: {}", record["backend"]["task_id"]);
            }
            if let Some(state) = response.state {
                println!(
                    "state: {}",
                    serde_json::to_value(state).unwrap().as_str().unwrap()
                );
            }
            if let Some(code) = response.exit_code {
                println!("exit_code: {code}");
            }
        }
    }
    if let Some(result) = response.output.as_ref().and_then(|o| o.get("result")) {
        if let Some(items) = result["items"].as_array() {
            for item in items {
                if let Some(text) = item["text"].as_str() {
                    println!("{}", visible(text));
                }
            }
        }
        if result["completeness"]["truncated"] == true {
            println!("truncated: true");
        }
        if let Some(cursor) = result["page"]["next_cursor"].as_str() {
            println!("next_cursor: {cursor}");
        }
    }
    if let Some(error) = &response.error {
        eprintln!("{}", visible(&error.to_string()));
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "__axvern_worker") {
        let result = internal_worker(&args);
        std::process::exit(match result {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{}", visible(&error.to_string()));
                1
            }
        });
    }
    if args == [OsString::from("--help")] {
        println!("{USAGE}");
        return;
    }
    if args == [OsString::from("--version")] {
        println!("axvern {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let boundary = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let fallback_json = !args[..boundary]
        .windows(2)
        .any(|w| w[0] == "--format" && w[1] == "human");
    let (mut response, code, json) = match parse(&args) {
        Ok(input) => {
            let (mut response, code) = execute(&input);
            response.command = Some(input.command.clone());
            (response, code, input.json)
        }
        Err(error) => {
            let (response, code) = failure(error);
            (response, code, fallback_json)
        }
    };
    if response.command.is_none() {
        response.command = args[..boundary]
            .iter()
            .filter_map(|a| a.to_str())
            .find(|a| {
                matches!(
                    *a,
                    "run" | "status" | "wait" | "history" | "doctor" | "policy" | "result"
                )
            })
            .map(|a| {
                if a == "policy" {
                    "policy check".into()
                } else {
                    a.into()
                }
            });
    }
    if response.command.as_deref() == Some("run")
        && serde_json::to_vec(&response).is_ok_and(|v| v.len() >= 65536)
        && let Some(output) = response.output.as_mut().and_then(Value::as_object_mut)
    {
        output.remove("record");
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&response).expect("response JSON")
        );
    } else {
        human(&response);
    }
    std::process::exit(code);
}

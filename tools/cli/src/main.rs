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

const USAGE: &str = "usage: axvern [--config PATH] [--format human|json] run [--cwd PATH] -- <program> [args...] | status <execution-id> | wait <execution-id> [--timeout 30s] | history [--limit 20] | doctor | policy check -- <program> [args...]";
struct Input {
    command: String,
    config: Option<PathBuf>,
    cwd: Option<PathBuf>,
    timeout: Duration,
    limit: usize,
    id: Option<ExecutionId>,
    target: Vec<OsString>,
    json: bool,
}
fn invalid() -> Error {
    Error::new(ErrorCode::InvalidInput, USAGE)
}
fn duration(s: &str) -> Result<Duration, Error> {
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
        .filter(|n| (1..=86_400_000).contains(n))
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
        limit: 20,
        id: None,
        target: args.get(boundary + 1..).unwrap_or_default().to_vec(),
        json: false,
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
                "--timeout" => input.timeout = duration(value.to_str().ok_or_else(invalid)?)?,
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
        ["status" | "wait", id] if boundary == args.len() => input.id = Some(id.parse()?),
        ["history" | "doctor"] if boundary == args.len() => (),
        _ => return Err(invalid()),
    }
    if (seen.contains("--cwd") && input.command != "run")
        || (seen.contains("--timeout") && input.command != "wait")
        || (seen.contains("--limit") && input.command != "history")
    {
        return Err(invalid());
    }
    Ok(input)
}
fn error_exit(error: &Error) -> i32 {
    match error.code {
        ErrorCode::InvalidInput | ErrorCode::InvalidEncoding => 2,
        ErrorCode::InvalidConfig => 3,
        ErrorCode::PermissionDenied => 4,
        ErrorCode::MissingTool => 5,
        ErrorCode::DaemonUnavailable => 6,
        ErrorCode::TaskNotFound => 7,
        ErrorCode::Expired => 8,
        ErrorCode::PolicyDenied => 9,
        ErrorCode::PartialResult | ErrorCode::OutputMissing => 11,
        ErrorCode::StoreFailure => 13,
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
        let report = doctor::diagnose(config, &cwd);
        let code = if report.level == CheckLevel::Fail {
            12
        } else {
            0
        };
        return (
            Response::success(serde_json::to_value(report).expect("report JSON")),
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
                Response::success(serde_json::to_value(decision).expect("decision JSON")),
                0,
            ),
            Err(error) => failure(error),
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
    match input.command.as_str() {
        "run" => {
            let p = run_plan.as_ref().expect("run plan");
            match service.run(p, interactive, approval) {
                Ok(record) => {
                    let mut response = Response::success(json!({"record": record}));
                    response.execution_id = Some(record.execution_id);
                    (response, 0)
                }
                Err(f) => {
                    let (mut response, code) = failure(f.error);
                    if let Some(receipt) = f.receipt {
                        response.execution_id = Some(receipt.execution_id.clone());
                        response.output = Some(json!({"registration_receipt": receipt}));
                    }
                    (response, code)
                }
            }
        }
        "status" | "wait" => {
            let id = input.id.as_ref().expect("parsed ID");
            let result = if input.command == "wait" {
                service.wait(id, input.timeout)
            } else {
                service.status(id)
            };
            match result {
                Ok(view) => {
                    let task = view.task.as_ref().expect("resolved task");
                    let code = if input.command == "wait" && task.state == ChildState::Failed {
                        10
                    } else {
                        0
                    };
                    let mut response =
                        Response::success(serde_json::to_value(&view).expect("view JSON"));
                    response.execution_id = Some(id.clone());
                    response.state = Some(task.state.into());
                    response.exit_code = task.exit_code;
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
                let partial = views.iter().any(|v| {
                    v.error
                        .as_ref()
                        .is_some_and(|e| e.code != ErrorCode::Expired)
                });
                let mut response = Response::success(json!({"executions": views}));
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
                output["reason"].as_str().unwrap_or(""),
                output["rule_id"].as_str().unwrap_or("none")
            );
        } else if let Some(checks) = output.get("checks").and_then(Value::as_array) {
            println!("doctor: {}", output["level"].as_str().unwrap_or("fail"));
            for c in checks {
                println!(
                    "{}: {} — {}",
                    c["name"].as_str().unwrap_or(""),
                    c["level"].as_str().unwrap_or(""),
                    c["message"].as_str().unwrap_or("")
                );
                if let Some(remedy) = c["remedy"].as_str() {
                    println!("  remedy: {remedy}");
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
    if let Some(error) = &response.error {
        eprintln!("{error}");
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == [OsString::from("--help")] {
        println!("{USAGE}");
        return;
    }
    if args == [OsString::from("--version")] {
        println!("axvern {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let boundary = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let fallback_json = args[..boundary]
        .windows(2)
        .any(|w| w[0] == "--format" && w[1] == "json");
    let (response, code, json) = match parse(&args) {
        Ok(input) => {
            let (response, code) = execute(&input);
            (response, code, input.json)
        }
        Err(error) => {
            let (response, code) = failure(error);
            (response, code, fallback_json)
        }
    };
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

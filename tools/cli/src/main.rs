use axvern_core::{
    analyze, discovery, doctor,
    planner::{self, Planned},
    policy,
    rewrite::{self, Rewrite},
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

const USAGE: &str = "usage: axvern [--config PATH] [--format human|json] run [--cwd PATH] [--wait 1s] [--request-id KEY] -- <program> [args...] | result <execution-id> [--cursor TOKEN] [--limit 100] [--max-bytes 65536] | status <execution-id> | wait <execution-id> [--timeout 30s] | history [--limit 20] | doctor [--scope core|discovery|logs|performance|all] | policy check [--cwd PATH] -- <program> [args...] | fs list [PATH] [--depth 1] [--include-ignored] [--wait 1s] | fs find PATTERN [PATH] [--glob] [--full-path] [--extension EXT] [--type file|directory|symlink|any] [--depth 32] [--include-ignored] [--ignore-case] [--wait 1s] | fs search PATTERN [PATH] [--fixed-strings] [--glob GLOB] [--context 0] [--ignore-case] [--include-ignored] [--wait 1s] | fs read FILE [--from-line 1] [--lines 80] [--wait 1s] | fs analyze PATH [--depth 3] [--include-ignored] [--wait 1s] | code search --pattern PATTERN --lang rust|javascript|typescript|python|go [PATH] [--include-ignored] [--wait 1s] | code stats [PATH] [--include-ignored] [--wait 1s] | system disk-usage [PATH] [--depth 3] [--top 20] [--size allocated|apparent] [--wait 1s] | code diff [--staged] [--wait 1s] | system processes [--match TEXT] [--limit 100] [--wait 1s]";
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
    fs_args: Vec<String>,
    switches: std::collections::HashSet<String>,
    depth: Option<u32>,
    context: Option<u32>,
    from_line: Option<u64>,
    lines: Option<u32>,
    entry_type: Option<EntryFilter>,
    extensions: Vec<String>,
    globs: Vec<String>,
    pattern: Option<String>,
    lang: Option<String>,
    top: Option<u32>,
    size_mode: Option<SizeMode>,
    matching: Option<String>,
    process_limit: Option<u32>,
}
/// Flags that only the discovery commands accept, and which of them each command accepts.
const FS_FLAGS: [&str; 17] = [
    "--staged",
    "--match",
    "--top",
    "--size",
    "--depth",
    "--include-ignored",
    "--glob",
    "--full-path",
    "--extension",
    "--type",
    "--ignore-case",
    "--fixed-strings",
    "--context",
    "--from-line",
    "--lines",
    "--pattern",
    "--lang",
];
fn fs_flags(command: &str) -> &'static [&'static str] {
    match command {
        "fs list" | "fs analyze" => &["--depth", "--include-ignored"],
        "fs find" => &[
            "--glob",
            "--full-path",
            "--extension",
            "--type",
            "--depth",
            "--include-ignored",
            "--ignore-case",
        ],
        "fs search" => &[
            "--fixed-strings",
            "--glob",
            "--context",
            "--ignore-case",
            "--include-ignored",
        ],
        "fs read" => &["--from-line", "--lines"],
        "code search" => &["--pattern", "--lang", "--include-ignored"],
        "code stats" => &["--include-ignored"],
        "system disk-usage" => &["--depth", "--top", "--size"],
        "code diff" => &["--staged"],
        "system processes" => &["--match"],
        _ => &[],
    }
}
fn digits(value: &OsString) -> Result<u64, Error> {
    let text = value.to_str().ok_or_else(invalid)?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    text.parse().map_err(|_| invalid())
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
        fs_args: Vec::new(),
        switches: Default::default(),
        depth: None,
        context: None,
        from_line: None,
        lines: None,
        entry_type: None,
        extensions: Vec::new(),
        globs: Vec::new(),
        pattern: None,
        lang: None,
        top: None,
        size_mode: None,
        matching: None,
        process_limit: None,
    };
    let mut positional = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while i < boundary {
        let arg = args[i].to_str().ok_or_else(invalid)?;
        if arg.starts_with('-') {
            // `--glob` selects the pattern syntax for find and takes a value for search.
            let switch = match arg {
                "--include-ignored" | "--full-path" | "--ignore-case" | "--fixed-strings"
                | "--staged" => true,
                "--glob" => positional.starts_with(&["fs", "find"]),
                _ => false,
            };
            let repeatable = arg == "--extension" || (arg == "--glob" && !switch);
            if !seen.insert(arg) && !repeatable {
                return Err(invalid());
            }
            if switch {
                input.switches.insert(arg.to_owned());
                i += 1;
                continue;
            }
            if i + 1 >= boundary {
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
                "--depth" => {
                    input.depth = Some(u32::try_from(digits(value)?).map_err(|_| invalid())?)
                }
                "--top" => input.top = Some(u32::try_from(digits(value)?).map_err(|_| invalid())?),
                "--size" => {
                    input.size_mode = Some(match value.to_str() {
                        Some("allocated") => SizeMode::Allocated,
                        Some("apparent") => SizeMode::Apparent,
                        _ => return Err(invalid()),
                    })
                }
                "--context" => {
                    input.context = Some(u32::try_from(digits(value)?).map_err(|_| invalid())?)
                }
                "--from-line" => input.from_line = Some(digits(value)?),
                "--lines" => {
                    input.lines = Some(u32::try_from(digits(value)?).map_err(|_| invalid())?)
                }
                "--type" => {
                    input.entry_type = Some(match value.to_str() {
                        Some("file") => EntryFilter::File,
                        Some("directory") => EntryFilter::Directory,
                        Some("symlink") => EntryFilter::Symlink,
                        Some("any") => EntryFilter::Any,
                        _ => return Err(invalid()),
                    })
                }
                "--extension" if !value.is_empty() => input
                    .extensions
                    .push(value.to_str().ok_or_else(invalid)?.into()),
                "--match" if !value.is_empty() => {
                    input.matching = Some(value.to_str().ok_or_else(invalid)?.into())
                }
                "--pattern" if !value.is_empty() => {
                    input.pattern = Some(value.to_str().ok_or_else(invalid)?.into())
                }
                "--lang" if !value.is_empty() => {
                    input.lang = Some(value.to_str().ok_or_else(invalid)?.into())
                }
                "--glob" if !value.is_empty() => {
                    input.globs.push(value.to_str().ok_or_else(invalid)?.into())
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
        [
            "fs",
            sub @ ("list" | "find" | "search" | "read" | "analyze"),
            rest @ ..,
        ] => {
            // After `--` everything is a literal operand, so patterns may start with a dash.
            let mut words: Vec<String> = rest.iter().map(|w| w.to_string()).collect();
            for word in &input.target {
                words.push(word.to_str().ok_or_else(invalid)?.to_owned());
            }
            let count_ok = match *sub {
                "list" => words.len() <= 1,
                "find" | "search" => (1..=2).contains(&words.len()),
                // read FILE, analyze PATH
                _ => words.len() == 1,
            };
            if !count_ok {
                return Err(invalid());
            }
            input.command = format!("fs {sub}");
            input.fs_args = words;
        }
        ["code", "diff"] if boundary == args.len() => input.command = "code diff".into(),
        ["system", "processes"] if boundary == args.len() => {
            input.command = "system processes".into();
            input.process_limit = seen
                .contains("--limit")
                .then(|| u32::try_from(input.limit).ok())
                .flatten();
        }
        ["code", sub @ ("search" | "stats"), rest @ ..] => {
            let mut words: Vec<String> = rest.iter().map(|w| w.to_string()).collect();
            for word in &input.target {
                words.push(word.to_str().ok_or_else(invalid)?.to_owned());
            }
            if words.len() > 1
                || (*sub == "search" && (input.pattern.is_none() || input.lang.is_none()))
            {
                return Err(invalid());
            }
            input.command = format!("code {sub}");
            input.fs_args = words;
        }
        ["system", "disk-usage", rest @ ..] => {
            let mut words: Vec<String> = rest.iter().map(|w| w.to_string()).collect();
            for word in &input.target {
                words.push(word.to_str().ok_or_else(invalid)?.to_owned());
            }
            if words.len() > 1 {
                return Err(invalid());
            }
            input.command = "system disk-usage".into();
            input.fs_args = words;
        }
        _ => return Err(invalid()),
    }
    let discovery = is_discovery(&input.command);
    if FS_FLAGS.iter().any(|flag| {
        (seen.contains(flag) || input.switches.contains(*flag))
            && !fs_flags(&input.command).contains(flag)
    }) {
        return Err(invalid());
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
        || (seen.contains("--wait") && input.command != "run" && !discovery)
        || (seen.contains("--request-id") && input.command != "run")
        || (seen.contains("--limit")
            && !matches!(
                input.command.as_str(),
                "history" | "result" | "system processes"
            ))
        || ((seen.contains("--cursor") || seen.contains("--max-bytes"))
            && input.command != "result")
    {
        return Err(invalid());
    }
    Ok(input)
}
/// Commands that run an external tool in the background and store a [`DiscoveryDocument`].
fn is_discovery(command: &str) -> bool {
    command.starts_with("fs ")
        || command.starts_with("code ")
        || matches!(command, "system disk-usage" | "system processes")
}
fn discovery_request(input: &Input) -> Result<DiscoveryRequest, Error> {
    let path = |i: usize| input.fs_args.get(i).map(PathBuf::from);
    let switch = |name: &str| input.switches.contains(name);
    Ok(match input.command.as_str() {
        "fs list" => DiscoveryRequest::List {
            path: path(0),
            depth: input.depth.unwrap_or(1),
            include_ignored: switch("--include-ignored"),
        },
        "fs find" => DiscoveryRequest::Find {
            pattern: input.fs_args[0].clone(),
            path: path(1),
            syntax: if switch("--glob") {
                PatternSyntax::Glob
            } else {
                PatternSyntax::Regex
            },
            full_path: switch("--full-path"),
            extensions: input.extensions.clone(),
            entry_type: input.entry_type.unwrap_or(EntryFilter::Any),
            depth: input.depth.unwrap_or(MAX_DEPTH),
            include_ignored: switch("--include-ignored"),
            ignore_case: switch("--ignore-case"),
        },
        "fs search" => DiscoveryRequest::Search {
            pattern: input.fs_args[0].clone(),
            path: path(1),
            fixed_strings: switch("--fixed-strings"),
            globs: input.globs.clone(),
            context: input.context.unwrap_or(0),
            ignore_case: switch("--ignore-case"),
            include_ignored: switch("--include-ignored"),
        },
        "fs read" => DiscoveryRequest::Read {
            file: PathBuf::from(&input.fs_args[0]),
            from_line: input.from_line.unwrap_or(1),
            lines: input.lines.unwrap_or(DEFAULT_READ_LINES),
        },
        "code search" => DiscoveryRequest::CodeSearch {
            pattern: input.pattern.clone().ok_or_else(invalid)?,
            lang: input.lang.clone().ok_or_else(invalid)?,
            path: path(0),
            include_ignored: switch("--include-ignored"),
        },
        "code stats" => DiscoveryRequest::CodeStats {
            path: path(0),
            include_ignored: switch("--include-ignored"),
        },
        "system disk-usage" => DiscoveryRequest::DiskUsage {
            path: path(0),
            depth: input.depth.unwrap_or(DEFAULT_DISK_DEPTH),
            top: input.top.unwrap_or(DEFAULT_DISK_TOP),
            size_mode: input.size_mode.unwrap_or(SizeMode::Allocated),
        },
        "code diff" => DiscoveryRequest::CodeDiff {
            staged: switch("--staged"),
        },
        "system processes" => DiscoveryRequest::Processes {
            matching: input.matching.clone(),
            limit: input.process_limit.unwrap_or(DEFAULT_PROCESS_LIMIT),
        },
        _ => return Err(invalid()),
    })
}
fn error_exit(error: &Error) -> i32 {
    match error.code {
        ErrorCode::RequestConflict => 15,
        ErrorCode::UnsupportedCapability
        | ErrorCode::UnsupportedLanguage
        | ErrorCode::UnsupportedToolCapability => 16,
        ErrorCode::InvalidInput
        | ErrorCode::InvalidPattern
        | ErrorCode::InvalidTarget
        | ErrorCode::TargetNotFound
        | ErrorCode::NotARepository
        | ErrorCode::UnsupportedEncoding
        | ErrorCode::InvalidEncoding
        | ErrorCode::InvalidCursor => 2,
        ErrorCode::InvalidConfig => 3,
        ErrorCode::PermissionDenied => 4,
        ErrorCode::MissingTool => 5,
        ErrorCode::DaemonUnavailable => 6,
        ErrorCode::TaskNotFound => 7,
        ErrorCode::Expired => 8,
        ErrorCode::PolicyDenied => 9,
        ErrorCode::PartialResult
        | ErrorCode::ParseError
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
fn planned(config: &axvern_core::config::Config, input: &Input) -> Result<Planned, Error> {
    planner::plan(config, plan(input)?, rewrite::on_path)
}
/// Surfaces a rewrite or an explicit skip; unchanged requests add nothing.
fn with_rewrite(mut response: Response<Value>, rewrite: &Rewrite) -> Response<Value> {
    if *rewrite != Rewrite::Unchanged
        && let Some(output) = response.output.as_mut()
    {
        output["rewrite"] = serde_json::to_value(rewrite).expect("rewrite JSON");
    }
    response
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
        return match planned(&config, input).and_then(|p| Ok((planner::evaluate(&config, &p)?, p)))
        {
            Ok((decision, p)) => (
                with_rewrite(
                    Response::success({
                        let mut v = serde_json::to_value(decision).expect("decision JSON");
                        v["kind"] = json!("policy");
                        v
                    }),
                    &p.rewrite,
                ),
                0,
            ),
            Err(error) => failure(error),
        };
    }
    if input.command == "run" && input.request_id.is_some() {
        let planned = match planned(&config, input) {
            Ok(p) => p,
            Err(e) => return failure(e),
        };
        let p = planned.original.clone();
        // Validate encoding and limits even for a replay; evaluation never prompts.
        if let Err(e) = planner::evaluate(&config, &planned) {
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
                let decision = match planner::evaluate(&config, &planned) {
                    Ok(d) => d,
                    Err(e) => return failure(e),
                };
                if decision.effect == PolicyEffect::Ask && interactive {
                    approval = approve();
                }
            }
            Err(e) => return failure(e),
        }
        let executable = &planned.executable;
        let registration = service.store.register_request_rewritten(
            key,
            &p,
            planned.record().cloned(),
            || {
                planner::authorize(&config, &planned, interactive, approval)?;
                axvern_core::results::preflight_registration_locked(
                    &service.store,
                    &settings,
                    executable,
                )?;
                check_adapter.check()
            },
            |id| {
                let worker =
                    services::worker_plan(&runner, &worker_store, &settings, executable, id, None)?;
                service.adapter.register_with_label(&worker, id)
            },
        );
        return match registration {
            Ok(request) if request.record.is_some() => {
                let (response, code) = run_response(&service, request.record.unwrap(), input.wait);
                (with_rewrite(response, &planned.rewrite), code)
            }
            Ok(request) => unknown_request(request.execution_id),
            Err(error) => match service.store.request(key, &p) {
                Ok(Some(request)) => unknown_request(request.execution_id),
                _ => failure(error),
            },
        };
    }
    if input.command == "fs analyze" {
        return analyze(input, config);
    }
    // Validate input and policy before any backend request or local store initialization.
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let mut approval = Approval::NotProvided;
    let mut job = None;
    let run_plan = if input.command == "run" || is_discovery(&input.command) {
        let p = if input.command == "run" {
            match planned(&config, input) {
                Ok(p) => p,
                Err(e) => return failure(e),
            }
        } else {
            let prepared = std::env::current_dir()
                .map_err(|_| {
                    Error::new(ErrorCode::InvalidInput, "cannot resolve working directory")
                })
                .and_then(|cwd| {
                    discovery::prepare(&discovery_request(input)?, &cwd, &config.discovery.limits())
                });
            match prepared {
                Ok(prepared) => {
                    job = Some(prepared.job);
                    Planned::unchanged(prepared.plan)
                }
                Err(e) => return failure(e),
            }
        };
        let decision = match planner::evaluate(&config, &p) {
            Ok(d) => d,
            Err(e) => return failure(e),
        };
        if decision.effect == PolicyEffect::Ask && interactive {
            approval = approve();
        }
        if let Err(e) = planner::authorize(&config, &p, interactive, approval) {
            return failure(e);
        }
        if job.is_some()
            && let Err(e) =
                discovery::ensure_tool(job.as_ref().expect("discovery job"), &p.executable)
        {
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
        "run" | "fs list" | "fs find" | "fs search" | "fs read" | "code search" | "code stats"
        | "system disk-usage" | "code diff" | "system processes" => {
            let p = run_plan.as_ref().expect("run plan");
            match std::env::current_exe()
                .map_err(|_| axvern_core::store::RegistrationFailure {
                    error: Error::new(ErrorCode::BackendFailure, "cannot locate internal runner"),
                    receipt: None,
                    execution_id: None,
                })
                .and_then(|runner| {
                    service.run_planned_job(&runner, p, job.as_ref(), interactive, approval)
                }) {
                Ok(record) => {
                    let (response, code) = run_response(&service, record, input.wait);
                    (with_rewrite(response, &p.rewrite), code)
                }
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
                        if parent.analysis.is_some() {
                            let state = match analyze::observe_with_tasks(
                                &service,
                                &parent,
                                tasks.as_ref().unwrap(),
                            ) {
                                Ok(state) => state,
                                Err(e) => return failure(e),
                            };
                            partial |= state
                                .error
                                .as_ref()
                                .is_some_and(|e| e.code != ErrorCode::Expired);
                            *value = analysis_view(&parent, &state);
                            continue;
                        }
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
/// Register every child of `fs analyze` after all of them passed validation, Policy and the
/// tool check; nothing is registered when any of them fails.
fn analyze(input: &Input, config: axvern_core::config::Config) -> (Response<Value>, i32) {
    let prepared = std::env::current_dir()
        .map_err(|_| Error::new(ErrorCode::InvalidInput, "cannot resolve working directory"))
        .and_then(|cwd| {
            analyze::prepare(
                std::path::Path::new(&input.fs_args[0]),
                input.depth.unwrap_or(DEFAULT_TREE_DEPTH),
                input.switches.contains("--include-ignored"),
                &cwd,
                &config.discovery.limits(),
            )
        });
    let prepared = match prepared {
        Ok(p) => p,
        Err(e) => return failure(e),
    };
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let mut asks = false;
    for (_, child) in &prepared.children {
        match policy::evaluate(&config, &child.plan) {
            Ok(d) if d.effect == PolicyEffect::Deny => {
                return failure(Error::new(
                    ErrorCode::PolicyDenied,
                    "policy or approval refused registration",
                ));
            }
            Ok(d) => asks |= d.effect == PolicyEffect::Ask,
            Err(e) => return failure(e),
        }
    }
    // One prompt answers every child that needs approval.
    let approval = if asks && interactive {
        approve()
    } else {
        Approval::NotProvided
    };
    if let Err(e) = analyze::preflight(&config, &prepared, interactive, approval) {
        return failure(e);
    }
    let mut service = match ExecutionService::new(config) {
        Ok(s) => s,
        Err(e) => return failure(e),
    };
    let runner = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => {
            return failure(Error::new(
                ErrorCode::BackendFailure,
                "cannot locate internal runner",
            ));
        }
    };
    let store = service.store.clone();
    let registration = analyze::register(&store, &prepared, |child| {
        service.run_captured_job(
            &runner,
            &child.plan,
            Some(&child.job),
            interactive,
            approval,
        )
    });
    match registration {
        Ok((record, registration_error)) => {
            let (mut response, mut code) = analysis_start_response(&service, &record, input.wait);
            if let Some(error) = registration_error {
                code = error_exit(&error);
                response.error = Some(error);
            }
            (response, code)
        }
        Err(f) => {
            let (mut response, code) = failure(f.error);
            if !f.registered.is_empty() {
                response.output = Some(json!({"registered_children": f.registered}));
            }
            (response, code)
        }
    }
}
fn analysis_view(record: &ParentRecord, state: &analyze::ParentState) -> Value {
    json!({"kind":analyze::KIND, "command":"fs analyze", "record":record,
        "task":{"state":state.state,"exit_code":null,"started_at":null,"finished_at":null},
        "children":state.children, "error":state.error,
        "source":if state.manifest.is_some() { "store" } else { "backend" }, "observed_at":now(),
        "result_availability":state.manifest.as_ref().map_or("pending", |m| m.availability.as_str())})
}
/// First result page of a published parent, or its sections while children still run.
fn analysis_result(
    service: &ExecutionService,
    record: &ParentRecord,
    state: &analyze::ParentState,
    cursor: Option<&str>,
    limit: usize,
    max_bytes: usize,
) -> (Response<Value>, i32) {
    let (mut response, code) = if state.manifest.is_some() {
        result_response(service, &record.execution_id, cursor, limit, max_bytes)
    } else {
        let mut response = Response::success(
            json!({"result_availability":"pending","result":null,"children":state.children}),
        );
        response.execution_id = Some(record.execution_id.clone());
        response.error = state.error.clone();
        let code = state.error.as_ref().map_or(0, error_exit);
        (response, code)
    };
    response.state = state.state;
    response.exit_code = None;
    (response, code)
}
fn analysis_start_response(
    service: &ExecutionService,
    record: &ParentRecord,
    wait: Duration,
) -> (Response<Value>, i32) {
    let observed = if wait.is_zero() {
        Ok(analyze::ParentState {
            state: None,
            children: analyze::registered(record),
            manifest: None,
            error: None,
        })
    } else {
        analyze::wait(service, record, wait).map(|(state, _)| state)
    };
    let mut state = match observed {
        Ok(state) => state,
        Err(e) => {
            let (mut response, _) = failure(e);
            response.execution_id = Some(record.execution_id.clone());
            response.output =
                Some(json!({"record":record,"result_availability":"pending","result":null}));
            return (response, 0);
        }
    };
    // Running out of the wait budget is not an error; the children continue.
    if state
        .error
        .as_ref()
        .is_some_and(|e| e.code == ErrorCode::Timeout)
    {
        state.error = None;
    }
    // Leave room for the parent record next to the first page.
    let (mut response, _) = analysis_result(service, record, &state, None, 100, 60000);
    if let Some(output) = response.output.as_mut() {
        output["record"] = json!(record);
    }
    (response, 0)
}
fn analysis_response(
    service: &ExecutionService,
    parent: &ParentRecord,
    input: &Input,
) -> (Response<Value>, i32) {
    let observed = match input.command.as_str() {
        "wait" => analyze::wait(service, parent, input.timeout),
        _ => analyze::observe(service, parent, Duration::from_secs(5)).map(|s| (s, false)),
    };
    let (state, timed_out) = match observed {
        Ok(v) => v,
        Err(e) => {
            let (mut response, code) = failure(e);
            response.execution_id = Some(parent.execution_id.clone());
            return (response, code);
        }
    };
    if input.command == "result" {
        return analysis_result(
            service,
            parent,
            &state,
            input.cursor.as_deref(),
            input.limit,
            input.max_bytes,
        );
    }
    let mut response = Response::success(analysis_view(parent, &state));
    response.execution_id = Some(parent.execution_id.clone());
    response.state = state.state;
    response.error = state.error.clone();
    let code = if timed_out {
        response.error = Some(Error::new(
            ErrorCode::Timeout,
            "wait timed out; children continue",
        ));
        124
    } else if let Some(error) = &response.error {
        error_exit(error)
    } else if input.command == "wait" && state.state == Some(TaskState::Partial) {
        11
    } else if input.command == "wait" && state.state == Some(TaskState::Failed) {
        10
    } else {
        0
    };
    (response, code)
}
fn parent_response(
    service: &ExecutionService,
    parent: &ParentRecord,
    input: &Input,
) -> (Response<Value>, i32) {
    if parent.analysis.is_some() {
        return analysis_response(service, parent, input);
    }
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
/// `__axvern_discovery STORE ID RETENTION CAPACITY MAX_ARTIFACT JOB -- program args...`
fn internal_discovery(args: &[OsString]) -> Result<i32, Error> {
    if args.len() < 9 || args[7] != "--" {
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
    axvern_core::config::Config {
        results: settings.clone(),
        ..Default::default()
    }
    .validate()?;
    let id = args[2].to_str().ok_or_else(invalid)?.parse()?;
    let job: DiscoveryJob =
        serde_json::from_str(args[6].to_str().ok_or_else(invalid)?).map_err(|_| invalid())?;
    let store = axvern_core::store::ExecutionStore::open(PathBuf::from(&args[1]))?;
    let plan = ExecutionPlan {
        program: args[8].clone(),
        args: args[9..].to_vec(),
        cwd: std::env::current_dir().map_err(|_| invalid())?,
    };
    discovery::run_worker(&store, &settings, &id, &job, &plan)
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
            // A parent record has children instead of a backend task.
            if let Some(record) = output
                .get("record")
                .or_else(|| output.get("registration_receipt"))
                .filter(|r| r.get("backend").is_some())
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
                if let Some(section) = item["section"].as_str() {
                    print!("{section}\t");
                }
                if let Some(pid) = item["pid"].as_u64() {
                    let field = |key: &str| match &item[key] {
                        Value::Null => "-".to_owned(),
                        Value::String(s) => visible(s),
                        other => other.to_string(),
                    };
                    println!(
                        "{pid}\t{}\t{}\t{}\t{}\t{}\t{}",
                        field("ppid"),
                        field("uid"),
                        field("state"),
                        field("rss_bytes"),
                        field("started_at"),
                        field("name")
                    );
                    continue;
                }
                if let Some(change) = item["change"].as_str() {
                    let side = |key: &str| item[key].as_str().map_or("-".to_owned(), visible);
                    println!(
                        "{change}\t{}\t{}{}",
                        side("old_path"),
                        side("new_path"),
                        if item["binary"] == true {
                            "\tbinary"
                        } else {
                            ""
                        }
                    );
                    for hunk in item["hunks"].as_array().into_iter().flatten() {
                        for line in hunk["patch"].as_str().unwrap_or("").lines() {
                            println!("{}", visible(line));
                        }
                    }
                    if item["patch_truncated"] == true {
                        println!("patch_truncated: true");
                    }
                    continue;
                }
                if let (Some(path), Some(bytes)) = (item["path"].as_str(), item["bytes"].as_u64()) {
                    println!("{bytes}\t{}", visible(path));
                    continue;
                }
                if let Some(language) = item["language"].as_str() {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        visible(language),
                        item["files"],
                        item["code"],
                        item["comments"],
                        item["blanks"]
                    );
                    continue;
                }
                if let (Some(path), Some(snippet)) =
                    (item["path"].as_str(), item["snippet"].as_str())
                {
                    println!(
                        "{}:{}:{}",
                        visible(path),
                        item["start"]["line"],
                        visible(snippet.lines().next().unwrap_or(""))
                    );
                    continue;
                }
                let text = item["text"].as_str().map(visible);
                let path = item["path"].as_str().map(visible);
                match (item["type"].as_str(), path, item["line"].as_u64(), text) {
                    (Some(kind), Some(path), _, _) => println!("{kind}\t{path}"),
                    (_, Some(path), Some(line), Some(text)) => println!("{path}:{line}:{text}"),
                    (_, None, Some(line), Some(text)) => println!("{line}\t{text}"),
                    (_, _, _, Some(text)) => println!("{text}"),
                    _ => (),
                }
            }
        }
        if let Some(errors) = result["errors"].as_array() {
            for issue in errors {
                println!(
                    "error: {} {}",
                    issue["code"].as_str().unwrap_or(""),
                    visible(issue["path"].as_str().unwrap_or(""))
                );
            }
        }
        if result["completeness"]["partial"] == true {
            println!("partial: true");
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
    if args
        .first()
        .is_some_and(|a| a == "__axvern_worker" || a == "__axvern_discovery")
    {
        let result = if args[0] == "__axvern_worker" {
            internal_worker(&args)
        } else {
            internal_discovery(&args)
        };
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
        let words: Vec<_> = args[..boundary].iter().filter_map(|a| a.to_str()).collect();
        response.command = words
            .iter()
            .position(|a| matches!(*a, "fs" | "code"))
            .and_then(|i| Some((words[i], words.get(i + 1)?)))
            .filter(|(group, sub)| match *group {
                "fs" => matches!(**sub, "list" | "find" | "search" | "read" | "analyze"),
                _ => matches!(**sub, "search" | "stats"),
            })
            .map(|(group, sub)| format!("{group} {sub}"));
    }
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
    if response
        .command
        .as_deref()
        .is_some_and(|c| c == "run" || is_discovery(c))
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

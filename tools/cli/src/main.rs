use axvern_core::{config, policy};
use axvern_models::{Error, ErrorCode, ExecutionPlan, PolicyDecision, Response};
use std::{ffi::OsString, path::PathBuf};

fn run(args: Vec<OsString>) -> Result<(PolicyDecision, bool), (Error, bool)> {
    // Locate formatting only in axvern's option region, never in the target argv.
    let boundary = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let json = args[..boundary]
        .windows(2)
        .any(|w| w[0] == "--format" && w[1] == "json");
    let fail = || {
        (
            Error::new(
                ErrorCode::InvalidInput,
                "usage: axvern policy check [--config PATH] [--format human|json] -- <program> [args...]",
            ),
            json,
        )
    };
    let mut path: Option<PathBuf> = None;
    let mut format_seen = false;
    let mut command = Vec::new();
    let mut i = 0;
    while i < boundary {
        match args[i].to_str() {
            Some("--config") if path.is_none() && i + 1 < boundary && !args[i + 1].is_empty() => {
                path = Some((&args[i + 1]).into());
                i += 2;
            }
            Some("--format")
                if !format_seen
                    && i + 1 < boundary
                    && (args[i + 1] == "json" || args[i + 1] == "human") =>
            {
                format_seen = true;
                i += 2;
            }
            Some("policy" | "check") => {
                command.push(args[i].clone());
                i += 1;
            }
            _ => return Err(fail()),
        }
    }
    if command != [OsString::from("policy"), OsString::from("check")]
        || boundary == args.len()
        || boundary + 1 >= args.len()
    {
        return Err(fail());
    }
    let env_config = std::env::var_os("AXVERN_CONFIG");
    let implicit = if path.is_none() && env_config.is_none() {
        config::UserPaths::discover().map_err(|e| (e, json))?.config
    } else {
        PathBuf::new()
    };
    let config = config::load(path.as_deref(), env_config, &implicit).map_err(|e| (e, json))?;
    let plan = ExecutionPlan {
        program: args[boundary + 1].clone(),
        args: args[boundary + 2..].to_vec(),
        cwd: std::env::current_dir().map_err(|_| {
            (
                Error::new(ErrorCode::InvalidInput, "cannot resolve working directory"),
                json,
            )
        })?,
    };
    policy::evaluate(&config, &plan)
        .map(|decision| (decision, json))
        .map_err(|e| (e, json))
}
fn main() {
    match run(std::env::args_os().skip(1).collect()) {
        Ok((decision, json)) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&Response::success(decision))
                        .expect("JSON serialization")
                );
            } else {
                println!(
                    "decision: {:?}\nreason: {}\nrule_id: {}",
                    decision.effect,
                    decision.reason,
                    decision.rule_id.as_deref().unwrap_or("none")
                );
            }
        }
        Err((error, json)) => {
            let code = match error.code {
                ErrorCode::InvalidConfig => 3,
                ErrorCode::PermissionDenied => 4,
                _ => 2,
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&Response::<PolicyDecision>::failure(error))
                        .expect("JSON serialization")
                );
            } else {
                eprintln!("{error}");
            }
            std::process::exit(code);
        }
    }
}

//! Read-only diagnosis. Every wait, including store contention, shares one deadline.
use crate::{
    adapters::{
        pueue::probe,
        tools::{self, ToolSpec, ToolStatus, version_token},
    },
    config::Config,
    services,
    store::ExecutionStore,
};
use axvern_models::*;
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn check(name: &str, result: Result<(), Error>, remedy: &str) -> Diagnostic {
    match result {
        Ok(()) => Diagnostic {
            name: name.into(),
            level: CheckLevel::Pass,
            message: "available".into(),
            remedy: None,
            tool: None,
        },
        Err(error) => Diagnostic {
            name: name.into(),
            level: CheckLevel::Fail,
            message: error.to_string(),
            remedy: Some(remedy.into()),
            tool: None,
        },
    }
}
fn warning(name: &str, message: &str, remedy: &str) -> Diagnostic {
    Diagnostic {
        name: name.into(),
        level: CheckLevel::Warning,
        message: message.into(),
        remedy: Some(remedy.into()),
        tool: None,
    }
}
fn remaining(deadline: Instant) -> Result<Duration, Error> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(Error::new(
            ErrorCode::Timeout,
            "doctor overall deadline reached; check not performed",
        ))
    } else {
        Ok(remaining.min(Duration::from_secs(5)))
    }
}
/// A tool with a provided adapter: pass only when installed at a supported version.
fn adapter_tool(spec: &ToolSpec, deadline: Instant) -> Diagnostic {
    let status = remaining(deadline)
        .map(|timeout| tools::detect(spec, timeout))
        .unwrap_or(ToolStatus::Unknown);
    let (installed, version, level, message) = match &status {
        ToolStatus::Supported(v) => (
            Some(true),
            Some(v.clone()),
            CheckLevel::Pass,
            "available".to_owned(),
        ),
        ToolStatus::Unsupported(v) => (
            Some(true),
            Some(v.clone()),
            CheckLevel::Fail,
            format!("unsupported tool capability: {} {v}", spec.name),
        ),
        ToolStatus::Missing => (
            Some(false),
            None,
            CheckLevel::Fail,
            format!("missing tool: {} is not installed", spec.name),
        ),
        ToolStatus::Unknown => (
            None,
            None,
            CheckLevel::Fail,
            format!(
                "{} was not checked: probe failed or deadline reached",
                spec.name
            ),
        ),
    };
    Diagnostic {
        name: spec.name.into(),
        level,
        message,
        remedy: (level != CheckLevel::Pass).then(|| {
            format!(
                "install {} {} and put it on PATH",
                spec.name,
                spec.supported_range()
            )
        }),
        tool: Some(ToolDiagnostic {
            installed,
            version,
            supported_versions: vec![spec.supported_range()],
            feature_available: level == CheckLevel::Pass,
            purpose: spec.purpose.into(),
            dependencies: vec!["pueue 4.0.4".into()],
        }),
    }
}
fn future_tool(name: &str, purpose: &str, required: bool, deadline: Instant) -> Diagnostic {
    let result =
        remaining(deadline).and_then(|timeout| probe(Command::new(name).arg("--version"), timeout));
    let (installed, version) = match &result {
        Ok((_, bytes)) => (Some(true), version_token(bytes)),
        Err(error) if error.code == ErrorCode::MissingTool => (Some(false), None),
        _ => (None, None),
    };
    let message = match result {
        Err(error) => error.to_string(),
        Ok((false, _)) => "unsupported tool capability: version probe failed".into(),
        Ok((true, _)) => "unsupported tool capability: integration is not provided; installed version is not certified".into(),
    };
    Diagnostic {
        name: name.into(), level: if required { CheckLevel::Fail } else { CheckLevel::Warning }, message,
        remedy: Some("use the supported core commands; enable this integration only after its adapter/version spike is implemented".into()),
        tool: Some(ToolDiagnostic { installed, version, supported_versions: vec![], feature_available: false, purpose: purpose.into(), dependencies: vec!["pueue 4.0.4".into()] }),
    }
}
pub fn diagnose(config: Result<Config, Error>, cwd: &Path) -> DoctorReport {
    diagnose_scope(config, cwd, "core")
}
pub fn diagnose_scope(config: Result<Config, Error>, cwd: &Path, scope: &str) -> DoctorReport {
    diagnose_scope_with_timeout(config, cwd, scope, Duration::from_secs(30))
}
/// Embedders may shorten the total budget; they cannot exceed the CLI's 30s cap.
pub fn diagnose_scope_with_timeout(
    config: Result<Config, Error>,
    cwd: &Path,
    scope: &str,
    timeout: Duration,
) -> DoctorReport {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(30));
    let mut checks = vec![check(
        "config_policy",
        config
            .as_ref()
            .map(|c| c.validate())
            .unwrap_or_else(|e| Err(e.clone())),
        "repair selected TOML configuration and policy; use schema_version=1 and private permissions",
    )];
    if let Ok(config) = config {
        let store = services::store_path(&config).and_then(|path| {
            services::prepare_parent(&path)?;
            ExecutionStore::diagnose_until(path, deadline)
        });
        checks.push(check(
            "execution_store",
            store,
            "use a writable private store (directory 0700, files 0600), without symlinks",
        ));
        match services::adapter(&config) {
            Err(error) => checks.push(check(
                "pueue_config",
                Err(error),
                "set execution.pueue_config to the absolute existing Pueue configuration path",
            )),
            Ok(mut adapter) => {
                let version = remaining(deadline).and_then(|timeout| {
                    adapter.timeout = timeout;
                    adapter.version()
                });
                let installed = match &version {
                    Ok(()) => Some(true),
                    Err(e) if e.code == ErrorCode::MissingTool => Some(false),
                    Err(e) if e.code == ErrorCode::Timeout => None,
                    Err(_) => Some(true),
                };
                let mut diagnostic = check(
                    "pueue_version",
                    version.clone(),
                    "install Pueue 4.0.4 and put pueue on PATH",
                );
                diagnostic.tool = Some(ToolDiagnostic {
                    installed,
                    version: version.ok().map(|_| "4.0.4".into()),
                    supported_versions: vec!["4.0.4".into()],
                    feature_available: diagnostic.level == CheckLevel::Pass,
                    purpose: "background execution and owned task/log inspection".into(),
                    dependencies: vec!["explicit dedicated Pueue configuration and daemon".into()],
                });
                checks.push(diagnostic);
                let tasks = remaining(deadline).and_then(|timeout| {
                    adapter.timeout = timeout;
                    adapter.list()
                });
                checks.push(check("pueue_daemon", tasks.as_ref().map(|_| ()).map_err(Clone::clone), "start pueued --config <the configured Pueue path>; check its socket permissions"));
                let owned = services::store_path(&config)
                    .and_then(|path| ExecutionStore::diagnostic_history(path, deadline));
                let candidate = owned.ok().and_then(|records| {
                    records.into_iter().find(|r| {
                        tasks
                            .as_ref()
                            .is_ok_and(|ts| ts.iter().any(|t| t.identity == r.backend))
                    })
                });
                let log = candidate.map(|r| {
                    remaining(deadline).and_then(|timeout| {
                        adapter.timeout = timeout;
                        adapter.output_owned(&r.backend, 65536)
                    })
                });
                let diagnostic = match log {
                    Some(Ok(_)) => check("pueue_logs", Ok(()), "inspect the owned Pueue log"),
                    Some(Err(e)) if e.code == ErrorCode::PartialResult => Diagnostic {
                        message: "owned task log is readable (bounded probe)".into(),
                        ..check("pueue_logs", Ok(()), "inspect the owned Pueue log")
                    },
                    Some(Err(e))
                        if scope == "logs"
                            && !matches!(e.code, ErrorCode::Expired | ErrorCode::OutputMissing) =>
                    {
                        check(
                            "pueue_logs",
                            Err(e),
                            "check permissions and accessibility of the dedicated Pueue log store",
                        )
                    }
                    _ => warning(
                        "pueue_logs",
                        "no readable owned task log could be verified",
                        "after an approved run, inspect its log with pueue --config <path> log <pueue-task-id>",
                    ),
                };
                checks.push(diagnostic);
            }
        }
    }
    if matches!(scope, "discovery" | "all") {
        for spec in [
            &tools::FD,
            &tools::RG,
            &tools::BAT,
            &tools::AST_GREP,
            &tools::TOKEI,
        ] {
            checks.push(adapter_tool(spec, deadline));
        }
        checks.push(future_tool(
            "eza",
            "directory display; fs list reads entries through fd",
            false,
            deadline,
        ));
    }
    if matches!(scope, "performance" | "all") {
        checks.push(future_tool(
            "hyperfine",
            "repeatable command benchmarks",
            true,
            deadline,
        ));
    }
    if scope == "all" {
        for (tool, purpose) in [
            ("delta", "diff presentation"),
            ("dust", "disk usage"),
            ("procs", "process inspection"),
            ("samply", "sampling profiles"),
        ] {
            checks.push(future_tool(tool, purpose, false, deadline));
        }
    }
    for (marker, tool) in [
        ("devbox.json", "devbox"),
        (".moon", "moon"),
        ("Cargo.toml", "cargo"),
        ("package.json", "pnpm"),
    ] {
        if !cwd.join(marker).exists() {
            continue;
        }
        let result = remaining(deadline)
            .and_then(|timeout| probe(Command::new(tool).arg("--version"), timeout));
        checks.push(match result {
            Ok((true, _)) => check(&format!("project_{tool}"), Ok(()), "enable the project tool"),
            Err(e) if e.code == ErrorCode::Timeout => warning(&format!("project_{tool}"), "project tool not checked: overall deadline exhausted", &format!("check {tool} separately or retry when earlier checks can finish")),
            _ => warning(&format!("project_{tool}"), "project tool unavailable", &format!("install/enable {tool} for this project; it is not required by the axvern runtime")),
        });
    }
    let level = if checks.iter().any(|c| c.level == CheckLevel::Fail) {
        CheckLevel::Fail
    } else if checks.iter().any(|c| c.level == CheckLevel::Warning) {
        CheckLevel::Warning
    } else {
        CheckLevel::Pass
    };
    DoctorReport { level, checks }
}

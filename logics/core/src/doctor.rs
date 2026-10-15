//! Read-only backend diagnosis. Scratch writes are confined to the private store.
use crate::{adapters::pueue::probe, config::Config, services, store::ExecutionStore};
use axvern_models::*;
use std::{path::Path, process::Command, time::Duration};

fn check(name: &str, result: Result<(), Error>, remedy: &str) -> Diagnostic {
    match result {
        Ok(()) => Diagnostic {
            name: name.into(),
            level: CheckLevel::Pass,
            message: "available".into(),
            remedy: None,
        },
        Err(error) => Diagnostic {
            name: name.into(),
            level: CheckLevel::Fail,
            message: error.to_string(),
            remedy: Some(remedy.into()),
        },
    }
}
pub fn diagnose(config: Result<Config, Error>, cwd: &Path) -> DoctorReport {
    let mut checks = vec![check(
        "config_policy",
        config.as_ref().map(|_| ()).map_err(Clone::clone),
        "repair selected TOML configuration and policy; use schema_version=1 and private permissions",
    )];
    if let Ok(config) = config {
        let store = services::store_path(&config).and_then(|path| {
            services::prepare_parent(&path)?;
            ExecutionStore::diagnose(path)
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
            Ok(adapter) => {
                checks.push(check(
                    "pueue_version",
                    adapter.version(),
                    "install Pueue 4.0.4 and put pueue on PATH",
                ));
                let tasks = adapter.list();
                checks.push(check("pueue_daemon", tasks.as_ref().map(|_| ()).map_err(Clone::clone), "start pueued --config <the configured Pueue path>; check its socket permissions"));
                // Only our own mapped task, never a registration or an unrelated user's log.
                let owned =
                    services::store_path(&config).and_then(ExecutionStore::diagnostic_history);
                let candidate = owned.ok().and_then(|records| {
                    records.into_iter().find(|r| {
                        tasks
                            .as_ref()
                            .is_ok_and(|ts| ts.iter().any(|t| t.identity == r.backend))
                    })
                });
                let log = candidate.map(|r| adapter.output_owned(&r.backend, 65536));
                checks.push(match log {
                    Some(Ok(_)) => Diagnostic { name: "pueue_logs".into(), level: CheckLevel::Pass, message: "owned task log is readable".into(), remedy: None },
                    Some(Err(e)) if e.code == ErrorCode::PartialResult => Diagnostic { name: "pueue_logs".into(), level: CheckLevel::Pass, message: "owned task log is readable (bounded probe)".into(), remedy: None },
                    _ => Diagnostic { name: "pueue_logs".into(), level: CheckLevel::Warning, message: "no readable owned task log could be verified".into(), remedy: Some("after an approved run, inspect its log with pueue --config <path> log <pueue-task-id>".into()) },
                });
            }
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
        let result = probe(Command::new(tool).arg("--version"), Duration::from_secs(5));
        checks.push(match result {
            Ok((true, bytes)) => Diagnostic { name: format!("project_{tool}"), level: CheckLevel::Pass, message: String::from_utf8_lossy(&bytes).trim().chars().take(128).collect(), remedy: None },
            _ => Diagnostic { name: format!("project_{tool}"), level: CheckLevel::Warning, message: "project tool unavailable".into(), remedy: Some(format!("install/enable {tool} for this project; it is not required by the axvern runtime")) },
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

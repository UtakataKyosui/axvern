//! Real isolated daemon contract test. Never attaches to the user's queue.
use axvern_core::store::{ExecutionStore, register_and_save};
use axvern_models::*;
mod common;
use common::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    thread,
    time::{Duration, Instant},
};
#[test]
fn isolated_registration_wait_cwd_argv_expiry_and_receipt_recovery() {
    let mut q = Queue::new();
    let store = ExecutionStore::open(q._temp.path().join("store")).unwrap();
    let args = [
        "",
        "a b",
        "single'quote",
        "double\"quote",
        "line\nbreak",
        "$(touch INJECTED)",
        "; touch INJECTED",
        "日本語",
        "--config",
    ];
    let plan = q.plan(
        "import sys,json,os; print(json.dumps([os.getcwd(),sys.argv[1:]]))",
        &args,
    );
    let record = register_and_save(&mut q.adapter, &store, &plan).unwrap();
    let task = q
        .adapter
        .wait(&record.backend, Duration::from_secs(10))
        .unwrap();
    assert_eq!(task.state, ChildState::Done);
    let output = q.adapter.output(record.backend.task_id, 10000).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value[0], q._temp.path().to_str().unwrap());
    assert_eq!(value[1], serde_json::json!(args));
    assert!(!q._temp.path().join("INJECTED").exists());
    for lines in [100, 101, 150] {
        let plan = q.plan(&format!("for i in range({lines}): print(i)"), &[]);
        let identity = q.adapter.register_identified(&plan).unwrap();
        q.adapter.wait(&identity, Duration::from_secs(10)).unwrap();
        let output = q.adapter.output(identity.task_id, 10000);
        if lines == 100 {
            assert_eq!(
                String::from_utf8(output.unwrap()).unwrap().lines().count(),
                100
            );
            assert_eq!(
                q.adapter.output(identity.task_id, 1).unwrap_err().code,
                ErrorCode::PartialResult
            );
        } else {
            assert_eq!(output.unwrap_err().code, ErrorCode::PartialResult);
        }
    }
    assert_eq!(
        q.adapter.inspect(999999).unwrap_err().code,
        ErrorCode::TaskNotFound
    );
    let plan = q.plan("import sys; sys.exit(23)", &[]);
    let failed = q.adapter.register_identified(&plan).unwrap();
    assert_eq!(
        q.adapter
            .wait(&failed, Duration::from_secs(10))
            .unwrap()
            .exit_code,
        Some(23)
    );
    let plan = q.plan(
        "import time; print('start',flush=True); time.sleep(0.8)",
        &[],
    );
    let running = q.adapter.register_identified(&plan).unwrap();
    assert_eq!(
        q.adapter
            .wait(&running, Duration::from_millis(10))
            .unwrap_err()
            .code,
        ErrorCode::Timeout
    );
    q.adapter.wait(&running, Duration::from_secs(10)).unwrap();
    assert_eq!(store.history().unwrap().len(), 1); // external adapter registrations excluded
    q.client(&["clean"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !q.adapter.list().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        store
            .resolve(&record.execution_id, &q.adapter)
            .unwrap_err()
            .code,
        ErrorCode::Expired
    );
    let plan = q.plan("print('reused')", &[]);
    let reused = q.adapter.register_identified(&plan).unwrap();
    assert_eq!(reused.task_id, record.backend.task_id);
    assert_eq!(
        store
            .resolve(&record.execution_id, &q.adapter)
            .unwrap_err()
            .code,
        ErrorCode::Expired
    );
    // Known successful registration + failed local persistence: retain a receipt, never add again.
    fs::set_permissions(
        q._temp.path().join("store"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let failure = register_and_save(&mut q.adapter, &store, &plan).unwrap_err();
    let receipt = failure.receipt.unwrap();
    assert_eq!(failure.error.code, ErrorCode::PermissionDenied);
    let count = q.adapter.list().unwrap().len();
    fs::set_permissions(
        q._temp.path().join("store"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    store.save_record(&receipt).unwrap();
    store.save_record(&receipt).unwrap();
    assert_eq!(q.adapter.list().unwrap().len(), count);
    let mut missing = q.adapter.clone();
    missing.executable = q._temp.path().join("missing");
    assert_eq!(missing.check().unwrap_err().code, ErrorCode::MissingTool);
    let mut offline = q.adapter.clone();
    offline.config = q._temp.path().join("offline.yml");
    fs::write(
        &offline.config,
        "shared:\n  unix_socket_path: /nonexistent-axvern/socket\n",
    )
    .unwrap();
    assert_eq!(
        offline.list().unwrap_err().code,
        ErrorCode::DaemonUnavailable
    );
}

#[test]
fn public_cli_execute_doctor_and_history_contracts() {
    let mut q = Queue::new();
    let config = q._temp.path().join("axvern.toml");
    let store_path = q._temp.path().join("cli-store");
    let text = format!(
        "schema_version=1\n[execution]\npueue_config={}\nstore={}\n[[policy.rules]]\nid='python'\neffect='allow'\ncommand='python3'\n[[policy.rules]]\nid='deny'\neffect='deny'\ncommand='false'\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&store_path).unwrap()
    );
    fs::write(&config, &text).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let before = q.adapter.list().unwrap();
    let report = cli_json(&q, &config, &["doctor", "--format", "json"], 0);
    assert_eq!(report["output"]["level"], "warning"); // no owned logs yet
    assert_eq!(q.adapter.list().unwrap(), before);
    assert!(
        !fs::read_dir(&store_path).unwrap().any(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "probe"))
    );
    // No registration on Deny or noninteractive Ask, including the default shell denial.
    for program in ["false", "true", "sh"] {
        let denied = cli_json(&q, &config, &["run", "--format", "json", "--", program], 9);
        assert_eq!(denied["error"]["code"], "policy_denied");
    }
    assert_eq!(q.adapter.list().unwrap(), before);
    let human = public_cli(&q, &config, &["run", "--format", "human", "--", "false"]);
    assert_eq!(human.stdout, b"");
    assert_eq!(
        human.stderr,
        b"PolicyDenied: policy or approval refused registration\n"
    );
    let unknown = "ex_550e8400-e29b-41d4-a716-446655440000";
    let unknown_json = cli_json(&q, &config, &["status", unknown, "--format", "json"], 7);
    assert_eq!(
        unknown_json,
        serde_json::json!({"schema_version":2,"command":"status","execution_id":unknown,"state":null,"exit_code":null,"output":null,"error":{"code":"task_not_found","message":"unknown axvern execution"}})
    );
    for args in [
        vec!["history", "--limit", "0", "--format", "json"],
        vec!["history", "--limit", "1001", "--format", "json"],
        vec!["wait", unknown, "--timeout", "0s", "--format", "json"],
        vec!["wait", unknown, "--timeout", "-1s", "--format", "json"],
        vec!["run", "--format", "json"],
        vec!["history", "--cwd", "/", "--format", "json"],
    ] {
        assert_eq!(
            cli_json(&q, &config, &args, 2)["error"]["code"],
            "invalid_input"
        );
    }
    // argv region includes strings that look like axvern flags; no re-parsing.
    let args = [
        "",
        "a b",
        "'\"",
        "line\nbreak",
        "$(touch CLI_INJECTED)",
        ";touch CLI_INJECTED",
        "日本語",
        "--format",
        "json",
    ];
    let code = "import sys,json,os,time\ndeadline=time.monotonic()+15\nwhile not os.path.exists('CLI_RELEASE') and time.monotonic()<deadline: time.sleep(0.02)\nprint(json.dumps([os.getcwd(),sys.argv[1:]]))";
    let mut argv = vec![
        "run",
        "--format",
        "json",
        "--cwd",
        q._temp.path().to_str().unwrap(),
        "--",
        "python3",
        "-c",
        code,
    ];
    argv.extend(args);
    let registered = cli_json(&q, &config, &argv, 0);
    let id = registered["execution_id"].as_str().unwrap();
    let record: ExecutionRecord =
        serde_json::from_value(registered["output"]["record"].clone()).unwrap();
    assert!(
        matches!(
            q.adapter.inspect(record.backend.task_id).unwrap().state,
            ChildState::Queued | ChildState::Running
        ),
        "run waited for completion"
    );
    let status = cli_json(&q, &config, &["status", id, "--format", "json"], 0);
    assert!(status["state"] == "running" || status["state"] == "queued");
    let timeout = cli_json(
        &q,
        &config,
        &["wait", id, "--timeout", "1ms", "--format", "json"],
        124,
    );
    assert_eq!(timeout["error"]["code"], "timeout");
    fs::write(q._temp.path().join("CLI_RELEASE"), b"finish").unwrap();
    let done = cli_json(
        &q,
        &config,
        &["wait", id, "--timeout", "10s", "--format", "json"],
        0,
    );
    assert_eq!(done["state"], "done");
    assert_eq!(done["exit_code"], 0);
    assert!(done["output"]["task"]["started_at"].is_string());
    assert!(done["output"]["task"]["finished_at"].is_string());
    let raw = q.adapter.output(record.backend.task_id, 10000).unwrap();
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        result,
        serde_json::json!([q._temp.path().to_str().unwrap(), args])
    );
    assert!(!q._temp.path().join("CLI_INJECTED").exists());
    // External tasks do not appear in history. Child failure is data in status,
    // but wait has a stable wrapper exit=10 and retains the child's exit=23.
    let external = q.plan("print('external')", &[]);
    q.adapter.register_identified(&external).unwrap();
    let failed = cli_json(
        &q,
        &config,
        &[
            "run",
            "--format",
            "json",
            "--",
            "python3",
            "-c",
            "raise SystemExit(23)",
        ],
        0,
    );
    let failed_id = failed["execution_id"].as_str().unwrap();
    let waited = cli_json(&q, &config, &["wait", failed_id, "--format", "json"], 10);
    assert_eq!(waited["exit_code"], 23);
    assert_eq!(
        cli_json(&q, &config, &["status", failed_id, "--format", "json"], 0)["state"],
        "failed"
    );
    let history = cli_json(&q, &config, &["history", "--format", "json"], 0);
    assert_eq!(history["output"]["executions"].as_array().unwrap().len(), 2);
    let limited = cli_json(
        &q,
        &config,
        &["history", "--limit", "1", "--format", "json"],
        0,
    );
    assert_eq!(
        limited["output"]["executions"][0],
        history["output"]["executions"][0]
    );
    let parent = ParentRecord {
        execution_id: ExecutionStore::allocate_id().unwrap(),
        registered_at: u64::MAX - 1,
        cwd: q._temp.path().to_str().unwrap().into(),
        children: vec![id.parse().unwrap(), failed_id.parse().unwrap()],
        analysis: None,
    };
    let local_store = ExecutionStore::open(&store_path).unwrap();
    axvern_core::parents::save(&local_store, &parent).unwrap();
    let parent_id = parent.execution_id.to_string();
    let aggregate = cli_json(&q, &config, &["status", &parent_id], 0);
    assert_eq!(aggregate["state"], "partial");
    assert_eq!(aggregate["output"]["children"].as_array().unwrap().len(), 2);
    assert_eq!(
        cli_json(&q, &config, &["wait", &parent_id], 11)["state"],
        "partial"
    );
    assert_eq!(
        cli_json(&q, &config, &["history", "--limit", "1"], 0)["output"]["executions"][0]["record"]
            ["execution_id"],
        parent_id
    );
    // Doctor leaves even abandoned writer scratch untouched.
    let pending = store_path.join("untouched.pending");
    fs::write(&pending, b"keep").unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).unwrap();
    let before = q.adapter.list().unwrap();
    let report = cli_json(&q, &config, &["doctor", "--format", "json"], 0);
    assert_eq!(report["output"]["level"], "pass");
    assert_eq!(fs::read(&pending).unwrap(), b"keep");
    assert_eq!(q.adapter.list().unwrap(), before);
    fs::remove_file(pending).unwrap();
    q.client(&["clean"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !q.adapter.list().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        cli_json(&q, &config, &["status", id, "--format", "json"], 8)["error"]["code"],
        "expired"
    );
    assert_eq!(
        cli_json(&q, &config, &["history", "--format", "json"], 0)["output"]["executions"][0]["error"]
            ["code"],
        "expired"
    );
    q.client(&["shutdown"]);
    q.daemon.wait().unwrap();
    let offline = cli_json(&q, &config, &["doctor", "--format", "json"], 12);
    let checks = offline["output"]["checks"].as_array().unwrap();
    let daemon = checks.iter().find(|c| c["name"] == "pueue_daemon").unwrap();
    assert_eq!(daemon["level"], "fail");
    assert!(
        daemon["remedy"]
            .as_str()
            .unwrap()
            .contains("start pueued --config")
    );
    let human = public_cli(&q, &config, &["doctor", "--format", "human"]);
    assert_eq!(human.status.code(), Some(12));
    assert!(String::from_utf8_lossy(&human.stdout).contains("pueue_daemon: fail"));
    assert!(String::from_utf8_lossy(&human.stdout).contains("start pueued --config"));
    assert!(human.stderr.is_empty());
    let offline_history = cli_json(&q, &config, &["history", "--format", "json"], 11);
    assert_eq!(offline_history["error"]["code"], "partial_result");
    assert_eq!(
        offline_history["output"]["executions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn terminal_ask_requires_an_explicit_yes() {
    let q = Queue::new();
    let config = q._temp.path().join("ask.toml");
    fs::write(
        &config,
        format!(
            "schema_version=1\n[execution]\npueue_config={}\nstore={}\n",
            serde_json::to_string(&q.adapter.config).unwrap(),
            serde_json::to_string(&q._temp.path().join("store")).unwrap()
        ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    for (answer, code, count) in [("no", 9, 0), ("yes", 0, 1)] {
        let out = Command::new("python3")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cli/approve.py"))
            .arg(answer)
            .arg(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
            .args([
                "--config",
                config.to_str().unwrap(),
                "run",
                "--format",
                "json",
                "--",
                "true",
            ])
            .current_dir(q._temp.path())
            .env_remove("AXVERN_CONFIG")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["exit"], code);
        assert!(
            value["prompt"]
                .as_str()
                .unwrap()
                .starts_with("Policy asks for approval.")
        );
        let response: serde_json::Value =
            serde_json::from_str(value["stdout"].as_str().unwrap()).unwrap();
        if code == 0 {
            assert!(response["execution_id"].is_string());
        } else {
            assert_eq!(response["error"]["code"], "policy_denied");
        }
        assert_eq!(q.adapter.list().unwrap().len(), count);
    }
}

#[test]
fn pending_approval_for_a_keyed_run_does_not_hold_the_store_lock() {
    let q = Queue::new();
    let config = q._temp.path().join("ask.toml");
    fs::write(
        &config,
        format!(
            "schema_version=1\n[execution]\npueue_config={}\nstore={}\n",
            serde_json::to_string(&q.adapter.config).unwrap(),
            serde_json::to_string(&q._temp.path().join("store")).unwrap()
        ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let axvern = std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test");
    // While the prompt waits for an answer, another client must still reach the store.
    let probe = serde_json::json!([
        axvern.to_str().unwrap(),
        "--config",
        config.to_str().unwrap(),
        "history",
        "--format",
        "json"
    ]);
    let out = Command::new("python3")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cli/approve.py"))
        .arg("yes")
        .arg(&axvern)
        .args([
            "--config",
            config.to_str().unwrap(),
            "run",
            "--request-id",
            "approval-key",
            "--format",
            "json",
            "--",
            "true",
        ])
        .current_dir(q._temp.path())
        .env_remove("AXVERN_CONFIG")
        .env("AXVERN_APPROVE_PROBE", probe.to_string())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        value["prompt"]
            .as_str()
            .unwrap()
            .starts_with("Policy asks for approval.")
    );
    assert_eq!(
        value["probe"],
        serde_json::json!({"exit": 0, "timed_out": false}),
        "a pending approval prompt blocked another client"
    );
    assert_eq!(value["exit"], 0);
    assert_eq!(q.adapter.list().unwrap().len(), 1);
}

#[test]
fn schema_two_keyed_run_replays_after_policy_change_and_backend_log_deletion() {
    let q = Queue::new();
    let config = q._temp.path().join("v2.toml");
    let base = format!(
        "schema_version=1\n[execution]\npueue_config={}\nstore={}\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&q._temp.path().join("store")).unwrap()
    );
    fs::write(
        &config,
        format!("{base}[[policy.rules]]\nid='python'\ncommand='python3'\neffect='allow'\n"),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let args = [
        "run",
        "--request-id",
        "one",
        "--wait",
        "0s",
        "--",
        "python3",
        "-c",
        "import time; time.sleep(0.6); print('保存済み'); raise SystemExit(23)",
    ];
    let first = cli_json(&q, &config, &args, 0);
    assert_eq!(first["schema_version"], 2);
    assert_eq!(first["command"], "run");
    assert_eq!(first["state"], serde_json::Value::Null);
    let id = first["execution_id"].as_str().unwrap();
    let observed = cli_json(&q, &config, &["wait", id, "--timeout", "100ms"], 124);
    assert!(observed["state"] == "running" || observed["state"] == "queued");
    let completed = cli_json(&q, &config, &["wait", id, "--timeout", "10s"], 10);
    assert_eq!(completed["exit_code"], 23);
    assert_eq!(completed["output"]["result_availability"], "available");
    let saved = cli_json(&q, &config, &["result", id], 0);
    assert_eq!(saved["output"]["result"]["items"][0]["text"], "保存済み\n");
    q.client(&["clean"]);
    assert_eq!(
        cli_json(&q, &config, &["result", id], 0)["output"],
        saved["output"]
    );
    // A replay is a reference to the original execution, even under a new Deny rule.
    fs::write(
        &config,
        format!("{base}[[policy.rules]]\nid='python'\ncommand='python3'\neffect='deny'\n"),
    )
    .unwrap();
    let replay = cli_json(&q, &config, &args, 0);
    assert_eq!(replay["execution_id"], id);
    // With --wait 0s a completed execution reports its saved result, never "pending".
    assert_eq!(replay["state"], "failed");
    assert_eq!(replay["exit_code"], 23);
    assert_eq!(replay["output"]["result_availability"], "available");
    assert_eq!(replay["output"]["record"]["execution_id"], id);
    assert_eq!(replay["output"]["result"]["items"][0]["text"], "保存済み\n");
    assert!(q.adapter.list().unwrap().is_empty());
    let conflict = cli_json(
        &q,
        &config,
        &[
            "run",
            "--request-id",
            "one",
            "--",
            "python3",
            "-c",
            "print('different')",
        ],
        15,
    );
    assert_eq!(conflict["error"]["code"], "request_conflict");
}

#[test]
fn concurrent_service_runs_without_request_id_register_one_task_within_capacity() {
    use axvern_core::{config::Config, results, services::ExecutionService};
    let q = Queue::new();
    let store = q._temp.path().join("store");
    let config = Config::parse(&format!(
        "schema_version=1\n[execution]\npueue_config={}\nstore={}\n[[policy.rules]]\nid='python'\ncommand='python3'\neffect='allow'\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&store).unwrap()
    ))
    .unwrap();
    let plan = q.plan("print('once')", &[]);
    // Smallest capacity at which exactly one registration's metadata still fits.
    let probe = ExecutionService::new(config.clone()).unwrap();
    let (mut low, mut high) = (0, 1u64 << 24);
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        let mut settings = config.results.clone();
        settings.capacity_bytes = mid;
        if results::preflight_registration_locked(&probe.store, &settings, &plan).is_ok() {
            high = mid;
        } else {
            low = mid;
        }
    }
    // The valid minimum capacity is fixed; existing stored data leaves exactly one registration of room.
    let filler = store.join("filler.pad");
    fs::write(
        &filler,
        vec![0; (config.results.capacity_bytes - high) as usize],
    )
    .unwrap();
    fs::set_permissions(&filler, fs::Permissions::from_mode(0o600)).unwrap();
    let runner = std::path::PathBuf::from(std::env::var_os("AXVERN_TEST_BIN").unwrap());
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let (config, runner, plan) = (config.clone(), runner.clone(), plan.clone());
            thread::spawn(move || {
                ExecutionService::new(config).unwrap().run_captured(
                    &runner,
                    &plan,
                    false,
                    Approval::NotProvided,
                )
            })
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    for refused in outcomes.iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(refused.error.code, ErrorCode::StorageFull);
    }
    assert_eq!(q.adapter.list().unwrap().len(), 1);
}

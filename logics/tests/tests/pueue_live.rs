//! Real isolated daemon contract test. Never attaches to the user's queue.
use axvern_core::{
    adapters::pueue::PueueAdapter,
    store::{ExecutionStore, register_and_save},
};
use axvern_models::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Queue {
    daemon: Child,
    adapter: PueueAdapter,
    _temp: tempfile::TempDir,
}
impl Queue {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("av-")
            .tempdir_in(fs::canonicalize("/tmp").unwrap())
            .unwrap();
        let root = temp.path();
        let config = root.join("config.yml");
        let mut yaml = "shared:\n".to_owned();
        for (key, file) in [
            ("pueue_directory", "data"),
            ("runtime_directory", "runtime"),
            ("unix_socket_path", "socket"),
            ("pid_path", "pid"),
            ("alias_file", "aliases.yml"),
        ] {
            yaml += &format!(
                "  {key}: {}\n",
                serde_json::to_string(&root.join(file)).unwrap()
            );
        }
        yaml += "  use_unix_socket: true\ndaemon:\n  shell_command: [sh, '-c', '{{ pueue_command_string }}']\n";
        fs::write(&config, yaml).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let daemon = Command::new("pueued")
            .arg("--config")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pueued 4.0.4 required");
        let adapter = PueueAdapter {
            executable: "pueue".into(),
            config,
            queue: "test-private".into(),
            timeout: Duration::from_secs(5),
        };
        let mut queue = Self {
            daemon,
            adapter,
            _temp: temp,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while queue.adapter.check().is_err() {
            assert!(
                queue.daemon.try_wait().unwrap().is_none(),
                "daemon start failed"
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(25));
        }
        queue
    }
    fn client(&self, args: &[&str]) {
        assert!(
            Command::new("pueue")
                .arg("--config")
                .arg(&self.adapter.config)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
    fn plan(&self, code: &str, args: &[&str]) -> ExecutionPlan {
        let mut argv = vec!["-c".into(), code.into()];
        argv.extend(args.iter().map(|v| (*v).into()));
        ExecutionPlan {
            program: "python3".into(),
            args: argv,
            cwd: self._temp.path().into(),
        }
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        let _ = Command::new("pueue")
            .arg("--config")
            .arg(&self.adapter.config)
            .arg("shutdown")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}
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

fn public_cli(q: &Queue, config: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
        .current_dir(q._temp.path())
        .env_remove("AXVERN_CONFIG")
        .arg("--config")
        .arg(config)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn cli_json(q: &Queue, config: &std::path::Path, args: &[&str], code: i32) -> serde_json::Value {
    let out = public_cli(q, config, args);
    assert_eq!(
        out.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty(), "JSON diagnostics leaked to stderr");
    serde_json::from_slice(&out.stdout).expect("one JSON response, no diagnostics")
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
    let human = public_cli(&q, &config, &["run", "--", "false"]);
    assert_eq!(human.stdout, b"");
    assert_eq!(
        human.stderr,
        b"PolicyDenied: policy or approval refused registration\n"
    );
    let unknown = "ex_550e8400-e29b-41d4-a716-446655440000";
    let unknown_json = cli_json(&q, &config, &["status", unknown, "--format", "json"], 7);
    assert_eq!(
        unknown_json,
        serde_json::json!({"schema_version":1,"execution_id":unknown,"state":null,"exit_code":null,"output":null,"error":{"code":"task_not_found","message":"unknown axvern execution"}})
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
    let human = public_cli(&q, &config, &["doctor"]);
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
        2
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

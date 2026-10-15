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

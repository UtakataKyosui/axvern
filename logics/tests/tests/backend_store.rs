use axvern_core::{adapters::pueue::PueueAdapter, store::ExecutionStore};
use axvern_models::*;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, sync::Arc, thread};
fn record() -> ExecutionRecord {
    ExecutionRecord {
        execution_id: ExecutionStore::allocate_id().unwrap(),
        backend: BackendIdentity {
            queue: "private".into(),
            task_id: 0,
            created_at: "2026-10-15T10:00:00+09:00".into(),
        },
        registered_at: 100,
        cwd: "/tmp".into(),
    }
}
fn temp() -> tempfile::TempDir {
    tempfile::tempdir_in(fs::canonicalize("/tmp").unwrap()).unwrap()
}
#[test]
fn concurrent_records_idempotency_and_cleanup() {
    let temp = temp();
    let store = Arc::new(ExecutionStore::open(temp.path().join("store")).unwrap());
    let workers: Vec<_> = (0..24)
        .map(|_| {
            let store = store.clone();
            thread::spawn(move || {
                let record = record();
                store.save_record(&record).unwrap();
                store.save_record(&record).unwrap();
                record
            })
        })
        .collect();
    let records: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(store.history().unwrap().len(), 24);
    for record in &records {
        assert_eq!(store.find_record(&record.execution_id).unwrap(), *record);
    }
    let mut conflict = records[0].clone();
    conflict.backend.created_at = "another".into();
    assert_eq!(
        store.save_record(&conflict).unwrap_err().code,
        ErrorCode::StoreFailure
    );
    assert_eq!(store.prune_before(100).unwrap(), 0);
    assert_eq!(store.prune_before(101).unwrap(), 24);
    assert!(store.history().unwrap().is_empty());
}
#[test]
fn recovery_permissions_corruption_and_write_failure() {
    let temp = temp();
    let root = temp.path().join("store");
    let store = ExecutionStore::open(&root).unwrap();
    let record = record();
    store.save_record(&record).unwrap();
    let pending = root.join("crashed.pending");
    fs::write(&pending, "partial").unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).unwrap();
    let store = ExecutionStore::open(&root).unwrap();
    assert!(!pending.exists());
    assert_eq!(store.history().unwrap(), vec![record.clone()]);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        store.save_record(&record).unwrap_err().code,
        ErrorCode::PermissionDenied
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join(format!("{}.json", record.execution_id));
    fs::write(&path, "broken").unwrap();
    assert_eq!(store.history().unwrap_err().code, ErrorCode::StoreFailure);
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(temp.path(), &path).unwrap();
    assert_eq!(
        store.find_record(&record.execution_id).unwrap_err().code,
        ErrorCode::PermissionDenied
    );
}
#[test]
fn fixtures_normalize_all_results_and_reject_unknown_schema() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/pueue/macos-4.0.4.json")).unwrap();
    for (status, state, code) in [
        (&fixture["success_status"], ChildState::Done, Some(0)),
        (
            &fixture["checks"]["child_failure"]["status"],
            ChildState::Failed,
            Some(23),
        ),
        (&fixture["checks"]["killed"], ChildState::Failed, None),
    ] {
        let data = json!({"tasks":{"0":{"id":0,"created_at":"time","status":status}}});
        let tasks =
            PueueAdapter::parse_status("private", &serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!((tasks[0].state, tasks[0].exit_code), (state, code));
    }
    for status in [
        json!({"Queued":{}}),
        json!({"Stashed":{"enqueue_at":null}}),
        json!({"Locked":{"previous_status":{"Queued":{}}}}),
        json!({"Running":{"start":"now"}}),
        json!({"Paused":{"start":"now"}}),
    ] {
        assert!(
            PueueAdapter::parse_status(
                "q",
                &serde_json::to_vec(
                    &json!({"tasks":{"0":{"id":0,"created_at":"time","status":status}}})
                )
                .unwrap()
            )
            .is_ok()
        );
    }
    for bytes in [b"invalid".as_slice(), b"{}", br#"{"tasks":{"0":{"id":1}}}"#] {
        assert_eq!(
            PueueAdapter::parse_status("q", bytes).unwrap_err().code,
            ErrorCode::BackendFailure
        );
    }
}

#[test]
fn store_process_worker() {
    let Some(root) = std::env::var_os("AXVERN_STORE_TEST_ROOT") else {
        return;
    };
    let store = ExecutionStore::open(std::path::PathBuf::from(&root)).unwrap();
    if std::env::var_os("AXVERN_STORE_TEST_CRASH").is_some() {
        use std::io::Write;
        let root = std::path::PathBuf::from(root);
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("store.lock"))
            .unwrap();
        lock.lock().unwrap();
        let path = root.join("interrupted.pending");
        let mut file = fs::File::create(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        file.write_all(b"incomplete").unwrap();
        file.sync_all().unwrap();
        std::process::exit(17); // OS releases lock without Rust destructors.
    }
    for _ in 0..8 {
        store.save_record(&record()).unwrap();
    }
}
#[test]
fn process_concurrency_and_interrupted_writer_recovery() {
    use std::process::{Command, Stdio};
    let temp = temp();
    let root = temp.path().join("store");
    let store = ExecutionStore::open(&root).unwrap();
    let spawn = |crash| {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "store_process_worker"])
            .env("AXVERN_STORE_TEST_ROOT", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if crash {
            cmd.env("AXVERN_STORE_TEST_CRASH", "1");
        }
        cmd.spawn().unwrap()
    };
    let mut crash = spawn(true);
    assert_eq!(crash.wait().unwrap().code(), Some(17));
    let workers: Vec<_> = (0..8).map(|_| spawn(false)).collect();
    for mut child in workers {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(store.history().unwrap().len(), 64);
    assert!(!root.join("interrupted.pending").exists());
}
#[test]
fn registration_unknown_never_retries_and_bounds_client_wait() {
    use std::time::Duration;
    let temp = temp();
    let script = temp.path().join("pueue");
    fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys,json,time,pathlib
root=pathlib.Path(__file__).parent
args=sys.argv
if '--version' in args: print('pueue 4.0.4')
elif 'status' in args: print(json.dumps({'tasks':{}}))
elif 'add' in args:
    with (root/'adds').open('a') as f: f.write('add\n')
    if (root/'timeout').exists(): time.sleep(0.3)
    print('not an id')
"#,
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let mut adapter = PueueAdapter {
        executable: script,
        config: temp.path().join("config"),
        queue: "q".into(),
        timeout: Duration::from_secs(2),
    };
    let plan = ExecutionPlan {
        program: "true".into(),
        args: vec![],
        cwd: temp.path().into(),
    };
    let err = adapter.register_identified(&plan).unwrap_err();
    assert_eq!(err.code, ErrorCode::RegistrationUnknown);
    assert!(err.message.contains("label ex_"));
    assert_eq!(
        fs::read_to_string(temp.path().join("adds"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    fs::write(temp.path().join("timeout"), "").unwrap();
    adapter.timeout = Duration::from_millis(100);
    assert_eq!(
        adapter.register_identified(&plan).unwrap_err().code,
        ErrorCode::RegistrationUnknown
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("adds"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    fs::write(
        &adapter.executable,
        "#!/usr/bin/env python3\nprint('bad json')\n",
    )
    .unwrap();
    assert_eq!(adapter.list().unwrap_err().code, ErrorCode::BackendFailure);
}
#[test]
fn unsupported_store_schema_is_preserved() {
    let temp = temp();
    let root = temp.path().join("store");
    let store = ExecutionStore::open(&root).unwrap();
    let record = record();
    store.save_record(&record).unwrap();
    let path = root.join(format!("{}.json", record.execution_id));
    let bytes = serde_json::to_vec(&json!({"schema_version":999,"record":record})).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert_eq!(store.history().unwrap_err().code, ErrorCode::StoreFailure);
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn status_preserves_start_and_response_failures() {
    use std::time::Duration;
    let temp = temp();
    let script = temp.path().join("pueue");
    fs::write(&script, "#!/usr/bin/env python3\nprint('{}')\n").unwrap();
    let adapter = PueueAdapter {
        executable: script.clone(),
        config: temp.path().join("config"),
        queue: "q".into(),
        timeout: Duration::from_secs(5),
    };
    fs::set_permissions(&script, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(adapter.list().unwrap_err().code, ErrorCode::BackendFailure);
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        &script,
        "#!/usr/bin/env python3\nimport sys\nsys.stdout.write('x' * (8*1024*1024+1))\n",
    )
    .unwrap();
    assert_eq!(adapter.list().unwrap_err().code, ErrorCode::BackendFailure);
    fs::write(&script, "#!/usr/bin/env python3\nimport sys\nsys.exit(1)\n").unwrap();
    assert_eq!(
        adapter.list().unwrap_err().code,
        ErrorCode::DaemonUnavailable
    );
}

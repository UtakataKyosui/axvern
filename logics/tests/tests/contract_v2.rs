use axvern_core::{
    config::{Config, Results},
    results,
    store::ExecutionStore,
};
use axvern_models::*;
use std::{
    fs,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
fn store() -> (tempfile::TempDir, ExecutionStore) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("executions");
    let store = ExecutionStore::open(path).unwrap();
    (temp, store)
}
fn plan() -> ExecutionPlan {
    ExecutionPlan {
        program: "printf".into(),
        args: vec!["hello".into()],
        cwd: std::env::current_dir().unwrap(),
    }
}
fn identity() -> BackendIdentity {
    BackendIdentity {
        queue: "q".into(),
        task_id: 1,
        created_at: "created".into(),
    }
}
#[test]
fn simultaneous_request_replay_conflict_and_unknown_never_register_twice() {
    let (_temp, store) = store();
    let calls = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(store);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let s = store.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                s.register_request(
                    &"a".repeat(128),
                    &plan(),
                    || Ok(()),
                    |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(identity())
                    },
                )
                .unwrap()
                .execution_id
            })
        })
        .collect();
    let ids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut other = plan();
    other.args.push("different".into());
    assert_eq!(
        store
            .register_request(
                &"a".repeat(128),
                &other,
                || panic!("must not authorize replay"),
                |_| panic!("must not register replay")
            )
            .unwrap_err()
            .code,
        ErrorCode::RequestConflict
    );
    assert_eq!(
        store
            .register_request(
                "uncertain",
                &plan(),
                || Ok(()),
                |_| Err(Error::new(ErrorCode::RegistrationUnknown, "lost response"))
            )
            .unwrap_err()
            .code,
        ErrorCode::RegistrationUnknown
    );
    let pending = store
        .register_request(
            "uncertain",
            &plan(),
            || panic!("no new approval"),
            |_| panic!("no new add"),
        )
        .unwrap();
    assert!(pending.record.is_none());
    assert_eq!(
        store.prune_before(u64::MAX).unwrap_err().code,
        ErrorCode::StoreFailure
    );
}
#[test]
fn settings_validate_limits_without_changing_config_schema() {
    for body in [
        "[results]\nretention_seconds=3599",
        "[results]\ncapacity_bytes=1048576",
        "[discovery]\nmax_items=0",
        "[discovery]\nmax_entries=1",
        "[discovery]\ntimeout_ms=999",
    ] {
        assert_eq!(
            Config::parse(&format!("schema_version=1\n{body}"))
                .unwrap_err()
                .code,
            ErrorCode::InvalidConfig
        );
    }
    assert!(Config::parse("schema_version=1\n[results]\ncapacity_bytes=1048576\nmax_artifact_bytes=65536\n[discovery]\nmax_items=1\nmax_entries=1\ntimeout_ms=1000").is_ok());
}
fn worker(
    store_path: &std::path::Path,
    id: &ExecutionId,
    script: &str,
    maximum: u64,
) -> std::process::ExitStatus {
    Command::new(std::env::var_os("AXVERN_TEST_BIN").unwrap())
        .args(["__axvern_worker"])
        .arg(store_path)
        .arg(id.to_string())
        .args([
            "3600",
            "1048576",
            &maximum.to_string(),
            "--",
            "python3",
            "-c",
            script,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
}
#[test]
fn worker_captures_before_completion_and_pages_unicode_with_exact_wire_budget() {
    let (temp, store) = store();
    let root = temp.path().canonicalize().unwrap().join("executions");
    let id = ExecutionStore::allocate_id().unwrap();
    assert!(
        worker(
            &root,
            &id,
            "import sys; sys.stdout.write('あ\\x1b\\n'*10000); sys.exit(23)",
            250000
        )
        .code()
            == Some(23)
    );
    let settings = Results {
        retention_seconds: 3600,
        capacity_bytes: 1048576,
        max_artifact_bytes: 250000,
    };
    let mut token = None;
    let mut combined = String::new();
    let mut expected_index = 0;
    loop {
        let (manifest, output) =
            results::page(&store, &settings, &id, token.as_deref(), 2, 1024).unwrap();
        let manifest = manifest.unwrap();
        assert_eq!(manifest.exit_code, Some(23));
        let mut response = Response::success(&output);
        response.command = Some("result".into());
        response.execution_id = Some(id.clone());
        response.state = manifest.state.map(Into::into);
        response.exit_code = manifest.exit_code;
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        assert_eq!(
            results::page(&store, &settings, &id, token.as_deref(), 2, 1024)
                .unwrap()
                .1,
            output
        );
        for item in output["result"]["items"].as_array().unwrap() {
            combined.push_str(item["text"].as_str().unwrap());
            assert_eq!(item["fragment_index"], expected_index);
            expected_index += 1;
        }
        token = output["result"]["page"]["next_cursor"]
            .as_str()
            .map(str::to_owned);
        if token.is_none() {
            break;
        }
        let other = ExecutionStore::allocate_id().unwrap();
        assert_eq!(
            results::page(&store, &settings, &other, token.as_deref(), 2, 1024)
                .unwrap_err()
                .code,
            ErrorCode::InvalidCursor
        );
    }
    assert_eq!(combined, "あ\x1b\n".repeat(10000));
    // Expiry never extends on read, and leaves a tombstone.
    let path = root.join("results").join(format!("{id}.result"));
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    document["finished_at"] = serde_json::json!(results::now() - 3600);
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    assert_eq!(
        results::inspect(&store, &settings, &id)
            .unwrap()
            .unwrap()
            .availability,
        "expired"
    );
    assert!(!root.join("results").join(format!("{id}.output")).exists());
    assert!(path.exists());
}
#[test]
fn preview_truncation_capacity_eviction_and_active_spool_reservations() {
    let (temp, store) = store();
    let root = temp.path().canonicalize().unwrap().join("executions");
    let settings = Results {
        retention_seconds: 3600,
        capacity_bytes: 1048576,
        max_artifact_bytes: 700000,
    };
    let first = ExecutionStore::allocate_id().unwrap();
    assert!(worker(&root, &first, "print('x'*800000)", 700000).success());
    let (_, output) = results::page(&store, &settings, &first, None, 1, 65536).unwrap();
    assert_eq!(output["result"]["completeness"]["truncated"], true);
    let second = ExecutionStore::allocate_id().unwrap();
    let (pending, _file) = results::reserve(&store, &settings, &second).unwrap();
    assert_eq!(pending.availability, "pending");
    assert_eq!(
        results::inspect(&store, &settings, &first)
            .unwrap()
            .unwrap()
            .availability,
        "evicted"
    );
    // Active reservations consume capacity and cannot be evicted.
    let third = ExecutionStore::allocate_id().unwrap();
    let (_third, _file) = results::reserve(&store, &settings, &third).unwrap();
    assert_eq!(
        results::inspect(&store, &settings, &second)
            .unwrap()
            .unwrap()
            .availability,
        "pending"
    );
    let fourth = ExecutionStore::allocate_id().unwrap();
    assert_eq!(
        results::reserve(&store, &settings, &fourth)
            .unwrap_err()
            .code,
        ErrorCode::StorageFull
    );
    let marker = temp.path().join("must-not-execute");
    assert!(
        !worker(
            &root,
            &fourth,
            &format!(
                "open({:?}, 'w').write('executed')",
                marker.to_str().unwrap()
            ),
            700000
        )
        .success()
    );
    assert!(!marker.exists());
    let failed_capture = results::inspect(&store, &settings, &fourth)
        .unwrap()
        .unwrap();
    assert_eq!(failed_capture.availability, "unavailable");
    assert_eq!(failed_capture.error.unwrap().code, ErrorCode::StorageFull);
}

#[test]
fn interrupted_request_process_worker() {
    let Some(root) = std::env::var_os("AXVERN_REQUEST_CRASH_STORE") else {
        return;
    };
    let store = ExecutionStore::open(std::path::PathBuf::from(root)).unwrap();
    let _ = store.register_request("crashed", &plan(), || Ok(()), |_| std::process::exit(31));
    panic!("worker should exit inside submission");
}
#[test]
fn process_crash_after_durable_intent_is_never_resubmitted() {
    let (temp, store) = store();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "interrupted_request_process_worker"])
        .env(
            "AXVERN_REQUEST_CRASH_STORE",
            temp.path().canonicalize().unwrap().join("executions"),
        )
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(31));
    let pending = store
        .register_request(
            "crashed",
            &plan(),
            || panic!("no new authorization"),
            |_| panic!("no second submission"),
        )
        .unwrap();
    assert!(pending.record.is_none());
}

#[test]
fn parent_state_aggregation_does_not_invent_child_exit_codes() {
    use axvern_core::parents::aggregate;
    for (states, expected) in [
        (
            vec![ChildState::Queued, ChildState::Queued],
            TaskState::Queued,
        ),
        (
            vec![ChildState::Queued, ChildState::Done],
            TaskState::Running,
        ),
        (
            vec![ChildState::Running, ChildState::Failed],
            TaskState::Running,
        ),
        (vec![ChildState::Done, ChildState::Done], TaskState::Done),
        (
            vec![ChildState::Done, ChildState::Failed],
            TaskState::Partial,
        ),
        (
            vec![ChildState::Failed, ChildState::Failed],
            TaskState::Failed,
        ),
    ] {
        assert_eq!(aggregate(&states), Some(expected));
    }
    assert_eq!(aggregate(&[]), None);
}

#[test]
fn concurrent_registrations_without_request_id_share_one_capacity_reservation() {
    let (_temp, store) = store();
    let store = Arc::new(store);
    let settings_for = |capacity_bytes| Results {
        retention_seconds: 3600,
        capacity_bytes,
        max_artifact_bytes: 65536,
    };
    // Smallest capacity at which exactly one registration still fits.
    let (mut low, mut high) = (0, 1u64 << 24);
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        let fits =
            results::preflight_registration_locked(&store, &settings_for(mid), &plan()).is_ok();
        if fits {
            high = mid;
        } else {
            low = mid;
        }
    }
    let settings = Arc::new(settings_for(high));
    let registered = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (store, settings, registered) =
                (store.clone(), settings.clone(), registered.clone());
            std::thread::spawn(move || {
                store.register_intent(
                    &ExecutionStore::allocate_id().unwrap(),
                    &plan(),
                    || results::preflight_registration_locked(&store, &settings, &plan()),
                    |_| {
                        registered.fetch_add(1, Ordering::SeqCst);
                        Ok(identity())
                    },
                )
            })
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(registered.load(Ordering::SeqCst), 1);
    for refused in outcomes.iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(refused.code, ErrorCode::StorageFull);
    }
}

#[test]
fn doctor_deadline_bounds_waiting_for_another_clients_store_lock() {
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let backend = root.join("pueue.yml");
    fs::write(&backend, "").unwrap();
    let store_path = root.join("executions");
    let _store = ExecutionStore::open(&store_path).unwrap();
    let config = Config::parse(&format!(
        "schema_version=1\n[execution]\npueue_config={}\nstore={}\n",
        serde_json::to_string(&backend).unwrap(),
        serde_json::to_string(&store_path).unwrap()
    ))
    .unwrap();
    // Another client holds the store lock for the whole diagnosis.
    let held = fs::File::open(store_path.join("store.lock")).unwrap();
    held.lock().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let cwd = root.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let report = axvern_core::doctor::diagnose_scope_with_timeout(
            Ok(config),
            &cwd,
            "core",
            Duration::from_millis(500),
        );
        let _ = sender.send((started.elapsed(), report));
    });
    let (elapsed, report) = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("doctor must not wait indefinitely for another client's lock");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    let store_check = report
        .checks
        .iter()
        .find(|c| c.name == "execution_store")
        .unwrap();
    assert_eq!(store_check.level, CheckLevel::Fail);
    assert!(store_check.message.contains("Timeout"), "{store_check:?}");
    drop(held);
}

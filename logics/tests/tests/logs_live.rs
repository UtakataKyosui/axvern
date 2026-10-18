//! `query`/`recall` through the public CLI and a real, isolated Pueue 4.0.4 daemon: the original
//! command must run exactly once however often its retained log is read.
mod common;
use common::*;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

macro_rules! live {
    () => {
        if !discovery_tools(&["pueue", "pueued"]) {
            return;
        }
    };
}
fn configure(q: &Queue, extra: &str) -> PathBuf {
    let path = q._temp.path().join("axvern.toml");
    let text = format!(
        "schema_version=1\n{extra}\n[execution]\npueue_config={}\nstore={}\n[[policy.rules]]\nid='py'\neffect='allow'\ncommand='python3'\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&q._temp.path().join("store")).unwrap()
    );
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
/// Registers `python3 -c code` through the CLI and waits for it to finish.
fn run(q: &Queue, config: &Path, code: &str, finish: bool) -> String {
    let started = cli_json(
        q,
        config,
        &["run", "--wait", "0ms", "--", "python3", "-c", code],
        0,
    );
    let id = started["execution_id"].as_str().unwrap().to_owned();
    if finish {
        wait(q, config, &id);
    }
    id
}
fn wait(q: &Queue, config: &Path, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = cli_json(q, config, &["status", id], 0);
        if matches!(status["state"].as_str(), Some("done" | "failed")) {
            return;
        }
        assert!(Instant::now() < deadline, "task did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn wait_for_log(q: &Queue, id: &str, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let task_id = {
        let store = q._temp.path().join("store").join(format!("{id}.json"));
        let record: Value = serde_json::from_slice(&fs::read(store).unwrap()).unwrap();
        record["record"]["backend"]["task_id"].as_u64().unwrap()
    };
    let log = q
        ._temp
        .path()
        .join("data/task_logs")
        .join(format!("{task_id}.log"));
    while !fs::read_to_string(&log).is_ok_and(|t| t.contains(needle)) {
        assert!(Instant::now() < deadline, "log never contained {needle}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn log_file(q: &Queue, id: &str) -> PathBuf {
    let store = q._temp.path().join("store").join(format!("{id}.json"));
    let record: Value = serde_json::from_slice(&fs::read(store).unwrap()).unwrap();
    let task = record["record"]["backend"]["task_id"].as_u64().unwrap();
    q._temp
        .path()
        .join("data/task_logs")
        .join(format!("{task}.log"))
}

const PROGRAM: &str = "import sys\nopen(sys.argv[1], 'a').write('ran\\n')\nfor i in range(1, 6):\n    print('line', i)\nprint('error: first failure')\nprint('Result: FATAL again')\nprint('done')\n";

#[test]
fn query_and_recall_read_the_log_without_running_the_command_again() {
    live!();
    let q = Queue::new();
    let config = configure(&q, "");
    let counter = q._temp.path().join("counter");
    let code = PROGRAM.replace("sys.argv[1]", &format!("{:?}", counter.to_str().unwrap()));
    let id = run(&q, &config, &code, true);
    assert_eq!(fs::read_to_string(&counter).unwrap(), "ran\n");
    for _ in 0..3 {
        let m = cli_json(&q, &config, &["query", &id, "--match", "line 3"], 0);
        assert_eq!(m["output"]["kind"], "query");
        assert_eq!(m["output"]["items"][0]["line"], 3);
        assert_eq!(m["output"]["items"][0]["text"], "line 3");
        assert_eq!(m["output"]["source_execution_id"], id);
        assert_eq!(m["output"]["partial_log"], false);
        assert_eq!(m["output"]["stream"], "combined");
        assert_eq!(m["execution_id"], id);
        let e = cli_json(&q, &config, &["query", &id, "--errors"], 0);
        let lines: Vec<_> = e["output"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines, [6, 7]);
        assert_eq!(e["output"]["selector"]["errors"]["rule_version"], 1);
        let t = cli_json(&q, &config, &["query", &id, "--tail", "2"], 0);
        assert_eq!(t["output"]["items"][1]["text"], "done");
        assert_eq!(t["output"]["items"][0]["line"], 7);
        let r = cli_json(
            &q,
            &config,
            &["query", &id, "--from-line", "2", "--lines", "2"],
            0,
        );
        assert_eq!(r["output"]["items"].as_array().unwrap().len(), 2);
        let none = cli_json(&q, &config, &["query", &id, "--match", "absent"], 0);
        assert!(none["output"]["items"].as_array().unwrap().is_empty());
        let all = cli_json(&q, &config, &["recall", &id], 0);
        assert_eq!(all["output"]["complete"], true);
        assert!(
            all["output"]["text"]
                .as_str()
                .unwrap()
                .starts_with("line 1\n")
        );
        assert!(all["output"]["text"].as_str().unwrap().ends_with("done\n"));
    }
    // Reading never started another run: one registered task, one execution of the program.
    assert_eq!(fs::read_to_string(&counter).unwrap(), "ran\n");
    let history = cli_json(&q, &config, &["history"], 0);
    assert_eq!(history["output"]["executions"].as_array().unwrap().len(), 1);
    // recall --output: a new private file with the exact bytes and digest.
    let out = q._temp.path().join("recalled.log");
    let saved = cli_json(
        &q,
        &config,
        &["recall", &id, "--output", out.to_str().unwrap()],
        0,
    );
    assert_eq!(saved["output"]["encoding"], "bytes");
    assert_eq!(
        fs::read(&out).unwrap(),
        fs::read(log_file(&q, &id)).unwrap()
    );
    assert_eq!(saved["output"]["bytes"], fs::metadata(&out).unwrap().len());
    assert!(saved["output"].get("text").is_none());
    let again = cli_json(
        &q,
        &config,
        &["recall", &id, "--output", out.to_str().unwrap()],
        15,
    );
    assert_eq!(again["error"]["code"], "output_exists");
    // Human output is escaped and never mixes metadata into a JSON document.
    let human = public_cli(
        &q,
        &config,
        &["--format", "human", "query", &id, "--match", "line 5"],
    );
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("5:line 5"));
    assert_eq!(fs::read_to_string(&counter).unwrap(), "ran\n");
}
#[test]
fn a_running_task_gives_a_partial_snapshot_and_keeps_running() {
    live!();
    let q = Queue::new();
    let config = configure(&q, "");
    let marker = q._temp.path().join("release");
    let code = format!(
        "import os,time\nprint('first',flush=True)\nwhile not os.path.exists({:?}): time.sleep(0.05)\nprint('last',flush=True)",
        marker.to_str().unwrap()
    );
    let id = run(&q, &config, &code, false);
    wait_for_log(&q, &id, "first");
    let tail = cli_json(&q, &config, &["query", &id, "--tail", "1"], 0);
    assert_eq!(tail["output"]["partial_log"], true);
    assert_eq!(tail["output"]["log_availability"], "partial");
    assert_eq!(tail["output"]["items"][0]["text"], "first");
    assert_eq!(tail["state"], "running");
    let all = cli_json(&q, &config, &["recall", &id], 0);
    assert_eq!(all["output"]["complete"], false);
    assert_eq!(all["output"]["partial_log"], true);
    fs::write(&marker, "").unwrap();
    wait(&q, &config, &id);
    let done = cli_json(&q, &config, &["recall", &id], 0);
    assert_eq!(done["output"]["complete"], true);
    assert_eq!(done["output"]["text"], "first\nlast\n");
}
#[test]
fn removed_or_deleted_logs_are_explicit_and_consistent_with_history() {
    live!();
    let q = Queue::new();
    let config = configure(&q, "");
    let deleted = run(&q, &config, "print('a')", true);
    let cleaned = run(&q, &config, "print('b')", true);
    let queued_log = log_file(&q, &deleted);
    // The log file disappears while Pueue still lists the task: log_missing, not "no output".
    fs::remove_file(&queued_log).unwrap();
    for args in [
        vec!["query", &deleted, "--tail", "1"],
        vec!["recall", &deleted],
    ] {
        let r = cli_json(&q, &config, &args, 11);
        assert_eq!(r["error"]["code"], "log_missing");
        assert!(r["output"].is_null());
    }
    // `pueue clean` removes the task and its log: the same condition history reports.
    q.client(&["clean"]);
    for args in [
        vec!["query", &cleaned, "--errors"],
        vec!["recall", &cleaned],
    ] {
        let r = cli_json(&q, &config, &args, 8);
        assert_eq!(r["error"]["code"], "expired");
    }
    let history = cli_json(&q, &config, &["history"], 0);
    for view in history["output"]["executions"].as_array().unwrap() {
        assert_eq!(view["error"]["code"], "expired");
    }
    // Nothing was created for a missing log.
    let out = q._temp.path().join("never.log");
    cli_json(
        &q,
        &config,
        &["recall", &cleaned, "--output", out.to_str().unwrap()],
        8,
    );
    assert!(!out.exists());
    // Unknown and malformed IDs.
    let unknown = "ex_00000000-0000-4000-8000-000000000000";
    assert_eq!(
        cli_json(&q, &config, &["query", unknown, "--errors"], 7)["error"]["code"],
        "task_not_found"
    );
    assert_eq!(
        cli_json(&q, &config, &["recall", "nope"], 2)["error"]["code"],
        "invalid_input"
    );
}
#[test]
fn unreadable_logs_are_reported_as_permission_denied() {
    live!();
    let q = Queue::new();
    let config = configure(&q, "");
    let id = run(&q, &config, "print('secret')", true);
    let path = log_file(&q, &id);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&path).is_ok() {
        eprintln!("skipped: running as a user that ignores file modes");
        return;
    }
    let r = cli_json(&q, &config, &["recall", &id], 4);
    assert_eq!(r["error"]["code"], "permission_denied");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}
#[test]
fn invalid_encoding_and_inline_limits_have_distinct_results() {
    live!();
    let q = Queue::new();
    let config = configure(&q, "");
    let binary = run(
        &q,
        &config,
        "import sys\nsys.stdout.buffer.write(b'ok\\n\\xff\\xfe\\x00bad\\n')",
        true,
    );
    let r = cli_json(&q, &config, &["recall", &binary], 2);
    assert_eq!(r["error"]["code"], "invalid_encoding");
    let r = cli_json(&q, &config, &["query", &binary, "--match", "ok"], 2);
    assert_eq!(r["error"]["code"], "invalid_encoding");
    let out = q._temp.path().join("raw.bin");
    let saved = cli_json(
        &q,
        &config,
        &["recall", &binary, "--output", out.to_str().unwrap()],
        0,
    );
    assert_eq!(saved["output"]["encoding"], "bytes");
    assert_eq!(fs::read(&out).unwrap(), b"ok\n\xff\xfe\x00bad\n");
    let big = run(
        &q,
        &config,
        "print('x' * 100 + '\\n') or [print('y' * 100) for _ in range(2000)]",
        true,
    );
    let r = cli_json(&q, &config, &["recall", &big], 11);
    assert_eq!(r["error"]["code"], "output_too_large");
    // query still works on the same log, under its own limits.
    let q1 = cli_json(
        &q,
        &config,
        &["query", &big, "--tail", "5", "--limit", "2"],
        0,
    );
    assert_eq!(q1["output"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(q1["output"]["truncated"], true);
    assert_eq!(q1["output"]["omitted_count"], 3);
}
#[test]
fn huge_logs_and_long_lines_stay_within_bounded_memory() {
    live!();
    let q = Queue::new();
    let config = configure(&q, "");
    // ~96 MiB of ordinary lines, then a single 8 MiB line, then an error line.
    let code = "import sys\nw=sys.stdout.write\nfor i in range(1200000):\n    w('line %d some filler text to make the line longer than usual padding\\n' % i)\nw('z' * (8 << 20) + '\\n')\nw('error: at the very end\\n')";
    let id = run(&q, &config, code, true);
    let size = fs::metadata(log_file(&q, &id)).unwrap().len();
    assert!(size > 80 << 20, "{size}");
    let mut peaks = Vec::new();
    for args in [
        vec!["query", &id, "--match", "line 1199999"],
        vec!["query", &id, "--errors"],
        vec!["query", &id, "--tail", "3"],
        vec!["query", &id, "--from-line", "10", "--lines", "5"],
    ] {
        let (out, peak) = measured(&q, &config, &args);
        let json: Value = serde_json::from_slice(&out.stdout).unwrap();
        eprintln!("RSS {:>6} KiB for {:?}", peak, &args[2..]);
        peaks.push(peak);
        match args[2] {
            // Within the 64 MiB scan budget: found, exit 0.
            "--from-line" => assert_eq!(out.status.code(), Some(0)),
            // Beyond it: bounded work, incomplete scan, exit 11 and no invented line numbers.
            _ => {
                assert_eq!(out.status.code(), Some(11), "{json}");
                assert_eq!(json["output"]["scan_complete"], false);
                assert_eq!(json["error"]["code"], "partial_result");
                assert!(json["output"]["omitted_count"].is_null());
            }
        }
    }
    assert!(peaks.iter().all(|kib| *kib < 96 * 1024), "{peaks:?}");
    // A long line inside the budget is cut at the byte cap and flagged.
    let long = run(&q, &config, "print('a'*300000); print('error tail')", true);
    let r = cli_json(&q, &config, &["query", &long, "--match", "aaa"], 0);
    assert_eq!(r["output"]["items"][0]["line_truncated"], true);
    assert_eq!(
        r["output"]["items"][0]["text"].as_str().unwrap().len(),
        65536
    );
    let r = cli_json(&q, &config, &["query", &long, "--errors"], 0);
    assert_eq!(r["output"]["items"][0]["line"], 2);
    // recall --output streams the whole log with bounded memory.
    let out = q._temp.path().join("big.log");
    let (res, peak) = measured(
        &q,
        &config,
        &["recall", &id, "--output", out.to_str().unwrap()],
    );
    assert_eq!(
        res.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&res.stdout)
    );
    eprintln!("RSS {peak:>6} KiB for recall --output of {size} bytes");
    assert!(peak < 64 * 1024, "{peak}");
    assert_eq!(fs::metadata(&out).unwrap().len(), size);
}
/// Runs the public CLI and returns its output with the peak resident set size in KiB.
fn measured(q: &Queue, config: &Path, args: &[&str]) -> (std::process::Output, u64) {
    use std::io::Read;
    let mut child = Command::new(std::env::var_os("AXVERN_TEST_BIN").unwrap())
        .current_dir(q._temp.path())
        .env_remove("AXVERN_CONFIG")
        .arg("--config")
        .arg(config)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let drain = |mut stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            bytes
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let status_file = format!("/proc/{}/status", child.id());
    let mut peak = 0;
    let status = loop {
        // VmHWM is the kernel's own high-water mark, so the last readable value is the peak.
        if let Ok(text) = fs::read_to_string(&status_file)
            && let Some(kib) = text
                .lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        {
            peak = peak.max(kib);
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let output = std::process::Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    };
    (output, peak)
}

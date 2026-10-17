//! Real fd/tokei children of `fs analyze` through an isolated Pueue daemon and the public CLI.
//! Skipped (with a message) when pueue, fd or tokei is not installed.
mod common;
use axvern_models::ChildState;
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
        if !discovery_tools(&["pueue", "pueued", "fd", "tokei"]) {
            return;
        }
    };
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
/// A Git worktree with a manifest below the tree depth, an ignored directory and a symbolic
/// link to a directory outside the root that holds code and a manifest of its own.
fn repo(base: &Path) -> PathBuf {
    let root = base.join("repo");
    write(&root, ".git/HEAD", "ref: refs/heads/main\n");
    write(&root, ".gitignore", "build/\n");
    write(&root, "Cargo.toml", "[workspace]\n");
    write(&root, "src/main.rs", "fn main() {}\n");
    write(
        &root,
        "a/b/c/d/package.json",
        "{\"scripts\":{\"x\":\"touch RAN\"}}\n",
    );
    write(&root, "build/Cargo.toml", "[package]\n");
    write(&root, "build/gen.rs", "fn gen() {}\n");
    write(base, "outside/Cargo.toml", "[package]\n");
    write(base, "outside/x.rs", "fn x() {}\nfn y() {}\n");
    std::os::unix::fs::symlink(base.join("outside"), root.join("link")).unwrap();
    root
}
fn configure(q: &Queue, extra: &str) -> PathBuf {
    let path = q._temp.path().join("axvern.toml");
    let mut text = format!(
        "schema_version=1\n{extra}\n[execution]\npueue_config={}\nstore={}\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&q._temp.path().join("store")).unwrap()
    );
    for tool in ["fd", "tokei"] {
        text += &format!("[[policy.rules]]\nid='{tool}'\neffect='allow'\ncommand='{tool}'\n");
    }
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
fn items(result: &Value, section: &str) -> Vec<Value> {
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["section"] == section)
        .cloned()
        .collect()
}
fn paths(result: &Value, section: &str) -> Vec<String> {
    items(result, section)
        .iter()
        .map(|i| i["path"].as_str().unwrap().to_owned())
        .collect()
}
fn child<'a>(result: &'a Value, section: &str) -> &'a Value {
    result["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["section"] == section)
        .unwrap()
}
fn tasks(q: &Queue) -> usize {
    q.adapter.list().unwrap().len()
}

#[test]
fn a_directory_is_analyzed_by_three_children_and_reread_without_them() {
    live!();
    let q = Queue::new();
    let root = repo(q._temp.path());
    let config = configure(&q, "");
    let started = cli_json(&q, &config, &["fs", "analyze", "repo", "--wait", "30s"], 0);
    assert_eq!(started["command"], "fs analyze");
    assert_eq!(started["state"], "done");
    assert_eq!(started["exit_code"], Value::Null);
    let id = started["execution_id"].as_str().unwrap().to_owned();
    let output = &started["output"];
    assert_eq!(output["record"]["children"].as_array().unwrap().len(), 3);
    assert_eq!(output["result_availability"], "available");
    let result = &output["result"];
    assert_eq!(result["kind"], "fs.analyze");
    let summary = &result["summary"];
    assert_eq!(
        summary["path"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(
        (
            summary["tree_depth"].as_u64(),
            summary["scan_depth"].as_u64()
        ),
        (Some(3), Some(32))
    );
    // The tree stops at its display depth; the manifest search does not.
    let tree = paths(result, "tree");
    assert!(tree.iter().all(|p| p.split('/').count() <= 3), "{tree:?}");
    assert!(tree.contains(&"a/b/c".to_owned()));
    assert!(!tree.contains(&"a/b/c/d".to_owned()));
    assert_eq!(
        paths(result, "manifests"),
        ["Cargo.toml", "a/b/c/d/package.json"]
    );
    // `.git` is an entry, its contents are not; the link is an entry, its target is not read.
    assert!(tree.contains(&".git".to_owned()));
    assert!(tree.contains(&"link".to_owned()));
    for section in ["tree", "manifests"] {
        assert!(
            !paths(result, section).iter().any(|p| p.starts_with(".git/")
                || p.starts_with("link/")
                || p.starts_with("build")),
            "{section}"
        );
    }
    let link = items(result, "tree")
        .into_iter()
        .find(|i| i["path"] == "link")
        .unwrap();
    assert_eq!(link["type"], "symlink");
    // Statistics count the root's own code only: not the ignored directory, not the link target.
    let rust = items(result, "languages")
        .into_iter()
        .find(|i| i["language"] == "Rust")
        .unwrap();
    assert_eq!(
        (rust["files"].as_u64(), rust["code"].as_u64()),
        (Some(1), Some(1))
    );
    assert!(summary["totals"]["files"].as_u64().unwrap() >= 3);
    for section in ["languages", "manifests", "tree"] {
        assert_eq!(child(result, section)["state"], "done", "{section}");
    }
    // Manifest contents and scripts are never run.
    assert!(!root.join("a/b/c/d/RAN").exists());
    // AC-SPEC-45-05: re-reading registers nothing and returns the stored aggregate.
    let before = tasks(&q);
    let first = cli_json(&q, &config, &["result", &id], 0);
    let second = cli_json(&q, &config, &["result", &id], 0);
    assert_eq!(first["output"], second["output"]);
    assert_eq!(first["output"]["result"]["items"], result["items"]);
    assert_eq!(first["state"], "done");
    assert_eq!(tasks(&q), before);
    let status = cli_json(&q, &config, &["status", &id], 0);
    assert_eq!(status["state"], "done");
    assert_eq!(status["output"]["result_availability"], "available");
    let history = cli_json(&q, &config, &["history"], 0);
    let parent = history["output"]["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["record"]["execution_id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(parent["kind"], "fs.analyze");
    assert_eq!(parent["task"]["state"], "done");
    assert_eq!(parent["children"].as_array().unwrap().len(), 3);
    // Removing the children's Pueue tasks does not remove the parent's stored result. A worker
    // publishes its result just before its task ends, so clean until every task is gone.
    let deadline = Instant::now() + Duration::from_secs(10);
    while tasks(&q) != 0 {
        assert!(Instant::now() < deadline, "pueue clean left tasks");
        q.client(&["clean"]);
        std::thread::sleep(Duration::from_millis(50));
    }
    let cleaned = cli_json(&q, &config, &["result", &id], 0);
    assert_eq!(cleaned["output"], first["output"]);
    assert_eq!(cli_json(&q, &config, &["wait", &id], 0)["state"], "done");
}

#[test]
fn an_unwaited_start_is_pending_until_the_children_are_observed_finished() {
    live!();
    let q = Queue::new();
    repo(q._temp.path());
    let config = configure(&q, "");
    let started = cli_json(&q, &config, &["fs", "analyze", "repo", "--wait", "0s"], 0);
    assert_eq!(started["state"], Value::Null);
    assert_eq!(started["output"]["result_availability"], "pending");
    assert_eq!(started["output"]["result"], Value::Null);
    let id = started["execution_id"].as_str().unwrap().to_owned();
    let pending = &started["output"]["children"];
    assert_eq!(pending.as_array().unwrap().len(), 3);
    assert!(pending[0]["execution_id"].is_string());
    // Let every child finish without observing the parent, then list it.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !q
        .adapter
        .list()
        .unwrap()
        .iter()
        .all(|t| t.state == ChildState::Done)
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    let history = cli_json(&q, &config, &["history"], 0);
    let parent = history["output"]["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["record"]["execution_id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(parent["kind"], "fs.analyze");
    assert_eq!(parent["task"]["state"], "done");
    assert_eq!(parent["result_availability"], "available");
    assert_eq!(parent["children"][0]["section"], "languages");
    assert_eq!(cli_json(&q, &config, &["wait", &id], 0)["state"], "done");
    let result = cli_json(&q, &config, &["result", &id], 0);
    assert_eq!(result["output"]["result_availability"], "available");
    assert_eq!(result["state"], "done");
}

#[test]
fn a_file_is_analyzed_alone() {
    live!();
    let q = Queue::new();
    repo(q._temp.path());
    let config = configure(&q, "");
    let before = tasks(&q);
    let started = cli_json(
        &q,
        &config,
        &["fs", "analyze", "repo/src/main.rs", "--wait", "30s"],
        0,
    );
    // Only the statistics child runs; the tree is the file itself.
    assert_eq!(tasks(&q), before + 1);
    let result = &started["output"]["result"];
    assert_eq!(started["state"], "done");
    assert_eq!(result["summary"]["input_type"], "file");
    assert_eq!(paths(result, "tree"), ["main.rs"]);
    assert!(paths(result, "manifests").is_empty());
    assert_eq!(items(result, "languages").len(), 1);
    assert_eq!(result["summary"]["totals"]["files"], 1);
    let manifest = cli_json(
        &q,
        &config,
        &["fs", "analyze", "repo/Cargo.toml", "--wait", "30s"],
        0,
    );
    assert_eq!(
        paths(&manifest["output"]["result"], "manifests"),
        ["Cargo.toml"]
    );
}

/// The public CLI with a changed `PATH`, which Pueue passes on to the tasks it registers.
fn cli_with_path(q: &Queue, config: &Path, path: &str, args: &[&str]) -> (i32, Value) {
    let out =
        Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
            .current_dir(q._temp.path())
            .env_remove("AXVERN_CONFIG")
            .env("PATH", path)
            .arg("--config")
            .arg(config)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn a_failed_child_leaves_a_partial_result_and_the_others_run_once() {
    live!();
    let q = Queue::new();
    repo(q._temp.path());
    let config = configure(&q, "");
    // A tokei that passes the version check but fails when it runs.
    let bin = q._temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("tokei"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'tokei 15.0.0 compiled with serialization support: json'; exit 0; fi\necho broken >&2\nexit 3\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("tokei"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (code, started) = cli_with_path(
        &q,
        &config,
        &path,
        &["fs", "analyze", "repo", "--wait", "30s"],
    );
    assert_eq!(code, 0, "{started}");
    let id = started["execution_id"].as_str().unwrap().to_owned();
    if started["state"] == "done" {
        assert!(
            std::env::var_os("AXVERN_REQUIRE_DISCOVERY_TOOLS").is_none(),
            "v0.2 live gate requires the failed-child PATH fixture to execute"
        );
        eprintln!("skipped: this Pueue does not pass the client's PATH to its tasks");
        return;
    }
    // AC-SPEC-45-02: partial, the failed child's error, the successful sections.
    assert_eq!(started["state"], "partial");
    let result = &started["output"]["result"];
    let failed = child(result, "languages");
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["exit_code"], 3);
    assert_eq!(failed["error"]["code"], "backend_failure");
    assert!(items(result, "languages").is_empty());
    assert_eq!(paths(result, "manifests").len(), 2);
    assert!(!paths(result, "tree").is_empty());
    assert_eq!(result["completeness"]["partial"], true);
    let before = tasks(&q);
    let fetched = cli_json(&q, &config, &["result", &id], 0);
    assert_eq!(fetched["state"], "partial");
    assert_eq!(fetched["output"]["result"]["children"], result["children"]);
    assert_eq!(
        cli_json(&q, &config, &["wait", &id], 11)["state"],
        "partial"
    );
    assert_eq!(tasks(&q), before);
}

#[test]
fn unreadable_directories_and_bounds_are_reported() {
    live!();
    let q = Queue::new();
    let root = repo(q._temp.path());
    for i in 0..150 {
        write(&root, &format!("many/f{i:03}"), "");
    }
    write(&root, "locked/Cargo.toml", "[package]\n");
    write(&root, "locked/l.rs", "fn l() {}\n");
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(root.join("locked")).is_ok();
    let config = configure(&q, "");
    let started = cli_json(
        &q,
        &config,
        &["fs", "analyze", "repo", "--depth", "2", "--wait", "30s"],
        0,
    );
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    let id = started["execution_id"].as_str().unwrap().to_owned();
    let result = &started["output"]["result"];
    if readable {
        eprintln!("skipped permission checks: running with access to mode 000 directories");
    } else {
        // Every child finished; the unreadable directory makes the scope partial.
        assert_eq!(started["state"], "done");
        assert_eq!(result["completeness"]["partial"], true);
        let denied: Vec<_> = result["errors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["code"] == "permission_denied")
            .map(|e| e["section"].as_str().unwrap().to_owned())
            .collect();
        for section in ["languages", "manifests", "tree"] {
            assert!(
                denied.contains(&section.to_owned()),
                "{section}: {denied:?}"
            );
        }
    }
    // More than one page of items: every page repeats the children, the cursor continues.
    assert_eq!(result["page"]["has_more"], true);
    let cursor = result["page"]["next_cursor"].as_str().unwrap();
    let next = cli_json(&q, &config, &["result", &id, "--cursor", cursor], 0);
    assert_eq!(next["output"]["result"]["children"], result["children"]);
    let total = result["items"].as_array().unwrap().len()
        + next["output"]["result"]["items"].as_array().unwrap().len();
    let summary = &result["summary"]["items"];
    assert_eq!(
        total as u64,
        summary["languages"].as_u64().unwrap()
            + summary["manifests"].as_u64().unwrap()
            + summary["tree"].as_u64().unwrap()
    );
    // A small item bound truncates the children and the parent says so.
    let small = configure(
        &q,
        "[discovery]\nmax_items=5\nmax_entries=100000\ntimeout_ms=60000",
    );
    let bounded = cli_json(&q, &small, &["fs", "analyze", "repo", "--wait", "30s"], 0);
    let result = &bounded["output"]["result"];
    assert_eq!(result["completeness"]["truncated"], true);
    assert!(
        result["completeness"]["truncation_reasons"]
            .as_array()
            .unwrap()
            .contains(&"max_items".into())
    );
    assert_eq!(child(result, "tree")["completeness"]["truncated"], true);
    assert_eq!(paths(result, "tree").len(), 5);
}

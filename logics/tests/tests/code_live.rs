//! Real ast-grep and tokei through an isolated Pueue daemon and the public CLI.
//! Skipped (with a message) when pueue, ast-grep or tokei is not installed.
mod common;
use common::*;
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

macro_rules! live {
    () => {
        if !discovery_tools(&["pueue", "pueued", "ast-grep", "tokei"]) {
            return;
        }
    };
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
/// A worktree (the marker directory is all the tools look for) with ignored, hidden and
/// symlinked sources and one file that does not parse.
fn repo(root: &Path) {
    write(root, ".git/HEAD", "ref: refs/heads/main\n");
    write(root, ".git/objects/x.rs", "fn target() {}\n");
    write(root, ".gitignore", "build/\n");
    write(root, "src/a.rs", "fn target() {}\nfn other() { 1 }\n");
    write(
        root,
        "src/sub/b.rs",
        "// \u{3042}\nfn  \u{3042}() {}  fn target() {}\n",
    );
    write(root, ".hidden/h.rs", "fn target() {}\n");
    write(root, "build/out.rs", "fn target() {}\n");
    write(root, "broken.rs", "fn target() {}\nfn bad( {\n");
    write(root, "script.py", "def target():\n    pass\n");
    write(
        root,
        "page.html",
        "<p>\n<script>\nvar a=1;\n// c\n</script>\n</p>\n",
    );
    std::os::unix::fs::symlink("src", root.join("link")).unwrap();
}
fn config(q: &Queue, extra: &str) -> std::path::PathBuf {
    let path = q._temp.path().join("axvern.toml");
    let mut text = format!(
        "schema_version=1\n{extra}\n[execution]\npueue_config={}\nstore={}\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&q._temp.path().join("store")).unwrap()
    );
    for tool in ["ast-grep", "tokei"] {
        text += &format!("[[policy.rules]]\nid='{tool}'\neffect='allow'\ncommand='{tool}'\n");
    }
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
fn start(q: &Queue, config: &Path, args: &[&str]) -> Value {
    let mut argv = args.to_vec();
    argv.extend(["--wait", "30s"]);
    cli_json(q, config, &argv, 0)
}
fn items(response: &Value) -> Vec<Value> {
    response["output"]["result"]["items"]
        .as_array()
        .unwrap()
        .clone()
}
fn matches(response: &Value) -> Vec<(String, u64, u64)> {
    items(response)
        .iter()
        .map(|i| {
            (
                i["path"].as_str().unwrap().to_owned(),
                i["start"]["line"].as_u64().unwrap(),
                i["start"]["byte_column"].as_u64().unwrap(),
            )
        })
        .collect()
}
fn search(pattern: &str, rest: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["code", "search", "--pattern", pattern, "--lang", "rust"]
        .map(String::from)
        .into();
    args.extend(rest.iter().map(|s| s.to_string()));
    args
}
fn run(q: &Queue, config: &Path, args: &[String]) -> Value {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    start(q, config, &args)
}

#[test]
fn search_finds_positions_and_distrusts_files_that_do_not_parse() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    let config = config(&q, "");
    let found = run(&q, &config, &search("fn target() {}", &["repo"]));
    assert_eq!(found["command"], "code search");
    assert_eq!(found["state"], "done");
    let result = &found["output"]["result"];
    assert_eq!(result["kind"], "code.search");
    assert_eq!(result["summary"]["lang"], "rust");
    // Hidden sources are searched; ignored ones, `.git`, symlinked trees and other
    // languages are not. The byte column of the second match counts the 3-byte character.
    assert_eq!(
        matches(&found),
        [
            (".hidden/h.rs".into(), 1, 1),
            ("src/a.rs".into(), 1, 1),
            ("src/sub/b.rs".into(), 2, 15),
        ]
    );
    let item = &items(&found)[2];
    assert_eq!(item["end"]["byte_column"], 29);
    assert_eq!(item["snippet"], "fn target() {}");
    assert_eq!(item["lang"], "rust");
    // broken.rs matched too, but it does not parse: reported, not trusted.
    assert_eq!(result["errors"][0]["code"], "parse_error");
    assert_eq!(result["errors"][0]["path"], "broken.rs");
    assert_eq!(result["completeness"]["partial"], true);
    assert_eq!(result["completeness"]["scope_complete"], false);
    let all = run(
        &q,
        &config,
        &search("fn target() {}", &["repo", "--include-ignored"]),
    );
    assert!(matches(&all).iter().any(|m| m.0 == "build/out.rs"));
    // Reading the stored result registers nothing and returns the same items.
    let id = found["execution_id"].as_str().unwrap();
    let tasks = q.adapter.list().unwrap().len();
    let again = cli_json(&q, &config, &["result", id], 0);
    assert_eq!(items(&again), items(&found));
    assert_eq!(q.adapter.list().unwrap().len(), tasks);
}

#[test]
fn search_counts_nonmatching_files_in_every_supported_language() {
    live!();
    let q = Queue::new();
    let config = config(&q, "[discovery]\nmax_items=1\nmax_entries=1\n");
    for (lang, extension, source, pattern) in [
        ("rust", "rs", "fn other() {}\n", "fn target() {}"),
        ("javascript", "js", "other();\n", "target()"),
        ("typescript", "ts", "other();\n", "target()"),
        ("python", "py", "other()\n", "target()"),
        ("go", "go", "package main\nfunc other() {}\n", "target()"),
    ] {
        for i in 0..3 {
            write(q._temp.path(), &format!("{lang}/{i}.{extension}"), source);
        }
        let result = start(
            &q,
            &config,
            &["code", "search", "--pattern", pattern, "--lang", lang, lang],
        );
        assert!(items(&result).is_empty(), "{lang}: {result}");
        assert_eq!(
            result["output"]["result"]["completeness"]["truncation_reasons"],
            serde_json::json!(["max_entries"]),
            "{lang}: {result}"
        );
    }
}

#[test]
fn search_distinguishes_zero_matches_bad_patterns_and_bad_input() {
    live!();
    let q = Queue::new();
    repo(&q._temp.path().join("repo"));
    let config = config(&q, "");
    let none = run(&q, &config, &search("struct $X;", &["repo/src"]));
    assert_eq!(none["state"], "done");
    assert!(items(&none).is_empty());
    assert_eq!(
        none["output"]["result"]["completeness"]["scope_complete"],
        true
    );
    assert_eq!(none["output"]["result"]["errors"], serde_json::json!([]));
    let bad = run(&q, &config, &search("fn $F( {", &["repo/src"]));
    assert_eq!(bad["state"], "failed");
    assert_eq!(
        bad["output"]["result"]["errors"][0]["code"],
        "invalid_pattern"
    );
    // One explicit file is searched on its own.
    let single = run(
        &q,
        &config,
        &search("fn other() { $$$B }", &["repo/src/a.rs"]),
    );
    assert_eq!(matches(&single), [("a.rs".into(), 2, 1)]);
    let python = run(
        &q,
        &config,
        &[
            "code",
            "search",
            "--pattern",
            "def $F(): $$$B",
            "--lang",
            "python",
            "repo",
        ]
        .map(String::from),
    );
    assert_eq!(matches(&python), [("script.py".into(), 1, 1)]);
    for (args, code, error) in [
        (search("x", &["repo/script.py"]), 2, "invalid_input"),
        (
            ["code", "search", "--pattern", "x", "--lang", "cobol"]
                .map(String::from)
                .into(),
            16,
            "unsupported_language",
        ),
        (
            ["code", "search", "--pattern", "x"]
                .map(String::from)
                .into(),
            2,
            "invalid_input",
        ),
        (search("x", &["missing"]), 2, "target_not_found"),
    ] {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let before = q.adapter.list().unwrap().len();
        let response = cli_json(&q, &config, &args, code);
        assert_eq!(response["error"]["code"], error, "{args:?}");
        assert_eq!(q.adapter.list().unwrap().len(), before, "{args:?}");
    }
}

#[test]
fn search_output_is_bounded_and_paged() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    let config = config(&q, "[discovery]\nmax_items=2\n");
    let limited = run(&q, &config, &search("fn target() {}", &["repo"]));
    let completeness = &limited["output"]["result"]["completeness"];
    assert_eq!(completeness["truncated"], true);
    assert_eq!(
        completeness["truncation_reasons"],
        serde_json::json!(["max_items"])
    );
    assert!(items(&limited).len() <= 2);
    let unlimited = self::config(&q, "");
    let full = run(&q, &unlimited, &search("fn target() {}", &["repo"]));
    let id = full["execution_id"].as_str().unwrap();
    let first = cli_json(&q, &unlimited, &["result", id, "--limit", "1"], 0);
    assert_eq!(items(&first).len(), 1);
    let cursor = first["output"]["result"]["page"]["next_cursor"]
        .as_str()
        .unwrap();
    let second = cli_json(
        &q,
        &unlimited,
        &["result", id, "--limit", "1", "--cursor", cursor],
        0,
    );
    assert_eq!(items(&second)[0]["path"], "src/a.rs");
}

#[test]
fn stats_count_languages_and_keep_embedded_ones_apart() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    let config = config(&q, "");
    let stats = run(&q, &config, &["code", "stats", "repo"].map(String::from));
    assert_eq!(stats["command"], "code stats");
    let result = &stats["output"]["result"];
    assert_eq!(result["kind"], "code.stats");
    let languages: Vec<_> = items(&stats)
        .iter()
        .map(|i| i["language"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(languages, ["HTML", "Python", "Rust"]);
    // Hidden sources count; `.git`, the ignored build directory and symlinks do not:
    // a.rs, b.rs, h.rs and broken.rs.
    let rust = &items(&stats)[2];
    assert_eq!(rust["files"], 4);
    let html = &items(&stats)[0];
    assert_eq!(html["embedded"][0]["language"], "JavaScript");
    assert_eq!(html["embedded"][0]["files"], 1);
    let sum = |field: &str| {
        items(&stats)
            .iter()
            .map(|i| i[field].as_u64().unwrap())
            .sum::<u64>()
    };
    assert_eq!(result["summary"]["files"], 6);
    assert_eq!(result["summary"]["code"], sum("code"));
    assert_eq!(result["summary"]["comments"], sum("comments"));
    assert_eq!(result["summary"]["blanks"], sum("blanks"));
    assert_eq!(result["summary"]["scope_complete"], true);
    assert_eq!(result["summary"]["skipped_files"], Value::Null);
    let all = run(
        &q,
        &config,
        &["code", "stats", "repo", "--include-ignored"].map(String::from),
    );
    assert_eq!(items(&all)[2]["files"], 5);
    let single = run(
        &q,
        &config,
        &["code", "stats", "repo/src/a.rs"].map(String::from),
    );
    assert_eq!(items(&single).len(), 1);
    assert_eq!(items(&single)[0]["files"], 1);
    let empty_dir = q._temp.path().join("empty");
    fs::create_dir(&empty_dir).unwrap();
    let empty = run(&q, &config, &["code", "stats", "empty"].map(String::from));
    assert!(items(&empty).is_empty());
    assert_eq!(empty["output"]["result"]["summary"]["files"], 0);
    assert_eq!(
        empty["output"]["result"]["completeness"]["scope_complete"],
        true
    );
}

#[test]
fn stats_report_unreadable_directories_as_partial() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    let locked = root.join("locked");
    write(&root, "locked/x.rs", "fn x() {}\n");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let config = config(&q, "");
    let stats = run(&q, &config, &["code", "stats", "repo"].map(String::from));
    let search = run(&q, &config, &search("fn target() {}", &["repo"]));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    // Running as root would read the directory anyway.
    if nix_is_root() {
        return;
    }
    for response in [&stats, &search] {
        let result = &response["output"]["result"];
        assert_eq!(result["completeness"]["partial"], true);
        assert_eq!(
            result["errors"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["code"] == "permission_denied")
                .count(),
            1
        );
    }
    assert_eq!(
        stats["output"]["result"]["summary"]["scope_complete"],
        false
    );
}

fn nix_is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| o.stdout == b"0\n")
}

#[test]
fn doctor_reports_ast_grep_and_tokei_with_versions() {
    live!();
    let q = Queue::new();
    let config = config(&q, "");
    let report = cli_json(&q, &config, &["doctor", "--scope", "discovery"], 0);
    let checks = report["output"]["checks"].as_array().unwrap();
    for name in ["ast-grep", "tokei"] {
        let check = checks.iter().find(|c| c["name"] == name).unwrap();
        assert_eq!(check["level"], "pass", "{name}");
        assert_eq!(check["tool"]["installed"], true);
        assert_eq!(check["tool"]["feature_available"], true);
        assert!(check["tool"]["version"].as_str().is_some());
    }
}

#[test]
fn human_output_lists_matches_and_language_rows() {
    live!();
    let q = Queue::new();
    repo(&q._temp.path().join("repo"));
    let config = config(&q, "");
    let text = |args: &[&str]| {
        let mut argv = vec!["--format", "human"];
        argv.extend(args);
        argv.extend(["--wait", "30s"]);
        let out = public_cli(&q, &config, &argv);
        assert_eq!(out.status.code(), Some(0));
        String::from_utf8(out.stdout).unwrap()
    };
    let found = text(&[
        "code",
        "search",
        "--pattern",
        "fn other() { $$$B }",
        "--lang",
        "rust",
        "repo/src",
    ]);
    assert!(found.contains("a.rs:2:fn other() { 1 }"), "{found}");
    assert!(!found.contains('\u{1b}'));
    let stats = text(&["code", "stats", "repo/src/a.rs"]);
    assert!(stats.contains("Rust\t1\t"), "{stats}");
}

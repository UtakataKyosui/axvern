//! Public CLI behaviour that is decided before anything is registered: no daemon or tool needed.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn config(dir: &Path, rules: &str) -> PathBuf {
    let path = dir.join("axvern.toml");
    fs::write(&path, format!("schema_version=1\n{rules}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
fn cli(
    dir: &Path,
    config: &Path,
    path_env: Option<&Path>,
    args: &[&str],
) -> (i32, serde_json::Value) {
    let mut command =
        Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"));
    command
        .current_dir(dir)
        .env_remove("AXVERN_CONFIG")
        .arg("--config")
        .arg(config)
        .args(args)
        .stdin(std::process::Stdio::null());
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let out: Output = command.output().unwrap();
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).expect("one JSON document"),
    )
}

#[test]
fn invalid_input_is_refused_with_exit_2_before_registration() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("file.txt"), "x\n").unwrap();
    fs::write(dir.join("bin.dat"), b"\0").unwrap();
    std::os::unix::fs::symlink("loop2", dir.join("loop1")).unwrap();
    std::os::unix::fs::symlink("loop1", dir.join("loop2")).unwrap();
    std::os::unix::fs::symlink("nowhere", dir.join("dangling")).unwrap();
    let config = config(&dir, "");
    for (args, error) in [
        (vec!["fs", "list", "--depth", "0"], "invalid_input"),
        (vec!["fs", "list", "--depth", "33"], "invalid_input"),
        (vec!["fs", "list", "--depth", "-1"], "invalid_input"),
        (vec!["fs", "list", "missing"], "target_not_found"),
        (vec!["fs", "list", "file.txt"], "invalid_target"),
        (vec!["fs", "list", "dangling"], "invalid_target"),
        (vec!["fs", "list", "loop1"], "invalid_target"),
        (vec!["fs", "list", ".git"], "target_not_found"),
        (vec!["fs", "list", "--unknown"], "invalid_input"),
        (
            vec!["fs", "list", "--depth", "1", "--depth", "2"],
            "invalid_input",
        ),
        (vec!["fs", "list", "a", "b"], "invalid_input"),
        (vec!["fs", "list", "--glob", "x"], "invalid_input"),
        (vec!["fs", "find"], "invalid_input"),
        (vec!["fs", "find", ""], "invalid_pattern"),
        (vec!["fs", "find", "("], "invalid_pattern"),
        (vec!["fs", "find", "[", "--glob"], "invalid_pattern"),
        (
            vec![
                "fs",
                "find",
                "x",
                "--type",
                "directory",
                "--extension",
                "rs",
            ],
            "invalid_input",
        ),
        (
            vec!["fs", "find", "x", "--extension", ".rs"],
            "invalid_input",
        ),
        (vec!["fs", "find", "x", "--type", "socket"], "invalid_input"),
        (vec!["fs", "search", "("], "invalid_pattern"),
        (vec!["fs", "search", "a\nb"], "invalid_pattern"),
        (
            vec!["fs", "search", "x", "--context", "21"],
            "invalid_input",
        ),
        (vec!["fs", "search", "x", "--glob", "["], "invalid_pattern"),
        (vec!["fs", "search", "x", "--depth", "1"], "invalid_input"),
        (vec!["fs", "read"], "invalid_input"),
        (vec!["fs", "read", "bin.dat"], "unsupported_encoding"),
        (vec!["fs", "read", "."], "invalid_target"),
        (
            vec!["fs", "read", "file.txt", "--lines", "0"],
            "invalid_input",
        ),
        (
            vec!["fs", "read", "file.txt", "--from-line", "x"],
            "invalid_input",
        ),
        (
            vec!["fs", "read", "file.txt", "--request-id", "k"],
            "invalid_input",
        ),
    ] {
        let (code, json) = cli(&dir, &config, None, &args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(json["error"]["code"], error, "{args:?}");
        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["command"], format!("fs {}", args[1]));
        assert_eq!(json["execution_id"], serde_json::Value::Null);
    }
    // A pattern that looks like an option is accepted after `--`; the invalid regex is still found.
    let (code, json) = cli(&dir, &config, None, &["fs", "search", "--", "-("]);
    assert_eq!(
        (code, &json["error"]["code"]),
        (2, &"invalid_pattern".into())
    );
}

#[test]
fn policy_refuses_unlisted_tools_and_a_missing_tool_is_reported() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.txt"), "x\n").unwrap();
    let ask = config(&dir, "");
    for args in [
        vec!["fs", "list"],
        vec!["fs", "find", "a"],
        vec!["fs", "search", "x"],
        vec!["fs", "read", "a.txt"],
    ] {
        let (code, json) = cli(&dir, &ask, None, &args);
        assert_eq!(
            (code, &json["error"]["code"]),
            (9, &"policy_denied".into()),
            "{args:?}"
        );
    }
    let deny = config(
        &dir,
        "[[policy.rules]]\nid='d'\neffect='deny'\ncommand='fd'\n",
    );
    assert_eq!(cli(&dir, &deny, None, &["fs", "list"]).0, 9);
    let allow = config(
        &dir,
        "[[policy.rules]]\nid='a'\neffect='allow'\ncommand='fd'\n[[policy.rules]]\nid='b'\neffect='allow'\ncommand='bat'\n",
    );
    let empty = dir.join("empty-path");
    fs::create_dir(&empty).unwrap();
    for args in [vec!["fs", "list"], vec!["fs", "read", "a.txt"]] {
        let (code, json) = cli(&dir, &allow, Some(&empty), &args);
        assert_eq!(
            (code, &json["error"]["code"]),
            (5, &"missing_tool".into()),
            "{args:?}"
        );
    }
}

#[test]
fn help_lists_the_discovery_commands() {
    let out =
        Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
            .arg("--help")
            .output()
            .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for command in [
        "fs list",
        "fs find",
        "fs search",
        "fs read",
        "fs analyze",
        "code search",
        "code stats",
    ] {
        assert!(text.contains(command), "{command}");
    }
}

#[test]
fn code_input_is_checked_before_registration() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    fs::write(dir.join("a.py"), "x = 1\n").unwrap();
    let config = config(&dir, "");
    let search = ["code", "search", "--pattern", "x", "--lang", "rust"];
    for (args, code, error) in [
        (vec!["code", "search", "--lang", "rust"], 2, "invalid_input"),
        (vec!["code", "search", "--pattern", "x"], 2, "invalid_input"),
        (
            vec!["code", "search", "--pattern", "", "--lang", "rust"],
            2,
            "invalid_input",
        ),
        (
            vec!["code", "search", "--pattern", "x", "--lang", ""],
            2,
            "invalid_input",
        ),
        (
            vec!["code", "search", "--pattern", "x", "--lang", "cobol"],
            16,
            "unsupported_language",
        ),
        (
            vec!["code", "search", "--pattern", "x", "--lang", "Rust"],
            16,
            "unsupported_language",
        ),
        ([&search[..], &["a.py"]].concat(), 2, "invalid_input"),
        ([&search[..], &["gone"]].concat(), 2, "target_not_found"),
        ([&search[..], &["a", "b"]].concat(), 2, "invalid_input"),
        (
            [&search[..], &["--pattern", "y"]].concat(),
            2,
            "invalid_input",
        ),
        (
            [&search[..], &["--rewrite", "y"]].concat(),
            2,
            "invalid_input",
        ),
        (vec!["code", "stats", "a", "b"], 2, "invalid_input"),
        (vec!["code", "stats", "--lang", "rust"], 2, "invalid_input"),
        (vec!["code", "stats", "--pattern", "x"], 2, "invalid_input"),
        (vec!["code", "stats", "gone"], 2, "target_not_found"),
        (vec!["code", "stats", "--depth", "1"], 2, "invalid_input"),
    ] {
        let (exit, json) = cli(&dir, &config, None, &args);
        assert_eq!(
            (exit, &json["error"]["code"]),
            (code, &error.into()),
            "{args:?}"
        );
        assert_eq!(json["execution_id"], serde_json::Value::Null);
        assert_eq!(json["command"], format!("code {}", args[1]), "{args:?}");
    }
    for args in [
        vec!["code", "frobnicate"],
        vec!["fs", "list", "--lang", "rust"],
        vec!["fs", "search", "x", "--pattern", "y"],
    ] {
        let (exit, json) = cli(&dir, &config, None, &args);
        assert_eq!((exit, &json["error"]["code"]), (2, &"invalid_input".into()));
    }
}

#[test]
fn code_tools_need_policy_and_the_right_build() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    let ask = config(&dir, "");
    let search = vec!["code", "search", "--pattern", "x", "--lang", "rust"];
    for args in [search.clone(), vec!["code", "stats"]] {
        let (code, json) = cli(&dir, &ask, None, &args);
        assert_eq!(
            (code, &json["error"]["code"]),
            (9, &"policy_denied".into()),
            "{args:?}"
        );
    }
    let allow = config(
        &dir,
        "[[policy.rules]]\nid='a'\neffect='allow'\ncommand='ast-grep'\n[[policy.rules]]\nid='b'\neffect='allow'\ncommand='tokei'\n",
    );
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    for args in [search.clone(), vec!["code", "stats"]] {
        let (code, json) = cli(&dir, &allow, Some(&bin), &args);
        assert_eq!(
            (code, &json["error"]["code"]),
            (5, &"missing_tool".into()),
            "{args:?}"
        );
    }
    // A tool this adapter was not verified against is refused before any task is registered.
    let fake = |name: &str, text: &str| {
        fs::write(bin.join(name), format!("#!/bin/sh\nprintf '{text}\\n'\n")).unwrap();
        fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    };
    for (name, text, args, reason) in [
        (
            "tokei",
            "tokei 15.0.0 compiled with serialization support: cbor, yaml",
            vec!["code", "stats"],
            "tokei 15.0.0 was built without JSON output support",
        ),
        (
            "tokei",
            "tokei 14.0.0 compiled with serialization support: json",
            vec!["code", "stats"],
            "tokei version is not supported; supported: >=15.0.0",
        ),
        (
            "ast-grep",
            "ast-grep 0.1.0",
            search.clone(),
            "ast-grep version is not supported; supported: >=0.51.0",
        ),
    ] {
        fake(name, text);
        let (code, json) = cli(&dir, &allow, Some(&bin), &args);
        assert_eq!(
            (code, &json["error"]["code"]),
            (16, &"unsupported_tool_capability".into()),
            "{text}"
        );
        assert!(
            json["error"]["message"].as_str().unwrap().contains(reason),
            "{json}"
        );
        assert_eq!(json["execution_id"], serde_json::Value::Null);
    }
}

#[test]
fn analyze_is_validated_and_refused_before_any_child_is_registered() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::create_dir(dir.join("w")).unwrap();
    fs::write(dir.join("w/a.rs"), "fn a() {}\n").unwrap();
    let store = dir.join("store");
    let execution = format!(
        "[execution]\nstore={}\n",
        serde_json::to_string(&store).unwrap()
    );
    let ask = config(&dir, &execution);
    for (args, error) in [
        (vec!["fs", "analyze"], "invalid_input"),
        (vec!["fs", "analyze", "w", "x"], "invalid_input"),
        (vec!["fs", "analyze", "w", "--depth", "0"], "invalid_input"),
        (vec!["fs", "analyze", "w", "--depth", "33"], "invalid_input"),
        (vec!["fs", "analyze", "w", "--glob"], "invalid_input"),
        (
            vec!["fs", "analyze", "w", "--lang", "rust"],
            "invalid_input",
        ),
        (vec!["fs", "analyze", "gone"], "target_not_found"),
    ] {
        let (code, json) = cli(&dir, &ask, None, &args);
        assert_eq!(
            (code, &json["error"]["code"]),
            (2, &error.into()),
            "{args:?}"
        );
        assert_eq!(json["command"], "fs analyze");
        assert_eq!(json["execution_id"], serde_json::Value::Null);
    }
    // AC-SPEC-45-03: one child refused by Policy (asked, or denied) registers no child at all.
    let allow =
        |tool: &str| format!("[[policy.rules]]\nid='{tool}'\neffect='allow'\ncommand='{tool}'\n");
    for rules in [
        String::new(),
        allow("fd"),
        allow("fd") + "[[policy.rules]]\nid='d'\neffect='deny'\ncommand='tokei'\n",
    ] {
        let refused = config(&dir, &format!("{rules}{execution}"));
        let (code, json) = cli(&dir, &refused, None, &["fs", "analyze", "w"]);
        assert_eq!((code, &json["error"]["code"]), (9, &"policy_denied".into()));
        assert_eq!(json["execution_id"], serde_json::Value::Null);
        assert!(!store.exists(), "nothing may be registered");
    }
    // A missing tool is found before registration too.
    let allowed = config(
        &dir,
        &format!("{}{}{execution}", allow("fd"), allow("tokei")),
    );
    let empty = dir.join("empty-path");
    fs::create_dir(&empty).unwrap();
    let (code, json) = cli(&dir, &allowed, Some(&empty), &["fs", "analyze", "w"]);
    assert_eq!((code, &json["error"]["code"]), (5, &"missing_tool".into()));
    assert!(!store.exists());
}

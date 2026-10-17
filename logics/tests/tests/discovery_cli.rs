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
fn reading_a_fifo_is_rejected_without_waiting_for_a_writer() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let fifo = dir.join("pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let config = config(&dir, "");
    let mut child = Command::new(std::env::var_os("AXVERN_TEST_BIN").unwrap())
        .arg("--config")
        .arg(&config)
        .args(["fs", "read"])
        .arg(&fifo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("fs read blocked waiting for a FIFO writer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"]["code"], "invalid_target");
    assert_eq!(response["execution_id"], serde_json::Value::Null);
}

#[test]
fn read_line_range_accepts_the_last_valid_line_and_rejects_only_real_overflow() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("text"), "one\n").unwrap();
    let config = config(
        &dir,
        "[[policy.rules]]\nid='allow'\neffect='allow'\ncommand='bat'\n",
    );
    let empty = dir.join("bin");
    fs::create_dir(&empty).unwrap();
    for (from, lines, expected) in [
        (i64::MAX.to_string(), "1", 5),
        ((i64::MAX - 1).to_string(), "2", 5),
        (i64::MAX.to_string(), "2", 2),
        ((i64::MAX as u64 + 1).to_string(), "1", 2),
    ] {
        let (code, response) = cli(
            &dir,
            &config,
            Some(&empty),
            &["fs", "read", "text", "--from-line", &from, "--lines", lines],
        );
        assert_eq!(code, expected, "{from}: {lines}: {response}");
        assert_eq!(
            response["error"]["code"],
            if expected == 2 {
                "invalid_input"
            } else {
                "missing_tool"
            }
        );
        assert_eq!(response["execution_id"], serde_json::Value::Null);
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

#[test]
fn all_discovery_adapters_verify_versions_before_registration_like_doctor() {
    use axvern_core::adapters::tools;
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    let store = dir.join("store");
    let ran = dir.join("TARGET_RAN");
    let make_tool = |name: &str, output: &str, exit: i32| {
        fs::write(bin.join(name), format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s\\n' '{output}'; exit {exit}; fi\n: > '{}'\nexit 99\n",
            ran.display()
        )).unwrap();
        fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    };
    for (spec, version, args) in [
        (&tools::FD, "fd 10.5.0", vec!["fs", "list"]),
        (&tools::RG, "ripgrep 15.2.0", vec!["fs", "search", "x"]),
        (&tools::BAT, "bat 0.26.1", vec!["fs", "read", "a.rs"]),
        (
            &tools::AST_GREP,
            "ast-grep 0.51.0",
            vec!["code", "search", "--pattern", "x", "--lang", "rust"],
        ),
        (
            &tools::TOKEI,
            "tokei 15.0.0 compiled with serialization support: json",
            vec!["code", "stats"],
        ),
    ] {
        let name = spec.name;
        let [a, b, c] = spec.min_version;
        let prerelease =
            format!("{name} {a}.{b}.{c}-rc.1 compiled with serialization support: json");
        let config = config(
            &dir,
            &format!(
                "[execution]\nstore={}\n[[policy.rules]]\nid='allow'\neffect='allow'\ncommand='{name}'\n",
                serde_json::to_string(&store).unwrap()
            ),
        );
        for (output, exit) in [
            ("0.0.0", 0),
            ("unrecognized version", 0),
            (version, 1),
            (prerelease.as_str(), 0),
        ] {
            make_tool(name, output, exit);
            let (code, json) = cli(&dir, &config, Some(&bin), &args);
            assert_eq!(
                (code, &json["error"]["code"]),
                (16, &"unsupported_tool_capability".into()),
                "{name}: {output}"
            );
            assert_eq!(json["execution_id"], serde_json::Value::Null);
            assert!(
                !store.exists(),
                "unverified tool must not reach registration"
            );
            assert!(!ran.exists(), "only the read-only version probe may run");
        }
        // Diagnosis and preflight share the same ToolSpec and supported-version decision.
        for (output, expected) in [
            ("0.0.0", "fail"),
            (prerelease.as_str(), "fail"),
            (version, "pass"),
        ] {
            make_tool(name, output, 0);
            let (_, json) = cli(
                &dir,
                &config,
                Some(&bin),
                &["doctor", "--scope", "discovery"],
            );
            let diagnostic = json["output"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
                .unwrap();
            assert_eq!(diagnostic["level"], expected, "{name}: {output}");
            assert_eq!(diagnostic["tool"]["installed"], true);
            assert_eq!(diagnostic["tool"]["feature_available"], expected == "pass");
            assert!(!ran.exists());
        }
        // doctor may create the private store during its storage check.
        if store.exists() {
            fs::remove_dir_all(&store).unwrap();
        }
    }
    let config = config(&dir, "");
    for (version, level, capable) in [("0.1.0", "warning", false), ("v0.23.5", "pass", true)] {
        make_tool("eza", version, 0);
        let (_, json) = cli(
            &dir,
            &config,
            Some(&bin),
            &["doctor", "--scope", "discovery"],
        );
        let eza = json["output"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "eza")
            .unwrap();
        assert_eq!(eza["level"], level);
        assert_eq!(eza["tool"]["feature_available"], capable);
        assert!(!ran.exists());
    }
}

#[test]
fn a_version_probe_timeout_does_not_authorize_an_unverified_tool() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("fd"), "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    fs::set_permissions(bin.join("fd"), fs::Permissions::from_mode(0o755)).unwrap();
    let store = dir.join("store");
    let config = config(
        &dir,
        &format!(
            "[execution]\nstore={}\n[[policy.rules]]\nid='allow'\neffect='allow'\ncommand='fd'\n",
            serde_json::to_string(&store).unwrap()
        ),
    );
    let started = std::time::Instant::now();
    let (code, json) = cli(&dir, &config, Some(&bin), &["fs", "list"]);
    assert_eq!(code, 16);
    assert_eq!(json["error"]["code"], "unsupported_tool_capability");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("probe failed or timed out")
    );
    assert_eq!(json["execution_id"], serde_json::Value::Null);
    assert!(!store.exists());
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

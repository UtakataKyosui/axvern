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
fn help_lists_the_fs_commands() {
    let out =
        Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
            .arg("--help")
            .output()
            .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for command in ["fs list", "fs find", "fs search", "fs read"] {
        assert!(text.contains(command), "{command}");
    }
}

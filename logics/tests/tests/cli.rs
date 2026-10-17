use std::{
    path::PathBuf,
    process::{Command, Output},
};
fn cli(args: &[&str], config: Option<&std::path::Path>) -> Output {
    let binary = std::env::var_os("AXVERN_TEST_BIN")
        .expect("use moon run qualities:test to build and pass CLI binary path");
    let mut command = Command::new(binary);
    command.env_remove("AXVERN_CONFIG");
    // Disable implicit config discovery through a private HOME, never consult real policy.
    let home = tempfile::tempdir().unwrap();
    command
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .args(args);
    if let Some(config) = config {
        command.env("AXVERN_CONFIG", config);
    }
    command.output().unwrap()
}
fn private_config(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("config.toml");
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    (dir, path)
}
#[test]
fn policy_check_reports_all_decisions_without_executing_target() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("SHOULD_NOT_EXIST");
    for effect in ["allow", "ask", "deny"] {
        let (_config_dir, config) = private_config(&format!(
            "schema_version=1\n[[policy.rules]]\nid='rule'\neffect='{effect}'\ncommand='touch'"
        ));
        let result = cli(
            &[
                "policy",
                "check",
                "--format",
                "json",
                "--",
                "touch",
                marker.to_str().unwrap(),
            ],
            Some(&config),
        );
        assert!(result.status.success());
        assert!(result.stderr.is_empty());
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["output"]["effect"], effect);
        assert_eq!(json["output"]["rule_id"], "rule");
        assert!(!marker.exists());
    }
}
#[test]
fn syntax_config_errors_and_target_flags() {
    let (_dir, config) = private_config("schema_version=1");
    let result = cli(
        &[
            "policy", "check", "--format", "json", "--", "git", "--format", "json",
        ],
        Some(&config),
    );
    assert!(result.status.success());
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["output"]["effect"], "ask");
    for args in [
        vec!["policy", "check", "--format", "json"],
        vec!["policy", "check", "--format", "json", "--"],
        vec!["policy", "check", "--bad", "--format", "json", "--", "git"],
    ] {
        let result = cli(&args, Some(&config));
        assert_eq!(result.status.code(), Some(2));
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["error"]["code"], "invalid_input");
    }
    let (_dir, broken) = private_config("schema_version=99\nsecret='DO_NOT_PRINT'");
    let result = cli(
        &["policy", "check", "--format", "json", "--", "git"],
        Some(&broken),
    );
    assert_eq!(result.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("DO_NOT_PRINT"));
    let result = cli(
        &["policy", "check", "--format", "human", "--", "git"],
        Some(&config),
    );
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("Ask"));
}

#[test]
fn policy_check_reports_rewrite_and_evaluates_both_commands() {
    let rules = "[replacement]\nenabled=true\nrules=['ls-to-eza']\non_missing_tool='skip'\n";
    let check = |extra: &str| {
        let (_dir, config) = private_config(&format!("schema_version=1\n{rules}{extra}"));
        let result = cli(
            &["policy", "check", "--format", "json", "--", "ls", "-A"],
            Some(&config),
        );
        assert!(result.status.success());
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()
    };
    let allow_ls = "[[policy.rules]]\nid='a'\neffect='allow'\ncommand='ls'\n";
    let json = check(allow_ls);
    // Without an eza rule the converted command asks, even though `ls` is allowed.
    let rewrite = &json["output"]["rewrite"];
    match rewrite["status"].as_str() {
        Some("rewritten") => {
            assert_eq!(json["output"]["effect"], "ask");
            assert_eq!(rewrite["rule_id"], "ls-to-eza");
            assert_eq!(rewrite["original"]["program"], "ls");
            assert_eq!(rewrite["rewritten"]["program"], "eza");
            let json = check(&format!(
                "{allow_ls}[[policy.rules]]\nid='b'\neffect='deny'\ncommand='eza'\n"
            ));
            assert_eq!(json["output"]["effect"], "deny");
            assert_eq!(json["output"]["rule_id"], "b");
        }
        // eza is not installed: the skip is explicit and the original command is judged.
        status => {
            assert_eq!(status, Some("skipped"));
            assert_eq!(json["output"]["effect"], "allow");
        }
    }
}
#[test]
fn global_config_precedes_environment_and_flags_accept_global_position() {
    let (_dir, config) = private_config("schema_version=1");
    let result = cli(
        &[
            "--config",
            config.to_str().unwrap(),
            "--format",
            "json",
            "policy",
            "check",
            "--",
            "git",
        ],
        Some(std::path::Path::new("/missing/config")),
    );
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["output"]["effect"],
        "ask"
    );
}

#[cfg(unix)]
#[test]
fn relative_config_checks_working_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, config) = private_config("schema_version=1");
    let binary = std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    // Separate child processes avoid mutating cwd across concurrently running tests.
    for path in [config.as_path(), std::path::Path::new("config.toml")] {
        let result = Command::new(&binary)
            .current_dir(dir.path())
            .env_remove("AXVERN_CONFIG")
            .args(["policy", "check", "--config"])
            .arg(path)
            .args(["--format", "json", "--", "git"])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(4));
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["error"]["code"], "permission_denied");
    }
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn policy_output_golden_and_refusal_before_backend_setup() {
    let (_dir, config) = private_config("schema_version=1");
    let json = cli(
        &["policy", "check", "--format", "json", "--", "true"],
        Some(&config),
    );
    assert!(json.status.success());
    assert!(json.stderr.is_empty());
    assert_eq!(
        json.stdout,
        include_bytes!("../fixtures/cli/policy-ask.json")
    );
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/cli/policy-ask.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&json.stdout).unwrap(),
        expected
    );
    assert_eq!(json.stdout.iter().filter(|b| **b == b'\n').count(), 1);
    let human = cli(
        &["policy", "check", "--format", "human", "--", "true"],
        Some(&config),
    );
    assert!(human.status.success());
    assert!(human.stderr.is_empty());
    assert_eq!(
        human.stdout,
        include_bytes!("../fixtures/cli/policy-ask.txt")
    );
    // Backend configuration is deliberately absent. Refusal must happen first.
    let refused = cli(&["run", "--format", "json", "--", "true"], Some(&config));
    assert_eq!(refused.status.code(), Some(9));
    assert!(refused.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&refused.stdout).unwrap()["error"]["code"],
        "policy_denied"
    );
    let doctor = cli(
        &["doctor", "--format", "json"],
        Some(std::path::Path::new("/nonexistent/axvern.toml")),
    );
    assert_eq!(doctor.status.code(), Some(12));
    assert!(doctor.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&doctor.stdout).unwrap()["output"]["checks"][0]
            ["level"],
        "fail"
    );
}

#[test]
fn doctor_tool_and_store_failures_are_structured() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, config) = private_config("schema_version=1");
    let root = config.parent().unwrap();
    let backend = root.join("pueue.yml");
    std::fs::write(&backend, "").unwrap();
    let store = root.join("executions");
    std::fs::write(
        &config,
        format!(
            "schema_version=1\n[execution]\npueue_config={}\nstore={}\n",
            serde_json::to_string(&backend).unwrap(),
            serde_json::to_string(&store).unwrap()
        ),
    )
    .unwrap();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let binary = std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test");
    let diagnose = || {
        let out = Command::new(&binary)
            .current_dir(root)
            .env("PATH", &bin)
            .env_remove("AXVERN_CONFIG")
            .arg("--config")
            .arg(&config)
            .args(["doctor", "--format", "json"])
            .output()
            .unwrap();
        assert!(out.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        (
            out.status.code().unwrap(),
            value["output"]["checks"].as_array().unwrap().clone(),
        )
    };
    let (code, checks) = diagnose();
    assert_eq!(code, 12);
    let version = checks
        .iter()
        .find(|c| c["name"] == "pueue_version")
        .unwrap();
    assert_eq!(version["level"], "fail");
    assert!(version["message"].as_str().unwrap().contains("MissingTool"));
    let tool = bin.join("pueue");
    std::fs::write(&tool, "#!/bin/sh\nprintf 'pueue 0.0.0\\n'\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (code, checks) = diagnose();
    assert_eq!(code, 12);
    assert!(
        checks
            .iter()
            .find(|c| c["name"] == "pueue_version")
            .unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("requires Pueue 4.0.4")
    );
    std::fs::write(&tool, "#!/bin/sh\ncase \"$*\" in *--version*) printf 'pueue 4.0.4\\n';; *) printf '{\"tasks\":{}}\\n';; esac\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    let (code, checks) = diagnose();
    assert_eq!(code, 0); // project tool is optional, runtime checks pass
    assert_eq!(
        checks
            .iter()
            .find(|c| c["name"] == "project_cargo")
            .unwrap()["level"],
        "warning"
    );
    assert!(!checks.iter().any(|c| c["name"] == "project_pnpm"));
    // The first status snapshot is owned, but log retrieval races with ID reuse.
    let execution_store = axvern_core::store::ExecutionStore::open(&store).unwrap();
    let record = axvern_models::ExecutionRecord {
        execution_id: axvern_core::store::ExecutionStore::allocate_id().unwrap(),
        backend: axvern_models::BackendIdentity {
            queue: backend.canonicalize().unwrap().to_str().unwrap().into(),
            task_id: 0,
            created_at: "original".into(),
        },
        registered_at: 1,
        cwd: root.to_str().unwrap().into(),
        rewrite: None,
    };
    execution_store.save_record(&record).unwrap();
    for (created, expected) in [("replacement", "warning"), ("original", "pass")] {
        std::fs::write(&tool, format!(
            "#!/bin/sh\ncase \"$*\" in\n*--version*) printf 'pueue 4.0.4\\n';;\n*status*) printf '%s\\n' '{{\"tasks\":{{\"0\":{{\"id\":0,\"created_at\":\"original\",\"status\":{{\"Queued\":{{}}}}}}}}}}';;\n*log*) printf '%s\\n' '{{\"0\":{{\"task\":{{\"id\":0,\"created_at\":\"{created}\"}},\"output\":\"private log\"}}}}';;\nesac\n"
        )).unwrap();
        let (code, checks) = diagnose();
        assert_eq!(code, 0);
        assert_eq!(
            checks.iter().find(|c| c["name"] == "pueue_logs").unwrap()["level"],
            expected
        );
    }
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (code, checks) = diagnose();
    assert_eq!(code, 12);
    assert_eq!(
        checks
            .iter()
            .find(|c| c["name"] == "execution_store")
            .unwrap()["level"],
        "fail"
    );
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Future-integration scopes report each tool with its own typed capability data.
    std::fs::write(bin.join("rg"), "#!/bin/sh\nprintf 'ripgrep 14.1.0\\n'\n").unwrap();
    std::fs::set_permissions(bin.join("rg"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let scoped = |scope: &str| {
        let out = Command::new(&binary)
            .current_dir(root)
            .env("PATH", &bin)
            .env_remove("AXVERN_CONFIG")
            .arg("--config")
            .arg(&config)
            .args(["doctor", "--scope", scope, "--format", "json"])
            .output()
            .unwrap();
        assert!(out.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        (
            out.status.code().unwrap(),
            value["output"]["checks"].as_array().unwrap().clone(),
        )
    };
    let find = |checks: &[serde_json::Value], name: &str| {
        checks.iter().find(|c| c["name"] == name).cloned()
    };
    let (code, checks) = scoped("discovery");
    assert_eq!(code, 12);
    let missing = find(&checks, "eza").unwrap();
    assert_eq!(missing["level"], "fail");
    assert_eq!(missing["tool"]["installed"], false);
    assert_eq!(missing["tool"]["feature_available"], false);
    assert_eq!(missing["tool"]["supported_versions"], serde_json::json!([]));
    let installed = find(&checks, "rg").unwrap();
    assert_eq!(installed["level"], "fail");
    assert_eq!(installed["tool"]["installed"], true);
    assert_eq!(installed["tool"]["version"], "14.1.0");
    assert_eq!(installed["tool"]["feature_available"], false);
    assert!(
        installed["message"]
            .as_str()
            .unwrap()
            .contains("unsupported tool capability")
    );
    assert!(find(&checks, "hyperfine").is_none());
    let (_, checks) = scoped("performance");
    assert_eq!(find(&checks, "hyperfine").unwrap()["level"], "fail");
    assert!(find(&checks, "eza").is_none());
    let (_, checks) = scoped("all");
    for optional in ["git", "delta", "dust", "procs"] {
        assert_eq!(find(&checks, optional).unwrap()["level"], "warning");
    }
    assert!(find(&checks, "samply").is_some());
    let (code, checks) = scoped("core");
    assert_eq!(code, 0);
    assert!(find(&checks, "eza").is_none());
}

#[test]
fn schema_two_options_policy_cwd_matching_rules_and_invalid_boundaries() {
    let (_dir, config) = private_config(
        "schema_version=1\n[[policy.rules]]\nid='a'\ncommand='true'\neffect='allow'\n[[policy.rules]]\nid='d'\ncommand='true'\neffect='deny'",
    );
    let cwd = config.parent().unwrap().to_str().unwrap();
    let result = cli(
        &["policy", "check", "--cwd", cwd, "--", "true"],
        Some(&config),
    );
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["command"], "policy check");
    assert_eq!(
        value["output"]["matching_rule_ids"],
        serde_json::json!(["a", "d"])
    );
    assert_eq!(value["output"]["rule_id"], "d");
    for args in [
        vec!["run", "--wait", "-1s", "--", "true"],
        vec!["run", "--wait", "24h", "--", "true"],
        vec!["run", "--request-id", "bad/key", "--", "true"],
        vec!["run", "--request-id", "", "--", "true"],
        vec!["run", "--wait", "0s", "--wait", "0s", "--", "true"],
        vec!["doctor", "--scope", "unknown"],
        vec!["history", "--scope", "core"],
        vec![
            "policy",
            "check",
            "--cwd",
            "/axvern-nonexistent-cwd",
            "--",
            "true",
        ],
    ] {
        let result = cli(&args, Some(&config));
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn result_max_bytes_accepts_only_plain_digits_within_the_wire_budget() {
    let (_dir, config) = private_config("schema_version=1");
    let id = "ex_550e8400-e29b-41d4-a716-446655440000";
    let code_for = |value: &str| {
        let out = cli(&["result", id, "--max-bytes", value], Some(&config));
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        value["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    for rejected in [
        "", "+1024", "1024x", "0x400", " 1024", "1023", "65537", "-1024",
    ] {
        assert_eq!(code_for(rejected), "invalid_input", "{rejected:?}");
    }
    for accepted in ["1024", "65536"] {
        assert_ne!(code_for(accepted), "invalid_input", "{accepted:?}");
    }
}

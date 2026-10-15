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
    let result = cli(&["policy", "check", "--", "git"], Some(&config));
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("Ask"));
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
    let human = cli(&["policy", "check", "--", "true"], Some(&config));
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
}

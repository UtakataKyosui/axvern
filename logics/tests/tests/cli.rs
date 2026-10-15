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

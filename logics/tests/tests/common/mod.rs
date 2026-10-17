//! Isolated Pueue daemon and public-CLI helpers shared by the live tests.
#![allow(dead_code)]
use axvern_core::adapters::pueue::PueueAdapter;
use axvern_models::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
pub struct Queue {
    pub daemon: Child,
    pub adapter: PueueAdapter,
    pub _temp: tempfile::TempDir,
}
impl Queue {
    pub fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("av-")
            .tempdir_in(fs::canonicalize("/tmp").unwrap())
            .unwrap();
        let root = temp.path();
        let config = root.join("config.yml");
        let mut yaml = "shared:\n".to_owned();
        for (key, file) in [
            ("pueue_directory", "data"),
            ("runtime_directory", "runtime"),
            ("unix_socket_path", "socket"),
            ("pid_path", "pid"),
            ("alias_file", "aliases.yml"),
        ] {
            yaml += &format!(
                "  {key}: {}\n",
                serde_json::to_string(&root.join(file)).unwrap()
            );
        }
        yaml += "  use_unix_socket: true\ndaemon:\n  shell_command: [sh, '-c', '{{ pueue_command_string }}']\n";
        fs::write(&config, yaml).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let daemon = Command::new("pueued")
            .arg("--config")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pueued 4.0.4 required");
        let adapter = PueueAdapter {
            executable: "pueue".into(),
            config,
            queue: "test-private".into(),
            timeout: Duration::from_secs(5),
        };
        let mut queue = Self {
            daemon,
            adapter,
            _temp: temp,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while queue.adapter.check().is_err() {
            assert!(
                queue.daemon.try_wait().unwrap().is_none(),
                "daemon start failed"
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(25));
        }
        queue
    }
    pub fn client(&self, args: &[&str]) {
        assert!(
            Command::new("pueue")
                .arg("--config")
                .arg(&self.adapter.config)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
    pub fn plan(&self, code: &str, args: &[&str]) -> ExecutionPlan {
        let mut argv = vec!["-c".into(), code.into()];
        argv.extend(args.iter().map(|v| (*v).into()));
        ExecutionPlan {
            program: "python3".into(),
            args: argv,
            cwd: self._temp.path().into(),
        }
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        let _ = Command::new("pueue")
            .arg("--config")
            .arg(&self.adapter.config)
            .arg("shutdown")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

pub fn public_cli(q: &Queue, config: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
        .current_dir(q._temp.path())
        .env_remove("AXVERN_CONFIG")
        .arg("--config")
        .arg(config)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
pub fn cli_json(
    q: &Queue,
    config: &std::path::Path,
    args: &[&str],
    code: i32,
) -> serde_json::Value {
    let out = public_cli(q, config, args);
    assert_eq!(
        out.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty(), "JSON diagnostics leaked to stderr");
    serde_json::from_slice(&out.stdout).expect("one JSON response, no diagnostics")
}

/// Local discovery suites may skip absent optional development tools; the v0.2 CI
/// gate requires them so a green job cannot mean that all live coverage was skipped.
pub fn discovery_tools(names: &[&str]) -> bool {
    let missing: Vec<_> = names
        .iter()
        .filter(|name| {
            !std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| {
                    dir.join(name)
                        .metadata()
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                })
            })
        })
        .copied()
        .collect();
    if missing.is_empty() {
        return true;
    }
    assert!(
        std::env::var_os("AXVERN_REQUIRE_DISCOVERY_TOOLS").is_none(),
        "v0.2 live gate requires tools: {}",
        missing.join(", ")
    );
    eprintln!("skipped: discovery tools missing: {}", missing.join(", "));
    false
}

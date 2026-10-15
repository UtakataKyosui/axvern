use axvern_core::{
    config::{self, Config},
    policy,
};
use axvern_models::*;
use std::{ffi::OsString, path::PathBuf};
fn plan(program: &str, args: &[&str]) -> ExecutionPlan {
    ExecutionPlan {
        program: program.into(),
        args: args.iter().map(OsString::from).collect(),
        cwd: PathBuf::from("/work"),
    }
}
fn config(rules: &str) -> Config {
    Config::parse(&format!("schema_version=1\n{rules}")).unwrap()
}
#[test]
fn precedence_prefix_and_exact_command() {
    let c = config(
        "[[policy.rules]]\nid='allow'\neffect='allow'\ncommand='git'\nargs_prefix=['status']\n[[policy.rules]]\nid='ask'\neffect='ask'\ncommand='git'\nargs_prefix=['status']\n[[policy.rules]]\nid='deny'\neffect='deny'\ncommand='git'\nargs_prefix=['status', '--secret']",
    );
    assert_eq!(
        policy::evaluate(&c, &plan("git", &["status"]))
            .unwrap()
            .rule_id
            .as_deref(),
        Some("ask")
    );
    assert_eq!(
        policy::evaluate(&c, &plan("git", &["status", "--secret"]))
            .unwrap()
            .effect,
        PolicyEffect::Deny
    );
    for p in [
        plan("git", &["status-extra"]),
        plan("/usr/bin/git", &["status"]),
        plan("git-extra", &["status"]),
    ] {
        assert_eq!(policy::evaluate(&c, &p).unwrap().rule_id, None);
    }
}
#[test]
fn shells_require_explicit_rule_even_with_combined_flags() {
    for shell in ["sh", "/bin/bash", "zsh", "dash", "pwsh"] {
        let c = config(&format!(
            "[[policy.rules]]\nid='broad'\neffect='allow'\ncommand='{shell}'"
        ));
        for args in [&["-c", "true"][..], &["-lc", "true"], &["script.sh"], &[]] {
            assert_eq!(
                policy::evaluate(&c, &plan(shell, args)).unwrap().effect,
                PolicyEffect::Deny
            );
        }
    }
    let c = config(
        "[[policy.rules]]\nid='explicit'\neffect='allow'\ncommand='/bin/sh'\nargs_prefix=['-c','true']\nallow_shell=true",
    );
    assert_eq!(
        policy::evaluate(&c, &plan("/bin/sh", &["-c", "true"]))
            .unwrap()
            .effect,
        PolicyEffect::Allow
    );
}
#[derive(Default)]
struct Fake {
    registrations: usize,
}
impl Adapter for Fake {
    fn register(&mut self, _: &ExecutionPlan) -> Result<u64, Error> {
        self.registrations += 1;
        Ok(7)
    }
    fn state(&mut self, _: u64) -> Result<ChildState, Error> {
        unreachable!()
    }
    fn output(&mut self, _: u64, _: usize) -> Result<Vec<u8>, Error> {
        unreachable!()
    }
}
#[test]
fn refused_approval_never_calls_adapter() {
    let p = plan("git", &["status"]);
    let mut adapter = Fake::default();
    for interactive in [false, true] {
        for approval in [
            Approval::NotProvided,
            Approval::Rejected,
            Approval::Approved,
        ] {
            if interactive && approval == Approval::Approved {
                continue;
            }
            assert_eq!(
                policy::register(&Config::default(), &p, interactive, approval, &mut adapter)
                    .unwrap_err()
                    .code,
                ErrorCode::PolicyDenied
            );
        }
    }
    let deny = config("[[policy.rules]]\nid='deny'\neffect='deny'\ncommand='git'");
    assert!(policy::register(&deny, &p, true, Approval::Approved, &mut adapter).is_err());
    let invalid = Config {
        schema_version: 2,
        ..Config::default()
    };
    assert!(policy::register(&invalid, &p, true, Approval::Approved, &mut adapter).is_err());
    assert_eq!(adapter.registrations, 0);
    assert_eq!(
        policy::register(
            &Config::default(),
            &p,
            true,
            Approval::Approved,
            &mut adapter
        )
        .unwrap(),
        7
    );
    assert_eq!(adapter.registrations, 1);
}
#[test]
fn invalid_configs_fail_closed() {
    for text in [
        "",
        "schema_version=2",
        "schema_version=1\nunknown=true",
        "schema_version=1\n[policy]\nunknown=true",
        "schema_version=1\n[policy]\ndefault='allow'",
        "schema_version=1\n[replacement]\nenabled=true",
        "schema_version=1\n[[policy.rules]]\nid='x'\neffect='allow'\ncommand='sh'\nallow_shell=true",
        "schema_version=1\n[[policy.rules]]\nid='x'\neffect='allow'\ncommand='git'\n[[policy.rules]]\nid='x'\neffect='deny'\ncommand='git'",
    ] {
        assert_eq!(
            Config::parse(text).unwrap_err().code,
            ErrorCode::InvalidConfig,
            "{text}"
        );
    }
}
#[test]
fn config_selection_and_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let p = dir.join("config.toml");
    assert_eq!(
        config::load(None, None, &p).unwrap().policy.default,
        PolicyEffect::Ask
    );
    assert!(config::load(Some(&p), None, &p).is_err());
    assert!(config::load(None, Some("".into()), &p).is_err());
    std::fs::write(&p, "schema_version=1").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(config::load(Some(&p), Some("/missing".into()), &p).is_ok());
    assert!(config::load(None, Some(p.clone().into_os_string()), &dir.join("missing")).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let link = dir.join("link");
        symlink(&p, &link).unwrap();
        assert!(config::load(Some(&link), None, &p).is_err());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            config::load(Some(&p), None, &p).unwrap_err().code,
            ErrorCode::PermissionDenied
        );
    }
}
#[test]
fn execution_id_and_json_contract() {
    let id: ExecutionId = "ex_550e8400-e29b-41d4-a716-446655440000".parse().unwrap();
    for s in [
        "axv_550e8400-e29b-41d4-a716-446655440000",
        "ex_550e8400-e29b-11d4-a716-446655440000",
        "ex_550E8400-e29b-41d4-a716-446655440000",
    ] {
        assert!(s.parse::<ExecutionId>().is_err());
    }
    let mut response = Response::success(PolicyDecision {
        effect: PolicyEffect::Ask,
        reason: "approval required".into(),
        rule_id: None,
    });
    response.execution_id = Some(id);
    let json = serde_json::to_value(response).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["output"]["effect"], "ask");
    for key in ["state", "exit_code", "error"] {
        assert!(json[key].is_null());
    }
}
#[cfg(unix)]
#[test]
fn non_utf8_and_nul_are_rejected_without_lossy_conversion() {
    use std::os::unix::ffi::OsStringExt;
    for arg in [OsString::from_vec(vec![255]), OsString::from("a\0b")] {
        let mut p = plan("git", &[]);
        p.args.push(arg.clone());
        assert_eq!(
            policy::evaluate(&Config::default(), &p).unwrap_err().code,
            ErrorCode::InvalidEncoding
        );
        assert_eq!(p.args[0], arg);
    }
}

#[test]
fn shell_ask_still_overrides_explicit_allow_and_argv_is_not_shell_text() {
    let c = config(
        "[[policy.rules]]\nid='explicit'\neffect='allow'\ncommand='sh'\nargs_prefix=['-c']\nallow_shell=true\n[[policy.rules]]\nid='ask'\neffect='ask'\ncommand='sh'",
    );
    assert_eq!(
        policy::evaluate(&c, &plan("sh", &["-c", "true"]))
            .unwrap()
            .rule_id
            .as_deref(),
        Some("ask")
    );
    let c = config(
        "[[policy.rules]]\nid='literal'\neffect='allow'\ncommand='printf'\nargs_prefix=['; $(false)']",
    );
    assert_eq!(
        policy::evaluate(&c, &plan("printf", &["; $(false)"]))
            .unwrap()
            .effect,
        PolicyEffect::Allow
    );
    assert_eq!(
        policy::evaluate(&c, &plan("printf", &[";", "$(false)"]))
            .unwrap()
            .effect,
        PolicyEffect::Ask
    );
    for wrapper in [
        plan("env", &["sh", "-c", "true"]),
        plan("sudo", &["bash", "-c", "true"]),
    ] {
        assert_eq!(
            policy::evaluate(&Config::default(), &wrapper)
                .unwrap()
                .effect,
            PolicyEffect::Ask
        );
    }
}

#[test]
fn config_encoding_limits_and_unreadable_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("config.toml");
    std::fs::write(&path, [255]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(
        config::load(Some(&path), None, &path).unwrap_err().code,
        ErrorCode::InvalidConfig
    );
    std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        config::load(Some(&path), None, &path).unwrap_err().code,
        ErrorCode::InvalidConfig
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(
            config::load(Some(&path), None, &path).unwrap_err().code,
            ErrorCode::PermissionDenied
        );
    }
}

#[test]
fn shell_explicit_deny_retains_rule_identity() {
    for command in ["sh", "/bin/bash"] {
        let c = config(&format!(
            "[[policy.rules]]\nid='deny-shell'\neffect='deny'\ncommand='{command}'"
        ));
        let decision = policy::evaluate(&c, &plan(command, &["-c", "true"])).unwrap();
        assert_eq!(decision.effect, PolicyEffect::Deny);
        assert_eq!(decision.rule_id.as_deref(), Some("deny-shell"));
        assert_eq!(decision.reason, "matched rule");
    }
}

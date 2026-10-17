use axvern_core::{
    config::{Config, MissingTool},
    doctor, planner,
    rewrite::{self, Rewrite},
    store::ExecutionStore,
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
fn config(text: &str) -> Config {
    Config::parse(&format!("schema_version=1\n{text}")).unwrap()
}
const ALL_RULES: &str =
    "[replacement]\nenabled=true\nrules=['ls-to-eza','cat-to-bat','find-to-fd']";
fn enabled() -> Config {
    config(ALL_RULES)
}
fn rule_policy(effects: &[(&str, &str)]) -> String {
    let mut text = String::from(ALL_RULES);
    for (command, effect) in effects {
        text.push_str(&format!(
            "\n[[policy.rules]]\nid='r-{command}'\neffect='{effect}'\ncommand='{command}'"
        ));
    }
    text
}
fn args(plan: &ExecutionPlan) -> Vec<&str> {
    plan.args.iter().map(|a| a.to_str().unwrap()).collect()
}
fn installed(_: &str) -> bool {
    true
}
fn run(config: &Config, p: &ExecutionPlan) -> (ExecutionPlan, Rewrite) {
    rewrite::rewrite(config, p, installed).unwrap()
}
#[derive(Default)]
struct Fake {
    registered: Vec<ExecutionPlan>,
}
impl Adapter for Fake {
    fn register(&mut self, plan: &ExecutionPlan) -> Result<u64, Error> {
        self.registered.push(plan.clone());
        Ok(1)
    }
    fn state(&mut self, _: u64) -> Result<ChildState, Error> {
        unreachable!()
    }
    fn output(&mut self, _: u64, _: usize) -> Result<Vec<u8>, Error> {
        unreachable!()
    }
}

/// AC-16
#[test]
fn rewriting_is_off_by_default() {
    for c in [
        Config::default(),
        config("[replacement]\nrules=['ls-to-eza']"),
    ] {
        for p in [
            plan("ls", &[]),
            plan("cat", &["a.txt"]),
            plan("find", &[".", "-name", "*.rs"]),
        ] {
            let (executable, outcome) = run(&c, &p);
            assert_eq!(executable, p);
            assert_eq!(outcome, Rewrite::Unchanged);
        }
    }
}

/// AC-17
#[test]
fn only_listed_rules_rewrite_verified_forms() {
    let c = enabled();
    let cases: [(ExecutionPlan, &str, &str, Vec<&str>); 6] = [
        (plan("ls", &[]), "ls-to-eza", "eza", vec!["--color=never"]),
        (
            plan("ls", &["-A", "src"]),
            "ls-to-eza",
            "eza",
            vec!["--color=never", "--all", "--", "src"],
        ),
        (
            plan("cat", &["a.txt"]),
            "cat-to-bat",
            "bat",
            vec![
                "--paging=never",
                "--style=plain",
                "--color=never",
                "--",
                "a.txt",
            ],
        ),
        (
            plan("find", &[".", "-name", "*.rs"]),
            "find-to-fd",
            "fd",
            vec![
                "--hidden",
                "--no-ignore",
                "--case-sensitive",
                "--color=never",
                "--glob",
                "--",
                "*.rs",
                ".",
            ],
        ),
        (
            plan("find", &["src", "-type", "f", "-name", "m*"]),
            "find-to-fd",
            "fd",
            vec![
                "--hidden",
                "--no-ignore",
                "--case-sensitive",
                "--color=never",
                "--glob",
                "--type",
                "f",
                "--",
                "m*",
                "src",
            ],
        ),
        (
            plan("find", &["src", "-name", "m*", "-type", "d"]),
            "find-to-fd",
            "fd",
            vec![
                "--hidden",
                "--no-ignore",
                "--case-sensitive",
                "--color=never",
                "--glob",
                "--type",
                "d",
                "--",
                "m*",
                "src",
            ],
        ),
    ];
    for (original, id, target, expected) in cases {
        let (executable, outcome) = run(&c, &original);
        assert_eq!(executable.program, target);
        assert_eq!(args(&executable), expected);
        assert_eq!(executable.cwd, original.cwd);
        let Rewrite::Rewritten(record) = outcome else {
            panic!("expected a rewrite for {original:?}");
        };
        assert_eq!(record.rule_id, id);
        assert_eq!(
            record.original,
            JsonExecutionPlan::try_from(&original).unwrap()
        );
        assert_eq!(
            record.rewritten,
            JsonExecutionPlan::try_from(&executable).unwrap()
        );
    }
    // A rule that is not listed does not apply even though replacement is enabled.
    let only_ls = config("[replacement]\nenabled=true\nrules=['ls-to-eza']");
    assert_eq!(run(&only_ls, &plan("cat", &["a"])).1, Rewrite::Unchanged);
    // Only the bare program name matches; a path is a different program.
    assert_eq!(run(&enabled(), &plan("/bin/ls", &[])).1, Rewrite::Unchanged);
}

/// AC-18
#[test]
fn unknown_find_arguments_are_not_converted() {
    let c = enabled();
    let unknown: [&[&str]; 14] = [
        &[],
        &["."],
        &[".", "-iname", "*.rs"],
        &[".", "-name", "*.rs", "-mtime", "-1"],
        &[".", "-name", "*.rs", "-delete"],
        &[".", "-name", "*.rs", "-exec", "rm", "{}", ";"],
        &[".", "-name", "*.rs", "-o", "-name", "*.md"],
        &[".", "-name", "*.rs", "-type", "l"],
        &[".", "-name", "*.rs", "-type"],
        &[".", "-name", "a/b"],
        &[".", "-name", "*.rs", "-name", "*.md"],
        &["-L", ".", "-name", "*.rs"],
        &["!", "-name", "*.rs"],
        &[".", "-maxdepth", "1", "-name", "*.rs"],
    ];
    for a in unknown {
        let p = plan("find", a);
        let (executable, outcome) = run(&c, &p);
        assert_eq!(executable, p, "{a:?}");
        assert_eq!(outcome, Rewrite::Unchanged, "{a:?}");
    }
    for p in [
        plan("ls", &["-l"]),
        plan("ls", &["-a"]),
        plan("ls", &["-A", "a", "b"]),
        plan("cat", &["a", "b"]),
        plan("cat", &["-n", "a"]),
        plan("cat", &[]),
    ] {
        assert_eq!(run(&c, &p).1, Rewrite::Unchanged, "{p:?}");
    }
}

/// AC-25, AC-26
#[test]
fn writers_deleters_network_and_repeaters_are_never_converted() {
    let c = enabled();
    let programs = [
        "rm",
        "mv",
        "cp",
        "sd",
        "sed",
        "tee",
        "dd",
        "curl",
        "wget",
        "ssh",
        "hyperfine",
        "watch",
        "xargs",
        "git",
        "touch",
        "tar",
    ];
    for program in programs {
        let p = plan(program, &["-i", "x", "y"]);
        assert_eq!(run(&c, &p), (p, Rewrite::Unchanged), "{program}");
    }
    // Write-like invocations of source programs fall outside the verified forms.
    for p in [
        plan("find", &[".", "-name", "x", "-delete"]),
        plan("find", &[".", "-name", "x", "-exec", "rm", "{}", "+"]),
        plan("cat", &[">", "out.txt"]),
        plan("ls", &["-R"]),
    ] {
        assert_eq!(run(&c, &p).1, Rewrite::Unchanged, "{p:?}");
    }
    // The catalog itself contains no mutating, network or repeating program.
    let denied = [
        "rm",
        "mv",
        "cp",
        "sd",
        "sed",
        "tee",
        "dd",
        "curl",
        "wget",
        "hyperfine",
        "xargs",
        "watch",
    ];
    for spec in rewrite::RULES {
        assert!(!denied.contains(&spec.source), "{}", spec.id);
        assert!(!denied.contains(&spec.target), "{}", spec.id);
    }
    // Writers still go through Policy unchanged.
    let p = planner::plan(&c, plan("rm", &["x"]), installed).unwrap();
    assert_eq!(planner::evaluate(&c, &p).unwrap().effect, PolicyEffect::Ask);
}

/// AC-19, FR-MOD-004, FR-MOD-007
#[test]
fn both_plans_are_evaluated_and_registration_happens_once() {
    let original = plan("ls", &["-A"]);
    let register = |effects: &[(&str, &str)], interactive, approval| {
        let c = config(&rule_policy(effects));
        let planned = planner::plan(&c, original.clone(), installed).unwrap();
        assert!(planned.record().is_some());
        let mut adapter = Fake::default();
        let result = planner::register(&c, &planned, interactive, approval, &mut adapter);
        (result, adapter.registered)
    };
    // Allow on both: exactly one registration, of the rewritten plan.
    let (result, registered) = register(
        &[("ls", "allow"), ("eza", "allow")],
        false,
        Approval::NotProvided,
    );
    assert_eq!(result.unwrap(), 1);
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].program, "eza");
    // Deny after conversion: nothing is registered.
    for effects in [
        [("ls", "allow"), ("eza", "deny")],
        [("ls", "deny"), ("eza", "allow")],
        [("ls", "allow"), ("eza", "ask")],
        [("ls", "ask"), ("eza", "allow")],
    ] {
        let (result, registered) = register(&effects, false, Approval::NotProvided);
        assert_eq!(
            result.unwrap_err().code,
            ErrorCode::PolicyDenied,
            "{effects:?}"
        );
        assert!(registered.is_empty(), "{effects:?}");
    }
    // Missing rule for the converted program falls back to Ask, so an Allow on the
    // original alone does not authorize the new command.
    let (result, registered) = register(&[("ls", "allow")], false, Approval::NotProvided);
    assert_eq!(result.unwrap_err().code, ErrorCode::PolicyDenied);
    assert!(registered.is_empty());
    // An approved Ask covers the request once.
    let (result, registered) = register(&[("ls", "allow")], true, Approval::Approved);
    assert!(result.is_ok());
    assert_eq!(registered.len(), 1);
    // The reported decision is the stricter of the two.
    let c = config(&rule_policy(&[("ls", "allow"), ("eza", "deny")]));
    let planned = planner::plan(&c, original, installed).unwrap();
    let decision = planner::evaluate(&c, &planned).unwrap();
    assert_eq!(decision.effect, PolicyEffect::Deny);
    assert_eq!(decision.rule_id.as_deref(), Some("r-eza"));
}

/// FR-MOD-006
#[test]
fn missing_target_tool_is_skipped_or_reported() {
    let p = plan("cat", &["a.txt"]);
    let missing = |name: &str| name != "bat";
    let (executable, outcome) = rewrite::rewrite(&enabled(), &p, missing).unwrap();
    assert_eq!(executable, p);
    let Rewrite::Skipped(skip) = outcome else {
        panic!("expected an explicit skip");
    };
    assert_eq!(skip.rule_id, "cat-to-bat");
    assert!(skip.reason.contains("bat"));
    let strict = config(&format!("{ALL_RULES}\non_missing_tool='error'"));
    assert_eq!(strict.replacement.on_missing_tool, MissingTool::Error);
    assert_eq!(
        rewrite::rewrite(&strict, &p, missing).unwrap_err().code,
        ErrorCode::MissingTool
    );
    // Requests that no rule would touch are unaffected by a missing tool.
    let other = plan("git", &["status"]);
    assert_eq!(
        rewrite::rewrite(&strict, &other, |_| false).unwrap().1,
        Rewrite::Unchanged
    );
}

#[test]
fn replacement_config_is_strict() {
    for text in [
        "[replacement]\nenabled=true",
        "[replacement]\nenabled=true\nrules=[]",
        "[replacement]\nenabled=true\nrules=['unknown']",
        "[replacement]\nenabled=true\nrules=['ls-to-eza','ls-to-eza']",
        "[replacement]\nrules=['rm-to-trash']",
        "[replacement]\nenabled=true\nrules=['ls-to-eza']\non_missing_tool='ignore'",
        "[replacement]\nenabled=true\nrules=['ls-to-eza']\nextra=1",
    ] {
        assert_eq!(
            Config::parse(&format!("schema_version=1\n{text}"))
                .unwrap_err()
                .code,
            ErrorCode::InvalidConfig,
            "{text}"
        );
    }
    let c = config("[replacement]\nenabled=true\nrules=['ls-to-eza']");
    assert_eq!(c.replacement.on_missing_tool, MissingTool::Skip);
    assert!(!Config::default().replacement.enabled);
}

/// FR-MOD-005
#[test]
fn execution_record_keeps_original_command_and_rule_id() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        ExecutionStore::open(temp.path().canonicalize().unwrap().join("executions")).unwrap();
    let c = enabled();
    let planned = planner::plan(&c, plan("cat", &["a.txt"]), installed).unwrap();
    let id = ExecutionStore::allocate_id().unwrap();
    let identity = BackendIdentity {
        queue: "q".into(),
        task_id: 1,
        created_at: "t".into(),
    };
    let request = store
        .register_intent_rewritten(
            &id,
            &planned.original,
            planned.record().cloned(),
            || Ok(()),
            |_| Ok(identity),
        )
        .unwrap();
    let saved = store.find_record(&id).unwrap();
    assert_eq!(request.record.as_ref(), Some(&saved));
    let rewrite = saved.rewrite.expect("rewrite metadata");
    assert_eq!(rewrite.rule_id, "cat-to-bat");
    assert_eq!(rewrite.original.program, "cat");
    assert_eq!(rewrite.rewritten.program, "bat");
    // The request identity remains the original command.
    assert_eq!(request.plan, rewrite.original);
    // Unrewritten records carry no rewrite field at all.
    let plain = ExecutionStore::allocate_id().unwrap();
    store
        .register_intent(
            &plain,
            &plan("git", &["status"]),
            || Ok(()),
            |_| {
                Ok(BackendIdentity {
                    queue: "q".into(),
                    task_id: 2,
                    created_at: "t".into(),
                })
            },
        )
        .unwrap();
    let json = serde_json::to_value(store.find_record(&plain).unwrap()).unwrap();
    assert!(json.get("rewrite").is_none());
}

/// FR-MOD-008
#[test]
fn doctor_reports_rules_and_their_dependencies() {
    let cwd = std::env::temp_dir();
    let names = |c: Config| -> Vec<(String, CheckLevel)> {
        doctor::diagnose(Ok(c), &cwd)
            .checks
            .into_iter()
            .filter(|d| d.name.starts_with("replacement_"))
            .map(|d| (d.name, d.level))
            .collect()
    };
    assert_eq!(
        names(Config::default()),
        vec![("replacement_rules".to_string(), CheckLevel::Pass)]
    );
    let c = config("[replacement]\nenabled=true\nrules=['ls-to-eza','find-to-fd']");
    let report = doctor::diagnose(Ok(c), &cwd);
    let checks: Vec<_> = report
        .checks
        .iter()
        .filter(|d| d.name.starts_with("replacement_"))
        .collect();
    assert_eq!(
        checks.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["replacement_ls-to-eza", "replacement_find-to-fd"]
    );
    for check in checks {
        let tool = check.tool.as_ref().expect("tool diagnostic");
        assert_eq!(tool.feature_available, check.level == CheckLevel::Pass);
        // A missing target only degrades (skip mode); it never fails the report.
        assert_ne!(check.level, CheckLevel::Fail);
    }
}

#[test]
fn find_rewrite_preserves_case_sensitive_name_matches() {
    use std::process::Command;
    if Command::new("fd").arg("--version").output().is_err() {
        eprintln!("skipped: fd is required for live rewrite comparison");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Distinct basenames also coexist on case-insensitive filesystems (macOS CI).
    // The uppercase .RS extension must still be excluded by case-sensitive matching.
    for name in ["main.rs", "other.RS", "UPPER.rs", "other.txt"] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    let mut original = plan("find", &[".", "-name", "*.rs"]);
    original.cwd = dir.path().to_path_buf();
    let (rewritten, _) = run(&enabled(), &original);
    let paths = |p: &ExecutionPlan| {
        let out = Command::new(&p.program)
            .args(&p.args)
            .current_dir(&p.cwd)
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        let mut names: Vec<_> = text
            .lines()
            .map(|p| p.trim_start_matches("./").to_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(paths(&original), ["UPPER.rs", "main.rs"]);
    assert_eq!(paths(&rewritten), paths(&original));
}

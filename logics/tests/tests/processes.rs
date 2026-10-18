//! `system processes` reads the process table in-process: no daemon or external CLI is needed.
use axvern_core::{adapters::tools, discovery};
use axvern_models::*;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn limits() -> DiscoveryLimits {
    DiscoveryLimits {
        max_items: 1000,
        max_entries: 1_000_000,
        timeout_ms: 60000,
    }
}
fn request(matching: Option<&str>, limit: u32) -> DiscoveryRequest {
    DiscoveryRequest::Processes {
        matching: matching.map(str::to_owned),
        limit,
    }
}
fn prepare(request: &DiscoveryRequest) -> Result<discovery::Prepared, Error> {
    discovery::prepare(request, Path::new("/"), &limits())
}
fn run(request: DiscoveryRequest) -> DiscoveryOutcome {
    tools::adapter(&prepare(&request).unwrap().job)
        .run_native(Instant::now() + Duration::from_secs(60))
        .expect("system processes runs in the worker")
}
fn rows(outcome: &DiscoveryOutcome) -> Vec<&ProcessItem> {
    outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Process(p) => p,
            other => panic!("unexpected item {other:?}"),
        })
        .collect()
}
/// A child (`sh`) with a secret argument and a secret environment.
fn sleeper() -> Child {
    Command::new("sh")
        .args(["-c", "while :; do sleep 1; done", "ARGV_SECRET_MARKER"])
        .env("ENV_SECRET_MARKER", "x")
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn a_snapshot_lists_processes_in_pid_order_with_an_observation_time() {
    let outcome = run(request(None, 1000));
    assert_eq!(outcome.state, ChildState::Done);
    let rows = rows(&outcome);
    assert!(!rows.is_empty());
    assert!(rows.windows(2).all(|w| w[0].pid < w[1].pid));
    let own = rows
        .iter()
        .find(|r| r.pid == std::process::id())
        .expect("the test process itself");
    assert!(own.ppid.is_some() && own.uid.is_some() && !own.name.is_empty());
    #[cfg(target_os = "linux")]
    {
        assert!(own.state.is_some());
        assert!(own.rss_bytes.is_some_and(|b| b > 0));
        assert!(own.started_at.as_deref().is_some_and(|t| t.ends_with('Z')));
    }
    let DiscoverySummary::Processes {
        observed_at, limit, ..
    } = &outcome.document.summary
    else {
        panic!("summary")
    };
    assert_eq!(*limit, 1000);
    assert_eq!(observed_at.len(), 20);
    assert!(observed_at.starts_with("20") && observed_at.ends_with('Z'));
}

#[test]
fn match_is_a_case_sensitive_literal_on_the_executable_name_only() {
    let mut child = sleeper();
    let found = run(request(Some("sh"), 1000));
    let pids: Vec<u32> = rows(&found).iter().map(|r| r.pid).collect();
    assert!(pids.contains(&child.id()), "{pids:?}");
    assert!(rows(&found).iter().all(|r| r.name.contains("sh")));
    // Not case-insensitive, and not matched against the argument list.
    for text in ["SH", "ARGV_SECRET_MARKER", "while"] {
        let outcome = run(request(Some(text), 1000));
        assert!(
            !rows(&outcome).iter().any(|r| r.pid == child.id()),
            "{text}"
        );
    }
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn zero_matches_is_an_empty_complete_done_result() {
    let outcome = run(request(Some("no-such-process-name-zzz"), 100));
    assert_eq!(outcome.state, ChildState::Done);
    assert!(rows(&outcome).is_empty());
    assert!(outcome.document.completeness.scope_complete);
    assert_eq!(outcome.exit_code, Some(0));
}

#[test]
fn the_limit_cuts_the_list_and_says_so() {
    let mut child = sleeper();
    let outcome = run(request(None, 1));
    assert_eq!(rows(&outcome).len(), 1);
    assert!(outcome.document.completeness.truncated);
    assert!(
        outcome
            .document
            .completeness
            .truncation_reasons
            .contains(&TruncationReason::MaxItems)
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn the_result_never_contains_argv_or_environment_values() {
    let mut child = sleeper();
    let outcome = run(request(None, 1000));
    let json = serde_json::to_string(&outcome.document).unwrap();
    assert!(!json.contains("ARGV_SECRET_MARKER") && !json.contains("ENV_SECRET_MARKER"));
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn input_is_validated_before_anything_is_registered() {
    for (matching, limit) in [
        (None, 0),
        (None, 1001),
        (Some(""), 100),
        (Some("a/b"), 100),
        (Some("nul\0"), 100),
    ] {
        let error = prepare(&request(matching, limit)).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "{matching:?} {limit}");
    }
    let too_long = "x".repeat(MAX_PROCESS_MATCH + 1);
    assert!(prepare(&request(Some(&too_long), 100)).is_err());
    assert!(prepare(&request(Some("cargo"), 1000)).is_ok());
}

#[test]
fn the_match_text_is_not_copied_into_the_registered_plan() {
    let plan = prepare(&request(Some("secret-service"), 5)).unwrap().plan;
    let text = format!("{:?}", plan.args);
    assert!(!text.contains("secret-service"), "{text}");
}

//! `query`/`recall` argument contracts: decided at parse time, before any backend or store access.
use std::process::{Command, Stdio};

const ID: &str = "ex_00000000-0000-4000-8000-000000000000";
fn run(args: &[&str]) -> (i32, serde_json::Value) {
    let out =
        Command::new(std::env::var_os("AXVERN_TEST_BIN").expect("use moon run qualities:test"))
            .env_remove("AXVERN_CONFIG")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
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
fn malformed_query_and_recall_invocations_exit_2() {
    let cases: &[&[&str]] = &[
        &["query", ID],
        &["query", ID, "--match", "a", "--errors"],
        &["query", ID, "--errors", "--tail", "3"],
        &[
            "query",
            ID,
            "--tail",
            "3",
            "--from-line",
            "1",
            "--lines",
            "2",
        ],
        &["query", ID, "--match", ""],
        &["query", ID, "--match", "a", "--match", "b"],
        &["query", ID, "--from-line", "1"],
        &["query", ID, "--lines", "1"],
        &["query", ID, "--tail", "-1"],
        &["query", ID, "--tail", "x"],
        &["query", ID, "--errors", "--limit", "0"],
        &["query", ID, "--errors", "--limit", "1001"],
        &["query", ID, "--errors", "--max-bytes", "1023"],
        &["query", ID, "--errors", "--max-bytes", "65537"],
        &["query", ID, "--tail", "0"],
        &["query", ID, "--tail", "1001"],
        &["query", ID, "--from-line", "0", "--lines", "1"],
        &["query", ID, "--from-line", "1", "--lines", "0"],
        &["query", ID, "--from-line", "1", "--lines", "1001"],
        &["query", "--errors"],
        &["query", "nope", "--errors"],
        &["query", ID, "--errors", "--output", "x"],
        &["query", ID, "--errors", "--depth", "1"],
        &["query", ID, "--errors", "--cursor", "x"],
        &["query", ID, "--errors", "--wait", "1s"],
        &["query", ID, "--errors", "--unknown"],
        &["recall"],
        &["recall", "nope"],
        &["recall", ID, "--output"],
        &["recall", ID, "--output", ""],
        &["recall", ID, "--output", "a", "--output", "b"],
        &["recall", ID, "--match", "a"],
        &["recall", ID, "--limit", "5"],
        &["recall", ID, "--max-bytes", "2048"],
        &["recall", ID, "--tail", "2"],
        &["recall", ID, "extra"],
        // `logs` is not a command; direct users to query --tail / recall.
        &["logs", ID],
    ];
    for args in cases {
        let (code, json) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(json["error"]["code"], "invalid_input", "{args:?}");
        assert!(json["output"].is_null(), "{args:?}");
    }
}
#[test]
fn usage_documents_the_retrieve_commands() {
    let out = Command::new(std::env::var_os("AXVERN_TEST_BIN").unwrap())
        .arg("--help")
        .output()
        .unwrap();
    let usage = String::from_utf8(out.stdout).unwrap();
    assert!(usage.contains("query <execution-id>"));
    assert!(usage.contains("recall <execution-id> [--output FILE]"));
    assert!(!usage.contains("axvern logs"));
}

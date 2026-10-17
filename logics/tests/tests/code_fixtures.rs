//! `code search` and `code stats` adapter contracts against recorded ast-grep 0.51.0 and
//! tokei 15.0.0 output; no external CLI is needed.
use axvern_core::adapters::tools::{self, Flow, Stop, TargetExit, ast_grep, tokei};
use axvern_models::*;
use serde_json::{Value, json};
use std::fs;

fn fixture(name: &str) -> Vec<u8> {
    fs::read(format!(
        "{}/fixtures/code/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
fn limits(max_items: u64, max_entries: u64) -> DiscoveryLimits {
    DiscoveryLimits {
        max_items,
        max_entries,
        timeout_ms: 60_000,
    }
}
fn search_job(pattern: &str, max_items: u64) -> DiscoveryJob {
    DiscoveryJob::CodeSearch(CodeSearchJob {
        root: "/work".into(),
        file: None,
        in_repo: true,
        pattern: pattern.into(),
        lang: "rust".into(),
        include_ignored: false,
        limits: limits(max_items, 1000),
    })
}
fn stats_job(max_entries: u64) -> DiscoveryJob {
    DiscoveryJob::CodeStats(CodeStatsJob {
        root: "/work".into(),
        file: None,
        in_repo: true,
        include_ignored: false,
        limits: limits(100, max_entries),
    })
}
fn run(job: &DiscoveryJob, chunks: &[&[u8]], exit: TargetExit, stderr: &[u8]) -> DiscoveryOutcome {
    let mut normalizer = tools::adapter(job).normalizer(1 << 20).unwrap();
    let mut stopped = exit.stopped;
    for chunk in chunks {
        if let Flow::Stop(stop) = normalizer.feed(chunk) {
            stopped = Some(stop);
            break;
        }
    }
    normalizer.finish(&TargetExit { stopped, ..exit }, stderr)
}
fn ok(code: i32) -> TargetExit {
    TargetExit {
        code: Some(code),
        ..Default::default()
    }
}
fn args(job: &DiscoveryJob) -> Vec<String> {
    tools::adapter(job)
        .plan()
        .unwrap()
        .args
        .iter()
        .map(|a| a.to_str().unwrap().to_owned())
        .collect()
}
fn code_items(outcome: &DiscoveryOutcome) -> Vec<CodeMatchItem> {
    outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Code(c) => c.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}
fn stat_items(outcome: &DiscoveryOutcome) -> Vec<LanguageStatItem> {
    outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Stat(s) => s.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn search_plan_is_plain_argv_and_the_pattern_cannot_add_a_rule() {
    let hostile =
        "fn a() {}\n---\nid: evil\n\", \"rule\": {\"kind\": \"ERROR\"}, \"x\": \"$(touch x) `id`";
    let plan = tools::adapter(&search_job(hostile, 10)).plan().unwrap();
    assert_eq!(plan.program, "ast-grep");
    assert_eq!(plan.cwd.to_str(), Some("/work"));
    let argv = args(&search_job(hostile, 10));
    let rules = &argv[argv.iter().position(|a| a == "--inline-rules").unwrap() + 1];
    let documents: Vec<Value> = rules
        .split("\n---\n")
        .map(|d| serde_json::from_str(d).unwrap())
        .collect();
    assert_eq!(documents.len(), 3);
    assert_eq!(documents[0]["id"], "axvern-match");
    assert_eq!(documents[0]["rule"], json!({"pattern": hostile}));
    assert_eq!(documents[0].as_object().unwrap().len(), 3);
    assert_eq!(documents[1]["rule"], json!({"kind": "ERROR"}));
    assert_eq!(documents[2]["id"], "axvern-file");
    assert_eq!(documents[2]["rule"], json!({"kind": "source_file"}));
    for flag in [
        "--json=stream",
        "--color",
        "never",
        "--config",
        "/dev/null",
        "--globs",
        "!.git",
    ] {
        assert!(argv.iter().any(|a| a == flag), "{flag}");
    }
    assert!(argv.windows(2).any(|w| w == ["--no-ignore", "hidden"]));
    assert!(!argv.contains(&"--follow".into()));
    assert!(!argv.contains(&"--rewrite".into()));
    // The path comes last, after `--`, so it cannot be read as an option.
    assert_eq!(argv[argv.len() - 2..], ["--", "."]);
    let DiscoveryJob::CodeSearch(mut job) = search_job("x", 1) else {
        unreachable!()
    };
    job.file = Some("-weird.rs".into());
    job.in_repo = false;
    let single = args(&DiscoveryJob::CodeSearch(job.clone()));
    assert_eq!(single[single.len() - 2..], ["--", "-weird.rs"]);
    assert!(single.windows(2).any(|w| w == ["--no-ignore", "parent"]));
    job.include_ignored = true;
    let all = args(&DiscoveryJob::CodeSearch(job));
    assert!(all.windows(2).any(|w| w == ["--no-ignore", "vcs"]));
}

#[test]
fn stats_plan_is_plain_argv() {
    let argv = args(&stats_job(10));
    assert_eq!(
        &argv[..6],
        [
            "--streaming",
            "json",
            "--hidden",
            "--exclude",
            ".git",
            "--no-ignore-dot",
        ]
    );
    assert!(argv.contains(&"--no-ignore".into()));
    assert!(
        argv.windows(2)
            .any(|w| w[0] == "--exclude" && w[1].matches('*').count() == 33)
    );
    assert_eq!(argv[argv.len() - 2..], ["--", "."]);
    let DiscoveryJob::CodeStats(mut job) = stats_job(10) else {
        unreachable!()
    };
    job.in_repo = false;
    job.file = Some("-x.rs".into());
    let outside = args(&DiscoveryJob::CodeStats(job.clone()));
    assert!(outside.contains(&"--no-ignore".into()));
    assert_eq!(outside[outside.len() - 2..], ["--", "-x.rs"]);
    job.include_ignored = true;
    assert!(args(&DiscoveryJob::CodeStats(job)).contains(&"--no-ignore".into()));
}

#[test]
fn matches_use_byte_columns_and_files_with_syntax_errors_are_not_trusted() {
    let stream = fixture("ast-grep-stream.jsonl");
    let outcome = run(&search_job("fn f() {}", 100), &[&stream], ok(0), b"");
    assert_eq!(outcome.state, ChildState::Done);
    // The character column of the match is 12 and its byte column is 14.
    assert_eq!(
        code_items(&outcome),
        [CodeMatchItem {
            path: "a.rs".into(),
            lang: "rust".into(),
            start: CodePosition {
                line: 2,
                byte_column: 15
            },
            end: CodePosition {
                line: 2,
                byte_column: 24
            },
            snippet: "fn f() {}".into(),
            snippet_truncated: false,
        }]
    );
    // broken.rs also matched, but it does not parse: its match is dropped and reported.
    let document = &outcome.document;
    assert_eq!(document.errors.len(), 1);
    assert_eq!(document.errors[0].code, ErrorCode::ParseError);
    assert_eq!(document.errors[0].path.as_deref(), Some("broken.rs"));
    assert!(document.completeness.partial);
    assert!(!document.completeness.scope_complete);
    assert!(matches!(
        &document.summary,
        DiscoverySummary::CodeSearch { lang, .. } if lang == "rust"
    ));
    // The same stream split at arbitrary byte boundaries gives the same result.
    let chunks: Vec<&[u8]> = stream.chunks(7).collect();
    let split = run(&search_job("fn f() {}", 100), &chunks, ok(0), b"");
    assert_eq!(split.document, outcome.document);
}

#[test]
fn a_multi_line_match_ends_after_the_closing_token() {
    let outcome = run(
        &search_job("fn g() { $$$B }", 100),
        &[&fixture("ast-grep-multiline.jsonl")],
        ok(0),
        b"",
    );
    let items = code_items(&outcome);
    assert_eq!(
        items[0].start,
        CodePosition {
            line: 3,
            byte_column: 1
        }
    );
    assert_eq!(
        items[0].end,
        CodePosition {
            line: 5,
            byte_column: 2
        }
    );
    assert_eq!(items[0].snippet, "fn g() {\n    1\n}");
    assert!(outcome.document.completeness.scope_complete);
}

#[test]
fn zero_matches_invalid_pattern_and_failures_are_different_results() {
    let none = run(&search_job("struct $X;", 10), &[], ok(0), b"");
    assert_eq!(none.state, ChildState::Done);
    assert!(none.document.items.is_empty());
    assert!(none.document.errors.is_empty());
    assert!(none.document.completeness.scope_complete);
    let bad = run(
        &search_job("fn $F( {", 10),
        &[],
        ok(8),
        b"Error: Cannot parse rule INLINE_RULES\n  secret-pattern-text\n",
    );
    assert_eq!(bad.state, ChildState::Failed);
    assert_eq!(bad.document.errors[0].code, ErrorCode::InvalidPattern);
    assert!(!bad.document.errors[0].message.contains("secret"));
    let crashed = run(&search_job("x", 10), &[], ok(101), b"");
    assert_eq!(crashed.state, ChildState::Failed);
    assert_eq!(crashed.document.errors[0].code, ErrorCode::BackendFailure);
    let missing = run(
        &search_job("x", 10),
        &[],
        TargetExit {
            spawn_failed: true,
            ..Default::default()
        },
        b"",
    );
    assert_eq!(missing.state, ChildState::Failed);
    assert_eq!(missing.document.errors[0].code, ErrorCode::MissingTool);
}

#[test]
fn unreadable_paths_make_the_search_partial() {
    let outcome = run(
        &search_job("x", 10),
        &[],
        ok(0),
        b"ERROR: ./sub: Permission denied (os error 13)\nERROR: ./gone: No such file or directory (os error 2)\n",
    );
    assert_eq!(outcome.state, ChildState::Done);
    assert_eq!(outcome.document.errors.len(), 1);
    assert_eq!(outcome.document.errors[0].code, ErrorCode::PermissionDenied);
    assert_eq!(outcome.document.errors[0].path.as_deref(), Some("sub"));
    assert_eq!(
        outcome.document.warnings[0].code,
        WarningCode::EntryVanished
    );
    assert!(outcome.document.completeness.partial);
}

#[test]
fn search_is_bounded_by_items_entries_and_snippet_size() {
    let stream = fixture("ast-grep-stream.jsonl");
    let limited = run(&search_job("fn f() {}", 0), &[&stream], ok(0), b"");
    assert_eq!(
        limited.document.completeness.truncation_reasons,
        [TruncationReason::MaxItems]
    );
    assert!(limited.document.items.is_empty());
    assert!(limited.document.completeness.omitted_count.is_none());
    let DiscoveryJob::CodeSearch(mut job) = search_job("x", 10) else {
        unreachable!()
    };
    job.limits.max_entries = 1;
    let entries = run(&DiscoveryJob::CodeSearch(job), &[&stream], ok(0), b"");
    assert_eq!(
        entries.document.completeness.truncation_reasons,
        [TruncationReason::MaxEntries]
    );
    let big = "a".repeat(SNIPPET_BYTES + 10);
    let record = json!({"text": big, "lines": big, "file": "./big.rs", "ruleId": "axvern-match",
        "range": {"start": {"line": 0, "column": 0}, "end": {"line": 0, "column": SNIPPET_BYTES + 10}}});
    let outcome = run(
        &search_job("x", 10),
        &[format!("{record}\n").as_bytes()],
        ok(0),
        b"",
    );
    let item = &code_items(&outcome)[0];
    assert_eq!(item.snippet.len(), SNIPPET_BYTES);
    assert!(item.snippet_truncated);
    assert_eq!(item.path, "big.rs");
    // A budget smaller than one item stops the tool and flags the artifact limit.
    let mut tiny = tools::adapter(&search_job("fn f() {}", 10))
        .normalizer(10)
        .unwrap();
    assert!(matches!(
        tiny.feed(&stream),
        Flow::Stop(Stop::Truncated(TruncationReason::ArtifactBytes))
    ));
}

#[test]
fn supported_languages_and_extensions_are_checked() {
    for name in ["rust", "javascript", "typescript", "python", "go"] {
        assert!(ast_grep::language(name).is_some(), "{name}");
    }
    assert!(ast_grep::language("Rust").is_none());
    assert!(ast_grep::language("tsx").is_none());
    assert!(ast_grep::language("cobol").is_none());
    let rust = ast_grep::language("rust").unwrap();
    assert!(rust.owns("src/main.rs"));
    assert!(!rust.owns("main.py"));
    assert!(!rust.owns("rs"));
    assert!(ast_grep::language("typescript").unwrap().owns("a.mts"));
    assert!(!ast_grep::language("typescript").unwrap().owns("a.tsx"));
}

#[test]
fn files_without_matches_count_toward_the_entry_limit() {
    let DiscoveryJob::CodeSearch(mut job) = search_job("x", 10) else {
        unreachable!()
    };
    job.limits.max_entries = 1;
    let stream = b"{\"ruleId\":\"axvern-file\",\"file\":\"a.rs\"}\n{\"ruleId\":\"axvern-file\",\"file\":\"b.rs\"}\n";
    let outcome = run(&DiscoveryJob::CodeSearch(job), &[stream], ok(0), b"");
    assert!(outcome.document.items.is_empty());
    assert_eq!(
        outcome.document.completeness.truncation_reasons,
        [TruncationReason::MaxEntries]
    );
    assert!(!outcome.document.completeness.scope_complete);
}

#[test]
fn language_items_are_sorted_then_limited_without_changing_scan_totals() {
    let stream = fixture("tokei-stream.jsonl");
    let DiscoveryJob::CodeStats(mut job) = stats_job(100) else {
        unreachable!()
    };
    job.limits.max_items = 1;
    let limited = run(
        &DiscoveryJob::CodeStats(job.clone()),
        &[&stream],
        ok(0),
        b"",
    );
    assert_eq!(stat_items(&limited).len(), 1);
    assert_eq!(stat_items(&limited)[0].language, "HTML");
    assert_eq!(
        limited.document.completeness.truncation_reasons,
        [TruncationReason::MaxItems]
    );
    assert!(matches!(
        limited.document.summary,
        DiscoverySummary::CodeStats {
            files: 3,
            code: 8,
            scope_complete: false,
            ..
        }
    ));
    job.limits.max_items = 2;
    let exact = run(&DiscoveryJob::CodeStats(job), &[&stream], ok(0), b"");
    assert_eq!(stat_items(&exact).len(), 2);
    assert!(exact.document.completeness.scope_complete);
}

#[test]
fn language_stats_keep_embedded_languages_out_of_the_totals() {
    let stream = fixture("tokei-stream.jsonl");
    let outcome = run(&stats_job(100), &[&stream], ok(0), b"");
    assert_eq!(outcome.state, ChildState::Done);
    assert_eq!(
        stat_items(&outcome),
        [
            LanguageStatItem {
                language: "HTML".into(),
                files: 1,
                code: 4,
                comments: 0,
                blanks: 0,
                embedded: vec![EmbeddedStat {
                    language: "JavaScript".into(),
                    files: 1,
                    code: 2,
                    comments: 1,
                    blanks: 0
                }],
            },
            LanguageStatItem {
                language: "Rust".into(),
                files: 2,
                code: 4,
                comments: 1,
                blanks: 1,
                embedded: vec![],
            },
        ]
    );
    // Totals are the sum over host languages; JavaScript's 2 + 1 lines are not added again.
    assert_eq!(
        outcome.document.summary,
        DiscoverySummary::CodeStats {
            root: "/work".into(),
            scan_depth: Some(MAX_DEPTH),
            files: 3,
            code: 8,
            comments: 1,
            blanks: 1,
            scope_complete: true,
            skipped_files: None,
        }
    );
    assert!(outcome.document.completeness.scope_complete);
    let chunks: Vec<&[u8]> = stream.chunks(5).collect();
    assert_eq!(
        run(&stats_job(100), &chunks, ok(0), b"").document,
        outcome.document
    );
}

#[test]
fn an_empty_target_has_zero_totals() {
    let outcome = run(&stats_job(100), &[], ok(0), b"");
    assert!(outcome.document.items.is_empty());
    assert_eq!(
        outcome.document.summary,
        DiscoverySummary::CodeStats {
            root: "/work".into(),
            scan_depth: Some(MAX_DEPTH),
            files: 0,
            code: 0,
            comments: 0,
            blanks: 0,
            scope_complete: true,
            skipped_files: None,
        }
    );
}

#[test]
fn duplicate_stats_records_do_not_double_count_the_same_path() {
    let stream = fixture("tokei-stream.jsonl");
    let once = run(&stats_job(100), &[&stream], ok(0), b"");
    let twice = run(&stats_job(100), &[&stream, &stream], ok(0), b"");
    assert_eq!(once.document, twice.document);
}

#[test]
fn old_saved_statistics_do_not_claim_a_new_depth_bound() {
    let old = json!({"root":"/old", "files":0, "code":0, "comments":0,
        "blanks":0, "scope_complete":true, "skipped_files":null});
    let summary: DiscoverySummary = serde_json::from_value(old).unwrap();
    assert!(matches!(
        summary,
        DiscoverySummary::CodeStats {
            scan_depth: None,
            ..
        }
    ));
}

#[test]
fn stats_failures_and_limits_are_never_presented_as_complete() {
    let stream = fixture("tokei-stream.jsonl");
    let limited = run(&stats_job(2), &[&stream], ok(0), b"");
    assert_eq!(
        limited.document.completeness.truncation_reasons,
        [TruncationReason::MaxEntries]
    );
    let DiscoverySummary::CodeStats {
        files,
        scope_complete,
        ..
    } = limited.document.summary
    else {
        panic!()
    };
    assert_eq!((files, scope_complete), (2, false));
    let denied = run(
        &stats_job(100),
        &[&stream],
        ok(0),
        b"[2026-10-15T23:49:34Z ERROR tokei::utils::fs] Permission denied (os error 13) reading ./sub\n\
          [2026-10-15T23:49:34Z ERROR tokei::utils::fs] Error reading ./unread.rs:\n    Permission denied (os error 13)\n",
    );
    assert_eq!(denied.state, ChildState::Done);
    let paths: Vec<_> = denied
        .document
        .errors
        .iter()
        .map(|e| (e.code, e.path.as_deref()))
        .collect();
    assert_eq!(
        paths,
        [
            (ErrorCode::PermissionDenied, Some("sub")),
            (ErrorCode::PermissionDenied, Some("unread.rs"))
        ]
    );
    let DiscoverySummary::CodeStats { scope_complete, .. } = denied.document.summary else {
        panic!()
    };
    assert!(!scope_complete);
    assert!(denied.document.completeness.partial);
    let failed = run(&stats_job(100), &[], ok(1), b"");
    assert_eq!(failed.state, ChildState::Failed);
    assert_eq!(failed.document.errors[0].code, ErrorCode::BackendFailure);
    let garbage = run(&stats_job(100), &[b"not json\n"], ok(0), b"");
    assert_eq!(garbage.document.errors[0].code, ErrorCode::BackendFailure);
    assert!(garbage.document.completeness.partial);
    let missing = run(
        &stats_job(100),
        &[],
        TargetExit {
            spawn_failed: true,
            ..Default::default()
        },
        b"",
    );
    assert_eq!(missing.document.errors[0].code, ErrorCode::MissingTool);
}

#[test]
fn counts_saturate_and_reject_values_that_are_not_u64() {
    let big = |name: &str| {
        format!(
            "{{\"language\":\"Rust\",\"stats\":{{\"name\":\"{name}\",\"stats\":{{\"blanks\":0,\"blobs\":{{}},\"code\":{},\"comments\":0}}}}}}\n",
            u64::MAX
        )
    };
    let twice = format!("{}{}", big("./a.rs"), big("./b.rs"));
    let outcome = run(&stats_job(100), &[twice.as_bytes()], ok(0), b"");
    assert_eq!(stat_items(&outcome)[0].code, u64::MAX);
    assert_eq!(stat_items(&outcome)[0].files, 2);
    let negative = br#"{"language":"Rust","stats":{"name":"./a.rs","stats":{"blanks":0,"blobs":{},"code":-1,"comments":0}}}
"#;
    let outcome = run(&stats_job(100), &[negative], ok(0), b"");
    assert!(outcome.document.items.is_empty());
    assert_eq!(outcome.document.errors[0].code, ErrorCode::BackendFailure);
    assert!(tokei::parse_record(negative).is_none());
}

#[test]
fn tokei_records_flatten_nested_languages_and_count_a_file_once_per_language() {
    let record = br#"{"language":"Markdown","stats":{"name":"./doc.md","stats":{"blanks":1,"code":2,"comments":0,"blobs":{"HTML":{"blanks":0,"code":1,"comments":0,"blobs":{"JavaScript":{"blanks":0,"code":3,"comments":0,"blobs":{}}}},"JavaScript":{"blanks":0,"code":4,"comments":0,"blobs":{}}}}}}"#;
    let parsed = tokei::parse_record(record).unwrap();
    assert_eq!(parsed.path, "doc.md");
    assert_eq!(parsed.embedded.len(), 3);
    let mut tally = tokei::Tally::default();
    tally.add(&parsed);
    let items = tally.items();
    let js = items[0]
        .embedded
        .iter()
        .find(|e| e.language == "JavaScript")
        .unwrap();
    assert_eq!((js.files, js.code), (1, 7));
    assert_eq!(tally.totals(), (1, 2, 0, 1));
}

#[test]
fn a_tokei_without_json_support_is_not_capable() {
    use tools::{TOKEI, ToolStatus, classify_version, version_token};
    let full = b"tokei 15.0.0 compiled with serialization support: json, cbor, yaml\n";
    assert!(tokei::has_json(full));
    assert!(!tokei::has_json(
        b"tokei 15.0.0 compiled with serialization support: cbor, yaml\n"
    ));
    assert!(!tokei::has_json(b"tokei 15.0.0\n"));
    assert!(!tokei::has_json(
        b"tokei 15.0.0 compiled with serialization support: \n"
    ));
    assert_eq!(version_token(full).as_deref(), Some("15.0.0"));
    assert_eq!(
        classify_version(&TOKEI, "15.0.0"),
        ToolStatus::Supported("15.0.0".into())
    );
    assert_eq!(
        classify_version(&TOKEI, "14.0.0"),
        ToolStatus::Unsupported("14.0.0".into())
    );
    assert!(TOKEI.supported_range().contains("tested 15.0.0"));
    assert!(tools::AST_GREP.supported_range().contains("tested 0.51.0"));
}

#[test]
fn documents_round_trip_through_the_stored_json() {
    let search = run(
        &search_job("fn f() {}", 100),
        &[&fixture("ast-grep-stream.jsonl")],
        ok(0),
        b"",
    )
    .document;
    let stats = run(
        &stats_job(100),
        &[&fixture("tokei-stream.jsonl")],
        ok(0),
        b"",
    )
    .document;
    for document in [search, stats] {
        let text = serde_json::to_string(&document).unwrap();
        assert_eq!(
            serde_json::from_str::<DiscoveryDocument>(&text).unwrap(),
            document
        );
    }
    let item = serde_json::to_value(CodeMatchItem {
        path: "src/main.rs".into(),
        lang: "rust".into(),
        start: CodePosition {
            line: 1,
            byte_column: 1,
        },
        end: CodePosition {
            line: 1,
            byte_column: 5,
        },
        snippet: "fn f".into(),
        snippet_truncated: false,
    })
    .unwrap();
    assert_eq!(
        item,
        json!({"path": "src/main.rs", "lang": "rust",
            "start": {"line": 1, "byte_column": 1}, "end": {"line": 1, "byte_column": 5},
            "snippet": "fn f", "snippet_truncated": false})
    );
}

#[test]
fn jobs_survive_the_worker_hand_off() {
    for job in [search_job("fn $F() {}", 5), stats_job(5)] {
        let text = serde_json::to_string(&job).unwrap();
        assert_eq!(serde_json::from_str::<DiscoveryJob>(&text).unwrap(), job);
    }
    assert_eq!(search_job("x", 1).command(), "code search");
    assert_eq!(search_job("x", 1).kind(), "code.search");
    assert_eq!(stats_job(1).command(), "code stats");
    assert_eq!(stats_job(1).kind(), "code.stats");
}

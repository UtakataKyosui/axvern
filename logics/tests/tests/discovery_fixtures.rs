//! Adapter contracts against recorded tool output; no external CLI is needed.
use axvern_core::adapters::tools::{self, Flow, Stop, TargetExit};
use axvern_models::*;
use std::{ffi::OsString, fs};

fn limits(max_items: u64, max_entries: u64) -> DiscoveryLimits {
    DiscoveryLimits {
        max_items,
        max_entries,
        timeout_ms: 60_000,
    }
}
fn list_job(root: &str) -> DiscoveryJob {
    DiscoveryJob::List(ListJob {
        root: root.into(),
        in_repo: true,
        depth: 2,
        include_ignored: false,
        limits: limits(100, 1000),
    })
}
fn search_job(root: &str, max_items: u64) -> DiscoveryJob {
    DiscoveryJob::Search(SearchJob {
        root: root.into(),
        file: None,
        in_repo: true,
        pattern: "TODO".into(),
        fixed_strings: false,
        globs: vec![],
        context: 1,
        ignore_case: false,
        include_ignored: false,
        limits: limits(max_items, 1000),
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
fn args(plan: &ExecutionPlan) -> Vec<String> {
    plan.args
        .iter()
        .map(|a: &OsString| a.to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn plans_are_plain_argv_with_pagers_and_color_disabled() {
    let hostile = "$(touch x); `id` 'q' \"d\" \n --config";
    let search = tools::adapter(&DiscoveryJob::Search(SearchJob {
        pattern: hostile.into(),
        ..match search_job("/r", 10) {
            DiscoveryJob::Search(j) => j,
            _ => unreachable!(),
        }
    }))
    .plan()
    .unwrap();
    assert_eq!(search.program, "rg");
    let a = args(&search);
    assert!(a.windows(2).any(|w| w == ["--color", "never"]));
    // The pattern is one argv element that follows --regexp; nothing is concatenated.
    assert!(a.windows(2).any(|w| w[0] == "--regexp" && w[1] == hostile));
    assert_eq!(a.last().unwrap(), ".");
    JsonExecutionPlan::try_from(&search).unwrap();

    let list = tools::adapter(&list_job("/r")).plan().unwrap();
    let a = args(&list);
    assert_eq!(list.program, "fd");
    assert!(a.windows(2).any(|w| w == ["--color", "never"]));
    assert!(a.contains(&"--print0".to_owned()) && a.contains(&"--hidden".to_owned()));
    assert!(a.contains(&"--no-ignore".to_owned()));
    assert!(!a.contains(&"--no-require-git".to_owned()));
    // Outside a repository only ignore files below the root apply.
    let outside = tools::adapter(&DiscoveryJob::List(ListJob {
        in_repo: false,
        ..match list_job("/r") {
            DiscoveryJob::List(j) => j,
            _ => unreachable!(),
        }
    }))
    .plan()
    .unwrap();
    assert!(args(&outside).contains(&"--no-ignore".to_owned()));

    let read = tools::adapter(&DiscoveryJob::Read(ReadJob {
        path: "/r/-weird file.txt".into(),
        from_line: 5,
        lines: 3,
        limits: limits(1, 1),
    }))
    .plan()
    .unwrap();
    let a = args(&read);
    assert_eq!(read.program, "bat");
    assert!(a.contains(&"--paging=never".to_owned()) && a.contains(&"--color=never".to_owned()));
    // One extra line is requested to learn whether the file continues.
    assert!(a.contains(&"--line-range=5:8".to_owned()));
    assert_eq!(&a[a.len() - 2..], ["--", "/r/-weird file.txt"]);
}

#[test]
fn fd_records_are_sorted_typed_and_non_utf8_names_are_reported_by_parent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("b")).unwrap();
    fs::write(root.join("b/z"), "").unwrap();
    fs::write(root.join("a\nb"), "").unwrap();
    std::os::unix::fs::symlink("b", root.join("link")).unwrap();
    let job = list_job(root.to_str().unwrap());
    let outcome = run(
        &job,
        &[b"link\0b/z\0b/", b"\0a\nb\0b/\xff\xfe\0"],
        ok(0),
        b"[fd error]: ./locked: Permission denied (os error 13)\n",
    );
    let paths: Vec<_> = outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Entry(e) => (e.path.as_str(), e.entry_type),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        paths,
        [
            ("a\nb", EntryType::File),
            ("b", EntryType::Directory),
            ("b/z", EntryType::File),
            ("link", EntryType::Symlink),
        ]
    );
    assert_eq!(outcome.state, ChildState::Done);
    let codes: Vec<_> = outcome
        .document
        .errors
        .iter()
        .map(|e| (e.code, e.path.clone()))
        .collect();
    assert_eq!(
        codes,
        [
            (ErrorCode::InvalidEncoding, Some("b".into())),
            (ErrorCode::PermissionDenied, Some("locked".into())),
        ]
    );
    assert!(outcome.document.completeness.partial);
    assert!(!outcome.document.completeness.scope_complete);
    // Serialization never replaces bytes: the response is valid JSON text.
    serde_json::to_string(&outcome.document).unwrap();
}

#[test]
fn fd_limits_truncate_and_an_exact_fit_does_not() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_str().unwrap();
    for name in ["a", "b", "c"] {
        fs::write(temp.path().join(name), "").unwrap();
    }
    let job = |items, entries| {
        DiscoveryJob::List(ListJob {
            limits: limits(items, entries),
            ..match list_job(root) {
                DiscoveryJob::List(j) => j,
                _ => unreachable!(),
            }
        })
    };
    let exact = run(&job(3, 3), &[b"a\0b\0c\0"], ok(0), b"");
    assert!(!exact.document.completeness.truncated);
    assert_eq!(exact.document.completeness.omitted_count, Some(0));
    let cut = run(&job(2, 10), &[b"a\0b\0c\0"], ok(0), b"");
    assert_eq!(cut.document.items.len(), 2);
    assert_eq!(
        cut.document.completeness.truncation_reasons,
        [TruncationReason::MaxItems]
    );
    assert_eq!(cut.document.completeness.omitted_count, None);
    let entries = run(&job(10, 2), &[b"a\0b\0c\0"], ok(0), b"");
    assert_eq!(
        entries.document.completeness.truncation_reasons,
        [TruncationReason::MaxEntries]
    );
    let timeout = run(
        &job(10, 10),
        &[b"a\0"],
        TargetExit {
            stopped: Some(Stop::Truncated(TruncationReason::Timeout)),
            ..Default::default()
        },
        b"",
    );
    assert_eq!(
        timeout.document.completeness.truncation_reasons,
        [TruncationReason::Timeout]
    );
    assert_eq!(timeout.state, ChildState::Done);
}

#[test]
fn fd_failures_are_typed() {
    let job = list_job("/nonexistent-root");
    let bad = run(&job, &[], ok(1), b"[fd error]: regex parse error:\n    (\n");
    assert_eq!(bad.state, ChildState::Failed);
    assert_eq!(bad.document.errors[0].code, ErrorCode::InvalidPattern);
    let missing = run(
        &job,
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

fn rg_event(kind: &str, data: serde_json::Value) -> Vec<u8> {
    let mut line = serde_json::to_vec(&serde_json::json!({"type": kind, "data": data})).unwrap();
    line.push(b'\n');
    line
}
fn text(t: &str) -> serde_json::Value {
    serde_json::json!({"text": t})
}

#[test]
fn rg_json_becomes_sorted_items_with_ranges_and_context() {
    let mut out = Vec::new();
    // Files arrive in arbitrary order; contexts are attached from neighbouring events.
    out.extend(rg_event(
        "begin",
        serde_json::json!({"path": text("./z.rs")}),
    ));
    out.extend(rg_event("match", serde_json::json!({"path": text("./z.rs"), "lines": text("é TODO TODO\n"), "line_number": 2,
        "submatches": [{"match": text("TODO"), "start": 3, "end": 7}, {"match": text("TODO"), "start": 8, "end": 12}]})));
    out.extend(rg_event(
        "context",
        serde_json::json!({"path": text("./z.rs"), "lines": text("next\r\n"), "line_number": 3}),
    ));
    out.extend(rg_event(
        "end",
        serde_json::json!({"path": text("./z.rs"), "binary_offset": null}),
    ));
    out.extend(rg_event(
        "begin",
        serde_json::json!({"path": text("./a.rs")}),
    ));
    out.extend(rg_event(
        "context",
        serde_json::json!({"path": text("./a.rs"), "lines": text("before\n"), "line_number": 1}),
    ));
    out.extend(rg_event(
        "match",
        serde_json::json!({"path": text("./a.rs"), "lines": text("TODO\n"), "line_number": 2,
        "submatches": [{"match": text("TODO"), "start": 0, "end": 4}]}),
    ));
    out.extend(rg_event(
        "end",
        serde_json::json!({"path": text("./a.rs"), "binary_offset": null}),
    ));
    let (first, second) = out.split_at(out.len() / 2 + 7); // chunk boundaries do not matter
    let outcome = run(&search_job("/r", 10), &[first, second], ok(0), b"");
    let items: Vec<_> = outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Match(m) => m.clone(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!((items[0].path.as_str(), items[0].line), ("a.rs", 2));
    assert_eq!(
        items[0].context_before,
        [ContextLine {
            line: 1,
            text: "before".into()
        }]
    );
    assert_eq!((items[1].path.as_str(), items[1].line), ("z.rs", 2));
    assert_eq!(items[1].text, "é TODO TODO");
    // Columns are 1-based UTF-8 byte offsets, end exclusive.
    assert_eq!(
        items[1].ranges,
        [
            ByteRange {
                start_byte: 4,
                end_byte: 8
            },
            ByteRange {
                start_byte: 9,
                end_byte: 13
            }
        ]
    );
    assert_eq!(
        items[1].context_after,
        [ContextLine {
            line: 3,
            text: "next".into()
        }]
    );
}

#[test]
fn rg_exit_codes_distinguish_no_match_partial_and_invalid_pattern() {
    let job = search_job("/r", 10);
    let none = run(&job, &[], ok(1), b"");
    assert_eq!(none.state, ChildState::Done);
    assert!(none.document.completeness.scope_complete && none.document.errors.is_empty());
    let partial = run(
        &job,
        &[],
        ok(2),
        b"rg: ./locked: Permission denied (os error 13)\n",
    );
    assert_eq!(partial.state, ChildState::Done);
    assert_eq!(partial.document.errors[0].code, ErrorCode::PermissionDenied);
    assert!(partial.document.completeness.partial);
    let bad = run(&job, &[], ok(2), b"rg: regex parse error:\n    (\n");
    assert_eq!(bad.state, ChildState::Failed);
    assert_eq!(bad.document.errors[0].code, ErrorCode::InvalidPattern);
    let crash = run(&job, &[], TargetExit::default(), b"");
    assert_eq!(crash.state, ChildState::Failed);
}

#[test]
fn rg_binary_files_non_utf8_names_and_lines_and_the_match_limit() {
    let mut out = Vec::new();
    out.extend(rg_event("begin", serde_json::json!({"path": text("bin")})));
    out.extend(rg_event("match", serde_json::json!({"path": text("bin"), "lines": text("TODO\n"), "line_number": 1, "submatches": []})));
    out.extend(rg_event(
        "end",
        serde_json::json!({"path": text("bin"), "binary_offset": 4}),
    ));
    // "b\xff" in base64.
    out.extend(rg_event(
        "begin",
        serde_json::json!({"path": {"bytes": "L3NyYy9i/w=="}}),
    ));
    out.extend(rg_event("match", serde_json::json!({"path": {"bytes": "L3NyYy9i/w=="}, "lines": text("TODO\n"), "line_number": 1, "submatches": []})));
    out.extend(rg_event(
        "end",
        serde_json::json!({"path": {"bytes": "L3NyYy9i/w=="}, "binary_offset": null}),
    ));
    out.extend(rg_event(
        "begin",
        serde_json::json!({"path": text("latin")}),
    ));
    out.extend(rg_event("match", serde_json::json!({"path": text("latin"), "lines": {"bytes": "VE9ETy//Cg=="}, "line_number": 7, "submatches": []})));
    out.extend(rg_event(
        "end",
        serde_json::json!({"path": text("latin"), "binary_offset": null}),
    ));
    let outcome = run(&search_job("/r", 10), &[&out], ok(2), b"");
    assert!(outcome.document.items.is_empty());
    let DiscoverySummary::Search {
        binary_files_skipped,
        ..
    } = outcome.document.summary
    else {
        unreachable!()
    };
    assert_eq!(binary_files_skipped, 1);
    let errors: Vec<_> = outcome
        .document
        .errors
        .iter()
        .map(|e| (e.code, e.path.clone(), e.line))
        .collect();
    assert_eq!(
        errors,
        [
            (ErrorCode::InvalidEncoding, Some("/src".into()), None),
            (ErrorCode::InvalidEncoding, Some("latin".into()), Some(7)),
        ]
    );

    let mut many = Vec::new();
    many.extend(rg_event("begin", serde_json::json!({"path": text("f")})));
    for line in 1..=3 {
        many.extend(rg_event("match", serde_json::json!({"path": text("f"), "lines": text("TODO\n"), "line_number": line, "submatches": []})));
    }
    many.extend(rg_event(
        "end",
        serde_json::json!({"path": text("f"), "binary_offset": null}),
    ));
    let cut = run(&search_job("/r", 2), &[&many], ok(0), b"");
    assert_eq!(cut.document.items.len(), 2);
    assert_eq!(
        cut.document.completeness.truncation_reasons,
        [TruncationReason::MaxItems]
    );

    // Overflow must not stop before the binary classification arrives, even
    // when events are split across separate worker reads.
    let mut binary = rg_event("begin", serde_json::json!({"path": text("bin")}));
    for line in 1..=100 {
        binary.extend(rg_event(
            "match",
            serde_json::json!({"lines": text("TODO\n"), "line_number": line, "submatches": []}),
        ));
    }
    let end = rg_event("end", serde_json::json!({"binary_offset": 1000}));
    let text_file = many;
    let classified = run(
        &search_job("/r", 2),
        &[&binary, &end, &text_file],
        ok(0),
        b"",
    );
    assert_eq!(classified.document.items.len(), 2);
    assert!(matches!(
        classified.document.summary,
        DiscoverySummary::Search {
            binary_files_skipped: 1,
            ..
        }
    ));
    assert!(
        classified
            .document
            .items
            .iter()
            .all(|item| matches!(item, DiscoveryItem::Match(m) if m.path == "f"))
    );

    let interrupted = run(
        &search_job("/r", 2),
        &[&binary],
        TargetExit {
            stopped: Some(Stop::Truncated(TruncationReason::Timeout)),
            ..Default::default()
        },
        b"",
    );
    assert!(interrupted.document.items.is_empty());
}

fn read_job(from_line: u64, lines: u32) -> DiscoveryJob {
    DiscoveryJob::Read(ReadJob {
        path: "/nonexistent/file.txt".into(),
        from_line,
        lines,
        limits: limits(1, 1),
    })
}
fn eof(outcome: &DiscoveryOutcome) -> bool {
    let DiscoverySummary::Read { eof, .. } = outcome.document.summary else {
        unreachable!()
    };
    eof
}

#[test]
fn bat_stream_becomes_numbered_lines_with_eof_detection() {
    let two = run(
        &read_job(10, 2),
        &[b"\xef\xbb\xbfa\r\nb", b"\nc\nd\n"],
        ok(0),
        b"",
    );
    let lines: Vec<_> = two
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Line(l) => (l.line, l.text.as_str()),
            _ => unreachable!(),
        })
        .collect();
    // The BOM is only meaningful at the start of the file, so line 10 keeps it.
    assert_eq!(lines, [(10, "\u{feff}a"), (11, "b")]);
    assert!(!eof(&two));
    let exact = run(&read_job(1, 2), &[b"\xef\xbb\xbfa\r\nb"], ok(0), b"");
    assert!(eof(&exact));
    assert_eq!(exact.document.items.len(), 2);
    match &exact.document.items[0] {
        DiscoveryItem::Line(l) => assert_eq!(l.text, "a"),
        _ => unreachable!(),
    }
    let empty = run(&read_job(5, 2), &[], ok(0), b"");
    assert!(eof(&empty) && empty.document.items.is_empty());
}

#[test]
fn bat_rejects_binary_and_non_utf8_and_flags_long_lines() {
    for bytes in [&b"ok\nbi\0n\n"[..], &b"ok\n\xff\xfe\n"[..]] {
        let outcome = run(&read_job(1, 5), &[bytes], ok(0), b"");
        assert_eq!(outcome.state, ChildState::Failed);
        assert!(outcome.document.items.is_empty());
        assert_eq!(
            outcome.document.errors[0].code,
            ErrorCode::UnsupportedEncoding
        );
    }
    let mut long = "あ".repeat(10_000).into_bytes();
    long.push(b'\n');
    let outcome = run(&read_job(1, 1), &[&long], ok(0), b"");
    let DiscoveryItem::Line(line) = &outcome.document.items[0] else {
        unreachable!()
    };
    assert!(line.line_truncated && line.text.len() <= LINE_BYTES);
    let missing = run(
        &read_job(1, 1),
        &[],
        ok(1),
        b"[bat error]: 'x': No such file or directory (os error 2)\n",
    );
    assert_eq!(missing.state, ChildState::Failed);
    assert_eq!(missing.document.errors[0].code, ErrorCode::TargetNotFound);
}

#[test]
fn tool_versions_are_classified_against_the_supported_range() {
    use tools::{FD, ToolStatus, classify_version, version_token};
    assert_eq!(version_token(b"fd 10.5.0\n").as_deref(), Some("10.5.0"));
    assert_eq!(
        classify_version(&FD, "10.5.0"),
        ToolStatus::Supported("10.5.0".into())
    );
    assert_eq!(
        classify_version(&FD, "9.0.0"),
        ToolStatus::Unsupported("9.0.0".into())
    );
    assert!(FD.supported_range().contains("tested 10.5.0"));
}

#[test]
fn fd_uses_only_gitignore_with_nested_negations_and_explicit_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::create_dir_all(root.join("src/sub")).unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    fs::write(temp.path().join(".gitignore"), "parent-only\n").unwrap();
    fs::write(root.join(".gitignore"), "*.log\n!keep.log\nbuild/\n").unwrap();
    fs::write(root.join(".git/info/exclude"), "personal\n").unwrap();
    fs::write(root.join(".ignore"), "dot-only\n").unwrap();
    fs::write(root.join(".fdignore"), "fd-only\n").unwrap();
    fs::write(root.join("src/.gitignore"), "!nested.log\nsub/\n").unwrap();
    fs::write(root.join("src/sub/.gitignore"), "!child\n").unwrap();
    let names = [
        "parent-only",
        "personal",
        "dot-only",
        "fd-only",
        "drop.log",
        "keep.log",
        "src/drop.log",
        "src/nested.log",
        "src/sub/child",
        "build/out",
    ];
    for name in names {
        fs::write(root.join(name), "x").unwrap();
    }
    let bytes = names.join("\0") + "\0";
    let paths = |outcome: DiscoveryOutcome| {
        outcome
            .document
            .items
            .into_iter()
            .map(|item| {
                let DiscoveryItem::Entry(entry) = item else {
                    panic!()
                };
                entry.path
            })
            .collect::<Vec<_>>()
    };
    let normal = run(
        &list_job(root.to_str().unwrap()),
        &[bytes.as_bytes()],
        ok(0),
        b"",
    );
    assert!(normal.document.errors.is_empty());
    assert_eq!(
        paths(normal),
        [
            "dot-only",
            "fd-only",
            "keep.log",
            "parent-only",
            "personal",
            "src/nested.log"
        ]
    );
    let mut all = match list_job(root.to_str().unwrap()) {
        DiscoveryJob::List(job) => job,
        _ => unreachable!(),
    };
    all.include_ignored = true;
    assert_eq!(
        paths(run(
            &DiscoveryJob::List(all),
            &[bytes.as_bytes()],
            ok(0),
            b""
        ))
        .len(),
        names.len()
    );
    // Selecting an ignored directory explicitly permits its contents.
    assert_eq!(
        paths(run(
            &list_job(root.join("build").to_str().unwrap()),
            &[b"out\0"],
            ok(0),
            b""
        )),
        ["out"]
    );
    // Parent repository rules still apply when starting within a subtree.
    assert_eq!(
        paths(run(
            &list_job(root.join("src").to_str().unwrap()),
            &[b"drop.log\0nested.log\0sub/child\0"],
            ok(0),
            b""
        )),
        ["nested.log"]
    );
    // Outside a repository, rules above the explicit root do not participate.
    fs::remove_dir_all(root.join(".git")).unwrap();
    let mut outside = match list_job(root.to_str().unwrap()) {
        DiscoveryJob::List(job) => job,
        _ => unreachable!(),
    };
    outside.in_repo = false;
    assert_eq!(
        paths(run(
            &DiscoveryJob::List(outside),
            &[b"parent-only\0"],
            ok(0),
            b""
        )),
        ["parent-only"]
    );
}

#[test]
fn live_fd_does_not_apply_personal_excludes() {
    use std::process::Command;
    if Command::new("fd").arg("--version").output().is_err() {
        eprintln!("skipped: fd is required for live ignore comparison");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(root.join(".gitignore"), "tracked-ignore\n").unwrap();
    fs::write(root.join(".git/info/exclude"), "local-only\n").unwrap();
    fs::write(root.join(".fdignore"), "fd-only\n").unwrap();
    fs::write(root.join(".ignore"), "dot-only\n").unwrap();
    for name in ["tracked-ignore", "local-only", "fd-only", "dot-only"] {
        fs::write(root.join(name), "x").unwrap();
    }
    let job = list_job(root.to_str().unwrap());
    let plan = tools::adapter(&job).plan().unwrap();
    let output = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .output()
        .unwrap();
    assert!(output.status.success());
    let result = run(&job, &[&output.stdout], ok(0), &output.stderr);
    assert!(result.document.errors.is_empty());
    let paths: Vec<_> = result
        .document
        .items
        .iter()
        .map(|item| {
            let DiscoveryItem::Entry(e) = item else {
                panic!()
            };
            e.path.as_str()
        })
        .collect();
    for name in ["local-only", "fd-only", "dot-only"] {
        assert!(paths.contains(&name));
    }
    assert!(!paths.contains(&"tracked-ignore"));
    assert!(!paths.iter().any(|p| p.starts_with(".git/")));
}

#[test]
fn fd_reports_unreadable_ignore_rules_and_suppresses_errors_in_excluded_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    fs::write(root.join("ok"), "x").unwrap();
    let result = run(
        &list_job(root.to_str().unwrap()),
        &[b"ok\0"],
        ok(0),
        b"[fd error]: ./ignored: Permission denied (os error 13)\n",
    );
    assert!(result.document.errors.is_empty());
    fs::remove_file(root.join(".gitignore")).unwrap();
    fs::create_dir(root.join(".gitignore")).unwrap();
    let result = run(&list_job(root.to_str().unwrap()), &[b"ok\0"], ok(0), b"");
    assert!(result.document.completeness.partial);
    assert_eq!(result.document.errors[0].code, ErrorCode::PartialResult);
}

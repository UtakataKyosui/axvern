//! Retained-log Reader, `query` and `recall` against plain files: no daemon, no CLI.
use axvern_core::logs::{
    self, OpenedLog,
    query::{self, ScanBudget},
    recall::{self, RecallSpec},
    source::{PueueLogFiles, pueue_directory},
};
use axvern_models::*;
use std::{fs, io::Cursor, os::unix::fs::PermissionsExt, path::Path, time::Duration};

fn spec(selector: QuerySelector) -> QuerySpec {
    QuerySpec {
        selector,
        limit: QUERY_DEFAULT_LIMIT,
        max_bytes: QUERY_DEFAULT_MAX_BYTES,
    }
}
fn scan(text: &[u8], spec: &QuerySpec) -> query::QueryScan {
    query::scan(Cursor::new(text.to_vec()), spec, &ScanBudget::default()).unwrap()
}
fn lines(scan: &query::QueryScan) -> Vec<(u64, &str)> {
    scan.items
        .iter()
        .map(|i| (i.line, i.text.as_str()))
        .collect()
}
fn matching(text: &str) -> QuerySelector {
    QuerySelector::Match(text.into())
}

#[test]
fn match_is_literal_case_sensitive_and_numbers_original_lines() {
    let log = b"start\nerror: boom\nERROR again\na.b\naxb\nerror: last";
    let result = scan(log, &spec(matching("error")));
    assert_eq!(lines(&result), [(2, "error: boom"), (6, "error: last")]);
    assert!(result.scan_complete && !result.truncated);
    assert_eq!(result.omitted_count, Some(0));
    // The needle is not a regular expression.
    assert_eq!(lines(&scan(log, &spec(matching("a.b")))), [(4, "a.b")]);
    // The same snapshot answers a different question without any other input.
    assert_eq!(
        lines(&scan(log, &spec(matching("ERROR")))),
        [(3, "ERROR again")]
    );
    assert!(scan(log, &spec(matching("nothing"))).items.is_empty());
}
#[test]
fn line_endings_unicode_and_missing_final_newline() {
    let log = "一行目\r\n二行目\n\n最後\u{1F600}".as_bytes();
    let all = scan(
        log,
        &spec(QuerySelector::Range {
            from_line: 1,
            lines: 10,
        }),
    );
    assert_eq!(
        lines(&all),
        [(1, "一行目"), (2, "二行目"), (3, ""), (4, "最後\u{1F600}")]
    );
    // Neither a trailing newline nor its absence adds a phantom line.
    assert_eq!(
        scan(b"a\nb\n", &spec(QuerySelector::Tail(5))).items.len(),
        2
    );
    assert_eq!(scan(b"a\nb", &spec(QuerySelector::Tail(5))).items.len(), 2);
    assert!(scan(b"", &spec(QuerySelector::Tail(5))).items.is_empty());
}
#[test]
fn error_candidates_use_independent_words_only() {
    for (line, hit) in [
        ("error: x", true),
        ("ERROR", true),
        ("Task Failed.", true),
        ("fatal", true),
        ("thread panic!", true),
        ("[panic]", true),
        ("エラーerror", true),
        ("errors", false),
        ("terror", false),
        ("error_code", false),
        ("failed2", false),
        ("no problem", false),
        ("", false),
    ] {
        assert_eq!(query::is_error_candidate(line), hit, "{line}");
    }
    let result = scan(
        b"ok\nFATAL x\nfine\nerror_code\npanic",
        &spec(QuerySelector::Errors),
    );
    assert_eq!(lines(&result), [(2, "FATAL x"), (5, "panic")]);
}
#[test]
fn tail_returns_the_last_lines_with_exact_numbers() {
    let log: String = (1..=10).map(|n| format!("l{n}\n")).collect();
    let result = scan(log.as_bytes(), &spec(QuerySelector::Tail(3)));
    assert_eq!(lines(&result), [(8, "l8"), (9, "l9"), (10, "l10")]);
    assert_eq!(result.omitted_count, Some(0));
    let all = scan(log.as_bytes(), &spec(QuerySelector::Tail(1000)));
    assert_eq!(all.items.len(), 10);
}
#[test]
fn range_is_one_based_and_beyond_the_end_is_empty() {
    let log = b"a\nb\nc\nd\n";
    let range = |from_line, lines| QuerySelector::Range { from_line, lines };
    assert_eq!(lines(&scan(log, &spec(range(2, 2)))), [(2, "b"), (3, "c")]);
    assert_eq!(lines(&scan(log, &spec(range(4, 9)))), [(4, "d")]);
    let past = scan(log, &spec(range(99, 5)));
    assert!(past.items.is_empty() && past.scan_complete && !past.truncated);
}
#[test]
fn limits_truncate_and_count_what_was_omitted() {
    let log: String = (1..=50).map(|n| format!("hit {n}\n")).collect();
    let mut limited = spec(matching("hit"));
    limited.limit = 10;
    let result = scan(log.as_bytes(), &limited);
    assert_eq!(result.items.len(), 10);
    assert_eq!(result.items[0].line, 1);
    assert!(result.truncated);
    assert_eq!(result.omitted_count, Some(40));
    // Tail keeps the newest lines; the oldest of the window are the omitted ones.
    let mut tail = spec(QuerySelector::Tail(30));
    tail.limit = 5;
    let result = scan(log.as_bytes(), &tail);
    assert_eq!(result.items.first().map(|i| i.line), Some(46));
    assert_eq!(result.items.last().map(|i| i.line), Some(50));
    assert_eq!(result.omitted_count, Some(25));
    // The byte budget bounds the text, in UTF-8 characters.
    let big: String = (0..40).map(|_| format!("{}\n", "あ".repeat(100))).collect();
    let mut bytes = spec(matching("あ"));
    bytes.max_bytes = QUERY_MIN_MAX_BYTES;
    let result = scan(big.as_bytes(), &bytes);
    assert!(result.items.iter().map(|i| i.text.len()).sum::<usize>() <= QUERY_MIN_MAX_BYTES);
    assert!(result.truncated && result.omitted_count.unwrap() > 0);
    // A range longer than the limit reports the rest as omitted.
    let mut range = spec(QuerySelector::Range {
        from_line: 1,
        lines: 40,
    });
    range.limit = 3;
    let result = scan(log.as_bytes(), &range);
    assert_eq!((result.items.len(), result.omitted_count), (3, Some(37)));
}
#[test]
fn a_huge_single_line_is_capped_without_cutting_a_character() {
    let mut log = "あ".repeat(2000).into_bytes(); // 6000 bytes
    log.extend_from_slice(b"\nnext\n");
    let mut capped = spec(matching("あ"));
    capped.max_bytes = QUERY_MIN_MAX_BYTES; // 1024: not a multiple of 3 boundary issue (1024 % 3 = 1)
    let result = scan(&log, &capped);
    let item = &result.items[0];
    assert!(item.line_truncated);
    assert_eq!(item.text.len(), 1023);
    assert!(item.text.chars().all(|c| c == 'あ'));
    // The following line is still reachable and numbered correctly.
    assert_eq!(scan(&log, &spec(matching("next"))).items[0].line, 2);
    // A single line far beyond the cap never grows memory with the line.
    let giant = vec![b'x'; 8 * 1024 * 1024];
    let result = scan(&giant, &spec(matching("x")));
    assert_eq!(result.items[0].text.len(), QUERY_DEFAULT_MAX_BYTES);
    assert_eq!(result.scanned_bytes, giant.len() as u64);
}
#[test]
fn binary_and_non_utf8_logs_are_reported_not_replaced() {
    for log in [&b"ok\n\xff\xfe broken\n"[..], b"nul\0byte\n", b"\xe3\x81\n"] {
        let error = query::scan(
            Cursor::new(log.to_vec()),
            &spec(matching("ok")),
            &ScanBudget::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidEncoding);
        assert!(!error.message.contains('\u{fffd}'));
    }
}
#[test]
fn scan_budget_stops_work_and_never_invents_tail_line_numbers() {
    let log: String = (1..=1000).map(|n| format!("line {n} error\n")).collect();
    let budget = ScanBudget {
        max_bytes: 2000,
        timeout: Duration::from_secs(10),
    };
    let found = query::scan(
        Cursor::new(log.clone()),
        &spec(QuerySelector::Errors),
        &budget,
    )
    .unwrap();
    assert!(!found.scan_complete && found.truncated);
    assert_eq!(found.omitted_count, None);
    assert!(!found.items.is_empty() && found.scanned_bytes <= 2000);
    let tail = query::scan(
        Cursor::new(log.clone()),
        &spec(QuerySelector::Tail(3)),
        &budget,
    )
    .unwrap();
    assert!(tail.items.is_empty() && !tail.scan_complete);
    assert_eq!(tail.omitted_count, None);
    // An exhausted time budget behaves the same way.
    let zero = ScanBudget {
        max_bytes: u64::MAX,
        timeout: Duration::ZERO,
    };
    let none = query::scan(Cursor::new(log), &spec(matching("line")), &zero).unwrap();
    assert!(!none.scan_complete && none.items.is_empty());
}
#[test]
fn invalid_specs_are_rejected() {
    let bad = |selector, limit, max_bytes| {
        QuerySpec {
            selector,
            limit,
            max_bytes,
        }
        .validate()
        .unwrap_err()
        .code
    };
    use QuerySelector::*;
    for code in [
        bad(Match(String::new()), 10, 4096),
        bad(Match("x".into()), 0, 4096),
        bad(Match("x".into()), 1001, 4096),
        bad(Match("x".into()), 10, 1023),
        bad(Match("x".into()), 10, 65537),
        bad(Match("x".repeat(1025)), 10, 4096),
        bad(Tail(0), 10, 4096),
        bad(Tail(1001), 10, 4096),
        bad(
            Range {
                from_line: 0,
                lines: 1,
            },
            10,
            4096,
        ),
        bad(
            Range {
                from_line: 1,
                lines: 0,
            },
            10,
            4096,
        ),
        bad(
            Range {
                from_line: 1,
                lines: 1001,
            },
            10,
            4096,
        ),
    ] {
        assert_eq!(code, ErrorCode::InvalidInput);
    }
}

fn pueue_dir(root: &Path) -> PueueLogFiles {
    fs::create_dir_all(root.join("task_logs")).unwrap();
    PueueLogFiles {
        directory: root.to_path_buf(),
    }
}
fn task(state: ChildState) -> TaskSnapshot {
    TaskSnapshot {
        identity: BackendIdentity {
            queue: "q".into(),
            task_id: 7,
            created_at: "t".into(),
        },
        state,
        exit_code: None,
        started_at: None,
        finished_at: None,
    }
}
fn open(files: &PueueLogFiles, state: ChildState) -> Result<OpenedLog, Error> {
    logs::open_task(&|| Ok(task(state)), files, || {
        files.path(7).to_string_lossy().into_owned()
    })
}
fn id() -> ExecutionId {
    "ex_00000000-0000-4000-8000-000000000000".parse().unwrap()
}
fn recall_spec(output: Option<&Path>) -> RecallSpec {
    RecallSpec {
        output: output.map(Path::to_path_buf),
        max_output_bytes: RECALL_DEFAULT_OUTPUT_BYTES,
        deadline: Duration::from_secs(60),
    }
}

#[test]
fn the_reader_distinguishes_available_partial_missing_and_unreadable() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(temp.path());
    // Missing: the task exists but its log does not (clean/reset/deleted).
    let missing = open(&files, ChildState::Done).err().unwrap();
    assert_eq!(missing.code, ErrorCode::LogMissing);
    assert!(!missing.message.contains(temp.path().to_str().unwrap()));
    fs::write(files.path(7), "out\n").unwrap();
    let done = open(&files, ChildState::Done).unwrap();
    assert_eq!(done.availability(), LogAvailability::Available);
    assert!(!done.partial && done.handle.snapshot_len == 4);
    for state in [ChildState::Running, ChildState::Queued] {
        let running = open(&files, state).unwrap();
        assert_eq!(running.availability(), LogAvailability::Partial);
    }
    // A queued task that has no file yet is an empty partial log, not an error.
    fs::remove_file(files.path(7)).unwrap();
    let queued = open(&files, ChildState::Queued).unwrap();
    assert!(queued.partial && queued.handle.snapshot_len == 0);
    assert_eq!(
        open(&files, ChildState::Running).err().unwrap().code,
        ErrorCode::LogMissing
    );
    // Unreadable (not the same as missing). Root ignores the mode bits, so check first.
    fs::write(files.path(7), "secret\n").unwrap();
    fs::set_permissions(files.path(7), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(files.path(7)).is_err() {
        assert_eq!(
            open(&files, ChildState::Done).err().unwrap().code,
            ErrorCode::PermissionDenied
        );
    }
    fs::set_permissions(files.path(7), fs::Permissions::from_mode(0o600)).unwrap();
    // A symlink is never followed, and a directory is not a log.
    fs::remove_file(files.path(7)).unwrap();
    fs::write(temp.path().join("elsewhere"), "other\n").unwrap();
    std::os::unix::fs::symlink(temp.path().join("elsewhere"), files.path(7)).unwrap();
    assert_eq!(
        open(&files, ChildState::Done).err().unwrap().code,
        ErrorCode::BackendFailure
    );
}
#[test]
fn identity_failures_surface_before_and_after_the_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(temp.path());
    fs::write(files.path(7), "x\n").unwrap();
    let gone = |_: ()| Err(Error::new(ErrorCode::Expired, "Pueue task was removed"));
    let err = logs::open_task(&|| gone(()), &files, String::new)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Expired);
    // The task disappears (clean) between the snapshot and the second observation.
    let calls = std::cell::Cell::new(0);
    let err = logs::open_task(
        &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Ok(task(ChildState::Done))
            } else {
                gone(())
            }
        },
        &files,
        String::new,
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::Expired);
}
#[test]
fn a_running_log_is_a_fixed_snapshot_that_may_keep_growing() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(temp.path());
    fs::write(files.path(7), "one\ntwo\n").unwrap();
    let opened = open(&files, ChildState::Running).unwrap();
    // The task appends after the snapshot; the query neither follows nor fails.
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(files.path(7))
        .unwrap();
    std::io::Write::write_all(&mut file, b"three\nfour\n").unwrap();
    let report = logs::query_opened(
        &id(),
        opened,
        &spec(QuerySelector::Tail(10)),
        &ScanBudget::default(),
    )
    .unwrap();
    assert_eq!(report.output["partial_log"], true);
    assert_eq!(report.output["log_availability"], "partial");
    assert_eq!(report.output["snapshot_bytes"], 8);
    assert_eq!(report.output["stream"], "combined");
    assert_eq!(report.output["items"].as_array().unwrap().len(), 2);
    assert!(report.error.is_none());
    assert!(
        report.output["observed_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
}
#[test]
fn query_reports_conditions_and_detects_a_log_that_changed_while_read() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(temp.path());
    fs::write(files.path(7), "a error\nb\n").unwrap();
    let opened = open(&files, ChildState::Done).unwrap();
    let report = logs::query_opened(
        &id(),
        opened,
        &spec(QuerySelector::Errors),
        &ScanBudget::default(),
    )
    .unwrap();
    assert_eq!(report.output["kind"], "query");
    assert_eq!(report.output["source_execution_id"], id().to_string());
    assert_eq!(report.output["selector"]["errors"]["rule_version"], 1);
    assert_eq!(report.output["items"][0]["line"], 1);
    assert_eq!(report.output["partial_log"], false);
    assert_eq!(report.output["omitted_count"], 0);
    // A finished log that is rewritten during the scan is refused.
    let opened = open(&files, ChildState::Done).unwrap();
    fs::write(files.path(7), "short").unwrap();
    let err = logs::query_opened(&id(), opened, &spec(matching("a")), &ScanBudget::default())
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::LogChanged);
    // An exhausted budget keeps the partial answer but carries partial_result.
    fs::write(files.path(7), "e error\n".repeat(500)).unwrap();
    let opened = open(&files, ChildState::Done).unwrap();
    let budget = ScanBudget {
        max_bytes: 100,
        timeout: Duration::from_secs(5),
    };
    let report = logs::query_opened(&id(), opened, &spec(QuerySelector::Errors), &budget).unwrap();
    assert_eq!(report.output["scan_complete"], false);
    assert!(report.output["omitted_count"].is_null());
    assert_eq!(report.error.unwrap().code, ErrorCode::PartialResult);
}

#[test]
fn recall_returns_small_logs_inline_and_refuses_to_pretend_for_large_ones() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(temp.path());
    fs::write(files.path(7), "hello\n日本語\n").unwrap();
    let report = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &recall_spec(None),
        None,
    )
    .unwrap();
    assert_eq!(report.output["kind"], "recall");
    assert_eq!(report.output["text"], "hello\n日本語\n");
    assert_eq!(report.output["complete"], true);
    assert_eq!(report.output["encoding"], "utf-8");
    assert_eq!(report.output["stream"], "combined");
    let running = recall::recall_opened(
        &id(),
        open(&files, ChildState::Running).unwrap(),
        &recall_spec(None),
        None,
    )
    .unwrap();
    assert_eq!(running.output["complete"], false);
    assert_eq!(running.output["partial_log"], true);
    // Too large for inline output: an error, never a silently cut text.
    fs::write(files.path(7), "x".repeat(RECALL_INLINE_BYTES + 1)).unwrap();
    let err = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &recall_spec(None),
        None,
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::OutputTooLarge);
    // Control characters escape in JSON and can push the payload over the inline limit.
    fs::write(files.path(7), "\u{1}".repeat(20000)).unwrap();
    let err = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &recall_spec(None),
        None,
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::OutputTooLarge);
    fs::write(files.path(7), b"\xff\xfe").unwrap();
    let err = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &recall_spec(None),
        None,
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::InvalidEncoding);
}
#[test]
fn recall_output_streams_bytes_to_a_new_private_file() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(&temp.path().join("pueue"));
    let out_dir = temp.path().join("out");
    fs::create_dir(&out_dir).unwrap();
    let mut content = vec![b'a'; 3 * 1024 * 1024];
    content.extend_from_slice(b"\xff\xfe\0binary\n");
    fs::write(files.path(7), &content).unwrap();
    let target = recall::target_path(&out_dir.join("log.bin")).unwrap();
    let report = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &recall_spec(Some(&target)),
        Some(&target),
    )
    .unwrap();
    assert_eq!(fs::read(&target).unwrap(), content);
    assert_eq!(report.output["encoding"], "bytes");
    assert_eq!(report.output["bytes"], content.len());
    assert_eq!(report.output["output_file"], target.to_str().unwrap());
    assert_eq!(report.output["sha256"].as_str().unwrap().len(), 64);
    assert!(report.output.get("text").is_none());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // No scratch file is left behind.
    let names: Vec<_> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["log.bin"]);
}
#[test]
fn recall_output_never_overwrites_follows_links_or_leaves_partial_files() {
    let temp = tempfile::tempdir().unwrap();
    let files = pueue_dir(&temp.path().join("pueue"));
    let out_dir = temp.path().join("out");
    fs::create_dir(&out_dir).unwrap();
    fs::write(files.path(7), "log\n").unwrap();
    // Existing file, existing symlink and a dangling symlink are all refused untouched.
    fs::write(out_dir.join("exists"), "keep").unwrap();
    std::os::unix::fs::symlink(temp.path().join("victim"), out_dir.join("dangling")).unwrap();
    for name in ["exists", "dangling"] {
        let err = recall::target_path(&out_dir.join(name)).unwrap_err();
        assert_eq!(err.code, ErrorCode::OutputExists, "{name}");
    }
    assert_eq!(fs::read_to_string(out_dir.join("exists")).unwrap(), "keep");
    assert!(!temp.path().join("victim").exists());
    // A missing parent directory is an input error.
    let err = recall::target_path(&out_dir.join("no/such/dir/file")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidTarget);
    // Someone creates the name after validation: publication still refuses to replace it.
    let target = recall::target_path(&out_dir.join("race")).unwrap();
    fs::write(&target, "winner").unwrap();
    let err = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &recall_spec(Some(&target)),
        Some(&target),
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::OutputExists);
    assert_eq!(fs::read_to_string(&target).unwrap(), "winner");
    // Over the configured limit: an error, nothing is written.
    let fresh = recall::target_path(&out_dir.join("small")).unwrap();
    let mut limited = recall_spec(Some(&fresh));
    limited.max_output_bytes = 2;
    let err = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &limited,
        Some(&fresh),
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::OutputTooLarge);
    // A log that shrinks during the copy publishes nothing.
    let opened = open(&files, ChildState::Done).unwrap();
    fs::write(files.path(7), "l").unwrap();
    let err = recall::recall_opened(&id(), opened, &recall_spec(Some(&fresh)), Some(&fresh))
        .err()
        .unwrap();
    assert!(matches!(err.code, ErrorCode::LogChanged));
    assert!(!fresh.exists());
    let leftovers: Vec<_> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".axvern-recall-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // An expired deadline publishes nothing either.
    fs::write(files.path(7), "log\n").unwrap();
    let mut late = recall_spec(Some(&fresh));
    late.deadline = Duration::ZERO;
    let err = recall::recall_opened(
        &id(),
        open(&files, ChildState::Done).unwrap(),
        &late,
        Some(&fresh),
    )
    .err()
    .unwrap();
    assert_eq!(err.code, ErrorCode::Timeout);
    assert!(!fresh.exists());
}
#[test]
fn the_pueue_directory_comes_from_the_daemon_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.yml");
    for (yaml, expected) in [
        ("shared:\n  pueue_directory: \"/data/p q\"\n", "/data/p q"),
        ("shared:\n  pueue_directory: '/it''s'\n", "/it's"),
        (
            "shared:\n  pueue_directory: /plain/dir # comment\n",
            "/plain/dir",
        ),
        (
            "shared:\n  other: 1\n  pueue_directory: \"/x\"\ndaemon: {}\n",
            "/x",
        ),
    ] {
        fs::write(&config, yaml).unwrap();
        assert_eq!(
            pueue_directory(&config).unwrap(),
            Path::new(expected),
            "{yaml}"
        );
    }
    fs::write(&config, "shared:\n  pueue_directory:\n").unwrap();
    assert_eq!(
        pueue_directory(&config).unwrap_err().code,
        ErrorCode::InvalidConfig
    );
    fs::write(&config, "shared:\n  use_unix_socket: true\n").unwrap();
    assert!(pueue_directory(&config).unwrap().ends_with("pueue"));
}

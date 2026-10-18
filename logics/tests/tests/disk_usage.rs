//! `system disk-usage` is measured in-process, so no daemon or external CLI is needed.
use axvern_core::{adapters::tools, discovery};
use axvern_models::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    time::{Duration, Instant},
};

fn limits(max_items: u64, max_entries: u64) -> DiscoveryLimits {
    DiscoveryLimits {
        max_items,
        max_entries,
        timeout_ms: 60000,
    }
}
fn request(depth: u32, top: u32, size_mode: SizeMode) -> DiscoveryRequest {
    DiscoveryRequest::DiskUsage {
        path: None,
        depth,
        top,
        size_mode,
    }
}
fn run_with(
    root: &Path,
    request: DiscoveryRequest,
    limits: DiscoveryLimits,
    wait: u64,
) -> DiscoveryOutcome {
    let prepared = discovery::prepare(&request, root, &limits).unwrap();
    tools::adapter(&prepared.job)
        .run_native(Instant::now() + Duration::from_millis(wait))
        .expect("disk usage runs in the worker")
}
fn run(root: &Path, request: DiscoveryRequest) -> DiscoveryOutcome {
    run_with(root, request, limits(1000, 100000), 60000)
}
fn usage(outcome: &DiscoveryOutcome) -> Vec<(String, u64)> {
    outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Usage(u) => (u.path.clone(), u.bytes),
            other => panic!("unexpected item {other:?}"),
        })
        .collect()
}
fn summary(outcome: &DiscoveryOutcome) -> (u64, bool, u64) {
    match &outcome.document.summary {
        DiscoverySummary::DiskUsage {
            total_bytes,
            lower_bound,
            entries,
            ..
        } => (*total_bytes, *lower_bound, *entries),
        other => panic!("unexpected summary {other:?}"),
    }
}
fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

#[test]
fn sizes_are_listed_largest_first_with_parents_including_children() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("big/a.bin"), 5000);
    write(&temp.path().join("big/b.bin"), 3000);
    write(&temp.path().join("small.txt"), 10);
    write(&temp.path().join(".hidden/h.bin"), 700);
    write(&temp.path().join(".git/objects/o"), 400);
    let outcome = run(temp.path(), request(3, 20, SizeMode::Apparent));
    let items = usage(&outcome);
    let size = |p: &str| items.iter().find(|(n, _)| n == p).unwrap().1;
    assert_eq!(size("big/a.bin"), 5000);
    assert_eq!(size("big/b.bin"), 3000);
    assert_eq!(size("small.txt"), 10);
    assert!(size("big") >= 8000, "a directory includes its children");
    assert!(
        size(".hidden") >= 700 && size(".git") >= 400,
        "hidden and .git are measured"
    );
    assert!(
        items.windows(2).all(|w| w[0].1 >= w[1].1),
        "largest first: {items:?}"
    );
    let (total, lower_bound, entries) = summary(&outcome);
    assert!(total >= 5000 + 3000 + 10 + 700 + 400 && !lower_bound);
    assert_eq!(entries, 9);
    assert!(!outcome.document.completeness.partial);
    assert_eq!(outcome.state, ChildState::Done);
}

#[test]
fn display_depth_and_top_only_select_rows_and_never_change_the_total() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a/b/c/deep.bin"), 4000);
    write(&temp.path().join("z.bin"), 9);
    let shallow = run(temp.path(), request(1, 20, SizeMode::Apparent));
    let paths: Vec<_> = usage(&shallow).into_iter().map(|(p, _)| p).collect();
    assert!(paths.contains(&"a".to_owned()) && !paths.contains(&"a/b".to_owned()));
    let deep = run(temp.path(), request(32, 2, SizeMode::Apparent));
    assert_eq!(usage(&deep).len(), 2);
    assert!(
        !deep.document.completeness.truncated,
        "top is a request, not truncation"
    );
    assert_eq!(summary(&shallow).0, summary(&deep).0);
}

#[test]
fn equal_sizes_are_ordered_by_path_bytes() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["b", "a", "C"] {
        write(&temp.path().join(name), 100);
    }
    let outcome = run(temp.path(), request(1, 20, SizeMode::Apparent));
    let paths: Vec<_> = usage(&outcome).into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, ["C", "a", "b"]);
}

#[test]
fn a_hard_link_is_counted_once_in_both_modes() {
    for mode in [SizeMode::Apparent, SizeMode::Allocated] {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a"), 100_000);
        fs::hard_link(temp.path().join("a"), temp.path().join("b")).unwrap();
        let outcome = run(temp.path(), request(1, 20, mode));
        let items = usage(&outcome);
        let size = |p: &str| items.iter().find(|(n, _)| n == p).unwrap().1;
        assert!(size("a") >= 100_000 || mode == SizeMode::Allocated && size("a") > 0);
        assert_eq!(size("b"), 0, "{mode:?}: attributed to the first path");
    }
}

#[test]
fn a_symlink_is_counted_as_a_link_and_never_followed() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("huge.bin"), 1_000_000);
    write(&temp.path().join("real.bin"), 10);
    symlink(outside.path(), temp.path().join("link")).unwrap();
    let outcome = run(temp.path(), request(3, 20, SizeMode::Apparent));
    let items = usage(&outcome);
    let link = items.iter().find(|(p, _)| p == "link").unwrap().1;
    assert!(link < 1000, "the link itself, not its target: {link}");
    assert!(summary(&outcome).0 < 100_000);
}

#[test]
fn a_single_file_is_one_row() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("only.bin"), 1234);
    let outcome = run(
        temp.path(),
        DiscoveryRequest::DiskUsage {
            path: Some("only.bin".into()),
            depth: 3,
            top: 20,
            size_mode: SizeMode::Apparent,
        },
    );
    assert_eq!(usage(&outcome), [("only.bin".to_owned(), 1234)]);
    assert_eq!(summary(&outcome).0, 1234);
}

#[test]
fn an_unreadable_directory_makes_the_result_partial_with_a_lower_bound() {
    if std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| o.stdout == b"0\n")
    {
        return; // root reads everything
    }
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("open/f"), 50);
    write(&temp.path().join("locked/f"), 5000);
    let locked = temp.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = run(temp.path(), request(3, 20, SizeMode::Apparent));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(summary(&outcome).1, "lower_bound");
    assert!(outcome.document.completeness.partial);
    assert_eq!(outcome.state, ChildState::Done);
    let error = &outcome.document.errors[0];
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    assert_eq!(error.path.as_deref(), Some("locked"));
    assert!(usage(&outcome).iter().any(|(p, _)| p == "open/f"));
}

#[test]
fn entry_and_time_budgets_truncate_and_set_the_lower_bound() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(&temp.path().join(format!("f{i:02}")), 10);
    }
    let cut = run_with(
        temp.path(),
        request(1, 20, SizeMode::Apparent),
        limits(1000, 5),
        60000,
    );
    assert_eq!(
        cut.document.completeness.truncation_reasons,
        [TruncationReason::MaxEntries]
    );
    assert!(summary(&cut).1 && summary(&cut).2 == 5);
    let exact = run_with(
        temp.path(),
        request(1, 20, SizeMode::Apparent),
        limits(1000, 20),
        60000,
    );
    assert!(
        !exact.document.completeness.truncated,
        "an exact fit is complete"
    );
    let late = run_with(
        temp.path(),
        request(1, 20, SizeMode::Apparent),
        limits(1000, 1000),
        0,
    );
    assert_eq!(
        late.document.completeness.truncation_reasons,
        [TruncationReason::Timeout]
    );
    assert!(summary(&late).1);
}

#[test]
fn non_utf8_names_are_counted_but_reported() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let temp = tempfile::tempdir().unwrap();
    let bad = temp.path().join(OsStr::from_bytes(b"bad\xff"));
    if fs::write(&bad, vec![b'x'; 900]).is_err() {
        return; // the filesystem refuses such names
    }
    write(&temp.path().join("ok"), 100);
    let outcome = run(temp.path(), request(1, 20, SizeMode::Apparent));
    assert!(summary(&outcome).0 >= 1000);
    assert!(!summary(&outcome).1, "the total is exact");
    assert_eq!(outcome.document.errors[0].code, ErrorCode::InvalidEncoding);
    assert!(usage(&outcome).iter().all(|(p, _)| !p.contains('\u{fffd}')));
}

#[test]
fn input_is_checked_before_registration() {
    let temp = tempfile::tempdir().unwrap();
    for (depth, top) in [(0, 20), (33, 20), (3, 0), (3, 1001)] {
        let error = discovery::prepare(
            &request(depth, top, SizeMode::Allocated),
            temp.path(),
            &limits(10, 10),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "{depth}/{top}");
    }
    let missing = DiscoveryRequest::DiskUsage {
        path: Some("nope".into()),
        depth: 3,
        top: 20,
        size_mode: SizeMode::Allocated,
    };
    assert_eq!(
        discovery::prepare(&missing, temp.path(), &limits(10, 10))
            .unwrap_err()
            .code,
        ErrorCode::TargetNotFound
    );
    let ok = discovery::prepare(
        &request(3, 20, SizeMode::Allocated),
        temp.path(),
        &limits(10, 10),
    )
    .unwrap();
    assert_eq!(ok.plan.program, "axvern-disk-usage");
    assert!(
        discovery::ensure_tool(&ok.job, &ok.plan).is_ok(),
        "no external CLI is required"
    );
}

#[test]
fn the_stored_document_round_trips_without_turning_into_another_kind() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("x"), 10);
    let outcome = run(temp.path(), request(3, 20, SizeMode::Allocated));
    let json = serde_json::to_string(&outcome.document).unwrap();
    let back: DiscoveryDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(back, outcome.document);
    assert!(matches!(back.summary, DiscoverySummary::DiskUsage { .. }));
    assert!(
        back.items
            .iter()
            .all(|i| matches!(i, DiscoveryItem::Usage(_)))
    );
}

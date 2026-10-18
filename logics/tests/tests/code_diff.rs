//! `code diff` runs a fixed, isolated `git diff` inside the worker: only git is needed.
use axvern_core::{adapters::tools, discovery};
use axvern_models::*;
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}
fn repo() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    temp
}
fn commit_all(dir: &Path) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "c"]);
}
fn limits() -> DiscoveryLimits {
    DiscoveryLimits {
        max_items: 1000,
        max_entries: 100000,
        timeout_ms: 60000,
    }
}
fn prepared(dir: &Path, staged: bool) -> Result<discovery::Prepared, Error> {
    discovery::prepare(&DiscoveryRequest::CodeDiff { staged }, dir, &limits())
}
fn run(dir: &Path, staged: bool) -> DiscoveryOutcome {
    let prepared = prepared(dir, staged).unwrap();
    tools::adapter(&prepared.job)
        .run_native(Instant::now() + Duration::from_secs(60))
        .expect("code diff runs in the worker")
}
fn items(outcome: &DiscoveryOutcome) -> Vec<&DiffItem> {
    outcome
        .document
        .items
        .iter()
        .map(|i| match i {
            DiscoveryItem::Diff(d) => d,
            other => panic!("unexpected item {other:?}"),
        })
        .collect()
}
fn comparison(outcome: &DiscoveryOutcome) -> (DiffComparison, bool) {
    match &outcome.document.summary {
        DiscoverySummary::CodeDiff {
            comparison,
            untracked_included,
            ..
        } => (*comparison, *untracked_included),
        other => panic!("unexpected summary {other:?}"),
    }
}

#[test]
fn no_difference_is_an_empty_done_result_and_git_is_left_untouched() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.rs"), "fn main() {}\n").unwrap();
    commit_all(&dir);
    let index = fs::read(dir.join(".git/index")).unwrap();
    let outcome = run(&dir, false);
    assert!(items(&outcome).is_empty());
    assert_eq!(outcome.state, ChildState::Done);
    assert!(outcome.document.completeness.scope_complete);
    assert_eq!(
        comparison(&outcome),
        (DiffComparison::IndexToWorktree, false)
    );
    assert_eq!(fs::read(dir.join(".git/index")).unwrap(), index);
    assert!(!dir.join(".git/index.lock").exists());
}

#[test]
fn worktree_and_staged_comparisons_are_different_and_untracked_files_are_left_out() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.txt"), "1\n2\n3\n").unwrap();
    fs::write(dir.join("b.txt"), "x\n").unwrap();
    commit_all(&dir);
    fs::write(dir.join("a.txt"), "1\n2\n3\n4\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    fs::write(dir.join("b.txt"), "y\n").unwrap();
    fs::write(dir.join("untracked.txt"), "u\n").unwrap();

    let worktree = run(&dir, false);
    let listed = items(&worktree);
    assert_eq!(listed.len(), 1, "only b.txt differs from the index");
    assert_eq!(listed[0].new_path.as_deref(), Some("b.txt"));
    assert_eq!(listed[0].change, DiffChange::Modified);
    assert!(!listed[0].binary && !listed[0].patch_truncated);
    let hunk = &listed[0].hunks[0];
    assert_eq!(
        (
            hunk.old_start,
            hunk.old_lines,
            hunk.new_start,
            hunk.new_lines
        ),
        (1, 1, 1, 1)
    );
    assert!(hunk.patch.contains("-x\n") && hunk.patch.contains("+y\n"));

    let staged = run(&dir, true);
    assert_eq!(comparison(&staged), (DiffComparison::HeadToIndex, false));
    let listed = items(&staged);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].new_path.as_deref(), Some("a.txt"));
    assert!(listed[0].hunks[0].patch.contains("+4\n"));
}

#[test]
fn added_deleted_renamed_and_binary_files_are_classified() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(
        dir.join("old name.txt"),
        "same content\nmore\nlines\nhere\n",
    )
    .unwrap();
    fs::write(dir.join("gone.txt"), "bye\n").unwrap();
    fs::write(dir.join("bin.dat"), b"x\0y").unwrap();
    commit_all(&dir);
    git(&dir, &["mv", "old name.txt", "new name.txt"]);
    git(&dir, &["rm", "-q", "gone.txt"]);
    fs::write(dir.join("bin.dat"), b"x\0z").unwrap();
    fs::write(dir.join("fresh.txt"), "new\n").unwrap();
    git(&dir, &["add", "-A"]);
    let outcome = run(&dir, true);
    let listed = items(&outcome);
    let find = |change: DiffChange| listed.iter().find(|i| i.change == change).copied();
    let renamed = find(DiffChange::Renamed).expect("renamed");
    assert_eq!(renamed.old_path.as_deref(), Some("old name.txt"));
    assert_eq!(renamed.new_path.as_deref(), Some("new name.txt"));
    let deleted = find(DiffChange::Deleted).expect("deleted");
    assert_eq!(
        (deleted.old_path.as_deref(), deleted.new_path.as_deref()),
        (Some("gone.txt"), None)
    );
    let added = find(DiffChange::Added).expect("added");
    assert_eq!(
        (added.old_path.as_deref(), added.new_path.as_deref()),
        (None, Some("fresh.txt"))
    );
    let binary = listed.iter().find(|i| i.binary).expect("binary");
    assert_eq!(binary.new_path.as_deref(), Some("bin.dat"));
    assert!(binary.hunks.is_empty(), "binary content is never shown");
    assert!(!outcome.document.completeness.partial);
}

#[test]
fn staged_comparison_before_the_first_commit_uses_the_empty_tree() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("first.txt"), "hello\n").unwrap();
    git(&dir, &["add", "first.txt"]);
    let outcome = run(&dir, true);
    let listed = items(&outcome);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].change, DiffChange::Added);
    assert!(listed[0].hunks[0].patch.contains("+hello\n"));
}

#[test]
fn a_directory_outside_a_worktree_is_not_a_repository() {
    let temp = tempfile::tempdir().unwrap();
    let error = prepared(temp.path(), false).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotARepository);
    // Git's own storage is not a worktree either.
    let repo = repo();
    let error = prepared(&repo.path().join(".git"), false).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotARepository);
}

#[test]
fn a_subdirectory_compares_the_whole_worktree() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::create_dir(dir.join("sub")).unwrap();
    fs::write(dir.join("top.txt"), "a\n").unwrap();
    commit_all(&dir);
    fs::write(dir.join("top.txt"), "b\n").unwrap();
    let outcome = run(&dir.join("sub"), false);
    assert_eq!(items(&outcome)[0].new_path.as_deref(), Some("top.txt"));
}

#[test]
fn a_long_hunk_is_cut_and_marked() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("big.txt"), "start\n").unwrap();
    commit_all(&dir);
    let body: String = (0..4000).map(|i| format!("line number {i}\n")).collect();
    fs::write(dir.join("big.txt"), body).unwrap();
    let outcome = run(&dir, false);
    let listed = items(&outcome);
    assert!(listed[0].patch_truncated);
    let patch = &listed[0].hunks[0].patch;
    assert!(patch.len() <= 8 * 1024 && patch.ends_with('\n'));
}

#[test]
fn filters_named_by_repository_configuration_are_never_run() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("t.txt"), "old\n").unwrap();
    fs::write(dir.join(".gitattributes"), "t.txt filter=fx\n").unwrap();
    commit_all(&dir);
    let marker = dir.join("MARKER");
    git(
        &dir,
        &[
            "config",
            "filter.fx.clean",
            &format!("touch {}; cat", marker.display()),
        ],
    );
    git(&dir, &["config", "filter.fx.required", "true"]);
    git(
        &dir,
        &[
            "config",
            "core.fsmonitor",
            &format!("touch {}; echo", marker.display()),
        ],
    );
    fs::write(dir.join("t.txt"), "new\n").unwrap();
    let outcome = run(&dir, false);
    assert_eq!(items(&outcome).len(), 1);
    assert!(!marker.exists(), "no configured program may start");
}

#[test]
fn private_attributes_that_could_select_a_filter_are_refused_before_running() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("t.txt"), "old\n").unwrap();
    commit_all(&dir);
    fs::write(dir.join(".git/info/attributes"), "t.txt filter=fx\n").unwrap();
    let prepared = prepared(&dir, false).unwrap();
    let error = tools::adapter(&prepared.job).preflight().unwrap_err();
    assert_eq!(error.code, ErrorCode::UnsupportedCapability);
    let outcome = tools::adapter(&prepared.job)
        .run_native(Instant::now() + Duration::from_secs(60))
        .unwrap();
    assert_eq!(outcome.state, ChildState::Failed);
    // Attributes without a filter are harmless.
    fs::write(dir.join(".git/info/attributes"), "*.png binary\n").unwrap();
    assert!(tools::adapter(&prepared.job).preflight().is_ok());
}

#[test]
fn a_missing_object_never_starts_a_fetch_from_a_promisor_remote() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("t.txt"), "old content\n").unwrap();
    commit_all(&dir);
    // Drop the committed blob so the diff needs an object the repository does not have.
    let blob = String::from_utf8(
        Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "HEAD:t.txt"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let blob = blob.trim();
    fs::remove_file(dir.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    let marker = dir.join("MARKER");
    let helper = dir.join("helper.sh");
    fs::write(
        &helper,
        format!("#!/bin/sh\ntouch {}\nexit 1\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&helper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    git(&dir, &["config", "extensions.partialClone", "origin"]);
    git(&dir, &["config", "remote.origin.promisor", "true"]);
    git(
        &dir,
        &[
            "config",
            "remote.origin.url",
            &format!("ext::{} %S", helper.display()),
        ],
    );
    git(&dir, &["config", "protocol.ext.allow", "always"]);
    fs::write(dir.join("t.txt"), "new content\n").unwrap();
    let outcome = run(&dir, false);
    assert!(
        !marker.exists(),
        "no helper may be started to fetch the object"
    );
    assert_eq!(
        outcome.state,
        ChildState::Failed,
        "a missing object fails the diff safely"
    );
}

#[test]
fn a_type_change_keeps_the_hunks_of_other_changed_files() {
    let temp = repo();
    let dir = temp.path().canonicalize().unwrap();
    fs::write(dir.join("a.txt"), "one\n").unwrap();
    fs::write(dir.join("b.txt"), "left\n").unwrap();
    fs::write(dir.join("c.txt"), "right\n").unwrap();
    commit_all(&dir);
    fs::remove_file(dir.join("b.txt")).unwrap();
    std::os::unix::fs::symlink("a.txt", dir.join("b.txt")).unwrap();
    fs::write(dir.join("a.txt"), "two\n").unwrap();
    fs::write(dir.join("c.txt"), "changed\n").unwrap();
    let outcome = run(&dir, false);
    assert!(
        outcome.document.warnings.is_empty(),
        "{:?}",
        outcome.document.warnings
    );
    let items = items(&outcome);
    assert_eq!(items.len(), 3);
    let by = |name: &str| {
        items
            .iter()
            .find(|i| i.new_path.as_deref() == Some(name))
            .unwrap()
    };
    assert_eq!(by("b.txt").change, DiffChange::TypeChanged);
    assert!(!by("b.txt").hunks.is_empty() && !by("b.txt").patch_truncated);
    for name in ["a.txt", "c.txt"] {
        assert_eq!(by(name).hunks.len(), 1, "{name} keeps its hunk");
        assert!(!by(name).patch_truncated, "{name}");
    }
}

//! Native CLI scope checks need no daemon; Pueue wiring stays in the existing live suites.
mod common;
use axvern_core::{
    adapters::tools::{self, Flow, TargetExit},
    discovery,
};
use axvern_models::*;
use std::{fs, path::Path, process::Command};

fn requests(include_ignored: bool) -> [DiscoveryRequest; 3] {
    [
        DiscoveryRequest::Search {
            pattern: "target".into(),
            path: None,
            fixed_strings: true,
            globs: vec!["*.rs".into()],
            context: 0,
            ignore_case: false,
            include_ignored,
        },
        DiscoveryRequest::CodeSearch {
            pattern: "fn target() {}".into(),
            lang: "rust".into(),
            path: None,
            include_ignored,
        },
        DiscoveryRequest::CodeStats {
            path: None,
            include_ignored,
        },
    ]
}
fn native(root: &Path, request: DiscoveryRequest) -> DiscoveryOutcome {
    let prepared = discovery::prepare(
        &request,
        root,
        &DiscoveryLimits {
            max_items: 1000,
            max_entries: 10000,
            timeout_ms: 60000,
        },
    )
    .unwrap();
    let output = Command::new(&prepared.plan.program)
        .args(&prepared.plan.args)
        .current_dir(&prepared.plan.cwd)
        .output()
        .unwrap();
    let mut normalizer = tools::adapter(&prepared.job).normalizer(1 << 20).unwrap();
    let stopped = match normalizer.feed(&output.stdout) {
        Flow::Continue => None,
        Flow::Stop(s) => Some(s),
    };
    normalizer.finish(
        &TargetExit {
            code: output.status.code(),
            stopped,
            spawn_failed: false,
        },
        &output.stderr,
    )
}
fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}
fn count(outcome: &DiscoveryOutcome) -> u64 {
    match &outcome.document.summary {
        DiscoverySummary::CodeStats { files, .. } => *files,
        _ => outcome.document.items.len() as u64,
    }
}

#[test]
fn search_and_stats_apply_only_gitignore_inside_and_outside_repositories() {
    if !common::discovery_tools(&["rg", "ast-grep", "tokei"]) {
        return;
    }
    for repository in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("scope");
        if repository {
            write(&root, ".git/HEAD", "ref: refs/heads/main\n");
        }
        write(temp.path(), ".gitignore", "scope/personal.rs\n"); // above the scan scope
        write(
            &root,
            ".gitignore",
            "ignored.rs\nnested/*.rs\n!nested/keep.rs\n",
        );
        for ignore in [".ignore", ".fdignore", ".tokeignore"] {
            write(&root, ignore, "personal.rs\n");
        }
        if repository {
            write(&root, ".git/info/exclude", "personal.rs\n");
        }
        for source in [
            "keep.rs",
            "personal.rs",
            ".hidden.rs",
            "nested/keep.rs",
            "nested/ignored.rs",
            "ignored.rs",
        ] {
            write(&root, source, "fn target() {}\n");
        }
        write(&root, "inner/.git/private.rs", "fn target() {}\n");
        for request in requests(false) {
            let result = native(&root, request);
            assert_eq!(result.state, ChildState::Done, "{result:?}");
            assert_eq!(count(&result), 4, "{repository}: {result:?}");
            assert!(result.document.completeness.scope_complete, "{result:?}");
            assert!(
                result
                    .document
                    .warnings
                    .iter()
                    .all(|w| w.code != WarningCode::IgnoreRulesNotApplied)
            );
        }
        for request in requests(true) {
            let result = native(&root, request);
            assert_eq!(count(&result), 6, "{result:?}");
        }
        // Explicit ignored files remain valid targets even though a directory scan excludes them.
        for mut request in requests(false) {
            match &mut request {
                DiscoveryRequest::Search { path, .. }
                | DiscoveryRequest::CodeSearch { path, .. }
                | DiscoveryRequest::CodeStats { path, .. } => *path = Some("ignored.rs".into()),
                _ => unreachable!(),
            }
            assert_eq!(count(&native(&root, request)), 1);
        }
    }
}

#[test]
fn native_search_and_statistics_stop_descending_at_the_shared_depth() {
    if !common::discovery_tools(&["rg", "ast-grep", "tokei"]) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    for depth in [32, 33, 34] {
        let path = format!("{}level{depth}.rs", "d/".repeat(depth - 1));
        write(temp.path(), &path, "fn target() {}\n");
    }
    for request in requests(true) {
        let outcome = native(temp.path(), request);
        assert_eq!(count(&outcome), 1, "{outcome:?}");
        assert!(outcome.document.completeness.scope_complete);
        assert!(
            !outcome.document.completeness.truncated,
            "depth is requested scope, not a budget stop"
        );
    }
}

#[test]
fn invalid_ignore_files_make_code_results_partial_instead_of_silent() {
    if !common::discovery_tools(&["rg", "ast-grep", "tokei"]) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".gitignore")).unwrap();
    write(temp.path(), "one.rs", "fn target() {}\n");
    for request in requests(false) {
        let outcome = native(temp.path(), request);
        assert!(outcome.document.completeness.partial, "{outcome:?}");
        assert!(
            outcome
                .document
                .errors
                .iter()
                .any(|e| e.message == "cannot read or parse .gitignore")
        );
    }
}

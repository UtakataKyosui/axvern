//! `fs analyze` contracts without Pueue or the real tools: children are produced by the real
//! discovery worker running small fake tools, and the parent is built from their stored results.
use axvern_core::{
    analyze::{self, Materialized, SectionInput, SectionOutcome},
    config::Results,
    discovery, parents, results,
    store::{ExecutionStore, RegistrationFailure},
};
use axvern_models::*;
use serde_json::{Value, json};
use std::{
    cell::Cell,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir_in(fs::canonicalize("/tmp").unwrap()).unwrap()
}
fn limits() -> DiscoveryLimits {
    DiscoveryLimits {
        max_items: 10_000,
        max_entries: 100_000,
        timeout_ms: 60_000,
    }
}
fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn workspace(root: &Path) {
    write(root, "Cargo.toml", "[package]\n");
    write(root, "pkg/package.json", "{}\n");
    write(root, "src/main.rs", "fn main() {}\n");
}
fn args(plan: &ExecutionPlan) -> Vec<String> {
    plan.args
        .iter()
        .map(|a| a.to_str().unwrap().to_owned())
        .collect()
}
/// A fake tool: counts its runs in `<name>.runs`, then runs `body`.
fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    let runs = dir.join(format!("{name}.runs"));
    fs::write(
        &path,
        format!("#!/bin/sh\necho run >> '{}'\n{body}\n", runs.display()),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
fn runs(dir: &Path, name: &str) -> usize {
    fs::read_to_string(dir.join(format!("{name}.runs")))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}
fn tokei_fixture() -> String {
    format!(
        "cat '{}'",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/code/tokei-stream.jsonl")
            .display()
    )
}

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    store: ExecutionStore,
    settings: Results,
    tasks: Cell<u64>,
}
impl Fixture {
    fn new() -> Self {
        let temp = temp();
        let root = temp.path().join("w");
        workspace(&root);
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let store = ExecutionStore::open(temp.path().join("store")).unwrap();
        Self {
            root,
            bin,
            store,
            settings: Results::default(),
            tasks: Cell::new(0),
            temp,
        }
    }
    fn prepare(&self, target: &Path) -> analyze::PreparedAnalysis {
        analyze::prepare(target, 1, false, self.temp.path(), &limits()).unwrap()
    }
    /// Save a child record as if Pueue had accepted it.
    fn record(&self, plan: &ExecutionPlan) -> ExecutionRecord {
        self.tasks.set(self.tasks.get() + 1);
        let record = ExecutionRecord {
            execution_id: ExecutionStore::allocate_id().unwrap(),
            backend: BackendIdentity {
                queue: "fixture".into(),
                task_id: self.tasks.get(),
                created_at: "2026-10-16T00:00:00+09:00".into(),
            },
            registered_at: 100,
            cwd: plan.cwd.to_str().unwrap().into(),
        };
        self.store.save_record(&record).unwrap();
        record
    }
    fn start(&self, target: &Path) -> ParentRecord {
        let prepared = self.prepare(target);
        let (record, error) =
            analyze::register(&self.store, &prepared, |child| Ok(self.record(&child.plan)))
                .unwrap();
        assert_eq!(error, None);
        record
    }
    /// Run the real discovery worker for one child with a fake tool in place of the real one.
    fn produce(&self, parent: &ParentRecord, section: AnalysisSection, tool: &Path) {
        let prepared = self.prepare(Path::new(&parent.analysis.as_ref().unwrap().path));
        let (_, child) = prepared
            .children
            .iter()
            .find(|(s, _)| *s == section)
            .unwrap();
        let id = parent
            .analysis
            .as_ref()
            .unwrap()
            .sections
            .iter()
            .find(|s| s.section == section)
            .and_then(|s| s.execution_id.clone())
            .unwrap();
        let mut plan = child.plan.clone();
        plan.program = tool.into();
        discovery::run_worker(&self.store, &self.settings, &id, &child.job, &plan).unwrap();
    }
    fn materialize(
        &self,
        parent: &ParentRecord,
        backend: &dyn Fn(&ExecutionId) -> Result<ChildState, Error>,
    ) -> Materialized {
        analyze::materialize(&self.store, &self.settings, parent, backend).unwrap()
    }
    fn page(&self, id: &ExecutionId, cursor: Option<&str>) -> Value {
        results::page(&self.store, &self.settings, id, cursor, 100, 65536)
            .unwrap()
            .1
    }
}
fn never(_: &ExecutionId) -> Result<ChildState, Error> {
    panic!("a finished child must not need its backend")
}
fn sections(result: &Value, section: &str) -> Vec<Value> {
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["section"] == section)
        .cloned()
        .collect()
}
fn child<'a>(result: &'a Value, section: &str) -> &'a Value {
    result["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["section"] == section)
        .unwrap()
}

#[test]
fn a_directory_has_three_children_with_separate_display_and_scan_depths() {
    let f = Fixture::new();
    let prepared = f.prepare(&f.root);
    let root = f.root.to_str().unwrap();
    assert_eq!(prepared.spec.input_type, InputType::Directory);
    assert_eq!(
        (prepared.spec.tree_depth, prepared.spec.scan_depth),
        (1, 32)
    );
    assert_eq!(
        (prepared.spec.path.as_str(), prepared.spec.root.as_str()),
        (root, root)
    );
    let names: Vec<_> = prepared.children.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        names,
        [
            AnalysisSection::Languages,
            AnalysisSection::Manifests,
            AnalysisSection::Tree
        ]
    );
    let plans: Vec<_> = prepared.children.iter().map(|(_, c)| &c.plan).collect();
    assert_eq!(plans[0].program, "tokei");
    assert_eq!(plans[1].program, "fd");
    assert_eq!(plans[2].program, "fd");
    // Statistics and manifests walk the full scan depth; only the tree uses `--depth`.
    let manifests = args(plans[1]);
    let tree = args(plans[2]);
    assert!(manifests.windows(2).any(|w| w == ["--max-depth", "32"]));
    assert!(manifests.windows(2).any(|w| w == ["--type", "f"]));
    assert!(manifests.contains(&"--glob".into()));
    assert_eq!(
        manifests.last().unwrap(),
        "{Cargo.toml,package.json,pnpm-workspace.yaml,devbox.json,pyproject.toml,go.mod}"
    );
    assert!(tree.windows(2).any(|w| w == ["--max-depth", "1"]));
    // Neither tool is told to follow symbolic links.
    for plan in &plans {
        assert!(
            !args(plan)
                .iter()
                .any(|a| a == "-L" || a == "--follow" || a.starts_with("--follow"))
        );
    }
    for depth in [0, 33] {
        assert_eq!(
            analyze::prepare(&f.root, depth, false, f.temp.path(), &limits())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }
    assert_eq!(
        analyze::prepare(Path::new("gone"), 3, false, f.temp.path(), &limits())
            .unwrap_err()
            .code,
        ErrorCode::TargetNotFound
    );
}

#[test]
fn a_file_is_analyzed_alone_and_is_a_manifest_only_by_name() {
    let f = Fixture::new();
    let source = f.prepare(&f.root.join("src/main.rs"));
    assert_eq!(source.spec.input_type, InputType::File);
    assert_eq!(source.spec.root, f.root.join("src").to_str().unwrap());
    assert_eq!((source.spec.tree_depth, source.spec.scan_depth), (0, 0));
    // Only statistics run, on the file itself; nothing reads the directory around it.
    assert_eq!(source.children.len(), 1);
    let (section, statistics) = &source.children[0];
    assert_eq!(*section, AnalysisSection::Languages);
    assert_eq!(args(&statistics.plan).last().unwrap(), "main.rs");
    let entry = |name: &str| EntryItem {
        path: name.into(),
        entry_type: EntryType::File,
    };
    let fixed = |p: &analyze::PreparedAnalysis, s| {
        p.spec
            .sections
            .iter()
            .find(|r| r.section == s)
            .unwrap()
            .entries
            .clone()
    };
    assert_eq!(fixed(&source, AnalysisSection::Tree), [entry("main.rs")]);
    assert_eq!(fixed(&source, AnalysisSection::Manifests), []);
    let manifest = f.prepare(&f.root.join("Cargo.toml"));
    assert_eq!(
        fixed(&manifest, AnalysisSection::Manifests),
        [entry("Cargo.toml")]
    );
    // The parent result of a file has the fixed sections and the statistics.
    let parent = f.start(&f.root.join("Cargo.toml"));
    assert_eq!(parent.children.len(), 1);
    f.produce(
        &parent,
        AnalysisSection::Languages,
        &fake(&f.bin, "tokei", &tokei_fixture()),
    );
    let Materialized::Ready(m) = f.materialize(&parent, &never) else {
        panic!("all sections finished");
    };
    assert_eq!(m.aggregate_state, Some(TaskState::Done));
    let result = &f.page(&parent.execution_id, None)["result"];
    assert_eq!(result["summary"]["input_type"], "file");
    assert_eq!(
        sections(result, "tree"),
        [json!({"section":"tree","path":"Cargo.toml","type":"file"})]
    );
    assert_eq!(sections(result, "manifests").len(), 1);
    assert_eq!(child(result, "tree")["execution_id"], Value::Null);
    assert_eq!(child(result, "tree")["state"], "done");
}

#[test]
fn three_finished_children_are_published_once_and_reread_without_the_backend() {
    let f = Fixture::new();
    let parent = f.start(&f.root);
    assert_eq!(parent.children.len(), 3);
    let tokei = fake(&f.bin, "tokei", &tokei_fixture());
    let find = fake(
        &f.bin,
        "fd-find",
        r"printf 'Cargo.toml\0pkg/package.json\0'",
    );
    let list = fake(&f.bin, "fd-list", r"printf 'Cargo.toml\0pkg/\0src/\0'");
    f.produce(&parent, AnalysisSection::Languages, &tokei);
    f.produce(&parent, AnalysisSection::Manifests, &find);
    // A child still running keeps the parent pending, without publishing anything.
    let running = |_: &ExecutionId| Ok(ChildState::Running);
    let Materialized::Pending { children, error } = f.materialize(&parent, &running) else {
        panic!("tree is still running");
    };
    assert_eq!(error, None);
    assert_eq!(children[2].state, Some(ChildState::Running));
    assert_eq!(children[0].state, Some(ChildState::Done));
    assert!(
        analyze::stored(&f.store, &f.settings, &parent)
            .unwrap()
            .is_none()
    );
    f.produce(&parent, AnalysisSection::Tree, &list);
    let Materialized::Ready(first) = f.materialize(&parent, &never) else {
        panic!("all children finished");
    };
    assert_eq!(first.kind, "fs.analyze");
    assert_eq!(first.aggregate_state, Some(TaskState::Done));
    let output = f.page(&parent.execution_id, None);
    let result = &output["result"];
    assert_eq!(result["kind"], "fs.analyze");
    let summary = &result["summary"];
    assert_eq!(summary["path"], f.root.to_str().unwrap());
    assert_eq!(
        (summary["tree_depth"].clone(), summary["scan_depth"].clone()),
        (json!(1), json!(32))
    );
    assert_eq!(summary["snapshot"], false);
    assert_eq!(summary["follows_symlinks"], false);
    assert_eq!(
        summary["items"],
        json!({"languages":2,"manifests":2,"tree":3})
    );
    assert_eq!(
        summary["totals"],
        json!({"files":3,"code":8,"comments":1,"blanks":1})
    );
    assert_eq!(
        summary["scope"],
        json!([
            {"section":"languages","tool":"tokei","max_depth":null},
            {"section":"manifests","tool":"fd","max_depth":32},
            {"section":"tree","tool":"fd","max_depth":1},
        ])
    );
    assert!(
        summary["completed_at"].as_u64().unwrap() >= summary["registered_at"].as_u64().unwrap()
    );
    assert_eq!(
        sections(result, "manifests"),
        [
            json!({"section":"manifests","path":"Cargo.toml","type":"file"}),
            json!({"section":"manifests","path":"pkg/package.json","type":"file"}),
        ]
    );
    assert_eq!(sections(result, "tree").len(), 3);
    assert_eq!(sections(result, "languages")[1]["language"], "Rust");
    for (i, section) in ["languages", "manifests", "tree"].iter().enumerate() {
        let c = child(result, section);
        assert_eq!(c["state"], "done");
        assert_eq!(c["execution_id"], json!(parent.children[i]));
        assert_eq!(c["result_availability"], "available");
    }
    assert_eq!(result["completeness"]["scope_complete"], true);
    // Re-reading is served from the stored aggregate: no backend, no rebuild, no tool run.
    let again = analyze::stored(&f.store, &f.settings, &parent)
        .unwrap()
        .unwrap();
    assert_eq!(again.state, Some(TaskState::Done));
    assert_eq!(again.children.len(), 3);
    let Materialized::Ready(second) = f.materialize(&parent, &never) else {
        panic!("published");
    };
    assert_eq!(second.finished_at, first.finished_at);
    assert_eq!(f.page(&parent.execution_id, None), output);
    for tool in ["tokei", "fd-find", "fd-list"] {
        assert_eq!(runs(&f.bin, tool), 1, "{tool}");
    }
}

#[test]
fn a_failed_child_gives_a_partial_result_with_its_error() {
    let f = Fixture::new();
    let parent = f.start(&f.root);
    f.produce(
        &parent,
        AnalysisSection::Languages,
        &fake(&f.bin, "tokei", "echo broken >&2; exit 3"),
    );
    f.produce(
        &parent,
        AnalysisSection::Manifests,
        &fake(&f.bin, "fd-find", r"printf 'Cargo.toml\0'"),
    );
    f.produce(
        &parent,
        AnalysisSection::Tree,
        &fake(&f.bin, "fd-list", r"printf 'Cargo.toml\0src/\0'"),
    );
    let Materialized::Ready(m) = f.materialize(&parent, &never) else {
        panic!("all children finished");
    };
    assert_eq!(m.aggregate_state, Some(TaskState::Partial));
    let result = &f.page(&parent.execution_id, None)["result"];
    let failed = child(result, "languages");
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["exit_code"], 3);
    assert_eq!(failed["error"]["code"], "backend_failure");
    assert_eq!(child(result, "manifests")["state"], "done");
    // The failed section contributes its error, not its items; the others are kept.
    assert!(sections(result, "languages").is_empty());
    assert_eq!(sections(result, "manifests").len(), 1);
    assert_eq!(sections(result, "tree").len(), 2);
    assert_eq!(result["summary"]["totals"], Value::Null);
    assert_eq!(result["errors"][0]["section"], "languages");
    assert_eq!(result["completeness"]["partial"], true);
    assert_eq!(result["completeness"]["scope_complete"], false);
    for tool in ["tokei", "fd-find", "fd-list"] {
        assert_eq!(runs(&f.bin, tool), 1, "{tool}");
    }
}

#[test]
fn a_child_lost_before_storing_its_result_fails_only_its_section() {
    let f = Fixture::new();
    let parent = f.start(&f.root);
    f.produce(
        &parent,
        AnalysisSection::Languages,
        &fake(&f.bin, "tokei", &tokei_fixture()),
    );
    let ended = parent.children[1].clone();
    let backend = move |id: &ExecutionId| {
        if *id == ended {
            Ok(ChildState::Failed)
        } else {
            Err(Error::new(ErrorCode::Expired, "removed"))
        }
    };
    let Materialized::Ready(m) = f.materialize(&parent, &backend) else {
        panic!("every child ended");
    };
    assert_eq!(m.aggregate_state, Some(TaskState::Partial));
    let result = &f.page(&parent.execution_id, None)["result"];
    assert_eq!(
        child(result, "manifests")["error"]["code"],
        "output_missing"
    );
    assert_eq!(child(result, "tree")["error"]["code"], "expired");
    assert_eq!(child(result, "languages")["state"], "done");
    // An unobservable backend is not a failure: the parent stays pending.
    let g = Fixture::new();
    let parent = g.start(&g.root);
    let down = |_: &ExecutionId| Err(Error::new(ErrorCode::DaemonUnavailable, "down"));
    let Materialized::Pending { error, .. } = g.materialize(&parent, &down) else {
        panic!("unknown child states");
    };
    assert_eq!(error.unwrap().code, ErrorCode::DaemonUnavailable);
}

#[test]
fn registration_stops_at_the_first_failure_and_keeps_registered_children() {
    let f = Fixture::new();
    let prepared = f.prepare(&f.root);
    let calls = Cell::new(0);
    let (record, error) = analyze::register(&f.store, &prepared, |child| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            Ok(f.record(&child.plan))
        } else {
            Err(RegistrationFailure {
                error: Error::new(ErrorCode::DaemonUnavailable, "daemon stopped"),
                receipt: None,
                execution_id: None,
            })
        }
    })
    .unwrap();
    // No retry and no further registration after the failure.
    assert_eq!(calls.get(), 2);
    assert_eq!(error.unwrap().code, ErrorCode::DaemonUnavailable);
    assert_eq!(record.children.len(), 1);
    let spec = record.analysis.as_ref().unwrap();
    assert_eq!(
        spec.sections[1].error.as_ref().unwrap().code,
        ErrorCode::DaemonUnavailable
    );
    assert_eq!(
        spec.sections[2].error.as_ref().unwrap().code,
        ErrorCode::BackendFailure
    );
    assert_eq!(
        parents::find(&f.store, &record.execution_id).unwrap(),
        Some(record.clone())
    );
    // The registered child is collected; the unregistered sections are failed in the result.
    f.produce(
        &record,
        AnalysisSection::Languages,
        &fake(&f.bin, "tokei", &tokei_fixture()),
    );
    let Materialized::Ready(m) = f.materialize(&record, &never) else {
        panic!("the only child finished");
    };
    assert_eq!(m.aggregate_state, Some(TaskState::Partial));
    // A child whose record was saved before its registration reported an error is kept.
    let (with_receipt, _) = analyze::register(&f.store, &prepared, |child| {
        Err(RegistrationFailure {
            error: Error::new(ErrorCode::StoreFailure, "intent update failed"),
            receipt: Some(Box::new(f.record(&child.plan))),
            execution_id: None,
        })
    })
    .unwrap();
    assert_eq!(with_receipt.children.len(), 1);
    // Nothing registered: no parent.
    let parents_before = parents::history(&f.store).unwrap().len();
    let failure = analyze::register(&f.store, &prepared, |_| {
        Err(RegistrationFailure {
            error: Error::new(ErrorCode::DaemonUnavailable, "down"),
            receipt: None,
            execution_id: None,
        })
    })
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::DaemonUnavailable);
    assert!(failure.registered.is_empty());
    assert_eq!(parents::history(&f.store).unwrap().len(), parents_before);
    // The parent cannot be saved: the registered children are returned, not registered again.
    let store_root = f.temp.path().join("store");
    let calls = Cell::new(0);
    let failure = analyze::register(&f.store, &prepared, |child| {
        let record = f.record(&child.plan);
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            fs::set_permissions(&store_root, fs::Permissions::from_mode(0o750)).unwrap();
        }
        Ok(record)
    })
    .unwrap_err();
    fs::set_permissions(&store_root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(failure.error.code, ErrorCode::PermissionDenied);
    assert_eq!(failure.registered.len(), 3);
}

#[test]
fn a_large_aggregate_is_paged_and_a_small_budget_truncates_the_tree() {
    let f = Fixture::new();
    let mut listing = String::new();
    for i in 0..250 {
        write(&f.root, &format!("many/f{i:03}"), "");
        listing += &format!("many/f{i:03}\\0");
    }
    let parent = f.start(&f.root);
    f.produce(
        &parent,
        AnalysisSection::Languages,
        &fake(&f.bin, "tokei", &tokei_fixture()),
    );
    f.produce(
        &parent,
        AnalysisSection::Manifests,
        &fake(&f.bin, "fd-find", r"printf 'Cargo.toml\0'"),
    );
    f.produce(
        &parent,
        AnalysisSection::Tree,
        &fake(&f.bin, "fd-list", &format!("printf '{listing}'")),
    );
    assert!(matches!(
        f.materialize(&parent, &never),
        Materialized::Ready(_)
    ));
    let mut cursor: Option<String> = None;
    let mut seen = vec![];
    loop {
        let output = f.page(&parent.execution_id, cursor.as_deref());
        let result = &output["result"];
        assert!(result["items"].as_array().unwrap().len() <= 100);
        // Every page repeats the children's outcomes.
        assert_eq!(result["children"].as_array().unwrap().len(), 3);
        seen.extend(result["items"].as_array().unwrap().iter().cloned());
        match result["page"]["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.into()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 2 + 1 + 250);
    assert_eq!(seen.iter().filter(|i| i["section"] == "tree").count(), 250);
    // The byte budget drops tree entries first and says so.
    let prepared = f.prepare(&f.root);
    let entries = (0..250)
        .map(|i| EntryItem {
            path: format!("many/f{i:03}"),
            entry_type: EntryType::File,
        })
        .collect();
    let inputs = vec![
        SectionInput {
            section: AnalysisSection::Languages,
            execution_id: None,
            outcome: SectionOutcome::Fixed(vec![]),
        },
        SectionInput {
            section: AnalysisSection::Tree,
            execution_id: None,
            outcome: SectionOutcome::Fixed(entries),
        },
    ];
    let mut doc = analyze::build(&prepared.spec, 1, &inputs);
    assert_eq!(doc.summary.items.tree, 250);
    let (header, lines) = analyze::encode(&mut doc, 8192).unwrap();
    assert!(lines.len() < 250);
    assert_eq!(doc.summary.items.tree, lines.len() as u64);
    assert!(doc.completeness.truncated);
    assert_eq!(
        doc.completeness.truncation_reasons,
        [TruncationReason::ArtifactBytes]
    );
    assert!(header.len() + lines.iter().map(Vec::len).sum::<usize>() <= 8192);
}

#[test]
fn aggregate_rejects_an_oversized_header_with_or_without_items() {
    let f = Fixture::new();
    let mut prepared = f.prepare(&f.root);
    prepared.spec.path = "x".repeat(4096);
    for entries in [
        vec![],
        vec![EntryItem {
            path: "file".into(),
            entry_type: EntryType::File,
        }],
    ] {
        let inputs = [SectionInput {
            section: AnalysisSection::Tree,
            execution_id: None,
            outcome: SectionOutcome::Fixed(entries),
        }];
        let mut doc = analyze::build(&prepared.spec, 1, &inputs);
        assert_eq!(
            analyze::encode(&mut doc, 1024).unwrap_err().code,
            ErrorCode::OutputLimitTooSmall
        );
    }
}

#[test]
fn parent_time_precedes_child_registration_even_across_a_second_boundary() {
    let f = Fixture::new();
    let prepared = f.prepare(&f.root.join("src/main.rs"));
    let mut child_started = 0;
    let (parent, error) = analyze::register(&f.store, &prepared, |child| {
        child_started = results::now();
        while results::now() == child_started {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(f.record(&child.plan))
    })
    .unwrap();
    assert_eq!(error, None);
    assert!(parent.registered_at <= child_started);
}

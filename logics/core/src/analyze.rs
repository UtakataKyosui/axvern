//! `fs analyze`: one logical parent over a child execution per section.
//!
//! - languages: `code stats` (tokei) of the target, without a depth limit (tokei has none).
//! - manifests: `fs find` (fd) of [`MANIFEST_NAMES`] down to [`SCAN_DEPTH`].
//! - tree: `fs list` (fd) down to the requested depth only. fd rather than eza, for the reasons
//!   in `adapters::tools` (eza escapes names lossily and descends into `.git`).
//!
//! A file target registers only the languages child: its tree is the file itself and it is a
//! manifest only by name, both decided at registration without reading the directory around it.
//! Every child is prepared, judged by Policy and checked for its tool before any is registered.
//! The parent's result is built once, after every child finished, from the children's stored
//! results. It is then read like any other stored result and needs neither the children's
//! Pueue tasks nor their results; reading it never registers or reruns a child.
use crate::{
    config::{Config, Results},
    discovery::{self, Prepared},
    parents,
    results::{self, Derivation, Manifest},
    services::{self, ExecutionService},
    store::{ExecutionStore, RegistrationFailure},
};
use axvern_models::*;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

pub const KIND: &str = "fs.analyze";
/// Room kept for the header when truncation adds a reason to it.
const HEADER_SLACK: u64 = 512;

/// Validated children in section order, and the parent's spec before registration.
#[derive(Debug)]
pub struct PreparedAnalysis {
    pub spec: AnalysisSpec,
    pub children: Vec<(AnalysisSection, Prepared)>,
}

fn section(section: AnalysisSection) -> SectionRegistration {
    SectionRegistration {
        section,
        execution_id: None,
        entries: vec![],
        error: None,
    }
}
fn manifest_pattern() -> String {
    format!("{{{}}}", MANIFEST_NAMES.join(","))
}

pub fn prepare(
    path: &Path,
    tree_depth: u32,
    include_ignored: bool,
    cwd: &Path,
    limits: &DiscoveryLimits,
) -> Result<PreparedAnalysis, Error> {
    if !(1..=MAX_DEPTH).contains(&tree_depth) {
        return Err(Error::new(ErrorCode::InvalidInput, "depth must be 1..32"));
    }
    let target = discovery::resolve(Some(path), cwd)?;
    let meta = fs::metadata(&target).map_err(|e| discovery::target_error(&e, &target))?;
    let text = target.to_str().unwrap_or_default().to_owned();
    let child = |request: DiscoveryRequest| discovery::prepare(&request, cwd, limits);
    let languages = child(DiscoveryRequest::CodeStats {
        path: Some(target.clone()),
        include_ignored,
    })?;
    if meta.is_dir() {
        let manifests = child(DiscoveryRequest::Find {
            pattern: manifest_pattern(),
            path: Some(target.clone()),
            syntax: PatternSyntax::Glob,
            full_path: false,
            extensions: vec![],
            entry_type: EntryFilter::File,
            depth: SCAN_DEPTH,
            include_ignored,
            ignore_case: false,
        })?;
        let tree = child(DiscoveryRequest::List {
            path: Some(target.clone()),
            depth: tree_depth,
            include_ignored,
        })?;
        return Ok(PreparedAnalysis {
            spec: AnalysisSpec {
                path: text.clone(),
                root: text,
                input_type: InputType::Directory,
                tree_depth,
                scan_depth: SCAN_DEPTH,
                include_ignored,
                limits: limits.clone(),
                sections: [
                    AnalysisSection::Languages,
                    AnalysisSection::Manifests,
                    AnalysisSection::Tree,
                ]
                .map(section)
                .into(),
            },
            children: vec![
                (AnalysisSection::Languages, languages),
                (AnalysisSection::Manifests, manifests),
                (AnalysisSection::Tree, tree),
            ],
        });
    }
    // `code stats` accepted the target, so it is a readable regular file with a UTF-8 path.
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let root = target
        .parent()
        .and_then(Path::to_str)
        .unwrap_or_default()
        .to_owned();
    let entry = EntryItem {
        path: name.clone(),
        entry_type: EntryType::File,
    };
    let mut manifests = section(AnalysisSection::Manifests);
    if MANIFEST_NAMES.contains(&name.as_str()) {
        manifests.entries.push(entry.clone());
    }
    let mut tree = section(AnalysisSection::Tree);
    tree.entries.push(entry);
    Ok(PreparedAnalysis {
        spec: AnalysisSpec {
            path: text,
            root,
            input_type: InputType::File,
            tree_depth: 0,
            scan_depth: 0,
            include_ignored,
            limits: limits.clone(),
            sections: vec![section(AnalysisSection::Languages), manifests, tree],
        },
        children: vec![(AnalysisSection::Languages, languages)],
    })
}

/// Every child must pass Policy and find its tool; the first refusal stops everything before
/// anything is registered. `approval` answers every child whose Policy asks.
pub fn preflight(
    config: &Config,
    prepared: &PreparedAnalysis,
    interactive: bool,
    approval: Approval,
) -> Result<(), Error> {
    for (_, child) in &prepared.children {
        services::authorize(config, &child.plan, interactive, approval)?;
    }
    for (_, child) in &prepared.children {
        discovery::ensure_tool(&child.job, &child.plan)?;
    }
    Ok(())
}

/// Registration stopped after some children were registered: keep their IDs.
#[derive(Debug)]
pub struct StartFailure {
    pub error: Error,
    pub registered: Vec<ExecutionId>,
}

/// Register each child once, in section order, then persist the parent. A failed registration
/// stops the remaining ones; children already registered stay in the parent instead of being
/// registered again, and the failure is returned with it.
pub fn register(
    store: &ExecutionStore,
    prepared: &PreparedAnalysis,
    mut register: impl FnMut(&Prepared) -> Result<ExecutionRecord, RegistrationFailure>,
) -> Result<(ParentRecord, Option<Error>), StartFailure> {
    let execution_id = ExecutionStore::allocate_id().map_err(|error| StartFailure {
        error,
        registered: vec![],
    })?;
    let mut spec = prepared.spec.clone();
    let mut failed: Option<Error> = None;
    let mut children = vec![];
    for (name, child) in &prepared.children {
        let Some(slot) = spec.sections.iter_mut().find(|s| s.section == *name) else {
            continue;
        };
        if failed.is_some() {
            slot.error = Some(Error::new(
                ErrorCode::BackendFailure,
                "not registered because an earlier child registration failed",
            ));
            continue;
        }
        match register(child) {
            Ok(record) => slot.execution_id = Some(record.execution_id),
            Err(f) => {
                // A receipt means the child was registered and its record saved.
                if let Some(receipt) = f.receipt {
                    slot.execution_id = Some(receipt.execution_id);
                } else if let Some(id) = &f.execution_id {
                    slot.error = Some(Error::new(
                        ErrorCode::RegistrationUnknown,
                        format!("registration of {id} needs reconciliation; it was not retried"),
                    ));
                } else {
                    slot.error = Some(f.error.clone());
                }
                failed = Some(f.error);
            }
        }
        if let Some(id) = &slot.execution_id {
            children.push(id.clone());
        }
    }
    if children.is_empty() {
        return Err(StartFailure {
            error: failed.unwrap_or_else(|| {
                Error::new(ErrorCode::BackendFailure, "no child was registered")
            }),
            registered: vec![],
        });
    }
    let record = ParentRecord {
        execution_id,
        registered_at: results::now(),
        cwd: spec.root.clone(),
        children: children.clone(),
        analysis: Some(spec),
    };
    parents::save(store, &record).map_err(|error| StartFailure {
        error,
        registered: children,
    })?;
    Ok((record, failed))
}

/// A section as read from the store when the parent result is built.
#[derive(Debug)]
pub enum SectionOutcome {
    /// Decided at registration from the target's own metadata.
    Fixed(Vec<EntryItem>),
    /// The child finished and its result is available.
    Stored {
        state: ChildState,
        exit_code: Option<i32>,
        finished_at: u64,
        header: Value,
        lines: Vec<Vec<u8>>,
    },
    /// No usable result: not registered, ended without a result, or the result is gone.
    Failed {
        state: Option<ChildState>,
        exit_code: Option<i32>,
        finished_at: Option<u64>,
        result_availability: Option<String>,
        error: Error,
    },
    /// Still queued or running, or its state could not be observed.
    Active(Option<ChildState>),
}
#[derive(Debug)]
pub struct SectionInput {
    pub section: AnalysisSection,
    pub execution_id: Option<ExecutionId>,
    pub outcome: SectionOutcome,
}
impl SectionInput {
    fn usable(&self) -> bool {
        matches!(
            self.outcome,
            SectionOutcome::Fixed(_)
                | SectionOutcome::Stored {
                    state: ChildState::Done,
                    ..
                }
        )
    }
    fn child(&self) -> AnalysisChild {
        let mut child = AnalysisChild {
            section: self.section,
            execution_id: self.execution_id.clone(),
            state: None,
            exit_code: None,
            finished_at: None,
            result_availability: None,
            completeness: None,
            error: None,
        };
        match &self.outcome {
            SectionOutcome::Fixed(_) => child.state = Some(ChildState::Done),
            SectionOutcome::Stored {
                state,
                exit_code,
                finished_at,
                header,
                ..
            } => {
                child.state = Some(*state);
                child.exit_code = *exit_code;
                child.finished_at = Some(*finished_at);
                child.result_availability = Some("available".into());
                child.completeness = serde_json::from_value(header["completeness"].clone()).ok();
                if *state != ChildState::Done {
                    child.error = Some(first_error(header));
                }
            }
            SectionOutcome::Failed {
                state,
                exit_code,
                finished_at,
                result_availability,
                error,
            } => {
                child.state = *state;
                child.exit_code = *exit_code;
                child.finished_at = *finished_at;
                child.result_availability = result_availability.clone();
                child.error = Some(error.clone());
            }
            SectionOutcome::Active(state) => child.state = *state,
        }
        child
    }
}
fn first_error(header: &Value) -> Error {
    header["errors"]
        .get(0)
        .and_then(|e| serde_json::from_value::<ResultIssue>(e.clone()).ok())
        .map(|e| Error::new(e.code, e.message))
        .unwrap_or_else(|| Error::new(ErrorCode::BackendFailure, "the child failed"))
}

/// Aggregate state: done when every section is usable, partial when some are, else failed.
pub fn aggregate_state(inputs: &[SectionInput]) -> TaskState {
    let usable = inputs.iter().filter(|i| i.usable()).count();
    if usable == inputs.len() {
        TaskState::Done
    } else if usable > 0 {
        TaskState::Partial
    } else {
        TaskState::Failed
    }
}

/// Build the parent result from finished sections. Items of a failed child are left out; its
/// errors are kept. Issues carry the section they came from.
pub fn build(spec: &AnalysisSpec, registered_at: u64, inputs: &[SectionInput]) -> AnalysisDocument {
    let mut items = vec![];
    let mut counts = SectionCounts::default();
    let mut totals = None;
    let mut reasons: Vec<TruncationReason> = vec![];
    let mut partial = false;
    // Section failures come before the issues the children reported.
    let mut failures: Vec<ResultIssue> = vec![];
    let mut reported: Vec<ResultIssue> = vec![];
    let mut warnings: Vec<ResultWarning> = vec![];
    let mut add = |section: AnalysisSection, item: DiscoveryItem, items: &mut Vec<AnalysisItem>| {
        let expected = match section {
            AnalysisSection::Languages => matches!(item, DiscoveryItem::Stat(_)),
            _ => matches!(item, DiscoveryItem::Entry(_)),
        };
        if expected {
            match section {
                AnalysisSection::Languages => counts.languages += 1,
                AnalysisSection::Manifests => counts.manifests += 1,
                AnalysisSection::Tree => counts.tree += 1,
            }
            items.push(AnalysisItem { section, item });
        }
    };
    for input in inputs {
        if !input.usable() {
            partial = true;
        }
        match &input.outcome {
            SectionOutcome::Fixed(entries) => {
                for entry in entries {
                    add(
                        input.section,
                        DiscoveryItem::Entry(entry.clone()),
                        &mut items,
                    );
                }
            }
            SectionOutcome::Stored {
                state,
                header,
                lines,
                ..
            } => {
                if let Ok(c) =
                    serde_json::from_value::<Completeness>(header["completeness"].clone())
                {
                    partial |= c.partial;
                    for reason in c.truncation_reasons {
                        if !reasons.contains(&reason) {
                            reasons.push(reason);
                        }
                    }
                }
                for issue in header["errors"].as_array().into_iter().flatten() {
                    if let Ok(mut issue) = serde_json::from_value::<ResultIssue>(issue.clone()) {
                        issue.section = Some(input.section);
                        reported.push(issue);
                    }
                }
                for warning in header["warnings"].as_array().into_iter().flatten() {
                    if let Ok(mut warning) =
                        serde_json::from_value::<ResultWarning>(warning.clone())
                        && warnings.len() < MAX_ISSUES
                    {
                        warning.section = Some(input.section);
                        warnings.push(warning);
                    }
                }
                if *state != ChildState::Done {
                    continue;
                }
                if input.section == AnalysisSection::Languages {
                    let s = &header["summary"];
                    if let (Some(files), Some(code), Some(comments), Some(blanks)) = (
                        s["files"].as_u64(),
                        s["code"].as_u64(),
                        s["comments"].as_u64(),
                        s["blanks"].as_u64(),
                    ) {
                        totals = Some(LanguageTotals {
                            files,
                            code,
                            comments,
                            blanks,
                        });
                    }
                }
                for line in lines {
                    if let Ok(item) = serde_json::from_slice::<DiscoveryItem>(line) {
                        add(input.section, item, &mut items);
                    }
                }
            }
            SectionOutcome::Failed { error, .. } => {
                let mut issue = ResultIssue::new(error.code, error.message.clone());
                issue.section = Some(input.section);
                failures.push(issue);
            }
            SectionOutcome::Active(_) => (),
        }
    }
    let mut errors = failures;
    errors.extend(reported);
    if errors.len() > MAX_ISSUES {
        errors.truncate(MAX_ISSUES - 1);
        errors.push(ResultIssue::new(
            ErrorCode::PartialResult,
            "further errors were omitted",
        ));
    }
    let directory = spec.input_type == InputType::Directory;
    let scope = |section, tool: &str, depth: Option<u32>| SectionScope {
        section,
        tool: directory.then(|| tool.to_owned()),
        max_depth: depth,
    };
    let completed_at = inputs
        .iter()
        .filter_map(|i| match &i.outcome {
            SectionOutcome::Stored { finished_at, .. } => Some(*finished_at),
            SectionOutcome::Failed { finished_at, .. } => *finished_at,
            _ => None,
        })
        .max();
    AnalysisDocument {
        summary: AnalysisSummary {
            path: spec.path.clone(),
            root: spec.root.clone(),
            input_type: spec.input_type,
            tree_depth: spec.tree_depth,
            scan_depth: spec.scan_depth,
            include_ignored: spec.include_ignored,
            follows_symlinks: false,
            manifest_names: MANIFEST_NAMES.map(String::from).into(),
            scope: vec![
                SectionScope {
                    section: AnalysisSection::Languages,
                    tool: Some("tokei".into()),
                    max_depth: None,
                },
                scope(AnalysisSection::Manifests, "fd", Some(spec.scan_depth)),
                scope(AnalysisSection::Tree, "fd", Some(spec.tree_depth)),
            ],
            limits: spec.limits.clone(),
            registered_at,
            completed_at,
            snapshot: false,
            items: counts,
            totals,
        },
        children: inputs.iter().map(SectionInput::child).collect(),
        items,
        completeness: Completeness::new(reasons, partial),
        errors,
        warnings,
    }
}

fn encode_line(value: &impl serde::Serialize) -> Result<Vec<u8>, Error> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| Error::new(ErrorCode::StoreFailure, "result cannot be encoded"))?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn encode_header(doc: &AnalysisDocument) -> Result<Vec<u8>, Error> {
    encode_line(&serde_json::json!({
        "summary": doc.summary, "children": doc.children, "completeness": doc.completeness,
        "errors": doc.errors, "warnings": doc.warnings,
    }))
}
/// Header and as many item lines as `budget` allows, recording an `artifact_bytes` truncation.
pub fn encode(doc: &mut AnalysisDocument, budget: u64) -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
    let lines: Vec<Vec<u8>> = doc
        .items
        .iter()
        .map(encode_line)
        .collect::<Result<_, _>>()?;
    let mut header = encode_header(doc)?;
    let mut total = header.len() as u64 + HEADER_SLACK;
    let mut keep = lines.len();
    for (i, line) in lines.iter().enumerate() {
        if total + line.len() as u64 > budget {
            keep = i;
            break;
        }
        total += line.len() as u64;
    }
    if keep < lines.len() {
        for item in &doc.items[keep..] {
            let count = match item.section {
                AnalysisSection::Languages => &mut doc.summary.items.languages,
                AnalysisSection::Manifests => &mut doc.summary.items.manifests,
                AnalysisSection::Tree => &mut doc.summary.items.tree,
            };
            *count -= 1;
        }
        doc.items.truncate(keep);
        let mut reasons = doc.completeness.truncation_reasons.clone();
        if !reasons.contains(&TruncationReason::ArtifactBytes) {
            reasons.push(TruncationReason::ArtifactBytes);
        }
        doc.completeness = Completeness::new(reasons, doc.completeness.partial);
        header = encode_header(doc)?;
    }
    Ok((header, lines.into_iter().take(keep).collect()))
}

pub enum Materialized {
    Ready(Manifest),
    Pending {
        children: Vec<AnalysisChild>,
        error: Option<Error>,
    },
}
fn missing(code: ErrorCode, message: &str) -> SectionOutcome {
    SectionOutcome::Failed {
        state: Some(ChildState::Failed),
        exit_code: None,
        finished_at: None,
        result_availability: Some("unavailable".into()),
        error: Error::new(code, message),
    }
}
fn read_section(
    d: &Derivation,
    registration: &SectionRegistration,
    backend: &dyn Fn(&ExecutionId) -> Result<ChildState, Error>,
) -> Result<SectionOutcome, Error> {
    let Some(id) = &registration.execution_id else {
        return Ok(match &registration.error {
            Some(error) => SectionOutcome::Failed {
                state: None,
                exit_code: None,
                finished_at: None,
                result_availability: None,
                error: error.clone(),
            },
            None => SectionOutcome::Fixed(registration.entries.clone()),
        });
    };
    match d.manifest(id)? {
        Some(m) if m.finished_at.is_some() => {
            let state = m.state.unwrap_or(ChildState::Failed);
            let finished_at = m.finished_at.unwrap_or_default();
            let failed = |availability: &str, error: Error| SectionOutcome::Failed {
                state: Some(state),
                exit_code: m.exit_code,
                finished_at: Some(finished_at),
                result_availability: Some(availability.into()),
                error,
            };
            Ok(match m.availability.as_str() {
                "available" => match d.document(&m) {
                    Ok((header, lines)) => SectionOutcome::Stored {
                        state,
                        exit_code: m.exit_code,
                        finished_at,
                        header,
                        lines,
                    },
                    Err(error) => failed("unavailable", error),
                },
                "expired" => failed(
                    "expired",
                    Error::new(ErrorCode::ResultExpired, "the child's result expired"),
                ),
                "evicted" => failed(
                    "evicted",
                    Error::new(ErrorCode::ResultEvicted, "the child's result was evicted"),
                ),
                other => failed(
                    other,
                    m.error.clone().unwrap_or_else(|| {
                        Error::new(ErrorCode::OutputMissing, "the child stored no result")
                    }),
                ),
            })
        }
        // The Pueue task is the worker itself, so an ended task without a finished manifest
        // means the worker stopped before storing a result.
        _ => Ok(match backend(id) {
            Ok(state @ (ChildState::Queued | ChildState::Running)) => {
                SectionOutcome::Active(Some(state))
            }
            Ok(_) => missing(
                ErrorCode::OutputMissing,
                "the child ended without storing a result",
            ),
            Err(e) if e.code == ErrorCode::Expired => missing(
                ErrorCode::Expired,
                "the child's task was removed before it stored a result",
            ),
            Err(_) => SectionOutcome::Active(None),
        }),
    }
}

/// Publish the parent result once every section has finished; otherwise report the sections.
/// `backend` is consulted only for children without a finished result manifest; it must be
/// computed before the call because the store lock is held here.
pub fn materialize(
    store: &ExecutionStore,
    settings: &Results,
    record: &ParentRecord,
    backend: &dyn Fn(&ExecutionId) -> Result<ChildState, Error>,
) -> Result<Materialized, Error> {
    let spec = record
        .analysis
        .as_ref()
        .ok_or_else(|| Error::new(ErrorCode::InvalidInput, "not an fs analyze execution"))?;
    let d = Derivation::begin(store, settings)?;
    if let Some(m) = d.manifest(&record.execution_id)? {
        return Ok(Materialized::Ready(m));
    }
    let mut inputs = vec![];
    for registration in &spec.sections {
        inputs.push(SectionInput {
            section: registration.section,
            execution_id: registration.execution_id.clone(),
            outcome: read_section(&d, registration, backend)?,
        });
    }
    if inputs
        .iter()
        .any(|i| matches!(i.outcome, SectionOutcome::Active(_)))
    {
        let error = record
            .children
            .iter()
            .find_map(|id| backend(id).err().filter(|e| e.code != ErrorCode::Expired));
        return Ok(Materialized::Pending {
            children: inputs.iter().map(SectionInput::child).collect(),
            error,
        });
    }
    let state = aggregate_state(&inputs);
    let mut doc = build(spec, record.registered_at, &inputs);
    let finished_at = doc.summary.completed_at.unwrap_or_else(results::now);
    let (header, lines) = encode(&mut doc, u64::MAX)?;
    let wanted = lines.iter().map(|l| l.len() as u64).sum::<u64>() + header.len() as u64;
    let budget = d.budget(wanted + HEADER_SLACK)?;
    let (header, lines) = if budget < wanted + HEADER_SLACK {
        encode(&mut doc, budget)?
    } else {
        (header, lines)
    };
    let m = d.publish(
        &record.execution_id,
        KIND,
        state,
        finished_at,
        &header,
        &lines,
        doc.completeness.truncated,
    )?;
    Ok(Materialized::Ready(m))
}

/// What is known of a parent: its stored result, or the state of its sections so far.
#[derive(Debug)]
pub struct ParentState {
    pub state: Option<TaskState>,
    pub children: Vec<AnalysisChild>,
    pub manifest: Option<Manifest>,
    pub error: Option<Error>,
}
fn registered_children(spec: &AnalysisSpec) -> Vec<AnalysisChild> {
    spec.sections
        .iter()
        .map(|s| AnalysisChild {
            section: s.section,
            execution_id: s.execution_id.clone(),
            state: None,
            exit_code: None,
            finished_at: None,
            result_availability: None,
            completeness: None,
            error: s.error.clone(),
        })
        .collect()
}
/// The parent's stored result, read without consulting Pueue.
pub fn stored(
    store: &ExecutionStore,
    settings: &Results,
    record: &ParentRecord,
) -> Result<Option<ParentState>, Error> {
    let spec = record
        .analysis
        .as_ref()
        .ok_or_else(|| Error::new(ErrorCode::InvalidInput, "not an fs analyze execution"))?;
    let Some((m, head)) = results::header(store, settings, &record.execution_id)? else {
        return Ok(None);
    };
    let children = serde_json::from_value(head["children"].clone())
        .unwrap_or_else(|_| registered_children(spec));
    Ok(Some(ParentState {
        state: m.aggregate_state.or_else(|| m.state.map(TaskState::from)),
        children,
        manifest: Some(m),
        error: None,
    }))
}
/// The children's section states before anything is observed.
pub fn registered(record: &ParentRecord) -> Vec<AnalysisChild> {
    record
        .analysis
        .as_ref()
        .map(registered_children)
        .unwrap_or_default()
}
/// Observe the children once and publish the parent result when all have finished.
pub fn observe(
    service: &ExecutionService,
    record: &ParentRecord,
    timeout: Duration,
) -> Result<ParentState, Error> {
    let settings = service.result_settings();
    if let Some(state) = stored(&service.store, settings, record)? {
        return Ok(state);
    }
    let mut adapter = service.adapter.clone();
    adapter.timeout = adapter.timeout.min(timeout);
    let tasks = adapter.list();
    let mut states = HashMap::new();
    for id in &record.children {
        let child = service.store.find_record(id)?;
        let state = match &tasks {
            Ok(tasks) => tasks
                .iter()
                .find(|t| t.identity == child.backend)
                .map(|t| t.state)
                .ok_or_else(|| Error::new(ErrorCode::Expired, "child backend mapping expired")),
            Err(error) => Err(error.clone()),
        };
        states.insert(id.to_string(), state);
    }
    let backend = |id: &ExecutionId| {
        states.get(&id.to_string()).cloned().unwrap_or_else(|| {
            Err(Error::new(
                ErrorCode::TaskNotFound,
                "unknown child execution",
            ))
        })
    };
    match materialize(&service.store, settings, record, &backend)? {
        Materialized::Ready(_) => stored(&service.store, settings, record)?.ok_or_else(|| {
            Error::new(
                ErrorCode::StoreFailure,
                "parent result vanished after publication",
            )
        }),
        Materialized::Pending { children, error } => {
            let states: Option<Vec<ChildState>> = children.iter().map(|c| c.state).collect();
            Ok(ParentState {
                state: states.and_then(|s| parents::aggregate(&s)),
                children,
                manifest: None,
                error,
            })
        }
    }
}
/// Observe until the parent result is published or `timeout` passes (true when it passed).
pub fn wait(
    service: &ExecutionService,
    record: &ParentRecord,
    timeout: Duration,
) -> Result<(ParentState, bool), Error> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let state = observe(service, record, remaining.max(Duration::from_millis(100)))?;
        if state.manifest.is_some() {
            return Ok((state, false));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok((state, true));
        }
        std::thread::sleep(Duration::from_millis(250).min(remaining));
    }
}

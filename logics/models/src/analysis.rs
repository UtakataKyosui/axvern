//! `fs analyze` contracts. The parent is a logical record over one child execution per section;
//! it persists what it asked for ([`AnalysisSpec`]) and its stored result is an
//! [`AnalysisDocument`] built once from the children's stored results.
use crate::{
    ChildState, Completeness, DiscoveryItem, DiscoveryLimits, EntryItem, Error, ExecutionId,
    MAX_DEPTH, ResultIssue, ResultWarning,
};
use serde::{Deserialize, Serialize};

pub const DEFAULT_TREE_DEPTH: u32 = 3;
/// Depth of the manifest search. The tree's display depth never narrows it.
pub const SCAN_DEPTH: u32 = MAX_DEPTH;
/// File names reported in the `manifests` section. Their contents are never read or run.
pub const MANIFEST_NAMES: [&str; 6] = [
    "Cargo.toml",
    "package.json",
    "pnpm-workspace.yaml",
    "devbox.json",
    "pyproject.toml",
    "go.mod",
];

/// Sections in the order their items are stored: the small ones first, so that a result
/// truncated by its byte budget loses tree entries rather than statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisSection {
    Languages,
    Manifests,
    Tree,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    File,
    Directory,
}

/// One section of a parent. Without `execution_id` the section either failed to register
/// (`error`) or was decided at registration from the target's own metadata (`entries`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionRegistration {
    pub section: AnalysisSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<ExecutionId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<EntryItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

/// What an `fs analyze` parent asked for. `root` is the directory item paths are relative to:
/// the target itself, or the parent directory of a file target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisSpec {
    pub path: String,
    pub root: String,
    pub input_type: InputType,
    pub tree_depth: u32,
    pub scan_depth: u32,
    pub include_ignored: bool,
    pub limits: DiscoveryLimits,
    pub sections: Vec<SectionRegistration>,
}

/// How far one section looked. `tool: null` means the section was decided at registration;
/// `max_depth: null` means the tool walks without a depth limit (tokei has none).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionScope {
    pub section: AnalysisSection,
    pub tool: Option<String>,
    pub max_depth: Option<u32>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionCounts {
    pub languages: u64,
    pub manifests: u64,
    pub tree: u64,
}
/// Sums over host languages, as `code stats` reports them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageTotals {
    pub files: u64,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
}
/// `snapshot` is always false: each section is read by its own process between
/// `registered_at` and `completed_at` (Unix seconds), so entries may change in between.
/// `items` counts the stored items per section; `totals` is null without a languages result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub path: String,
    pub root: String,
    pub input_type: InputType,
    pub tree_depth: u32,
    pub scan_depth: u32,
    pub include_ignored: bool,
    pub follows_symlinks: bool,
    pub manifest_names: Vec<String>,
    pub scope: Vec<SectionScope>,
    pub limits: DiscoveryLimits,
    pub registered_at: u64,
    pub completed_at: Option<u64>,
    pub snapshot: bool,
    pub items: SectionCounts,
    pub totals: Option<LanguageTotals>,
}
/// Outcome of one section. `state` is null while the child has not finished; a section
/// decided at registration is `done` without `execution_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisChild {
    pub section: AnalysisSection,
    pub execution_id: Option<ExecutionId>,
    pub state: Option<ChildState>,
    pub exit_code: Option<i32>,
    pub finished_at: Option<u64>,
    pub result_availability: Option<String>,
    pub completeness: Option<Completeness>,
    pub error: Option<Error>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisItem {
    pub section: AnalysisSection,
    #[serde(flatten)]
    pub item: DiscoveryItem,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisDocument {
    pub summary: AnalysisSummary,
    pub children: Vec<AnalysisChild>,
    pub items: Vec<AnalysisItem>,
    pub completeness: Completeness,
    pub errors: Vec<ResultIssue>,
    pub warnings: Vec<ResultWarning>,
}

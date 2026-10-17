//! Shared result scope. Backends enumerate; only .gitignore filters their candidates.
use super::{Issues, fd::GitignoreFilter};
use axvern_models::{ErrorCode, MAX_DEPTH, ResultIssue};
use std::path::{Component, Path};

pub(super) struct FileScope {
    ignore: Option<GitignoreFilter>,
}
impl FileScope {
    pub(super) fn new(
        root: &str,
        in_repo: bool,
        include_ignored: bool,
        explicit_file: bool,
    ) -> Self {
        Self {
            ignore: (!include_ignored && !explicit_file)
                .then(|| GitignoreFilter::new(root, in_repo)),
        }
    }
    pub(super) fn allows(&mut self, path: &str, directory: bool, issues: &mut Issues) -> bool {
        let parts: Vec<_> = Path::new(path).components().collect();
        if parts.len() > MAX_DEPTH as usize
            || parts.iter().any(|p| match p {
                Component::Normal(name) => *name == ".git",
                Component::CurDir => false,
                _ => true,
            })
        {
            return false;
        }
        match self
            .ignore
            .as_mut()
            .map(|filter| filter.ignored(path, directory))
        {
            Some(Ok(true)) => false,
            Some(Err(path)) => {
                issues.error(
                    ResultIssue::new(ErrorCode::PartialResult, "cannot read or parse .gitignore")
                        .at(path),
                );
                false
            }
            _ => true,
        }
    }
}
/// Anchored ignore glob prunes paths deeper than the shared 32-level scan scope.
pub(super) fn depth_exclude() -> String {
    format!("/{}", vec!["*"; MAX_DEPTH as usize + 1].join("/"))
}

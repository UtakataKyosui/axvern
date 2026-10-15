//! Opt-in rewriting of verified (program, arguments) pairs to modern read-only CLIs.
//! Anything outside the catalog, or with an argument the rule does not know, is left as-is.
use crate::config::{Config, MissingTool};
use axvern_models::{Error, ErrorCode, ExecutionPlan, JsonExecutionPlan, RewriteRecord};
use serde::Serialize;
use std::{ffi::OsStr, path::Path};

/// Catalog entry: `convert` returns the target arguments only for fully understood input.
pub struct RuleSpec {
    pub id: &'static str,
    pub source: &'static str,
    pub target: &'static str,
    convert: fn(&[String]) -> Option<Vec<String>>,
}

pub const RULES: &[RuleSpec] = &[
    RuleSpec {
        id: "ls-to-eza",
        source: "ls",
        target: "eza",
        convert: ls_to_eza,
    },
    RuleSpec {
        id: "cat-to-bat",
        source: "cat",
        target: "bat",
        convert: cat_to_bat,
    },
    RuleSpec {
        id: "find-to-fd",
        source: "find",
        target: "fd",
        convert: find_to_fd,
    },
];

pub fn rule(id: &str) -> Option<&'static RuleSpec> {
    RULES.iter().find(|r| r.id == id)
}

/// Why an enabled rule did not change the request although its source matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RewriteSkip {
    pub rule_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Rewrite {
    Unchanged,
    Rewritten(RewriteRecord),
    Skipped(RewriteSkip),
}

/// An operand that cannot be mistaken for an option or an expression.
fn operand(s: &str) -> bool {
    !s.is_empty() && !s.starts_with(['-', '!', '(', ')'])
}

/// `ls [-A] [PATH]`. `ls -a` also lists `.` and `..`, so it is not mapped.
fn ls_to_eza(args: &[String]) -> Option<Vec<String>> {
    let (all, path) = match args {
        [] => (false, None),
        [a] if a == "-A" => (true, None),
        [p] if operand(p) => (false, Some(p)),
        [a, p] if a == "-A" && operand(p) => (true, Some(p)),
        _ => return None,
    };
    let mut out = vec!["--color=never".to_owned()];
    if all {
        out.push("--all".into());
    }
    if let Some(path) = path {
        out.extend(["--".into(), path.clone()]);
    }
    Some(out)
}

/// `cat FILE` with exactly one file operand.
fn cat_to_bat(args: &[String]) -> Option<Vec<String>> {
    match args {
        [file] if operand(file) => Some(
            [
                "--paging=never",
                "--style=plain",
                "--color=never",
                "--",
                file,
            ]
            .map(String::from)
            .to_vec(),
        ),
        _ => None,
    }
}

/// `find PATH -name GLOB [-type f|d]` (either order). `find` lists dotfiles and ignores
/// .gitignore, so fd is told to do the same.
fn find_to_fd(args: &[String]) -> Option<Vec<String>> {
    let [path, rest @ ..] = args else {
        return None;
    };
    if !operand(path) {
        return None;
    }
    let (mut name, mut kind) = (None, None);
    for pair in rest.chunks(2) {
        match pair {
            [flag, value] if flag == "-name" && name.is_none() => {
                if !operand(value) || value.contains('/') {
                    return None;
                }
                name = Some(value);
            }
            [flag, value] if flag == "-type" && kind.is_none() && matches!(&**value, "f" | "d") => {
                kind = Some(value);
            }
            _ => return None,
        }
    }
    let name = name?;
    let mut out: Vec<String> = ["--hidden", "--no-ignore", "--color=never", "--glob"]
        .map(String::from)
        .to_vec();
    if let Some(kind) = kind {
        out.extend(["--type".into(), kind.clone()]);
    }
    out.extend(["--".into(), name.clone(), path.clone()]);
    Some(out)
}

/// True when `program` is an executable file in a PATH directory.
pub fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            std::fs::metadata(dir.join(program))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// Pure decision: the caller supplies tool availability (`on_path` in production).
pub fn rewrite(
    config: &Config,
    plan: &ExecutionPlan,
    available: impl Fn(&str) -> bool,
) -> Result<(ExecutionPlan, Rewrite), Error> {
    let unchanged = || Ok((plan.clone(), Rewrite::Unchanged));
    config.validate()?;
    if !config.replacement.enabled {
        return unchanged();
    }
    let dto = JsonExecutionPlan::try_from(plan)?;
    let Some(spec) = config
        .replacement
        .rules
        .iter()
        .filter_map(|id| rule(id))
        .find(|r| dto.program == r.source)
    else {
        return unchanged();
    };
    let Some(args) = (spec.convert)(&dto.args) else {
        return unchanged();
    };
    if !available(spec.target) {
        return match config.replacement.on_missing_tool {
            MissingTool::Skip => Ok((
                plan.clone(),
                Rewrite::Skipped(RewriteSkip {
                    rule_id: spec.id.into(),
                    reason: format!("{} is not installed; original command kept", spec.target),
                }),
            )),
            MissingTool::Error => Err(Error::new(
                ErrorCode::MissingTool,
                format!(
                    "{} is required by replacement rule {}",
                    spec.target, spec.id
                ),
            )),
        };
    }
    let rewritten = ExecutionPlan {
        program: OsStr::new(spec.target).to_owned(),
        args: args.into_iter().map(Into::into).collect(),
        cwd: Path::new(&dto.cwd).to_owned(),
    };
    let record = RewriteRecord {
        rule_id: spec.id.into(),
        original: dto,
        rewritten: JsonExecutionPlan::try_from(&rewritten)?,
    };
    Ok((rewritten, Rewrite::Rewritten(record)))
}

//! Strict, single-file configuration. Loading never creates directories or repairs files.
use axvern_models::{Error, ErrorCode, PolicyEffect};
use serde::Deserialize;
use std::io::Read;
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub replacement: Replacement,
    #[serde(default)]
    pub execution: Execution,
    #[serde(default)]
    pub results: Results,
    #[serde(default)]
    pub discovery: Discovery,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Results {
    pub retention_seconds: u64,
    pub capacity_bytes: u64,
    pub max_artifact_bytes: u64,
}
impl Default for Results {
    fn default() -> Self {
        Self {
            retention_seconds: 604800,
            capacity_bytes: 1073741824,
            max_artifact_bytes: 67108864,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Discovery {
    pub max_items: u64,
    pub max_entries: u64,
    pub timeout_ms: u64,
}
impl Default for Discovery {
    fn default() -> Self {
        Self {
            max_items: 10000,
            max_entries: 100000,
            timeout_ms: 60000,
        }
    }
}
impl Discovery {
    pub fn limits(&self) -> axvern_models::DiscoveryLimits {
        axvern_models::DiscoveryLimits {
            max_items: self.max_items,
            max_entries: self.max_entries,
            timeout_ms: self.timeout_ms,
        }
    }
}
/// Explicit backend configuration avoids Pueue's project-local config discovery.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub pueue_config: Option<PathBuf>,
    pub store: Option<PathBuf>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default = "ask")]
    pub default: PolicyEffect,
    #[serde(default)]
    pub rules: Vec<Rule>,
}
fn ask() -> PolicyEffect {
    PolicyEffect::Ask
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            default: ask(),
            rules: vec![],
        }
    }
}
/// Opt-in rewriting. Only catalog rules named in `rules` apply, and only when `enabled`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Replacement {
    pub enabled: bool,
    pub rules: Vec<String>,
    pub on_missing_tool: MissingTool,
}
/// What to do when a matched rule's target CLI is not installed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingTool {
    #[default]
    Skip,
    Error,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub effect: PolicyEffect,
    pub command: String,
    #[serde(default)]
    pub args_prefix: Vec<String>,
    #[serde(default)]
    pub allow_shell: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            policy: Policy::default(),
            replacement: Replacement::default(),
            execution: Execution::default(),
            results: Results::default(),
            discovery: Discovery::default(),
        }
    }
}
impl Config {
    pub fn parse(text: &str) -> Result<Self, Error> {
        // Do not include parser diagnostics: they can quote secret configuration values.
        let value: Self =
            toml::from_str(text).map_err(|_| invalid("invalid TOML, type, or unknown key"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.schema_version != 1 {
            return Err(invalid("unsupported schema_version"));
        }
        if !(3600..=365 * 86400).contains(&self.results.retention_seconds)
            || !(1048576..=1099511627776).contains(&self.results.capacity_bytes)
            || !(1024..=1073741824).contains(&self.results.max_artifact_bytes)
            || self.results.max_artifact_bytes > self.results.capacity_bytes
            || !(1..=100000).contains(&self.discovery.max_items)
            || !(self.discovery.max_items..=1000000).contains(&self.discovery.max_entries)
            || !(1000..=600000).contains(&self.discovery.timeout_ms)
        {
            return Err(invalid("invalid results/discovery limits"));
        }
        if self.policy.default != PolicyEffect::Ask {
            return Err(invalid("policy.default must be ask"));
        }
        let mut rules = HashSet::new();
        if (self.replacement.enabled && self.replacement.rules.is_empty())
            || self
                .replacement
                .rules
                .iter()
                .any(|id| crate::rewrite::rule(id).is_none() || !rules.insert(id))
        {
            return Err(invalid(
                "replacement.rules must list unique known rule IDs when enabled",
            ));
        }
        if self.policy.rules.len() > 1024 {
            return Err(invalid("too many rules"));
        }
        let mut ids = HashSet::new();
        for r in &self.policy.rules {
            if r.id.is_empty()
                || r.id.len() > 128
                || !r
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || !ids.insert(&r.id)
                || r.command.is_empty()
                || r.command.contains('\0')
                || r.command.len() > 4096
                || r.args_prefix.len() > 256
                || r.args_prefix
                    .iter()
                    .any(|a| a.contains('\0') || a.len() > 65536)
                || (r.allow_shell
                    && (r.args_prefix.is_empty() || !crate::policy::is_shell(&r.command)))
            {
                return Err(invalid("invalid or duplicate policy rule"));
            }
        }
        Ok(())
    }
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidConfig, message)
}
#[derive(Debug)]
pub struct UserPaths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
}
impl UserPaths {
    pub fn discover() -> Result<Self, Error> {
        let dirs = directories::ProjectDirs::from("", "", "axvern")
            .ok_or_else(|| invalid("cannot resolve user directories"))?;
        Ok(Self {
            config: dirs.config_dir().join("config.toml"),
            data: dirs.data_local_dir().into(),
            cache: dirs.cache_dir().into(),
        })
    }
}
/// Environment and implicit path are supplied explicitly for deterministic tests (no env mutation).
pub fn load(
    explicit: Option<&Path>,
    env: Option<OsString>,
    implicit: &Path,
) -> Result<Config, Error> {
    let (path, required) = if let Some(path) = explicit {
        (path.to_owned(), true)
    } else if let Some(path) = env {
        if path.is_empty() {
            return Err(invalid("AXVERN_CONFIG is empty"));
        }
        (path.into(), true)
    } else {
        (implicit.to_owned(), false)
    };
    // Preserve symlinks for inspection, but include cwd and all its ancestors.
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    let meta = match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => {
            return Ok(Config::default());
        }
        Err(e) => return Err(io_error(e)),
        Ok(m) => m,
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(invalid("config must be a regular file, not a symlink"));
    }
    // Reject symlink ancestors rather than silently following an indirect configuration.
    for parent in path
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
    {
        let m = fs::symlink_metadata(parent).map_err(io_error)?;
        if m.file_type().is_symlink() {
            return Err(invalid("config ancestors must not be symlinks"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = m.permissions().mode();
            if mode & 0o022 != 0 && mode & 0o1000 == 0 {
                return Err(Error::new(
                    ErrorCode::PermissionDenied,
                    "config ancestor is writable by other users",
                ));
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o177 != 0 || meta.permissions().mode() & 0o400 == 0 {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "config permissions must be 0600 or stricter",
            ));
        }
    }
    if meta.len() > 1024 * 1024 {
        return Err(invalid("config exceeds 1 MiB"));
    }
    let file = fs::File::open(&path).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let opened = file.metadata().map_err(io_error)?;
        if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
            return Err(invalid("config changed while opening"));
        }
        if opened.permissions().mode() & 0o177 != 0 || opened.permissions().mode() & 0o400 == 0 {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "config permissions must be 0600 or stricter",
            ));
        }
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid("config exceeds 1 MiB"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid("config must be UTF-8"))?;
    Config::parse(text)
}
fn io_error(e: std::io::Error) -> Error {
    Error::new(
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            ErrorCode::PermissionDenied
        } else {
            ErrorCode::InvalidConfig
        },
        "cannot read selected config",
    )
}

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
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    #[serde(default)]
    pub enabled: bool,
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
        if self.policy.default != PolicyEffect::Ask {
            return Err(invalid("policy.default must be ask"));
        }
        if self.replacement.enabled {
            return Err(invalid("replacement is not implemented"));
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

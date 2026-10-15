//! Execution use cases. CLI owns parsing, approval input and rendering only.
use crate::{
    adapters::pueue::PueueAdapter,
    config::{Config, UserPaths},
    planner::{self, Planned},
    policy,
    store::{ExecutionStore, RegistrationFailure, register_and_save},
};
use axvern_models::*;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub fn load_config(explicit: Option<&Path>) -> Result<Config, Error> {
    let env = std::env::var_os("AXVERN_CONFIG");
    let implicit = if explicit.is_none() && env.is_none() {
        UserPaths::discover()?.config
    } else {
        PathBuf::new()
    };
    crate::config::load(explicit, env, &implicit)
}

pub fn adapter(config: &Config) -> Result<PueueAdapter, Error> {
    config.validate()?;
    let path = config.execution.pueue_config.clone().ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidConfig,
            "set execution.pueue_config to an absolute Pueue 4.0.4 configuration path",
        )
    })?;
    if !path.is_absolute() || !path.is_file() {
        return Err(Error::new(
            ErrorCode::InvalidConfig,
            "Pueue configuration must be an absolute existing file",
        ));
    }
    // The canonical config path is the namespace. Editing it to target another daemon
    // requires a different config path/store; do not silently rebind existing records.
    let path = path.canonicalize().map_err(|_| {
        Error::new(
            ErrorCode::InvalidConfig,
            "cannot resolve Pueue configuration",
        )
    })?;
    let queue = path
        .to_str()
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidEncoding,
                "Pueue configuration path must be UTF-8",
            )
        })?
        .to_owned();
    Ok(PueueAdapter {
        executable: "pueue".into(),
        config: path,
        queue,
        timeout: Duration::from_secs(5),
    })
}

pub fn store_path(config: &Config) -> Result<PathBuf, Error> {
    match &config.execution.store {
        Some(path) if path.is_absolute() => Ok(path.clone()),
        Some(_) => Err(Error::new(
            ErrorCode::InvalidConfig,
            "execution.store must be absolute",
        )),
        None => Ok(UserPaths::discover()?.data.join("executions")),
    }
}
/// Create private missing directories, inspecting each existing component first.
pub fn prepare_parent(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut ancestors: Vec<_> = path
        .parent()
        .ok_or_else(|| Error::new(ErrorCode::InvalidInput, "store needs a parent"))?
        .ancestors()
        .collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if !ancestor.exists() {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(ancestor)
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|_| {
                    Error::new(
                        ErrorCode::StoreFailure,
                        "cannot create private store parent",
                    )
                })?;
        }
        let meta = ancestor
            .symlink_metadata()
            .map_err(|_| Error::new(ErrorCode::StoreFailure, "cannot inspect store parent"))?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || (meta.permissions().mode() & 0o022 != 0 && meta.permissions().mode() & 0o1000 == 0)
        {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "unsafe store ancestor",
            ));
        }
    }
    Ok(())
}

pub struct ExecutionService {
    pub adapter: PueueAdapter,
    pub store: ExecutionStore,
    config: Config,
}
pub fn authorize(
    config: &Config,
    plan: &ExecutionPlan,
    interactive: bool,
    approval: Approval,
) -> Result<(), Error> {
    planner::authorize(
        config,
        &Planned::unchanged(plan.clone()),
        interactive,
        approval,
    )
}
impl ExecutionService {
    pub(crate) fn target_snapshot(
        &self,
        mut task: TaskSnapshot,
        id: &ExecutionId,
    ) -> Result<TaskSnapshot, Error> {
        if matches!(task.state, ChildState::Done | ChildState::Failed)
            && let Some(m) = crate::results::inspect(&self.store, &self.config.results, id)?
            && let Some(state) = m.state
        {
            task.state = state;
            task.exit_code = m.exit_code;
        }
        Ok(task)
    }
    pub fn result_settings(&self) -> &crate::config::Results {
        &self.config.results
    }
    pub fn worker_plan(
        &self,
        runner: &Path,
        plan: &ExecutionPlan,
        id: &ExecutionId,
    ) -> Result<ExecutionPlan, Error> {
        JsonExecutionPlan::try_from(plan)?;
        let mut args = vec![
            "__axvern_worker".into(),
            self.store.root.as_os_str().into(),
            id.to_string().into(),
            self.config.results.retention_seconds.to_string().into(),
            self.config.results.capacity_bytes.to_string().into(),
            self.config.results.max_artifact_bytes.to_string().into(),
            "--".into(),
            plan.program.clone(),
        ];
        args.extend(plan.args.clone());
        Ok(ExecutionPlan {
            program: runner.as_os_str().into(),
            args,
            cwd: plan.cwd.clone(),
        })
    }
    pub fn run_captured(
        &mut self,
        runner: &Path,
        plan: &ExecutionPlan,
        interactive: bool,
        approval: Approval,
    ) -> Result<ExecutionRecord, crate::store::RegistrationFailure> {
        self.run_planned(
            runner,
            &Planned::unchanged(plan.clone()),
            interactive,
            approval,
        )
    }
    /// Registers `planned.executable` once; the request identity and audit record keep
    /// `planned.original`.
    pub fn run_planned(
        &mut self,
        runner: &Path,
        planned: &Planned,
        interactive: bool,
        approval: Approval,
    ) -> Result<ExecutionRecord, crate::store::RegistrationFailure> {
        let fail = |error| crate::store::RegistrationFailure {
            error,
            receipt: None,
            execution_id: None,
        };
        planner::authorize(&self.config, planned, interactive, approval).map_err(fail)?;
        let id = ExecutionStore::allocate_id().map_err(fail)?;
        let worker = self
            .worker_plan(runner, &planned.executable, &id)
            .map_err(fail)?;
        let check_adapter = self.adapter.clone();
        let registration = self.store.register_intent_rewritten(
            &id,
            &planned.original,
            planned.record().cloned(),
            || {
                crate::results::preflight_registration_locked(
                    &self.store,
                    &self.config.results,
                    &planned.executable,
                )?;
                check_adapter.check()
            },
            |id| self.adapter.register_with_label(&worker, id),
        );
        match registration {
            Ok(request) => request
                .record
                .ok_or_else(|| crate::store::RegistrationFailure {
                    error: Error::new(
                        ErrorCode::RegistrationUnknown,
                        "registration intent needs reconciliation; do not retry add",
                    ),
                    receipt: None,
                    execution_id: Some(id),
                }),
            Err(error) => {
                let reserved = self.store.intent(&id).ok().flatten().is_some();
                Err(crate::store::RegistrationFailure {
                    error,
                    receipt: self.store.find_record(&id).ok().map(Box::new),
                    execution_id: reserved.then_some(id),
                })
            }
        }
    }
    /// Retain the last actual observation when the wait budget expires.
    pub fn observe_wait(
        &self,
        id: &ExecutionId,
        timeout: Duration,
    ) -> Result<(ExecutionView, bool), Error> {
        let record = self.store.find_record(id)?;
        let deadline = std::time::Instant::now() + timeout;
        let mut last = None;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok((
                    ExecutionView {
                        record,
                        task: last,
                        error: None,
                    },
                    true,
                ));
            }
            let mut client = self.adapter.clone();
            client.timeout = client.timeout.min(remaining);
            match client.inspect_owned(&record.backend) {
                Ok(task) => {
                    let task = self.target_snapshot(task, id)?;
                    let terminal = matches!(task.state, ChildState::Done | ChildState::Failed);
                    last = Some(task);
                    if terminal {
                        return Ok((
                            ExecutionView {
                                record,
                                task: last,
                                error: None,
                            },
                            false,
                        ));
                    }
                }
                Err(e) if e.code == ErrorCode::Timeout && std::time::Instant::now() >= deadline => {
                    return Ok((
                        ExecutionView {
                            record,
                            task: last,
                            error: None,
                        },
                        true,
                    ));
                }
                Err(e) => return Err(e),
            }
            std::thread::sleep(
                Duration::from_millis(250)
                    .min(deadline.saturating_duration_since(std::time::Instant::now())),
            );
        }
    }
    pub fn new(config: Config) -> Result<Self, Error> {
        let adapter = adapter(&config)?;
        let path = store_path(&config)?;
        prepare_parent(&path)?;
        Ok(Self {
            adapter,
            store: ExecutionStore::open(path)?,
            config,
        })
    }
    pub fn decision(&self, plan: &ExecutionPlan) -> Result<PolicyDecision, Error> {
        policy::evaluate(&self.config, plan)
    }
    pub fn run(
        &mut self,
        plan: &ExecutionPlan,
        interactive: bool,
        approval: Approval,
    ) -> Result<ExecutionRecord, RegistrationFailure> {
        authorize(&self.config, plan, interactive, approval).map_err(|error| {
            RegistrationFailure {
                error,
                receipt: None,
                execution_id: None,
            }
        })?;
        register_and_save(&mut self.adapter, &self.store, plan)
    }
    pub fn status(&self, id: &ExecutionId) -> Result<ExecutionView, Error> {
        let record = self.store.find_record(id)?;
        let task = self.target_snapshot(self.adapter.inspect_owned(&record.backend)?, id)?;
        Ok(ExecutionView {
            record,
            task: Some(task),
            error: None,
        })
    }
    pub fn wait(&self, id: &ExecutionId, timeout: Duration) -> Result<ExecutionView, Error> {
        let record = self.store.find_record(id)?;
        let task = self.target_snapshot(self.adapter.wait(&record.backend, timeout)?, id)?;
        Ok(ExecutionView {
            record,
            task: Some(task),
            error: None,
        })
    }
    pub fn history(&self, limit: usize) -> Result<Vec<ExecutionView>, Error> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::new(ErrorCode::InvalidInput, "limit must be 1..1000"));
        }
        // One bounded backend request, not one request per stored execution.
        let records: Vec<_> = self
            .store
            .history()?
            .into_iter()
            .rev()
            .take(limit)
            .collect();
        if records.is_empty() {
            return Ok(vec![]);
        }
        let tasks = self.adapter.list();
        records
            .into_iter()
            .map(|record| {
                let result = match &tasks {
                    Ok(tasks) => tasks
                        .iter()
                        .find(|t| t.identity == record.backend)
                        .cloned()
                        .ok_or_else(|| {
                            Error::new(
                                ErrorCode::Expired,
                                "Pueue task removed, reused, or belongs to another queue",
                            )
                        }),
                    Err(error) => Err(error.clone()),
                };
                match result {
                    Ok(task) => Ok(ExecutionView {
                        task: Some(self.target_snapshot(task, &record.execution_id)?),
                        record,
                        error: None,
                    }),
                    Err(error) => Ok(ExecutionView {
                        record,
                        task: None,
                        error: Some(error),
                    }),
                }
            })
            .collect()
    }
}

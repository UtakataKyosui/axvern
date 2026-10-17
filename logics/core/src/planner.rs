//! Turns a request into the single plan that will be registered, evaluating Policy on
//! both the request as given and the plan that will actually run.
use crate::{
    config::Config,
    policy,
    rewrite::{self, Rewrite},
};
use axvern_models::{
    Adapter, Approval, Error, ErrorCode, ExecutionPlan, PolicyDecision, PolicyEffect, RewriteRecord,
};

#[derive(Clone, Debug)]
pub struct Planned {
    /// The request as the caller gave it; identity for request keys and audit.
    pub original: ExecutionPlan,
    /// The one plan registered with Pueue: `original` unless a rule rewrote it.
    pub executable: ExecutionPlan,
    pub rewrite: Rewrite,
}

impl Planned {
    pub fn unchanged(plan: ExecutionPlan) -> Self {
        Self {
            executable: plan.clone(),
            original: plan,
            rewrite: Rewrite::Unchanged,
        }
    }
    pub fn record(&self) -> Option<&RewriteRecord> {
        match &self.rewrite {
            Rewrite::Rewritten(record) => Some(record),
            _ => None,
        }
    }
}

pub fn plan(
    config: &Config,
    original: ExecutionPlan,
    available: impl Fn(&str) -> bool,
) -> Result<Planned, Error> {
    let (executable, rewrite) = rewrite::rewrite(config, &original, available)?;
    Ok(Planned {
        original,
        executable,
        rewrite,
    })
}

/// Rewriting must never relax a decision: the stricter of the two evaluations applies.
pub fn evaluate(config: &Config, planned: &Planned) -> Result<PolicyDecision, Error> {
    let original = policy::evaluate(config, &planned.original)?;
    if planned.executable == planned.original {
        return Ok(original);
    }
    let executable = policy::evaluate(config, &planned.executable)?;
    let strictness = |d: &PolicyDecision| match d.effect {
        PolicyEffect::Allow => 0,
        PolicyEffect::Ask => 1,
        PolicyEffect::Deny => 2,
    };
    Ok(if strictness(&executable) > strictness(&original) {
        executable
    } else {
        original
    })
}

/// Both plans must pass; a denial of either one means nothing is registered.
pub fn authorize(
    config: &Config,
    planned: &Planned,
    interactive: bool,
    approval: Approval,
) -> Result<(), Error> {
    let decision = evaluate(config, planned)?;
    if decision.effect == PolicyEffect::Deny
        || (decision.effect == PolicyEffect::Ask
            && (!interactive || approval != Approval::Approved))
    {
        return Err(Error::new(
            ErrorCode::PolicyDenied,
            "policy or approval refused registration",
        ));
    }
    Ok(())
}

/// Registers `planned.executable` exactly once after both plans pass Policy.
pub fn register<A: Adapter>(
    config: &Config,
    planned: &Planned,
    interactive: bool,
    approval: Approval,
    adapter: &mut A,
) -> Result<u64, Error> {
    authorize(config, planned, interactive, approval)?;
    adapter.register(&planned.executable)
}

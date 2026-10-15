//! Pure policy evaluation and a single guarded registration boundary.
use crate::config::Config;
use axvern_models::{
    Adapter, Approval, Error, ErrorCode, ExecutionPlan, JsonExecutionPlan, PolicyDecision,
    PolicyEffect,
};
use std::path::Path;

/// Conservatively treat all invocations of supported shell names as shell evaluation,
/// including combined flags, stdin scripts and script paths. Wrapper commands need their own rules.
pub fn is_shell(program: &str) -> bool {
    matches!(
        Path::new(program).file_name().and_then(|s| s.to_str()),
        Some(
            "sh" | "bash"
                | "dash"
                | "zsh"
                | "ksh"
                | "fish"
                | "csh"
                | "tcsh"
                | "pwsh"
                | "powershell"
                | "cmd"
        )
    )
}
pub fn evaluate(config: &Config, plan: &ExecutionPlan) -> Result<PolicyDecision, Error> {
    config.validate()?;
    let dto = JsonExecutionPlan::try_from(plan)?;
    if dto.program.is_empty()
        || dto.args.len() > 256
        || dto.program.len() > 4096
        || dto.args.iter().any(|a| a.len() > 65536)
    {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            "invalid program or argument limit",
        ));
    }
    let shell = is_shell(&dto.program);
    let matched: Vec<_> = config
        .policy
        .rules
        .iter()
        .filter(|r| r.command == dto.program && dto.args.starts_with(&r.args_prefix))
        .collect();
    // Explicit Deny retains its reason and rule ID even for shell invocations.
    if let Some(rule) = matched.iter().find(|r| r.effect == PolicyEffect::Deny) {
        return Ok(PolicyDecision {
            effect: PolicyEffect::Deny,
            reason: "matched rule".into(),
            rule_id: Some(rule.id.clone()),
        });
    }
    // A generic Allow cannot authorize shell evaluation; an explicit rule is mandatory.
    if shell && !matched.iter().any(|r| r.allow_shell) {
        return Ok(PolicyDecision {
            effect: PolicyEffect::Deny,
            reason: "shell evaluation requires an explicit rule".into(),
            rule_id: None,
        });
    }
    for effect in [PolicyEffect::Ask, PolicyEffect::Allow] {
        if let Some(rule) = matched.iter().find(|r| {
            r.effect == effect && (!shell || r.allow_shell || effect != PolicyEffect::Allow)
        }) {
            return Ok(PolicyDecision {
                effect,
                reason: "matched rule".into(),
                rule_id: Some(rule.id.clone()),
            });
        }
    }
    Ok(PolicyDecision {
        effect: PolicyEffect::Ask,
        reason: "no matching rule; approval required".into(),
        rule_id: None,
    })
}
/// The caller passes an explicit interactive approval result. Noninteractive Ask always fails.
pub fn register<A: Adapter>(
    config: &Config,
    plan: &ExecutionPlan,
    interactive: bool,
    approval: Approval,
    adapter: &mut A,
) -> Result<u64, Error> {
    let decision = evaluate(config, plan)?;
    match decision.effect {
        PolicyEffect::Allow => adapter.register(plan),
        PolicyEffect::Ask if interactive && approval == Approval::Approved => {
            adapter.register(plan)
        }
        _ => Err(Error::new(
            ErrorCode::PolicyDenied,
            "policy or approval refused registration",
        )),
    }
}

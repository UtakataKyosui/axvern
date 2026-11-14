// Verus proof for the artifact budget arithmetic in logics/core/src/results.rs.
// The function body between the `verus-sync` markers must match the one in
// results.rs; qualities/scripts/verus_check.py enforces this.
use vstd::prelude::*;

verus! {

const BUDGET_RESERVE: u64 = 32768;
const BUDGET_MIN_FREE: u64 = 33792;

fn clamp_budget(wanted: u64, free: u64) -> (budget: Option<u64>)
    ensures
        match budget {
            None => free < BUDGET_MIN_FREE,
            Some(bytes) => {
                &&& free >= BUDGET_MIN_FREE
                &&& bytes <= wanted
                &&& bytes + BUDGET_RESERVE <= free
                &&& (bytes == wanted || bytes + BUDGET_RESERVE == free)
            },
        },
{
    // verus-sync: begin clamp_budget
    if free < BUDGET_MIN_FREE {
        return None;
    }
    let room = free - BUDGET_RESERVE;
    Some(if wanted < room { wanted } else { room })
    // verus-sync: end clamp_budget
}

} // verus!

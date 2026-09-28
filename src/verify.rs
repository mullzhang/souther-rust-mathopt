//! Independent check of a constructed candidate against the original domain input.
//! No solver variables, expressions, or floating-point values are used here.
use model::HostError;
use model::example::assignment::{AssignmentProblem, CandidatePlan};
use std::collections::{HashMap, HashSet};

pub fn verify(problem: AssignmentProblem<'_>, plan: CandidatePlan<'_>) -> Result<(), HostError> {
    let workers = problem.workers();
    let jobs: HashSet<_> = problem.jobs().iter().map(|j| j.id().value()).collect();
    let offers: HashMap<_, _> = problem
        .offers()
        .into_iter()
        .map(|o| ((o.workerId().value(), o.jobId().value()), o))
        .collect();
    let mut seen_jobs = HashSet::new();
    let mut minutes = HashMap::<String, i64>::new();
    let mut total = 0_i64;
    for assignment in plan.assignments() {
        let worker = assignment.workerId().value();
        let job = assignment.jobId().value();
        if !jobs.contains(&job) || !seen_jobs.insert(job.clone()) {
            return Err("candidate has unknown or duplicate job".into());
        }
        let offer = offers
            .get(&(worker.clone(), job))
            .ok_or("unqualified assignment")?;
        *minutes.entry(worker).or_default() += offer.minutes().value();
        total += offer.cost().value();
    }
    if seen_jobs != jobs {
        return Err("candidate does not cover every job".into());
    }
    let loads = plan.loads();
    if loads.len() != workers.len() {
        return Err("candidate does not cover every worker".into());
    }
    let mut seen_workers = HashSet::new();
    for load in loads {
        let id = load.workerId().value();
        let worker = workers
            .iter()
            .find(|w| w.id().value() == id)
            .ok_or("unknown worker load")?;
        if !seen_workers.insert(id.clone()) {
            return Err("duplicate worker load".into());
        }
        let used = minutes.get(&id).copied().unwrap_or(0);
        let extra = (used - worker.regularMinutes().value()).max(0);
        if used != load.minutes().value()
            || extra != load.overtime().value()
            || extra > worker.overtimeLimit().value()
        {
            return Err("candidate workload does not match assignments or exceeds capacity".into());
        }
        total += extra * worker.overtimeRate().value();
    }
    if total != plan.totalCost().value() {
        return Err("candidate cost does not match input prices".into());
    }
    Ok(())
}

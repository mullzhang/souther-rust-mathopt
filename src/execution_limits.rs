//! Host execution limits, independent of valid/hard/soft domain constraints.
//! These bounds keep this prototype's arithmetic within i64 and exact f64 integers.
use model::example::assignment::AssignmentProblem;
use std::fmt;

const MAX_WORKERS: i64 = 16;
const MAX_JOBS: i64 = 32;
const MAX_ID_LENGTH: i64 = 64;
const MAX_MINUTES: i64 = 1440;
const MAX_COST: i64 = 1_000_000;
const MAX_RATE: i64 = 10_000;
pub(crate) const MAX_LOAD_MINUTES: i64 = MAX_JOBS * MAX_MINUTES;

#[derive(Debug)]
pub(crate) struct ExecutionLimitExceeded {
    pub path: String,
    pub actual: i64,
    pub limit: i64,
}

impl fmt::Display for ExecutionLimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "execution limit exceeded at {}: {} > {}",
            self.path, self.actual, self.limit
        )
    }
}

impl std::error::Error for ExecutionLimitExceeded {}

fn upper_bound(path: String, actual: i64, limit: i64) -> Result<(), ExecutionLimitExceeded> {
    if actual > limit {
        return Err(ExecutionLimitExceeded {
            path,
            actual,
            limit,
        });
    }
    Ok(())
}

/// Check a valid problem before executing the solver or the Souther evaluator.
/// Unique, defined pairs imply at most MAX_WORKERS * MAX_JOBS offers and assignments,
/// including hard-infeasible candidates that assign every worker to every job.
pub(crate) fn check_problem(problem: AssignmentProblem<'_>) -> Result<(), ExecutionLimitExceeded> {
    let workers = problem.workers();
    let jobs = problem.jobs();
    upper_bound("/workers".into(), workers.len() as i64, MAX_WORKERS)?;
    upper_bound("/jobs".into(), jobs.len() as i64, MAX_JOBS)?;
    for (i, worker) in workers.iter().enumerate() {
        upper_bound(
            format!("/workers/{i}/id"),
            worker.id().value().len() as i64,
            MAX_ID_LENGTH,
        )?;
        for (field, value, limit) in [
            (
                "regularMinutes",
                worker.regularMinutes().value(),
                MAX_MINUTES,
            ),
            ("overtimeLimit", worker.overtimeLimit().value(), MAX_MINUTES),
            (
                "overtimeTarget",
                worker.overtimeTarget().value(),
                MAX_MINUTES,
            ),
            ("overtimeRate", worker.overtimeRate().value(), MAX_RATE),
            (
                "overtimePenalty",
                worker.overtimePenalty().value(),
                MAX_RATE,
            ),
        ] {
            upper_bound(format!("/workers/{i}/{field}"), value, limit)?;
        }
    }
    for (i, job) in jobs.iter().enumerate() {
        upper_bound(
            format!("/jobs/{i}/id"),
            job.id().value().len() as i64,
            MAX_ID_LENGTH,
        )?;
    }
    for (i, offer) in problem.offers().iter().enumerate() {
        upper_bound(
            format!("/offers/{i}/minutes"),
            offer.minutes().value(),
            MAX_MINUTES,
        )?;
        upper_bound(format!("/offers/{i}/cost"), offer.cost().value(), MAX_COST)?;
    }
    Ok(())
}

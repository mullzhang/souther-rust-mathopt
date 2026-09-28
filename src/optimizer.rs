//! Independent MILP formulation. This module neither calls nor imports the evaluator.
use crate::execution_limits::{MAX_LOAD_MINUTES, check_problem};
use good_lp::{Expression, ProblemVariables, SolverModel, variable};
use highs::{HighsModelStatus as Status, HighsSolutionStatus};
use model::example::assignment::*;
use model::{Construction, Failure, HostError, Run};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

pub const INTEGER_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
pub struct SolveOptions {
    pub time_limit_seconds: f64,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            time_limit_seconds: 30.0,
        }
    }
}

pub struct Optimizer {
    pub options: SolveOptions,
    pub statistics: Rc<RefCell<Option<Value>>>,
}

impl Optimizer {
    pub fn new(options: SolveOptions) -> Result<Self, HostError> {
        if !options.time_limit_seconds.is_finite() || options.time_limit_seconds < 0.0 {
            return Err("time limit must be finite and nonnegative".into());
        }
        Ok(Self {
            options,
            statistics: Rc::new(RefCell::new(None)),
        })
    }
}

pub fn made<T>(result: Result<Construction<T>, Failure>) -> Result<T, HostError> {
    Ok(result?.into_result()?)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Completion {
    Optimal,
    Feasible(String),
    Infeasible,
    NoSolution(String),
}

/// Retain the distinction between stopping without a solution and proving infeasibility.
pub fn classify(status: Status, has_solution: bool, gap: f64) -> Result<Completion, HostError> {
    match status {
        Status::Optimal if has_solution && gap.is_finite() && gap == 0.0 => Ok(Completion::Optimal),
        Status::Optimal if has_solution && gap.is_finite() && gap > 0.0 => {
            Ok(Completion::Feasible("gap_limit".into()))
        }
        Status::Infeasible if !has_solution => Ok(Completion::Infeasible),
        Status::ReachedTimeLimit
        | Status::ReachedIterationLimit
        | Status::ReachedSolutionLimit
        | Status::ReachedMemoryLimit
        | Status::ReachedInterrupt
        | Status::ObjectiveBound
        | Status::ObjectiveTarget => {
            let reason = format!("{status:?}");
            Ok(if has_solution {
                Completion::Feasible(reason)
            } else {
                Completion::NoSolution(reason)
            })
        }
        _ => Err(format!(
            "unexpected HiGHS status: {status:?}; feasible={has_solution}; gap={gap}"
        )
        .into()),
    }
}

fn integer_in_range(value: f64, maximum: i64) -> Result<i64, HostError> {
    if !value.is_finite()
        || (value - value.round()).abs() > INTEGER_TOLERANCE
        || !(0.0..=maximum as f64).contains(&value.round())
    {
        return Err(format!("invalid integer solution value: {value}, maximum {maximum}").into());
    }
    Ok(value.round() as i64)
}

pub fn binary(value: f64) -> Result<bool, HostError> {
    Ok(integer_in_range(value, 1)? == 1)
}

impl Solve for Optimizer {
    fn apply<'run>(
        &self,
        run: &mut Run<'run>,
        problem: AssignmentProblem<'run>,
    ) -> Result<SearchOutcome<'run>, HostError> {
        // Also protect callers that use the optimizer without App.
        check_problem(problem)?;
        let started = Instant::now();
        let workers = problem.workers();
        let jobs = problem.jobs();
        let offers = problem.offers();
        let worker_index: HashMap<_, _> = workers
            .iter()
            .enumerate()
            .map(|(i, w)| (w.id().value(), i))
            .collect();
        let job_index: HashMap<_, _> = jobs
            .iter()
            .enumerate()
            .map(|(i, j)| (j.id().value(), i))
            .collect();
        let mut vars = ProblemVariables::new();
        let x: Vec<_> = offers
            .iter()
            .map(|o| vars.add(variable().binary().max(if o.qualified() { 1 } else { 0 })))
            .collect();
        let overtime: Vec<_> = workers
            .iter()
            .map(|w| {
                vars.add(
                    variable()
                        .integer()
                        .min(0)
                        .max(w.overtimeLimit().value() as f64),
                )
            })
            .collect();
        let excess: Vec<_> = workers
            .iter()
            .map(|w| {
                vars.add(
                    variable()
                        .integer()
                        .min(0)
                        .max(w.overtimeLimit().value() as f64),
                )
            })
            .collect();
        let mut objective = Expression::from(0);
        let mut job_sums = vec![Expression::from(0); jobs.len()];
        let mut worker_sums = vec![Expression::from(0); workers.len()];
        for (offer, &v) in offers.iter().zip(&x) {
            objective += offer.cost().value() as f64 * v;
            job_sums[job_index[&offer.jobId().value()]] += v;
            worker_sums[worker_index[&offer.workerId().value()]] +=
                offer.minutes().value() as f64 * v;
        }
        for (w, &v) in workers.iter().zip(&overtime) {
            objective += w.overtimeRate().value() as f64 * v;
        }
        for (w, &v) in workers.iter().zip(&excess) {
            objective += w.overtimePenalty().value() as f64 * v;
        }
        // Keep the empty problem a normal MILP with a fixed zero column.
        let zero = vars.add(variable().min(0).max(0));
        objective += zero;
        let mut model = vars.minimise(objective).using(good_lp::highs);
        for sum in job_sums {
            model.add_constraint(sum.eq(1));
        }
        for (i, sum) in worker_sums.into_iter().enumerate() {
            model.add_constraint(
                (sum - overtime[i]).leq(workers[i].regularMinutes().value() as f64),
            );
        }
        for (i, w) in workers.iter().enumerate() {
            model.add_constraint((overtime[i] - excess[i]).leq(w.overtimeTarget().value() as f64));
        }
        let mut native = model.try_into_inner()?;
        native
            .try_set_option("output_flag", false)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        native
            .try_set_option("threads", 1_i32)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        native
            .try_set_option("random_seed", 0_i32)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        native
            .try_set_option("time_limit", self.options.time_limit_seconds)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        native
            .try_set_option("mip_rel_gap", 0.0)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        native
            .try_set_option("mip_abs_gap", 0.0)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        native
            .try_set_option("mip_feasibility_tolerance", INTEGER_TOLERANCE)
            .map_err(|e| format!("HiGHS option: {e:?}"))?;
        let solved = native
            .try_solve()
            .map_err(|e| format!("HiGHS solve: {e:?}"))?;
        let has_solution = solved.primal_solution_status() == HighsSolutionStatus::Feasible;
        // HiGHS reports an infinite MIP gap for a pure LP (no workers/offers).
        let gap = if x.is_empty() && overtime.is_empty() && solved.status() == Status::Optimal {
            0.0
        } else {
            solved.mip_gap()
        };
        let completion = classify(solved.status(), has_solution, gap)?;
        let bound = solved
            .double_info_value(c"mip_dual_bound")
            .ok()
            .filter(|v| v.is_finite());
        *self.statistics.borrow_mut() = Some(json!({
            "status": format!("{:?}", solved.status()),
            "incumbentObjective": if has_solution { Some(solved.objective_value()) } else { None },
            "elapsedSeconds": started.elapsed().as_secs_f64(),
            "bestBound": bound,
            "relativeGap": if gap.is_finite() { Some(gap) } else { None },
            "timeLimitSeconds": self.options.time_limit_seconds,
            "integerTolerance": INTEGER_TOLERANCE
        }));
        match completion {
            Completion::Infeasible => return Ok(made(Infeasible::new(run))?.into()),
            Completion::NoSolution(reason) => {
                return Ok(made(NoCandidate::new(run, &reason))?.into());
            }
            _ => {}
        }
        let plan = reconstruct(
            run,
            problem,
            solved.get_solution().columns(),
            solved.objective_value(),
        )?;
        Ok(match completion {
            Completion::Optimal => made(OptimalCandidate::new(run, plan))?.into(),
            Completion::Feasible(reason) => {
                made(StoppedWithCandidate::new(run, plan, &reason))?.into()
            }
            _ => unreachable!("non-solution cases returned above"),
        })
    }
}

/// Decode solver columns, checking numerical representation and its reported objective.
/// Business feasibility belongs to the independent evaluator, invoked by the application.
/// Overtime and soft slacks are solver internals; the candidate contains only decisions.
pub fn reconstruct<'run>(
    run: &mut Run<'run>,
    problem: AssignmentProblem<'run>,
    columns: &[f64],
    reported_objective: f64,
) -> Result<CandidatePlan<'run>, HostError> {
    check_problem(problem)?;
    let workers = problem.workers();
    let offers = problem.offers();
    if columns.len() != offers.len() + 2 * workers.len() + 1 {
        return Err("unexpected HiGHS column count".into());
    }
    let mut assignments = Vec::new();
    let mut reported_cost = 0_i64;
    for (column, offer) in offers.iter().enumerate() {
        if binary(columns[column])? {
            assignments.push(made(Assignment::new(run, offer.workerId(), offer.jobId()))?);
            reported_cost += offer.cost().value();
        }
    }
    for (i, worker) in workers.iter().enumerate() {
        let overtime = integer_in_range(columns[offers.len() + i], MAX_LOAD_MINUTES)?;
        let excess = integer_in_range(columns[offers.len() + workers.len() + i], MAX_LOAD_MINUTES)?;
        reported_cost +=
            overtime * worker.overtimeRate().value() + excess * worker.overtimePenalty().value();
    }
    integer_in_range(columns[columns.len() - 1], 0)?;
    if !reported_objective.is_finite() || (reported_objective - reported_cost as f64).abs() > 1e-4 {
        return Err("solver objective disagrees with decoded integer columns".into());
    }
    made(CandidatePlan::new(run, &assignments))
}

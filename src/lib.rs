pub mod optimizer;
pub mod verify;

use model::example::assignment::*;
use model::{HostError, Library, Reading};
use optimizer::{Optimizer, SolveOptions};
use serde_json::{Value, json};
use std::path::Path;

pub struct Response {
    pub document: Value,
    pub exit_code: u8,
}

pub struct App {
    library: Library,
}

impl App {
    /// Load the native model paired with this executable's generated binding.
    ///
    /// # Safety
    /// The library must be the artifact generated alongside this binding by bin/build.
    pub unsafe fn load(path: impl AsRef<Path>) -> Result<Self, HostError> {
        // SAFETY: the caller guarantees that the library and binding are paired.
        let library = unsafe { Library::load(path) }?;
        Ok(Self { library })
    }

    pub fn solve(&self, input: &str, options: SolveOptions) -> Result<Response, HostError> {
        let solver = Optimizer::new(options)?;
        let statistics = solver.statistics.clone();
        let implementation = SolveImplementation::new(&self.library, solver);
        let plan = PlanAssignments::bind(&self.library, &implementation);
        self.library.run(|run| {
            let problem = match AssignmentProblem::decode(run, input)? {
                Reading::Value(problem) => problem,
                Reading::Issues(issues) => {
                    return Ok(Response {
                        document: json!({"issues": issues.to_json()}),
                        exit_code: 2,
                    });
                }
            };
            let result = plan.call(run, problem)?;
            let exit_code = match result.search().case() {
                SearchOutcomeCase::Optimal(_) => 0,
                SearchOutcomeCase::Feasible(_) | SearchOutcomeCase::NoSolution(_) => 4,
                SearchOutcomeCase::Infeasible(_) => 3,
            };
            if matches!(result.assessment().case(), AssessmentCase::Rejected(_)) {
                return Err("Souther rejected a solver candidate after Rust verification".into());
            }
            let mut document: Value = serde_json::from_str(&result.encode())?;
            document["solver"] = statistics
                .borrow()
                .clone()
                .ok_or("missing solver statistics")?;
            Ok(Response {
                document,
                exit_code,
            })
        })?
    }

    /// Inspect or confirm a candidate against the supplied current problem.
    /// This is a local demonstration of approval rules, not authentication or persistent approval.
    pub fn review(
        &self,
        input: &str,
        candidate: &str,
        approval: Option<bool>,
    ) -> Result<Response, HostError> {
        self.library.run(|run| {
            let problem = match AssignmentProblem::decode(run, input)? {
                Reading::Value(problem) => problem,
                Reading::Issues(issues) => {
                    return Ok(Response {
                        document: json!({"issues": issues.to_json()}),
                        exit_code: 2,
                    });
                }
            };
            let candidate = match CandidatePlan::decode(run, candidate)? {
                Reading::Value(plan) => plan,
                Reading::Issues(issues) => {
                    return Ok(Response {
                        document: json!({"issues": issues.to_json()}),
                        exit_code: 2,
                    });
                }
            };
            match approval {
                None => {
                    let result = Assess::new(&self.library).call(run, problem, candidate)?;
                    let exit_code = match result.case() {
                        AssessmentCase::Ready(_) => 0,
                        AssessmentCase::RequiresApproval(_) => 5,
                        AssessmentCase::Rejected(_) | AssessmentCase::Unavailable(_) => 6,
                    };
                    Ok(Response {
                        document: serde_json::from_str(&result.encode())?,
                        exit_code,
                    })
                }
                Some(approved) => {
                    let result =
                        Confirm::new(&self.library).call(run, problem, candidate, approved)?;
                    let exit_code = match result.case() {
                        ConfirmationCase::Confirmed(_) => 0,
                        ConfirmationCase::RequiresApproval(_) => 5,
                        ConfirmationCase::Rejected(_) => 6,
                    };
                    Ok(Response {
                        document: serde_json::from_str(&result.encode())?,
                        exit_code,
                    })
                }
            }
        })?
    }
}

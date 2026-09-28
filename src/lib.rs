pub mod optimizer;

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

fn decision_code(decision: Decision<'_>) -> u8 {
    match decision.case() {
        DecisionCase::Ready(_) | DecisionCase::Confirmed(_) => 0,
        DecisionCase::RequiresApproval(_) => 5,
        DecisionCase::Rejected(_) => 6,
    }
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
        let mut response = self.solve_with(input, &solver)?;
        if response.exit_code != 2 {
            response.document["solver"] = solver
                .statistics
                .borrow()
                .clone()
                .ok_or("missing solver statistics")?;
        }
        Ok(response)
    }

    /// Orchestrate a solver and the independent Souther evaluator through domain contracts.
    /// A replacement solver's claims never replace the evaluator's judgment.
    pub fn solve_with(&self, input: &str, solver: &impl Solve) -> Result<Response, HostError> {
        self.library.run(|run| {
            let problem = match AssignmentProblem::decode(run, input)? {
                Reading::Value(problem) => problem,
                Reading::Issues(issues) => return Ok(Response {
                    document: json!({"issues": issues.to_json()}), exit_code: 2,
                }),
            };
            let result = solver.apply(run, problem)?;
            let (candidate, mut exit_code) = match result.case() {
                SearchOutcomeCase::OptimalCandidate(found) => (Some(found.plan()), 0),
                SearchOutcomeCase::StoppedWithCandidate(found) => (Some(found.plan()), 4),
                SearchOutcomeCase::Infeasible(_) => (None, 3),
                SearchOutcomeCase::NoCandidate(_) => (None, 4),
            };
            let review = match candidate {
                Some(plan) => {
                    // valid: resolve the candidate's references in this specific problem.
                    let input = EvaluationInput::new(run, problem, plan)?.into_result()?;
                    let reviewed = Review::new(&self.library).call(run, input)?;
                    if !reviewed.evaluation().feasible() {
                        exit_code = 6;
                    }
                    serde_json::from_str(&reviewed.encode())?
                }
                None => Value::Null,
            };
            Ok(Response {
                document: json!({"search": serde_json::from_str::<Value>(&result.encode())?, "review": review}),
                exit_code,
            })
        })?
    }

    /// Evaluate a saved or manually authored candidate without constructing an optimizer.
    /// Contextual valid constraints run in the generated decoder before the evaluator runs.
    pub fn review(
        &self,
        input: &str,
        candidate: &str,
        approval: Option<bool>,
    ) -> Result<Response, HostError> {
        self.library.run(|run| {
            // Decode both complete JSON documents before composing the contextual input.
            let problem = match AssignmentProblem::decode(run, input)? {
                Reading::Value(problem) => problem,
                Reading::Issues(issues) => {
                    return Ok(Response {
                        document: json!({"issues": issues.to_json()}),
                        exit_code: 2,
                    });
                }
            };
            let plan = match CandidatePlan::decode(run, candidate)? {
                Reading::Value(plan) => plan,
                Reading::Issues(issues) => {
                    return Ok(Response {
                        document: json!({"issues": issues.to_json()}),
                        exit_code: 2,
                    });
                }
            };
            let source = format!(
                "{{\"problem\":{},\"plan\":{}}}",
                problem.encode(),
                plan.encode()
            );
            let input = match EvaluationInput::decode(run, &source)? {
                Reading::Value(input) => input,
                Reading::Issues(issues) => {
                    return Ok(Response {
                        document: json!({"issues": issues.to_json()}),
                        exit_code: 2,
                    });
                }
            };
            let reviewed = match approval {
                None => Review::new(&self.library).call(run, input)?,
                Some(approved) => Confirm::new(&self.library).call(run, input, approved)?,
            };
            Ok(Response {
                document: serde_json::from_str(&reviewed.encode())?,
                exit_code: decision_code(reviewed.decision()),
            })
        })?
    }
}

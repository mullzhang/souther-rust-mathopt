use highs::HighsModelStatus as Status;
use model::example::assignment::*;
use model::{HostError, Library, Run};
use serde_json::{Value, json};
use souther_rust_mathopt::{
    App,
    optimizer::{Completion, SolveOptions, binary, classify, made},
    verify,
};
use std::path::PathBuf;
use std::process::Command;

fn library_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("build/native")
        .join(format!(
            "{}souther{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ))
}
fn app() -> App {
    // SAFETY: bin/build generated these artifacts together.
    unsafe { App::load(library_path()) }.unwrap()
}
fn library() -> Library {
    // SAFETY: bin/build generated these artifacts together.
    unsafe { Library::load(library_path()) }.unwrap()
}
fn fixture(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join(format!("{name}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}
fn solve(problem: &Value) -> souther_rust_mathopt::Response {
    app()
        .solve(&problem.to_string(), SolveOptions::default())
        .unwrap()
}

#[test]
fn regular_and_overtime_plans_have_separate_search_and_business_states() {
    for (name, assessment) in [("regular", "Ready"), ("overtime", "RequiresApproval")] {
        let result = solve(&fixture(name));
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.document["search"]["type"], "Optimal");
        assert_eq!(result.document["search"]["plan"]["totalCost"], 60);
        assert_eq!(result.document["assessment"]["type"], assessment);
        assert_eq!(result.document["solver"]["bestBound"], 60.0);
        assert_eq!(result.document["solver"]["relativeGap"], 0.0);
    }
}

#[test]
fn approval_is_required_and_cannot_override_a_rejected_plan() {
    let app = app();
    let problem = fixture("overtime").to_string();
    let plan = fixture("overtime-plan").to_string();
    for approval in [None, Some(false)] {
        let result = app.review(&problem, &plan, approval).unwrap();
        assert_eq!(result.exit_code, 5);
        assert_eq!(result.document["type"], "RequiresApproval");
        assert_eq!(result.document["overtimeMinutes"], 30);
    }
    let result = app.review(&problem, &plan, Some(true)).unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.document["type"], "Confirmed");
    let result = app
        .review(&problem, &fixture("rejected-plan").to_string(), Some(true))
        .unwrap();
    assert_eq!(result.exit_code, 6);
    assert_eq!(result.document["type"], "Rejected");
}

#[test]
fn regular_plan_can_be_confirmed_without_overtime_approval() {
    let problem = fixture("regular");
    let result = solve(&problem);
    let plan = result.document["search"]["plan"].to_string();
    assert_eq!(
        app()
            .review(&problem.to_string(), &plan, Some(false))
            .unwrap()
            .document["type"],
        "Confirmed"
    );
}

#[test]
fn infeasible_is_distinct_from_invalid_input() {
    let infeasible = solve(&fixture("infeasible"));
    assert_eq!(infeasible.exit_code, 3);
    assert_eq!(infeasible.document["search"]["type"], "Infeasible");
    assert_eq!(infeasible.document["assessment"]["type"], "Unavailable");
    let invalid = solve(&fixture("invalid"));
    assert_eq!(invalid.exit_code, 2);
    assert!(invalid.document.get("solver").is_none());
    assert!(
        invalid.document["issues"]
            .to_string()
            .contains("/offers/0/minutes")
    );
}

#[test]
fn souther_rejects_duplicate_references_ranges_and_missing_fields() {
    let original = fixture("regular");
    let mut cases = Vec::new();
    let mut p = original.clone();
    p["workers"][1]["id"] = json!("alice");
    cases.push(p);
    let mut p = original.clone();
    p["jobs"][1]["id"] = json!("job1");
    cases.push(p);
    let mut p = original.clone();
    p["offers"][1] = p["offers"][0].clone();
    cases.push(p);
    let mut p = original.clone();
    p["offers"][0]["workerId"] = json!("missing");
    cases.push(p);
    let mut p = original.clone();
    p["offers"][0]["jobId"] = json!("missing");
    cases.push(p);
    let mut p = original.clone();
    p["workers"][0]["regularMinutes"] = json!(-1);
    cases.push(p);
    let mut p = original.clone();
    p["workers"][0]["overtimeLimit"] = json!(1441);
    cases.push(p);
    let mut p = original.clone();
    p["workers"][0]["overtimeRate"] = json!(0);
    cases.push(p);
    let mut p = original.clone();
    p["offers"][0]["cost"] = json!(1000001);
    cases.push(p);
    let mut p = original.clone();
    p["offers"][0]["minutes"] = json!(0.5);
    cases.push(p);
    let mut p = original.clone();
    p["workers"][0]["id"] = json!("");
    cases.push(p);
    let mut p = original.clone();
    p.as_object_mut().unwrap().remove("offers");
    cases.push(p);
    let mut p = original;
    p["jobs"] = json!(
        (0..33)
            .map(|i| json!({"id":format!("j{i}")}))
            .collect::<Vec<_>>()
    );
    cases.push(p);
    for (i, p) in cases.iter().enumerate() {
        let result = solve(p);
        assert_eq!(result.exit_code, 2, "case {i}: {}", result.document);
        assert!(result.document.get("solver").is_none());
    }
    assert_eq!(
        app().solve("{", SolveOptions::default()).unwrap().exit_code,
        2
    );
}

#[test]
fn empty_problem_and_missing_offers_have_defined_results() {
    let empty = json!({"workers":[],"jobs":[],"offers":[]});
    let result = solve(&empty);
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.document["search"]["plan"]["totalCost"], 0);
    let mut p = fixture("regular");
    p["jobs"] = json!([]);
    p["offers"] = json!([]);
    assert_eq!(solve(&p).exit_code, 0);
    let p = json!({"workers":[],"jobs":[{"id":"j"}],"offers":[]});
    assert_eq!(solve(&p).exit_code, 3);
    let mut p = fixture("regular");
    p["offers"] = json!([]);
    assert_eq!(solve(&p).exit_code, 3);
}

#[test]
fn both_domain_review_and_independent_verifier_reject_tampering() {
    let problem_json = fixture("overtime");
    let original = fixture("overtime-plan");
    let mut cases = Vec::new();
    let mut p = original.clone();
    p["assignments"][1] = p["assignments"][0].clone();
    cases.push((p, "job_coverage"));
    let mut p = original.clone();
    p["assignments"].as_array_mut().unwrap().pop();
    cases.push((p, "job_coverage"));
    let mut p = original.clone();
    p["assignments"][0]["workerId"] = json!("unknown");
    cases.push((p, "qualification"));
    let mut p = original.clone();
    p["loads"][1] = p["loads"][0].clone();
    cases.push((p, "worker_coverage"));
    let mut p = original.clone();
    p["loads"][0]["minutes"] = json!(89);
    cases.push((p, "workload"));
    let mut p = original.clone();
    p["loads"][0]["overtime"] = json!(29);
    cases.push((p, "workload"));
    let mut p = original;
    p["totalCost"] = json!(59);
    cases.push((p, "total_cost"));
    let app = app();
    let lib = library();
    for (candidate, reason) in cases {
        let result = app
            .review(&problem_json.to_string(), &candidate.to_string(), None)
            .unwrap();
        assert_eq!(result.document, json!({"type":"Rejected", "reason":reason}));
        lib.run(|run| {
            let problem = AssignmentProblem::decode(run, &problem_json.to_string())
                .unwrap()
                .into_result()
                .unwrap();
            let plan = CandidatePlan::decode(run, &candidate.to_string())
                .unwrap()
                .into_result()
                .unwrap();
            assert!(verify::verify(problem, plan).is_err());
        })
        .unwrap();
    }
}

#[test]
fn rechecking_against_changed_capacity_refuses_previously_valid_plan() {
    let mut problem = fixture("overtime");
    problem["workers"][0]["overtimeLimit"] = json!(29);
    let result = app()
        .review(
            &problem.to_string(),
            &fixture("overtime-plan").to_string(),
            Some(true),
        )
        .unwrap();
    assert_eq!(result.document["type"], "Rejected");
    assert_eq!(result.document["reason"], "workload");
}

#[test]
fn statuses_keep_limits_and_ambiguous_failures_separate() {
    assert_eq!(
        classify(Status::Optimal, true, 0.0).unwrap(),
        Completion::Optimal
    );
    assert_eq!(
        classify(Status::Infeasible, false, f64::INFINITY).unwrap(),
        Completion::Infeasible
    );
    for status in [
        Status::ReachedTimeLimit,
        Status::ReachedIterationLimit,
        Status::ReachedSolutionLimit,
        Status::ReachedMemoryLimit,
        Status::ReachedInterrupt,
        Status::ObjectiveTarget,
        Status::ObjectiveBound,
    ] {
        assert!(matches!(
            classify(status, true, 0.5).unwrap(),
            Completion::Feasible(_)
        ));
        assert!(matches!(
            classify(status, false, f64::INFINITY).unwrap(),
            Completion::NoSolution(_)
        ));
    }
    assert!(matches!(
        classify(Status::Optimal, true, 0.01).unwrap(),
        Completion::Feasible(_)
    ));
    for status in [
        Status::UnboundedOrInfeasible,
        Status::Unbounded,
        Status::SolveError,
        Status::Unknown,
        Status::NotSet,
    ] {
        assert!(classify(status, false, f64::INFINITY).is_err());
    }
    assert!(classify(Status::Optimal, false, 0.0).is_err());
    assert!(classify(Status::Optimal, true, f64::NAN).is_err());
}

#[test]
fn integer_conversion_does_not_silently_threshold_fractional_values() {
    for value in [0.0, 1e-8, -1e-8] {
        assert!(!binary(value).unwrap());
    }
    for value in [1.0, 1.0 - 1e-8, 1.0 + 1e-8] {
        assert!(binary(value).unwrap());
    }
    for value in [0.4, 0.999, -0.1, 2.0, f64::NAN, f64::INFINITY] {
        assert!(binary(value).is_err());
    }
    for limit in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            app()
                .solve(
                    &fixture("regular").to_string(),
                    SolveOptions {
                        time_limit_seconds: limit
                    }
                )
                .is_err()
        );
    }
}

struct PartialSolver;
impl Solve for PartialSolver {
    fn apply<'run>(
        &self,
        run: &mut Run<'run>,
        _problem: AssignmentProblem<'run>,
    ) -> Result<SearchOutcome<'run>, HostError> {
        let plan =
            CandidatePlan::decode(run, &fixture("overtime-plan").to_string())?.into_result()?;
        Ok(made(Feasible::new(run, plan, "ReachedTimeLimit"))?.into())
    }
}

struct InterruptedSolver;
impl Solve for InterruptedSolver {
    fn apply<'run>(
        &self,
        run: &mut Run<'run>,
        _problem: AssignmentProblem<'run>,
    ) -> Result<SearchOutcome<'run>, HostError> {
        Ok(made(NoSolution::new(run, "ReachedTimeLimit"))?.into())
    }
}

#[test]
fn interruption_without_a_plan_survives_the_souther_boundary() {
    let lib = library();
    let solver = SolveImplementation::new(&lib, InterruptedSolver);
    let behavior = PlanAssignments::bind(&lib, &solver);
    lib.run(|run| {
        let problem = AssignmentProblem::decode(run, &fixture("regular").to_string())
            .unwrap()
            .into_result()
            .unwrap();
        let result = behavior.call(run, problem).unwrap();
        assert!(matches!(
            result.search().case(),
            SearchOutcomeCase::NoSolution(_)
        ));
        assert!(matches!(
            result.assessment().case(),
            AssessmentCase::Unavailable(_)
        ));
    })
    .unwrap();
}

#[test]
fn unproven_feasible_plan_still_gets_domain_review() {
    let lib = library();
    let solver = SolveImplementation::new(&lib, PartialSolver);
    let behavior = PlanAssignments::bind(&lib, &solver);
    lib.run(|run| {
        let problem = AssignmentProblem::decode(run, &fixture("overtime").to_string())
            .unwrap()
            .into_result()
            .unwrap();
        let result = behavior.call(run, problem).unwrap();
        assert!(matches!(
            result.search().case(),
            SearchOutcomeCase::Feasible(_)
        ));
        assert!(matches!(
            result.assessment().case(),
            AssessmentCase::RequiresApproval(_)
        ));
    })
    .unwrap();
}

// Enumerate the original offers, independently of good_lp, HiGHS, and production verification.
fn exhaustive(problem: &Value) -> Option<i64> {
    let workers = problem["workers"].as_array().unwrap();
    let jobs = problem["jobs"].as_array().unwrap();
    let offers = problem["offers"].as_array().unwrap();
    let mut states = vec![(vec![0_i64; workers.len()], 0_i64)];
    for job in jobs {
        let mut next = Vec::new();
        for (minutes, cost) in &states {
            for offer in offers.iter().filter(|o| o["jobId"] == job["id"]) {
                let i = workers
                    .iter()
                    .position(|w| w["id"] == offer["workerId"])
                    .unwrap();
                let mut m = minutes.clone();
                m[i] += offer["minutes"].as_i64().unwrap();
                if m[i]
                    <= workers[i]["regularMinutes"].as_i64().unwrap()
                        + workers[i]["overtimeLimit"].as_i64().unwrap()
                {
                    next.push((m, cost + offer["cost"].as_i64().unwrap()));
                }
            }
        }
        states = next;
    }
    states
        .into_iter()
        .map(|(minutes, cost)| {
            cost + workers
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    (minutes[i] - w["regularMinutes"].as_i64().unwrap()).max(0)
                        * w["overtimeRate"].as_i64().unwrap()
                })
                .sum::<i64>()
        })
        .min()
}

#[test]
fn eighty_small_instances_match_exhaustive_optima() {
    let mut state = 0x5eed_u64;
    let mut next = |bound: u64| {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 32) % bound
    };
    for case in 0..80 {
        let n = 1 + next(3);
        let m = next(6);
        let workers: Vec<_> = (0..n).map(|i| json!({"id":format!("w{i}"),"regularMinutes":next(8),"overtimeLimit":next(5),"overtimeRate":1+next(5)})).collect();
        let jobs: Vec<_> = (0..m).map(|i| json!({"id":format!("j{i}")})).collect();
        let mut offers = Vec::new();
        for worker in &workers {
            for job in &jobs {
                if next(4) > 0 {
                    offers.push(json!({"workerId":worker["id"],"jobId":job["id"],"minutes":1+next(6),"cost":next(20)}));
                }
            }
        }
        let p = json!({"workers":workers,"jobs":jobs,"offers":offers});
        let expected = exhaustive(&p);
        let actual = solve(&p);
        match expected {
            Some(cost) => {
                assert_eq!(actual.exit_code, 0, "case {case}: {}", actual.document);
                assert_eq!(
                    actual.document["search"]["plan"]["totalCost"], cost,
                    "case {case}"
                );
            }
            None => assert_eq!(actual.exit_code, 3, "case {case}: {}", actual.document),
        }
    }
}

#[test]
fn real_zero_time_limit_preserves_the_actual_solver_status() {
    let result = app()
        .solve(
            &fixture("regular").to_string(),
            SolveOptions {
                time_limit_seconds: 0.0,
            },
        )
        .unwrap();
    assert_eq!(result.document["solver"]["timeLimitSeconds"], 0.0);
    // A trivial instance may finish in presolve before HiGHS checks the clock.
    // Deterministic termination mapping is covered by statuses_keep_limits_*.
    match result.document["search"]["type"].as_str() {
        Some("Optimal") => {
            assert_eq!(result.exit_code, 0);
            assert_eq!(result.document["solver"]["status"], "Optimal");
            assert_eq!(result.document["search"]["plan"]["totalCost"], 60);
        }
        Some("NoSolution" | "Feasible") => assert_eq!(result.exit_code, 4),
        other => panic!("a known feasible instance cannot be {other:?}"),
    }
}

#[test]
fn maximum_supported_numbers_remain_exact_in_integer_reconstruction() {
    let workers: Vec<_> = (0..16).map(|i| json!({"id":format!("w{i}"),"regularMinutes":1440,"overtimeLimit":1440,"overtimeRate":10000})).collect();
    let jobs: Vec<_> = (0..32).map(|i| json!({"id":format!("j{i}")})).collect();
    let offers: Vec<_> = (0..32).map(|i| json!({"workerId":format!("w{}",i/2),"jobId":format!("j{i}"),"minutes":1440,"cost":1000000})).collect();
    let result = solve(&json!({"workers":workers,"jobs":jobs,"offers":offers}));
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.document["search"]["plan"]["totalCost"], 262400000);
    assert_eq!(result.document["assessment"]["overtimeMinutes"], 23040);
}

#[test]
fn a_feasible_incumbent_can_have_unnecessary_overtime_without_being_invalid() {
    let mut problem_json = fixture("overtime");
    problem_json["workers"][0]["overtimeLimit"] = json!(60);
    let lib = library();
    lib.run(|run| {
        let problem = AssignmentProblem::decode(run, &problem_json.to_string())
            .unwrap()
            .into_result()
            .unwrap();
        // Six offer columns, two overtime columns, then the fixed zero column.
        let columns = [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 45.0, 0.0, 0.0];
        let plan =
            souther_rust_mathopt::optimizer::reconstruct(run, problem, &columns, 75.0, false)
                .unwrap();
        assert_eq!(plan.totalCost().value(), 60);
        assert_eq!(plan.loads()[0].overtime().value(), 30);
        // The same suboptimal numbers cannot support an Optimal claim.
        assert!(
            souther_rust_mathopt::optimizer::reconstruct(run, problem, &columns, 75.0, true)
                .is_err()
        );
        let mut fractional = columns;
        fractional[6] = 45.5;
        assert!(
            souther_rust_mathopt::optimizer::reconstruct(run, problem, &fractional, 75.5, false)
                .is_err()
        );
    })
    .unwrap();
}

#[test]
fn cli_reports_json_and_process_exit_codes() {
    for (file, code) in [
        ("regular", 0),
        ("overtime", 0),
        ("infeasible", 3),
        ("invalid", 2),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_souther-rust-mathopt"))
            .args(["solve", &format!("examples/{file}.json")])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(code));
        serde_json::from_slice::<Value>(&result.stdout).unwrap();
        assert!(result.stderr.is_empty());
    }
}

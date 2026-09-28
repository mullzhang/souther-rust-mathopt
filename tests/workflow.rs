use highs::HighsModelStatus as Status;
use model::example::assignment::*;
use model::{HostError, Library, Run};
use serde_json::{Value, json};
use souther_rust_mathopt::{
    App,
    optimizer::{Completion, SolveOptions, binary, classify, made},
};
use std::{path::PathBuf, process::Command};

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
fn review(problem: &Value, plan: &Value) -> souther_rust_mathopt::Response {
    app()
        .review(&problem.to_string(), &plan.to_string(), None)
        .unwrap()
}

#[test]
fn regular_and_overtime_plans_separate_solver_evaluation_and_adoption() {
    for (name, decision) in [("regular", "Ready"), ("overtime", "RequiresApproval")] {
        let result = solve(&fixture(name));
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.document["search"]["type"], "OptimalCandidate");
        assert_eq!(result.document["review"]["evaluation"]["totalCost"], 60);
        assert_eq!(result.document["review"]["evaluation"]["feasible"], true);
        assert_eq!(result.document["review"]["decision"]["type"], decision);
        assert_eq!(result.document["solver"]["bestBound"], 60.0);
    }
}

#[test]
fn approval_is_required_but_cannot_override_hard_violations() {
    let app = app();
    let problem = fixture("overtime").to_string();
    let plan = fixture("overtime-plan").to_string();
    for approval in [None, Some(false)] {
        let r = app.review(&problem, &plan, approval).unwrap();
        assert_eq!(r.exit_code, 5);
        assert_eq!(
            r.document["decision"],
            json!({"type":"RequiresApproval","overtimeMinutes":30})
        );
    }
    let r = app.review(&problem, &plan, Some(true)).unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.document["decision"]["type"], "Confirmed");
    let r = app
        .review(&problem, &fixture("rejected-plan").to_string(), Some(true))
        .unwrap();
    assert_eq!(r.exit_code, 6);
    assert_eq!(r.document["decision"]["type"], "Rejected");
    assert_eq!(
        r.document["evaluation"]["hardViolations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_regular_plan_can_be_confirmed_without_approval() {
    let problem = fixture("regular");
    let result = solve(&problem);
    let r = app()
        .review(
            &problem.to_string(),
            &result.document["search"]["plan"].to_string(),
            Some(false),
        )
        .unwrap();
    assert_eq!(r.document["decision"]["type"], "Confirmed");
}

#[test]
fn all_hard_violations_and_soft_penalties_survive_in_one_report() {
    let r = review(&fixture("diagnostic"), &fixture("diagnostic-plan"));
    assert_eq!(r.exit_code, 6);
    assert_eq!(r.document["plan"], fixture("diagnostic-plan"));
    let e = &r.document["evaluation"];
    assert_eq!(e["feasible"], false);
    assert_eq!(
        e["hardViolations"],
        json!([
            {"rule":"job_coverage","subject":"job1","actual":2,"limit":1,"amount":1},
            {"rule":"qualification","subject":"alice/job1","actual":1,"limit":0,"amount":1},
            {"rule":"overtime_limit","subject":"alice","actual":30,"limit":0,"amount":30}
        ])
    );
    assert_eq!(
        e["softPenalties"],
        json!([
            {"rule":"overtime_target","subject":"alice","actual":30,"limit":0,"amount":30,"weight":3,"penalty":90}
        ])
    );
    assert_eq!(
        e["loads"][0],
        json!({"workerId":"alice","minutes":90,"overtime":30})
    );
    assert_eq!(e["totalCost"], 160);
    assert_eq!(e["objectiveValue"], 250);
}

#[test]
fn evaluator_runs_directly_without_an_optimizer_or_solver_status() {
    let lib = library();
    lib.run(|run| {
        let input = EvaluationInput::decode(
            run,
            &json!({"problem":fixture("diagnostic"),"plan":fixture("diagnostic-plan")}).to_string(),
        )
        .unwrap()
        .into_result()
        .unwrap();
        let e = Evaluate::new(&lib).call(run, input).unwrap();
        assert!(!e.feasible());
        assert_eq!(e.hardViolations().len(), 3);
        assert_eq!(e.softPenalty(), 90);
    })
    .unwrap();
}

#[test]
fn soft_penalties_change_the_optimum_without_changing_feasibility() {
    let original = review(&fixture("soft"), &fixture("overtime-plan"));
    assert_eq!(original.document["evaluation"]["feasible"], true);
    assert_eq!(original.document["evaluation"]["softPenalty"], 90);
    assert_eq!(original.document["evaluation"]["objectiveValue"], 150);
    let optimized = solve(&fixture("soft"));
    let e = &optimized.document["review"]["evaluation"];
    assert_eq!(e["totalCost"], 120);
    assert_eq!(e["softPenalty"], 0);
    assert_eq!(e["objectiveValue"], 120);
    assert_eq!(optimized.document["solver"]["incumbentObjective"], 120.0);
    // A soft penalty does not prevent adopting a hard-feasible plan.
    let r = app()
        .review(
            &fixture("soft").to_string(),
            &fixture("overtime-plan").to_string(),
            Some(true),
        )
        .unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.document["evaluation"]["softPenalty"], 90);
}

#[test]
fn infeasible_search_is_distinct_from_invalid_input_and_a_bad_candidate() {
    let r = solve(&fixture("infeasible"));
    assert_eq!(r.exit_code, 3);
    assert_eq!(r.document["search"]["type"], "Infeasible");
    assert_eq!(r.document["review"], Value::Null);
    let r = review(&fixture("infeasible"), &fixture("overtime-plan"));
    assert_eq!(r.exit_code, 6);
    assert_eq!(
        r.document["evaluation"]["hardViolations"][0]["rule"],
        "overtime_limit"
    );
    let r = solve(&fixture("invalid"));
    assert_eq!(r.exit_code, 2);
    assert!(
        r.document["issues"]
            .to_string()
            .contains("/offers/0/minutes")
    );
    assert!(r.document.get("solver").is_none());
}

#[test]
fn valid_constraints_reject_ambiguous_or_undefined_candidates_at_construction() {
    let problem = fixture("overtime");
    let original = fixture("overtime-plan");
    let mut cases = Vec::new();
    let mut p = original.clone();
    p["assignments"][1] = p["assignments"][0].clone();
    cases.push(p);
    let mut p = original.clone();
    p["assignments"][0]["workerId"] = json!("missing");
    cases.push(p);
    let mut p = original.clone();
    p["assignments"][0]["jobId"] = json!("missing");
    cases.push(p);
    for p in cases {
        let r = review(&problem, &p);
        assert_eq!(r.exit_code, 2, "{}", r.document);
        assert!(r.document.get("evaluation").is_none());
    }
    let mut missing_pair = problem.clone();
    missing_pair["offers"].as_array_mut().unwrap().remove(0);
    assert_eq!(review(&missing_pair, &original).exit_code, 2);
    let lib = library();
    lib.run(|run| {
        let problem = AssignmentProblem::decode(run, &missing_pair.to_string())
            .unwrap()
            .into_result()
            .unwrap();
        let plan = CandidatePlan::decode(run, &original.to_string())
            .unwrap()
            .into_result()
            .unwrap();
        assert!(
            EvaluationInput::new(run, problem, plan)
                .unwrap()
                .into_result()
                .is_err()
        );
    })
    .unwrap();
}

#[test]
fn partial_and_unqualified_plans_are_valid_but_hard_infeasible() {
    let p = fixture("diagnostic");
    let plan = json!({"assignments":[{"workerId":"alice","jobId":"job1"}]});
    let r = review(&p, &plan);
    assert_eq!(r.exit_code, 6);
    assert_eq!(
        r.document["evaluation"]["hardViolations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(r.document["evaluation"]["totalCost"], 10);
}

#[test]
fn souther_rejects_invalid_problem_data_before_solving() {
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
    p["workers"][0]["overtimeRate"] = json!(0);
    cases.push(p);
    let mut p = original.clone();
    p["workers"][0]["overtimePenalty"] = json!(0);
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
    let mut p = original.clone();
    p["offers"][0].as_object_mut().unwrap().remove("qualified");
    cases.push(p);
    for (i, p) in cases.iter().enumerate() {
        let r = solve(p);
        assert_eq!(r.exit_code, 2, "case {i}: {}", r.document);
        assert!(r.document.get("solver").is_none());
    }
    assert_eq!(
        app().solve("{", SolveOptions::default()).unwrap().exit_code,
        2
    );
    assert_eq!(app().review("{", "{}", None).unwrap().exit_code, 2);
    let fragment = format!("{},\"unexpected\":0", fixture("regular"));
    assert_eq!(
        app()
            .review(&fragment, "{\"assignments\":[]}", None)
            .unwrap()
            .exit_code,
        2
    );
}

#[test]
fn domain_construction_has_no_host_size_or_numeric_caps() {
    let workers: Vec<_> = (0..17)
        .map(|i| json!({"id":format!("{}w{i}", "w".repeat(65)),"regularMinutes":1441,"overtimeLimit":1441,"overtimeRate":10001,"overtimeTarget":1441,"overtimePenalty":10001}))
        .collect();
    let jobs: Vec<_> = (0..33).map(|i| json!({"id":format!("j{i}")})).collect();
    let mut offers = Vec::new();
    let mut assignments = Vec::new();
    for w in &workers {
        for j in &jobs {
            offers.push(json!({"workerId":w["id"],"jobId":j["id"],"minutes":1441,"cost":1000001,"qualified":true}));
            assignments.push(json!({"workerId":w["id"],"jobId":j["id"]}));
        }
    }
    let source = json!({
        "problem":{"workers":workers,"jobs":jobs,"offers":offers},
        "plan":{"assignments":assignments}
    });
    let lib = library();
    lib.run(|run| {
        let input = EvaluationInput::decode(run, &source.to_string())
            .unwrap()
            .into_result()
            .unwrap();
        assert_eq!(input.plan().assignments().len(), 561);
        assert_eq!(
            serde_json::from_str::<Value>(&input.encode()).unwrap(),
            source
        );
        assert_eq!(made(LoadMinutes::new(run, 46081)).unwrap().value(), 46081);
        assert_eq!(
            made(TotalCost::new(run, 8000000001)).unwrap().value(),
            8000000001
        );
    })
    .unwrap();
}

struct MustNotRun;
impl Solve for MustNotRun {
    fn apply<'run>(
        &self,
        _run: &mut Run<'run>,
        _problem: AssignmentProblem<'run>,
    ) -> Result<SearchOutcome<'run>, HostError> {
        panic!("execution limits must be checked before invoking the solver");
    }
}

#[test]
fn execution_limits_are_separate_from_validity_and_block_solve_review_and_confirm() {
    let mut cases = Vec::new();
    for (path, value, limit) in [
        ("/workers/0/regularMinutes", 1441, 1440),
        ("/workers/0/overtimeLimit", 1441, 1440),
        ("/workers/0/overtimeTarget", 1441, 1440),
        ("/workers/0/overtimeRate", 10001, 10000),
        ("/workers/0/overtimePenalty", 10001, 10000),
        ("/offers/0/minutes", 1441, 1440),
        ("/offers/0/cost", 1000001, 1000000),
        ("/offers/0/cost", i64::MAX, 1000000),
    ] {
        let mut p = fixture("regular");
        *p.pointer_mut(path).unwrap() = json!(value);
        cases.push((p, path, value, limit));
    }
    for (list, count, limit) in [("workers", 17, 16), ("jobs", 33, 32)] {
        let mut p = fixture("regular");
        let mut entries = Vec::new();
        for i in 0..count {
            let mut entry = p[list][0].clone();
            entry["id"] = json!(format!("id{i}"));
            entries.push(entry);
        }
        p[list] = json!(entries);
        p["offers"] = json!([]);
        cases.push((
            p,
            if list == "workers" {
                "/workers"
            } else {
                "/jobs"
            },
            count,
            limit,
        ));
    }
    for list in ["workers", "jobs"] {
        let mut p = fixture("regular");
        p[list][0]["id"] = json!("x".repeat(65));
        p["offers"] = json!([]);
        cases.push((
            p,
            if list == "workers" {
                "/workers/0/id"
            } else {
                "/jobs/0/id"
            },
            65,
            64,
        ));
    }
    let app = app();
    let lib = library();
    for (p, path, actual, limit) in cases {
        let input = p.to_string();
        lib.run(|run| {
            let problem = AssignmentProblem::decode(run, &input)
                .unwrap()
                .into_result()
                .unwrap();
            let encoded: Value = serde_json::from_str(&problem.encode()).unwrap();
            assert_eq!(encoded, p, "{path}");
            // Direct optimizer users cannot bypass the arithmetic bounds either.
            let optimizer =
                souther_rust_mathopt::optimizer::Optimizer::new(SolveOptions::default()).unwrap();
            assert!(
                optimizer
                    .apply(run, problem)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("execution limit")
            );
            assert!(optimizer.statistics.borrow().is_none());
        })
        .unwrap();
        let expected = json!({"error":"execution_limit","path":path,"actual":actual,"limit":limit});
        for result in [
            app.solve(&input, SolveOptions::default()).unwrap(),
            app.solve_with(&input, &MustNotRun).unwrap(),
            app.review(&input, "{\"assignments\":[]}", None).unwrap(),
            app.review(&input, "{\"assignments\":[]}", Some(false))
                .unwrap(),
            app.review(&input, "{\"assignments\":[]}", Some(true))
                .unwrap(),
        ] {
            assert_eq!(result.exit_code, 7, "{path}: {}", result.document);
            assert_eq!(result.document, expected);
        }
    }
}

#[test]
fn malformed_candidates_still_report_valid_issues_with_an_oversized_problem() {
    let mut problem = fixture("regular");
    problem["workers"][0]["overtimeLimit"] = json!(1441);
    let plan = json!({"assignments":[{"workerId":"missing","jobId":"job1"}]});
    let r = review(&problem, &plan);
    assert_eq!(r.exit_code, 2);
    assert!(r.document.get("issues").is_some());
    assert!(r.document.get("error").is_none());
}

#[test]
fn inclusive_execution_limit_boundaries_still_solve() {
    let workers: Vec<_> = (0..16).map(|i| json!({"id":format!("w{i:063}"),"regularMinutes":1440,"overtimeLimit":1440,"overtimeRate":10000,"overtimeTarget":1440,"overtimePenalty":10000})).collect();
    let jobs: Vec<_> = (0..32).map(|i| json!({"id":format!("j{i:063}")})).collect();
    let offers: Vec<_> = jobs.iter().enumerate().map(|(i,j)| json!({"workerId":workers[i/2]["id"],"jobId":j["id"],"minutes":1440,"cost":1000000,"qualified":true})).collect();
    let r = solve(&json!({"workers":workers,"jobs":jobs,"offers":offers}));
    assert_eq!(r.exit_code, 0, "{}", r.document);
    assert_eq!(r.document["review"]["evaluation"]["feasible"], true);
    assert_eq!(
        r.document["review"]["evaluation"]["objectiveValue"],
        262400000
    );
}

#[test]
fn empty_and_impossible_problems_remain_evaluable() {
    let empty = json!({"workers":[],"jobs":[],"offers":[]});
    assert_eq!(
        solve(&empty).document["review"]["evaluation"]["objectiveValue"],
        0
    );
    let p = json!({"workers":[],"jobs":[{"id":"j"}],"offers":[]});
    assert_eq!(solve(&p).exit_code, 3);
    assert_eq!(
        review(&p, &json!({"assignments":[]})).document["evaluation"]["hardViolations"][0]["amount"],
        1
    );
}

struct ClaimedOptimalButIncomplete;
impl Solve for ClaimedOptimalButIncomplete {
    fn apply<'run>(
        &self,
        run: &mut Run<'run>,
        _problem: AssignmentProblem<'run>,
    ) -> Result<SearchOutcome<'run>, HostError> {
        let plan = made(CandidatePlan::new(run, &[]))?;
        Ok(made(OptimalCandidate::new(run, plan))?.into())
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
        Ok(made(StoppedWithCandidate::new(run, plan, "ReachedTimeLimit"))?.into())
    }
}
struct InterruptedSolver;
impl Solve for InterruptedSolver {
    fn apply<'run>(
        &self,
        run: &mut Run<'run>,
        _problem: AssignmentProblem<'run>,
    ) -> Result<SearchOutcome<'run>, HostError> {
        Ok(made(NoCandidate::new(run, "ReachedTimeLimit"))?.into())
    }
}

#[test]
fn orchestration_preserves_bad_candidates_even_if_solver_claims_optimality() {
    let r = app()
        .solve_with(
            &fixture("regular").to_string(),
            &ClaimedOptimalButIncomplete,
        )
        .unwrap();
    assert_eq!(r.exit_code, 6);
    assert_eq!(r.document["search"]["type"], "OptimalCandidate");
    assert_eq!(r.document["review"]["evaluation"]["feasible"], false);
    assert_eq!(
        r.document["review"]["evaluation"]["hardViolations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(r.document["review"]["decision"]["type"], "Rejected");
}

#[test]
fn solver_limits_do_not_replace_domain_evaluation() {
    let r = app()
        .solve_with(&fixture("overtime").to_string(), &PartialSolver)
        .unwrap();
    assert_eq!(r.exit_code, 4);
    assert_eq!(r.document["review"]["evaluation"]["feasible"], true);
    assert_eq!(r.document["review"]["decision"]["type"], "RequiresApproval");
    let r = app()
        .solve_with(&fixture("regular").to_string(), &InterruptedSolver)
        .unwrap();
    assert_eq!(r.exit_code, 4);
    assert_eq!(r.document["search"]["type"], "NoCandidate");
    assert_eq!(r.document["review"], Value::Null);
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
    for s in [
        Status::ReachedTimeLimit,
        Status::ReachedIterationLimit,
        Status::ReachedSolutionLimit,
        Status::ReachedMemoryLimit,
        Status::ReachedInterrupt,
        Status::ObjectiveTarget,
        Status::ObjectiveBound,
    ] {
        assert!(matches!(
            classify(s, true, 0.5).unwrap(),
            Completion::Feasible(_)
        ));
        assert!(matches!(
            classify(s, false, f64::INFINITY).unwrap(),
            Completion::NoSolution(_)
        ));
    }
    assert!(matches!(
        classify(Status::Optimal, true, 0.01).unwrap(),
        Completion::Feasible(_)
    ));
    for s in [
        Status::UnboundedOrInfeasible,
        Status::Unbounded,
        Status::SolveError,
        Status::Unknown,
        Status::NotSet,
    ] {
        assert!(classify(s, false, f64::INFINITY).is_err());
    }
    assert!(classify(Status::Optimal, false, 0.0).is_err());
    assert!(classify(Status::Optimal, true, f64::NAN).is_err());
}

#[test]
fn numerical_adapter_failures_remain_technical_errors() {
    for v in [0.0, 1e-8, -1e-8] {
        assert!(!binary(v).unwrap());
    }
    for v in [1.0, 1.0 - 1e-8, 1.0 + 1e-8] {
        assert!(binary(v).unwrap());
    }
    for v in [0.4, 0.999, -0.1, 2.0, f64::NAN, f64::INFINITY] {
        assert!(binary(v).is_err());
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
    let lib = library();
    lib.run(|run| {
        let p = AssignmentProblem::decode(run, &fixture("overtime").to_string())
            .unwrap()
            .into_result()
            .unwrap();
        // Six assignments, two overtime columns, two soft slacks, fixed zero.
        let raw = [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 30.0, 0.0, 0.0, 0.0, 0.0];
        let plan = souther_rust_mathopt::optimizer::reconstruct(run, p, &raw, 60.0).unwrap();
        assert_eq!(plan.assignments().len(), 3);
        assert!(souther_rust_mathopt::optimizer::reconstruct(run, p, &raw, 61.0).is_err());
        let mut fractional = raw;
        fractional[6] = 30.5;
        assert!(souther_rust_mathopt::optimizer::reconstruct(run, p, &fractional, 60.5).is_err());
    })
    .unwrap();
}

// An independent enumeration oracle, including qualifications and overtime preference.
fn exhaustive(p: &Value) -> Option<i64> {
    let workers = p["workers"].as_array().unwrap();
    let offers = p["offers"].as_array().unwrap();
    let mut states = vec![(vec![0_i64; workers.len()], 0_i64)];
    for job in p["jobs"].as_array().unwrap() {
        let mut next = Vec::new();
        for (minutes, cost) in &states {
            for o in offers
                .iter()
                .filter(|o| o["jobId"] == job["id"] && o["qualified"] == true)
            {
                let i = workers
                    .iter()
                    .position(|w| w["id"] == o["workerId"])
                    .unwrap();
                let mut m = minutes.clone();
                m[i] += o["minutes"].as_i64().unwrap();
                if m[i]
                    <= workers[i]["regularMinutes"].as_i64().unwrap()
                        + workers[i]["overtimeLimit"].as_i64().unwrap()
                {
                    next.push((m, cost + o["cost"].as_i64().unwrap()));
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
                    let overtime = (minutes[i] - w["regularMinutes"].as_i64().unwrap()).max(0);
                    overtime * w["overtimeRate"].as_i64().unwrap()
                        + (overtime - w["overtimeTarget"].as_i64().unwrap()).max(0)
                            * w["overtimePenalty"].as_i64().unwrap()
                })
                .sum::<i64>()
        })
        .min()
}

#[test]
fn eighty_small_instances_match_exhaustive_optima_and_evaluation() {
    let mut state = 0x5eed_u64;
    let mut next = |bound: u64| {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 32) % bound
    };
    for case in 0..80 {
        let n = 1 + next(3);
        let m = next(6);
        let workers:Vec<_>=(0..n).map(|i|json!({"id":format!("w{i}"),"regularMinutes":next(8),"overtimeLimit":next(5),"overtimeRate":1+next(5),"overtimeTarget":next(5),"overtimePenalty":1+next(5)})).collect();
        let jobs: Vec<_> = (0..m).map(|i| json!({"id":format!("j{i}")})).collect();
        let mut offers = Vec::new();
        for w in &workers {
            for j in &jobs {
                if next(4) > 0 {
                    offers.push(json!({"workerId":w["id"],"jobId":j["id"],"minutes":1+next(6),"cost":next(20),"qualified":next(4)>0}));
                }
            }
        }
        let p = json!({"workers":workers,"jobs":jobs,"offers":offers});
        let actual = solve(&p);
        match exhaustive(&p) {
            Some(value) => {
                assert_eq!(actual.exit_code, 0, "case {case}: {}", actual.document);
                assert_eq!(actual.document["review"]["evaluation"]["feasible"], true);
                assert_eq!(
                    actual.document["review"]["evaluation"]["objectiveValue"], value,
                    "case {case}"
                );
                assert_eq!(
                    actual.document["solver"]["incumbentObjective"],
                    value as f64
                );
            }
            None => assert_eq!(actual.exit_code, 3, "case {case}: {}", actual.document),
        }
    }
}

#[test]
fn real_zero_time_limit_preserves_actual_solver_status() {
    let r = app()
        .solve(
            &fixture("regular").to_string(),
            SolveOptions {
                time_limit_seconds: 0.0,
            },
        )
        .unwrap();
    assert_eq!(r.document["solver"]["timeLimitSeconds"], 0.0);
    match r.document["search"]["type"].as_str() {
        Some("OptimalCandidate") => assert_eq!(r.exit_code, 0),
        Some("NoCandidate" | "StoppedWithCandidate") => assert_eq!(r.exit_code, 4),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn worst_case_valid_but_infeasible_candidate_does_not_overflow() {
    let workers:Vec<_>=(0..16).map(|i|json!({"id":format!("w{i}"),"regularMinutes":0,"overtimeLimit":0,"overtimeRate":10000,"overtimeTarget":0,"overtimePenalty":10000})).collect();
    let jobs: Vec<_> = (0..32).map(|i| json!({"id":format!("j{i}")})).collect();
    let mut offers = Vec::new();
    let mut assignments = Vec::new();
    for w in &workers {
        for j in &jobs {
            offers.push(json!({"workerId":w["id"],"jobId":j["id"],"minutes":1440,"cost":1000000,"qualified":false}));
            assignments.push(json!({"workerId":w["id"],"jobId":j["id"]}));
        }
    }
    let r = review(
        &json!({"workers":workers,"jobs":jobs,"offers":offers}),
        &json!({"assignments":assignments}),
    );
    assert_eq!(r.exit_code, 6);
    assert_eq!(
        r.document["evaluation"]["hardViolations"]
            .as_array()
            .unwrap()
            .len(),
        560
    );
    assert_eq!(r.document["evaluation"]["totalCost"], 7884800000_i64);
    assert_eq!(r.document["evaluation"]["objectiveValue"], 15257600000_i64);
}

#[test]
fn cli_reports_structured_hard_violations_without_technical_errors() {
    for (file, code) in [
        ("regular", 0),
        ("overtime", 0),
        ("infeasible", 3),
        ("invalid", 2),
    ] {
        let r = Command::new(env!("CARGO_BIN_EXE_souther-rust-mathopt"))
            .args(["solve", &format!("examples/{file}.json")])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert_eq!(r.status.code(), Some(code));
        serde_json::from_slice::<Value>(&r.stdout).unwrap();
        assert!(r.stderr.is_empty());
    }
    let r = Command::new(env!("CARGO_BIN_EXE_souther-rust-mathopt"))
        .args([
            "review",
            "examples/diagnostic.json",
            "examples/diagnostic-plan.json",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert_eq!(r.status.code(), Some(6));
    assert!(r.stderr.is_empty());
    let d: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(
        d["evaluation"]["hardViolations"].as_array().unwrap().len(),
        3
    );
}

#[test]
fn cli_distinguishes_execution_limits_from_invalidity_and_infeasibility() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("build/cli-execution-limit.json");
    let mut problem = fixture("regular");
    problem["workers"][0]["overtimeLimit"] = json!(1441);
    std::fs::write(&path, problem.to_string()).unwrap();
    for command in ["solve", "review", "confirm"] {
        let mut cli = Command::new(env!("CARGO_BIN_EXE_souther-rust-mathopt"));
        cli.current_dir(env!("CARGO_MANIFEST_DIR"))
            .arg(command)
            .arg(&path);
        if command != "solve" {
            cli.arg("examples/overtime-plan.json");
        }
        let r = cli.output().unwrap();
        assert_eq!(r.status.code(), Some(7), "{command}: {:?}", r);
        assert!(r.stderr.is_empty());
        let document: Value = serde_json::from_slice(&r.stdout).unwrap();
        assert_eq!(
            document,
            json!({"error":"execution_limit","path":"/workers/0/overtimeLimit","actual":1441,"limit":1440})
        );
    }
    std::fs::remove_file(path).unwrap();
}

use model::HostError;
use souther_rust_mathopt::{App, optimizer::SolveOptions};
use std::{env, fs, process::ExitCode};

fn execute() -> Result<u8, HostError> {
    let args: Vec<String> = env::args().skip(1).collect();
    const USAGE: &str = "usage: souther-rust-mathopt solve PROBLEM.json [--time-limit SECONDS] | review PROBLEM.json PLAN.json | confirm PROBLEM.json PLAN.json [--approve-overtime]";
    if args == ["--help"] {
        println!("{USAGE}");
        return Ok(0);
    }
    let executable = env::current_exe()?;
    let library = executable
        .parent()
        .ok_or("executable has no directory")?
        .join("lib")
        .join(format!(
            "{}souther{}",
            env::consts::DLL_PREFIX,
            env::consts::DLL_SUFFIX
        ));
    // SAFETY: bin/build and bin/package place the library paired with this binding here.
    let app = unsafe { App::load(library) }?;
    let response = match args.as_slice() {
        [command, problem] if command == "solve" => {
            app.solve(&fs::read_to_string(problem)?, SolveOptions::default())?
        }
        [command, problem, flag, seconds] if command == "solve" && flag == "--time-limit" => app
            .solve(
                &fs::read_to_string(problem)?,
                SolveOptions {
                    time_limit_seconds: seconds.parse()?,
                },
            )?,
        [command, problem, candidate] if command == "review" || command == "confirm" => app
            .review(
                &fs::read_to_string(problem)?,
                &fs::read_to_string(candidate)?,
                if command == "confirm" {
                    Some(false)
                } else {
                    None
                },
            )?,
        [command, problem, candidate, flag]
            if command == "confirm" && flag == "--approve-overtime" =>
        {
            app.review(
                &fs::read_to_string(problem)?,
                &fs::read_to_string(candidate)?,
                Some(true),
            )?
        }
        _ => return Err(USAGE.into()),
    };
    println!("{}", serde_json::to_string_pretty(&response.document)?);
    Ok(response.exit_code)
}

fn main() -> ExitCode {
    match execute() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"error": "technical_error", "message": error.to_string()})
            );
            ExitCode::from(1)
        }
    }
}

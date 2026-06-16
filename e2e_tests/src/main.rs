use e2e_tests::{
    config::RunConfig,
    report::CaseOutcome,
    runner::{exit_code, run},
};

#[tokio::main]
async fn main() {
    let config = match RunConfig::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };

    match run(config).await {
        Ok(report) => {
            println!("{}", report.to_markdown());
            std::process::exit(exit_code(report.outcome()));
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(exit_code(CaseOutcome::Failed));
        }
    }
}

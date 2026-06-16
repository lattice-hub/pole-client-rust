use std::{error::Error, fs, path::PathBuf};

use uuid::Uuid;

use crate::{
    cases::{default_cases, select_cases, CaseContext},
    config::RunConfig,
    report::{CaseOutcome, Report},
};

pub async fn run(config: RunConfig) -> Result<Report, Box<dyn Error + Send + Sync>> {
    let registry = default_cases();
    let selected = select_cases(&registry, &config.case_filter)?;
    let run_id = format!("e2e-{}", Uuid::new_v4());
    let mut report = Report::new(
        run_id.clone(),
        config.console_url.clone(),
        selected.iter().map(|case| case.name()).collect::<Vec<_>>(),
    );

    let ctx = CaseContext {
        config: &config,
        run_id: &run_id,
    };
    for case in selected {
        report.push(case.run(&ctx).await);
    }

    write_report_files(&config.report_dir, &run_id, &report)?;
    Ok(report)
}

pub fn write_report_files(
    report_dir: &PathBuf,
    run_id: &str,
    report: &Report,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::create_dir_all(report_dir)?;
    fs::write(report_dir.join(format!("{run_id}.json")), report.to_json()?)?;
    fs::write(
        report_dir.join(format!("{run_id}.md")),
        report.to_markdown(),
    )?;
    fs::write(
        report_dir.join(format!("{run_id}.xml")),
        report.to_junit_xml(),
    )?;
    Ok(())
}

pub fn exit_code(outcome: CaseOutcome) -> i32 {
    match outcome {
        CaseOutcome::Passed | CaseOutcome::Skipped => 0,
        CaseOutcome::Failed => 1,
    }
}

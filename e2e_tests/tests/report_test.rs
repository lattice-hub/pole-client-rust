use std::time::Duration;

use e2e_tests::report::{CaseOutcome, CaseReport, Report};

#[test]
fn serializes_json_markdown_and_junit_report() {
    let mut report = Report::new("run-1", "http://127.0.0.1:8080", vec!["connectivity"]);
    report.push(CaseReport::passed(
        "connectivity",
        Duration::from_millis(10),
        "console and sdk endpoints are reachable",
    ));
    report.push(CaseReport::failed(
        "service-discovery",
        Duration::from_millis(20),
        "consumer did not see registered instance",
    ));
    report.push(CaseReport::skipped(
        "mirror",
        Duration::from_millis(1),
        "case not selected",
    ));

    assert_eq!(report.summary().total, 3);
    assert_eq!(report.summary().passed, 1);
    assert_eq!(report.summary().failed, 1);
    assert_eq!(report.summary().skipped, 1);
    assert_eq!(report.outcome(), CaseOutcome::Failed);

    let json = report.to_json().expect("json report should serialize");
    assert!(json.contains("\"run_id\": \"run-1\""));
    assert!(json.contains("\"name\": \"service-discovery\""));

    let markdown = report.to_markdown();
    assert!(markdown.contains("# pole-client-rust e2e report"));
    assert!(markdown.contains("| service-discovery | failed |"));

    let junit = report.to_junit_xml();
    assert!(junit.contains(
        "<testsuite name=\"pole-client-rust-e2e\" tests=\"3\" failures=\"1\" skipped=\"1\""
    ));
    assert!(junit.contains("<failure message=\"consumer did not see registered instance\""));
    assert!(junit.contains("<skipped message=\"case not selected\""));
}

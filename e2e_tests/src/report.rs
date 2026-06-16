use std::{error::Error, fmt, time::Duration};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseOutcome {
    Passed,
    Failed,
    Skipped,
}

impl fmt::Display for CaseOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaseOutcome::Passed => write!(f, "passed"),
            CaseOutcome::Failed => write!(f, "failed"),
            CaseOutcome::Skipped => write!(f, "skipped"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseReport {
    pub name: String,
    pub outcome: CaseOutcome,
    pub duration_ms: u128,
    pub message: String,
}

impl CaseReport {
    pub fn passed(name: impl Into<String>, duration: Duration, message: impl Into<String>) -> Self {
        Self::new(name, CaseOutcome::Passed, duration, message)
    }

    pub fn failed(name: impl Into<String>, duration: Duration, message: impl Into<String>) -> Self {
        Self::new(name, CaseOutcome::Failed, duration, message)
    }

    pub fn skipped(
        name: impl Into<String>,
        duration: Duration,
        message: impl Into<String>,
    ) -> Self {
        Self::new(name, CaseOutcome::Skipped, duration, message)
    }

    fn new(
        name: impl Into<String>,
        outcome: CaseOutcome,
        duration: Duration,
        message: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            outcome,
            duration_ms: duration.as_millis(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub run_id: String,
    pub console_url: String,
    pub selected_cases: Vec<String>,
    pub cases: Vec<CaseReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl Report {
    pub fn new(
        run_id: impl Into<String>,
        console_url: impl Into<String>,
        selected_cases: Vec<impl Into<String>>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            console_url: console_url.into(),
            selected_cases: selected_cases.into_iter().map(Into::into).collect(),
            cases: Vec::new(),
        }
    }

    pub fn push(&mut self, case: CaseReport) {
        self.cases.push(case);
    }

    pub fn summary(&self) -> ReportSummary {
        let total = self.cases.len();
        let passed = self
            .cases
            .iter()
            .filter(|case| case.outcome == CaseOutcome::Passed)
            .count();
        let failed = self
            .cases
            .iter()
            .filter(|case| case.outcome == CaseOutcome::Failed)
            .count();
        let skipped = self
            .cases
            .iter()
            .filter(|case| case.outcome == CaseOutcome::Skipped)
            .count();

        ReportSummary {
            total,
            passed,
            failed,
            skipped,
        }
    }

    pub fn outcome(&self) -> CaseOutcome {
        if self
            .cases
            .iter()
            .any(|case| case.outcome == CaseOutcome::Failed)
        {
            CaseOutcome::Failed
        } else if self
            .cases
            .iter()
            .all(|case| case.outcome == CaseOutcome::Skipped)
        {
            CaseOutcome::Skipped
        } else {
            CaseOutcome::Passed
        }
    }

    pub fn to_json(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn to_markdown(&self) -> String {
        let summary = self.summary();
        let mut out = String::new();
        out.push_str("# pole-client-rust e2e report\n\n");
        out.push_str(&format!("- run_id: `{}`\n", self.run_id));
        out.push_str(&format!("- console_url: `{}`\n", self.console_url));
        out.push_str(&format!(
            "- summary: total={}, passed={}, failed={}, skipped={}\n\n",
            summary.total, summary.passed, summary.failed, summary.skipped
        ));
        out.push_str("| case | outcome | duration_ms | message |\n");
        out.push_str("| --- | --- | ---: | --- |\n");
        for case in &self.cases {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                case.name,
                case.outcome,
                case.duration_ms,
                case.message.replace('|', "\\|")
            ));
        }
        out
    }

    pub fn to_junit_xml(&self) -> String {
        let summary = self.summary();
        let mut out = String::new();
        out.push_str(&format!(
            "<testsuite name=\"pole-client-rust-e2e\" tests=\"{}\" failures=\"{}\" skipped=\"{}\">\n",
            summary.total, summary.failed, summary.skipped
        ));
        for case in &self.cases {
            out.push_str(&format!(
                "  <testcase name=\"{}\" time=\"{}\">",
                escape_xml(&case.name),
                (case.duration_ms as f64) / 1000.0
            ));
            match case.outcome {
                CaseOutcome::Passed => {}
                CaseOutcome::Failed => out.push_str(&format!(
                    "<failure message=\"{}\" />",
                    escape_xml(&case.message)
                )),
                CaseOutcome::Skipped => out.push_str(&format!(
                    "<skipped message=\"{}\" />",
                    escape_xml(&case.message)
                )),
            }
            out.push_str("</testcase>\n");
        }
        out.push_str("</testsuite>\n");
        out
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

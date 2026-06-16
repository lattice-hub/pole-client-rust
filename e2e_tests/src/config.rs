use std::{collections::HashMap, error::Error, fmt, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    pub console_url: String,
    pub client_addr: Option<String>,
    pub discover_addr: Option<String>,
    pub config_addr: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub case_filter: Vec<String>,
    pub report_dir: PathBuf,
    pub skip_cleanup: bool,
    pub execute_governance_control_plane: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunConfigError {
    MissingRequiredInput { field: &'static str },
    MissingValue { flag: String },
    UnknownFlag { flag: String },
}

impl fmt::Display for RunConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunConfigError::MissingRequiredInput { field } => {
                write!(f, "missing required input: {field}")
            }
            RunConfigError::MissingValue { flag } => write!(f, "missing value for {flag}"),
            RunConfigError::UnknownFlag { flag } => write!(f, "unknown flag: {flag}"),
        }
    }
}

impl Error for RunConfigError {}

impl RunConfig {
    pub fn from_env() -> Result<Self, RunConfigError> {
        Self::from_sources(std::env::args(), std::env::vars().collect())
    }

    pub fn from_sources<I, S>(args: I, env: HashMap<String, String>) -> Result<Self, RunConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cfg = RunConfig {
            console_url: env.get("POLE_E2E_CONSOLE_URL").cloned().unwrap_or_default(),
            client_addr: env.get("POLE_E2E_CLIENT_ADDR").cloned(),
            discover_addr: env.get("POLE_E2E_DISCOVER_ADDR").cloned(),
            config_addr: env.get("POLE_E2E_CONFIG_ADDR").cloned(),
            user: env.get("POLE_E2E_USER").cloned(),
            password: env.get("POLE_E2E_PASSWORD").cloned(),
            token: env.get("POLE_E2E_TOKEN").cloned(),
            case_filter: split_csv(env.get("POLE_E2E_CASES")),
            report_dir: env
                .get("POLE_E2E_REPORT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("target/e2e-reports")),
            skip_cleanup: env
                .get("POLE_E2E_SKIP_CLEANUP")
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            execute_governance_control_plane: env
                .get("POLE_E2E_EXECUTE_GOVERNANCE_CONTROL_PLANE")
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        };

        let mut iter = args.into_iter();
        let _program = iter.next();
        while let Some(arg) = iter.next() {
            let arg = arg.as_ref().to_string();
            match arg.as_str() {
                "--console-url" => cfg.console_url = next_value(&mut iter, &arg)?,
                "--client-addr" => cfg.client_addr = Some(next_value(&mut iter, &arg)?),
                "--discover-addr" => cfg.discover_addr = Some(next_value(&mut iter, &arg)?),
                "--config-addr" => cfg.config_addr = Some(next_value(&mut iter, &arg)?),
                "--user" => cfg.user = Some(next_value(&mut iter, &arg)?),
                "--password" => cfg.password = Some(next_value(&mut iter, &arg)?),
                "--token" => cfg.token = Some(next_value(&mut iter, &arg)?),
                "--case" => cfg.case_filter.push(next_value(&mut iter, &arg)?),
                "--report-dir" => cfg.report_dir = PathBuf::from(next_value(&mut iter, &arg)?),
                "--skip-cleanup" => cfg.skip_cleanup = true,
                "--execute-governance-control-plane" => cfg.execute_governance_control_plane = true,
                "--help" | "-h" => return Err(RunConfigError::UnknownFlag { flag: arg }),
                _ => return Err(RunConfigError::UnknownFlag { flag: arg }),
            }
        }

        cfg.console_url = trim_trailing_slashes(&cfg.console_url);
        if cfg.console_url.is_empty()
            || (cfg.client_addr.is_none()
                && (cfg.discover_addr.is_none() || cfg.config_addr.is_none()))
        {
            return Err(RunConfigError::MissingRequiredInput {
                field: "console-url/client-addr",
            });
        }

        Ok(cfg)
    }

    pub fn sdk_addresses(&self) -> Vec<String> {
        let discover = self
            .discover_addr
            .as_deref()
            .or(self.client_addr.as_deref())
            .expect("RunConfig is validated before sdk_addresses");
        let config = self
            .config_addr
            .as_deref()
            .or(self.client_addr.as_deref())
            .expect("RunConfig is validated before sdk_addresses");

        vec![
            connector_addr("discover", discover),
            connector_addr("config", config),
        ]
    }
}

fn next_value<I, S>(iter: &mut I, flag: &str) -> Result<String, RunConfigError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    iter.next()
        .map(|value| value.as_ref().to_string())
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| RunConfigError::MissingValue {
            flag: flag.to_string(),
        })
}

fn split_csv(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn trim_trailing_slashes(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn connector_addr(kind: &str, addr: &str) -> String {
    if addr.starts_with("discover://") || addr.starts_with("config://") {
        return addr.to_string();
    }
    format!("{kind}://{addr}")
}

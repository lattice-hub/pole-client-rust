use std::{error::Error, fmt, time::Duration};

#[derive(Debug)]
pub enum ConnectivityError {
    BuildClient(reqwest::Error),
    Request(reqwest::Error),
    HttpStatus(u16),
}

impl fmt::Display for ConnectivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectivityError::BuildClient(err) => write!(f, "failed to build HTTP client: {err}"),
            ConnectivityError::Request(err) => write!(f, "console probe request failed: {err}"),
            ConnectivityError::HttpStatus(status) => {
                write!(f, "console probe failed with HTTP {status}")
            }
        }
    }
}

impl Error for ConnectivityError {}

pub async fn probe_console(
    console_url: &str,
    timeout: Duration,
) -> Result<String, ConnectivityError> {
    let endpoint = format!(
        "{}/admin/v1/server/functions",
        console_url.trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(ConnectivityError::BuildClient)?;
    let response = client
        .get(&endpoint)
        .send()
        .await
        .map_err(ConnectivityError::Request)?;
    if !response.status().is_success() {
        return Err(ConnectivityError::HttpStatus(response.status().as_u16()));
    }

    Ok(format!("console endpoint reachable: {endpoint}"))
}

use std::{error::Error, fmt, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone)]
pub struct ConsoleClient {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleClientError {
    Build(String),
    Request(String),
    HttpStatus { status: u16, body: String },
    Decode(String),
}

impl fmt::Display for ConsoleClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsoleClientError::Build(err) => write!(f, "failed to build console client: {err}"),
            ConsoleClientError::Request(err) => write!(f, "console request failed: {err}"),
            ConsoleClientError::HttpStatus { status, body } => {
                write!(f, "console HTTP {status}: {body}")
            }
            ConsoleClientError::Decode(err) => write!(f, "console response decode failed: {err}"),
        }
    }
}

impl Error for ConsoleClientError {}

#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleResponse {
    pub status: u16,
    pub body: Value,
}

impl ConsoleClient {
    pub fn new(
        base_url: impl Into<String>,
        token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, ConsoleClientError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| ConsoleClientError::Build(err.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            client,
        })
    }

    pub async fn get_json(&self, path: &str) -> Result<ConsoleResponse, ConsoleClientError> {
        self.send_json(reqwest::Method::GET, path, None).await
    }

    pub async fn post_json(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<ConsoleResponse, ConsoleClientError> {
        self.send_json(reqwest::Method::POST, path, Some(body))
            .await
    }

    pub async fn execute_action(
        &self,
        action: &ControlPlaneAction,
    ) -> Result<ConsoleResponse, ConsoleClientError> {
        let method = reqwest::Method::from_bytes(action.method.as_bytes())
            .map_err(|err| ConsoleClientError::Build(err.to_string()))?;
        self.send_json(method, &action.path, Some(&action.body))
            .await
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<ConsoleResponse, ConsoleClientError> {
        let url = format!("{}{}", self.base_url, normalize_path(path));
        let mut request = self.client.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|err| ConsoleClientError::Request(err.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|err| ConsoleClientError::Request(err.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(ConsoleClientError::HttpStatus { status, body: text });
        }
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)
                .map_err(|err| ConsoleClientError::Decode(err.to_string()))?
        };

        Ok(ConsoleResponse { status, body })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneAction {
    pub method: String,
    pub path: String,
    pub body: Value,
}

impl ControlPlaneAction {
    pub fn post(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: "POST".to_string(),
            path: path.into(),
            body,
        }
    }

    pub fn delete(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: "POST".to_string(),
            path: path.into(),
            body,
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct CleanupPlan {
    actions: Vec<ControlPlaneAction>,
}

impl CleanupPlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, action: ControlPlaneAction) {
        self.actions.push(action);
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn drain_lifo(&mut self) -> Vec<ControlPlaneAction> {
        self.actions.drain(..).rev().collect()
    }
}

fn normalize_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

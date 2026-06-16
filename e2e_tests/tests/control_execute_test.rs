use std::{sync::Arc, time::Duration};

use e2e_tests::{
    cases::{CaseKind, E2eCase},
    console::ConsoleClient,
    control_plan::control_plane_plan,
    control_plan::{
        execute_control_plane_cleanup, execute_control_plane_plan, execute_control_plane_setup,
        ControlPlaneExecutionStatus,
    },
    flows::FlowResource,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

#[tokio::test]
async fn execute_plan_runs_cleanup_when_publish_fails() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = requests.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_requests.lock().await.push(path.clone());
            if path == "/naming/v1/routings/releases" {
                write_response(
                    &mut stream,
                    500,
                    br#"{"code":500000,"info":"publish failed"}"#,
                )
                .await;
            } else {
                write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
            }
        }
    });

    let client = ConsoleClient::new(format!("http://{addr}"), None, Duration::from_secs(1))
        .expect("client should build");
    let plan = control_plane_plan(
        E2eCase::new("routing", CaseKind::Routing),
        &FlowResource::new("run-1", "routing"),
    )
    .unwrap();

    let execution = execute_control_plane_plan(&client, &plan).await;

    assert_eq!(execution.status, ControlPlaneExecutionStatus::Failed);
    assert_eq!(execution.steps.len(), 4);
    assert_eq!(execution.steps[0].name, "create");
    assert!(execution.steps[0].success);
    assert_eq!(execution.steps[1].name, "publish");
    assert!(!execution.steps[1].success);
    assert_eq!(execution.steps[2].name, "cleanup");
    assert_eq!(execution.steps[3].name, "cleanup");
    assert!(execution.steps[2].success);
    assert!(execution.steps[3].success);
    assert!(execution.message().contains("publish failed"));

    server.await.unwrap();
    assert_eq!(
        *requests.lock().await,
        vec![
            "/naming/v1/routings",
            "/naming/v1/routings/releases",
            "/naming/v1/routings/releases/delete",
            "/naming/v1/routings/delete",
        ]
    );
}

#[tokio::test]
async fn setup_keeps_rule_published_until_explicit_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = requests.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_requests.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let client = ConsoleClient::new(format!("http://{addr}"), None, Duration::from_secs(1))
        .expect("client should build");
    let plan = control_plane_plan(
        E2eCase::new("mock", CaseKind::Mock),
        &FlowResource::new("run-1", "mock"),
    )
    .unwrap();

    let setup = execute_control_plane_setup(&client, &plan).await;
    assert_eq!(setup.status, ControlPlaneExecutionStatus::Passed);
    assert_eq!(
        *requests.lock().await,
        vec![
            "/naming/v1/traffic/mocks",
            "/naming/v1/traffic/mocks/releases"
        ]
    );

    let cleanup = execute_control_plane_cleanup(&client, &plan).await;
    assert_eq!(cleanup.status, ControlPlaneExecutionStatus::Passed);
    assert_eq!(
        *requests.lock().await,
        vec![
            "/naming/v1/traffic/mocks",
            "/naming/v1/traffic/mocks/releases",
            "/naming/v1/traffic/mocks/releases/delete",
            "/naming/v1/traffic/mocks/delete",
        ]
    );

    server.await.unwrap();
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if String::from_utf8_lossy(&raw).contains("\r\n\r\n") {
            break;
        }
    }
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|idx| idx + 4)
        .unwrap_or(raw.len());
    let headers = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while raw.len().saturating_sub(header_end) < content_length {
        let n = stream.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&raw).to_string()
}

async fn write_response(stream: &mut tokio::net::TcpStream, status: u16, body: &[u8]) {
    let status_text = if status == 200 {
        "OK"
    } else {
        "Internal Server Error"
    };
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status} {status_text}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(body).await.unwrap();
}

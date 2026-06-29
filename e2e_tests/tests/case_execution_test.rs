use std::sync::Arc;

use e2e_tests::{
    cases::{default_cases, CaseContext},
    config::RunConfig,
    report::CaseOutcome,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

fn config() -> RunConfig {
    RunConfig::from_sources(
        [
            "e2e_tests",
            "--console-url",
            "http://127.0.0.1:8080",
            "--client-addr",
            "127.0.0.1:8091",
        ],
        Default::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn governance_case_with_plan_fails_until_behavior_assertions_exist() {
    let cfg = config();
    let lossless = default_cases()
        .into_iter()
        .find(|case| case.name() == "lossless")
        .unwrap();

    let report = lossless
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(report
        .message
        .contains("control-plane plan exists but behavioral assertions are not implemented"));
    assert!(report.message.contains("/naming/v1/lossless"));
}

#[tokio::test]
async fn lossless_case_executes_behavior_assertion_before_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let server_paths = paths.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_request(&mut stream).await;
            let path = req
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_paths.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let mut cfg = config();
    cfg.console_url = format!("http://{addr}");
    cfg.execute_governance_control_plane = true;
    let lossless = default_cases()
        .into_iter()
        .find(|case| case.name() == "lossless")
        .unwrap();

    let report = lossless
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(
        report
            .message
            .contains("lossless behavior assertion failed"),
        "{}",
        report.message
    );
    assert!(!report
        .message
        .contains("behavioral assertions are not implemented"));

    server.await.unwrap();
    assert_eq!(
        *paths.lock().await,
        vec![
            "/naming/v1/lossless",
            "/naming/v1/lossless/releases",
            "/naming/v1/lossless/releases/delete",
            "/naming/v1/lossless/delete",
        ]
    );
}

#[tokio::test]
async fn fault_detect_case_executes_behavior_assertion_before_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let server_paths = paths.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_request(&mut stream).await;
            let path = req
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_paths.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let mut cfg = config();
    cfg.console_url = format!("http://{addr}");
    cfg.execute_governance_control_plane = true;
    let case = default_cases()
        .into_iter()
        .find(|case| case.name() == "fault-detect")
        .unwrap();

    let report = case
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(report
        .message
        .contains("fault-detect behavior assertion failed"));
    assert!(!report
        .message
        .contains("behavioral assertions are not implemented"));

    server.await.unwrap();
    assert_eq!(
        *paths.lock().await,
        vec![
            "/naming/v1/faultdetectors",
            "/naming/v1/faultdetectors/releases",
            "/naming/v1/faultdetectors/releases/delete",
            "/naming/v1/faultdetectors/delete",
        ]
    );
}

#[tokio::test]
async fn mock_case_executes_behavior_assertion_before_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let server_paths = paths.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_request(&mut stream).await;
            let path = req
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_paths.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let mut cfg = config();
    cfg.console_url = format!("http://{addr}");
    cfg.execute_governance_control_plane = true;
    let mock = default_cases()
        .into_iter()
        .find(|case| case.name() == "mock")
        .unwrap();

    let report = mock
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(report.message.contains("mock behavior assertion failed"));
    assert!(!report
        .message
        .contains("behavioral assertions are not implemented"));

    server.await.unwrap();
    assert_eq!(
        *paths.lock().await,
        vec![
            "/naming/v1/traffic/mocks",
            "/naming/v1/traffic/mocks/releases",
            "/naming/v1/traffic/mocks/releases/delete",
            "/naming/v1/traffic/mocks/delete",
        ]
    );
}

#[tokio::test]
async fn routing_case_executes_behavior_assertion_before_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let server_paths = paths.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_request(&mut stream).await;
            let path = req
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_paths.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let mut cfg = config();
    cfg.console_url = format!("http://{addr}");
    cfg.execute_governance_control_plane = true;
    let routing = default_cases()
        .into_iter()
        .find(|case| case.name() == "routing")
        .unwrap();

    let report = routing
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(
        report.message.contains("routing behavior assertion failed"),
        "{}",
        report.message
    );
    assert!(!report
        .message
        .contains("behavioral assertions are not implemented"));

    server.await.unwrap();
}

#[tokio::test]
async fn circuitbreaker_case_executes_behavior_assertion_before_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let server_paths = paths.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_request(&mut stream).await;
            let path = req
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_paths.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let mut cfg = config();
    cfg.console_url = format!("http://{addr}");
    cfg.execute_governance_control_plane = true;
    let case = default_cases()
        .into_iter()
        .find(|case| case.name() == "circuitbreaker")
        .unwrap();

    let report = case
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(
        report
            .message
            .contains("circuitbreaker behavior assertion failed"),
        "{}",
        report.message
    );
    assert!(!report
        .message
        .contains("behavioral assertions are not implemented"));

    server.await.unwrap();
    assert_eq!(
        *paths.lock().await,
        vec![
            "/naming/v1/circuitbreakers",
            "/naming/v1/circuitbreakers/releases",
            "/naming/v1/circuitbreakers/releases/delete",
            "/naming/v1/circuitbreakers/delete",
        ]
    );
}

#[tokio::test]
async fn mirror_case_executes_behavior_assertion_before_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let server_paths = paths.clone();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_request(&mut stream).await;
            let path = req
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            server_paths.lock().await.push(path);
            write_response(&mut stream, 200, br#"{"code":200000,"info":"ok"}"#).await;
        }
    });

    let mut cfg = config();
    cfg.console_url = format!("http://{addr}");
    cfg.execute_governance_control_plane = true;
    let case = default_cases()
        .into_iter()
        .find(|case| case.name() == "mirror")
        .unwrap();

    let report = case
        .run(&CaseContext {
            config: &cfg,
            run_id: "run-1",
        })
        .await;

    assert_eq!(report.outcome, CaseOutcome::Failed);
    assert!(report.message.contains("mirror behavior assertion failed"));
    assert!(!report
        .message
        .contains("behavioral assertions are not implemented"));

    server.await.unwrap();
    assert_eq!(
        *paths.lock().await,
        vec![
            "/naming/v1/traffic/mirrors",
            "/naming/v1/traffic/mirrors/releases",
            "/naming/v1/traffic/mirrors/releases/delete",
            "/naming/v1/traffic/mirrors/delete",
        ]
    );
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
                "HTTP/1.1 {status} {status_text}\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(body).await.unwrap();
}

use std::time::Duration;

use e2e_tests::console::{CleanupPlan, ConsoleClient, ControlPlaneAction};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn console_client_posts_json_with_control_plane_token_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&raw).contains(r#"{"name":"svc-a"}"#) {
                break;
            }
        }
        let request = String::from_utf8_lossy(&raw).to_string();
        let body = br#"{"code":200000,"info":"ok"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(body).await.unwrap();
        request
    });

    let client = ConsoleClient::new(
        format!("http://{addr}/"),
        Some("token-1".to_string()),
        Duration::from_secs(1),
    )
    .expect("client should build");
    let resp = client
        .post_json("/naming/v1/services", &json!({"name":"svc-a"}))
        .await
        .expect("request should pass");

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body["code"], json!(200000));

    let request = server.await.unwrap();
    let request_lower = request.to_ascii_lowercase();
    assert!(request.starts_with("POST /naming/v1/services HTTP/1.1"));
    assert!(request_lower.contains("authorization: token-1"));
    assert!(request_lower.contains("x-polaris-token: token-1"));
    assert!(request_lower.contains("polaris-token: token-1"));
    assert!(request_lower.contains("content-type: application/json"));
    assert!(request.contains(r#"{"name":"svc-a"}"#));
}

#[tokio::test]
async fn console_client_reports_non_success_http_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 4\r\n\r\nfail")
            .await
            .unwrap();
    });

    let client = ConsoleClient::new(format!("http://{addr}"), None, Duration::from_secs(1))
        .expect("client should build");
    let err = client
        .get_json("/admin/v1/server/functions")
        .await
        .unwrap_err();

    assert!(err.to_string().contains("console HTTP 500"));
    server.await.unwrap();
}

#[test]
fn cleanup_plan_drains_actions_in_lifo_order() {
    let mut plan = CleanupPlan::new();
    plan.push(ControlPlaneAction::delete(
        "/naming/v1/instances",
        json!({"id":"ins-1"}),
    ));
    plan.push(ControlPlaneAction::delete(
        "/naming/v1/services",
        json!({"name":"svc-1"}),
    ));

    let drained = plan.drain_lifo();

    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].path, "/naming/v1/services");
    assert_eq!(drained[1].path, "/naming/v1/instances");
    assert!(plan.is_empty());
}

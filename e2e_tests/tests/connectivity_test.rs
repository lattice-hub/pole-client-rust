use std::time::Duration;

use e2e_tests::connectivity::probe_console;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn console_probe_accepts_successful_admin_endpoint_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf).await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-length: 29\r\n\r\n{\"code\":200000,\"data\":[]}",
            )
            .await
            .unwrap();
    });

    let outcome = probe_console(&format!("http://{addr}"), Duration::from_secs(1))
        .await
        .expect("console probe should pass");

    assert!(outcome.contains("/admin/v1/server/functions"));
    server.await.unwrap();
}

#[tokio::test]
async fn console_probe_reports_non_success_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n")
            .await
            .unwrap();
    });

    let err = probe_console(&format!("http://{addr}"), Duration::from_secs(1))
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("console probe failed with HTTP 503"));
    server.await.unwrap();
}

use super::{get_json, validate_health, HTTP_BODY_LIMIT, HTTP_HEADER_LIMIT};
use crate::ApiError;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const TOKEN: &str = "artificial-health-probe-token";
const INSTANCE: &str = "artificial-health-probe-instance";

fn health() -> Value {
    json!({"protocolVersion": 1, "instanceId": INSTANCE, "status": "ok"})
}

// The fixture owns just one loopback listener and socket. Every server task is
// bounded and joined, including probes which reject a response or time out.
async fn probe_response(parts: Vec<Vec<u8>>, keep_open: bool) -> (Result<Value, ApiError>, String) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(6), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.set_nodelay(true).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            for part in parts {
                if socket.write_all(&part).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            if keep_open {
                // A completed Content-Length/chunked response must not wait for
                // EOF. The client should finish the probe and close its socket.
                let mut byte = [0];
                let count = socket.read(&mut byte).await.unwrap();
                assert_eq!(count, 0, "the probe left its connection open");
            }
            String::from_utf8(request).unwrap()
        })
        .await
        .expect("artificial health server did not finish")
    });
    let result = get_json(port, TOKEN, "/_platform/health").await;
    let request = server.await.unwrap();
    (result, request)
}

fn fixed_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

#[tokio::test]
async fn content_length_health_finishes_without_eof_and_authenticates_request() {
    let expected = health();
    let response = fixed_response(expected.to_string().as_bytes());
    let (result, request) = probe_response(vec![response], true).await;
    assert_eq!(result.unwrap(), expected);
    let request = request.to_ascii_lowercase();
    assert!(request.starts_with("get /_platform/health http/1.1\r\n"));
    assert!(request.contains(&format!("\r\nauthorization: bearer {TOKEN}\r\n")));
    assert!(request.contains("\r\nhost: 127.0.0.1:"));
}

#[tokio::test]
async fn chunked_health_accepts_fragments_extensions_trailers_and_informational_response() {
    let expected = health();
    let body = expected.to_string();
    let mut response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n".to_vec();
    for chunk in body.as_bytes().chunks(7) {
        response.extend_from_slice(format!("{:x};fixture=yes\r\n", chunk.len()).as_bytes());
        response.extend_from_slice(chunk);
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"0\r\nX-Health-Diagnostic: artificial\r\n\r\n");
    let parts = response.chunks(3).map(<[u8]>::to_vec).collect();
    let (result, _) = probe_response(parts, true).await;
    assert_eq!(result.unwrap(), expected);
}

#[tokio::test]
async fn http_10_close_delimited_json_remains_supported() {
    let expected = health();
    let response = format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{expected}");
    let (result, _) = probe_response(vec![response.into_bytes()], false).await;
    assert_eq!(result.unwrap(), expected);
}

#[tokio::test]
async fn malformed_or_truncated_http_bodies_and_invalid_json_are_rejected() {
    for response in [
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nz\r\n{}\r\n0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{}",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{}",
        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nnope",
    ] {
        let (result, _) = probe_response(vec![response.as_bytes().to_vec()], false).await;
        assert_eq!(result.unwrap_err().code, "TRIAL_HTTP_INVALID", "{response}");
    }
}

#[tokio::test]
async fn decoded_json_body_limit_applies_to_fixed_and_chunked_responses() {
    let body = vec![b' '; HTTP_BODY_LIMIT + 1];
    let mut chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    chunked.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
    chunked.extend_from_slice(&body);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    for response in [fixed_response(&body), chunked] {
        let (result, _) = probe_response(vec![response], false).await;
        assert_eq!(result.unwrap_err().code, "TRIAL_HTTP_LIMIT");
    }
    // The full body limit is usable, independent of the framing byte count.
    let body = format!("{}{{}}", " ".repeat(HTTP_BODY_LIMIT - 2));
    let (result, _) = probe_response(vec![fixed_response(body.as_bytes())], false).await;
    assert_eq!(result.unwrap(), json!({}));
}

#[tokio::test]
async fn response_headers_have_a_separate_bounded_budget() {
    let response = format!(
        "HTTP/1.1 200 OK\r\nX-Artificial: {}\r\nContent-Length: 2\r\n\r\n{{}}",
        "x".repeat(HTTP_HEADER_LIMIT + 1024)
    );
    let (result, _) = probe_response(vec![response.into_bytes()], false).await;
    assert!(result.is_err(), "oversized response headers were accepted");
}

#[tokio::test]
async fn non_success_statuses_are_reported_and_redirects_are_not_followed() {
    for status in [204, 302, 401, 500] {
        let response = format!(
            "HTTP/1.1 {status} Artificial\r\nLocation: http://127.0.0.1:1/never-follow\r\nContent-Length: 0\r\n\r\n"
        );
        let (result, _) = probe_response(vec![response.into_bytes()], false).await;
        let error = result.unwrap_err();
        assert_eq!(error.code, "TRIAL_HTTP_STATUS");
        assert_eq!(error.details["httpStatus"], status);
    }
}

#[tokio::test]
async fn stalled_response_times_out_and_releases_its_socket() {
    let (result, _) = probe_response(Vec::new(), true).await;
    assert_eq!(result.unwrap_err().code, "TRIAL_HTTP_TIMEOUT");
}

#[tokio::test]
async fn invalid_probe_paths_cannot_redirect_the_authenticated_request() {
    for path in [
        "http://example.invalid/health".to_owned(),
        "//example.invalid/health".to_owned(),
        "/health\r\nHost: example.invalid".to_owned(),
        format!("/{}", "x".repeat(512)),
    ] {
        assert_eq!(
            get_json(1, TOKEN, &path).await.unwrap_err().code,
            "TRIAL_HTTP_INVALID"
        );
    }
    assert_eq!(
        get_json(1, "invalid\r\nheader", "/_platform/health")
            .await
            .unwrap_err()
            .code,
        "TRIAL_HTTP_INVALID"
    );
}

#[test]
fn health_accepts_metadata_but_strictly_validates_identity_version_and_status() {
    let mut extended = health();
    extended["uptime"] = json!(12);
    extended["version"] = json!("artificial-app-version");
    validate_health(&extended, INSTANCE).unwrap();
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"protocolVersion": "1", "instanceId": INSTANCE, "status": "ok"}),
        json!({"protocolVersion": 1.0, "instanceId": INSTANCE, "status": "ok"}),
        json!({"protocolVersion": 2, "instanceId": INSTANCE, "status": "ok"}),
        json!({"protocolVersion": 1, "instanceId": "another-instance", "status": "ok"}),
        json!({"protocolVersion": 1, "instanceId": INSTANCE, "status": "starting"}),
        json!({"protocolVersion": 1, "instanceId": INSTANCE, "status": true}),
    ] {
        assert_eq!(
            validate_health(&value, INSTANCE).unwrap_err().code,
            "TRIAL_HEALTH_INVALID",
            "{value}"
        );
    }
}

//! Bounded HTTP/1 probes of the current authenticated loopback listener.
//! Drive the connection in this future so cancellation closes its socket too.

use super::{fail, TrialResult, HEALTH_TIMEOUT};
use axum::{
    body::Bytes,
    http::{header, Request, StatusCode},
};
use http_body_util::{BodyExt, Empty, LengthLimitError, Limited};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::net::TcpStream;

const HTTP_HEADER_LIMIT: usize = 16 * 1024;
const HTTP_BODY_LIMIT: usize = 16 * 1024;

pub(super) fn validate_health(value: &Value, instance_id: &str) -> TrialResult<()> {
    // Extra health metadata is harmless. The three protocol fields remain exact.
    if value.get("protocolVersion").and_then(Value::as_u64) != Some(1)
        || value.get("instanceId").and_then(Value::as_str) != Some(instance_id)
        || value.get("status").and_then(Value::as_str) != Some("ok")
    {
        return Err(fail(
            "TRIAL_HEALTH_INVALID",
            "Application HTTP health identity did not match",
        ));
    }
    Ok(())
}

pub(super) async fn get_json(port: u16, token: &str, path: &str) -> TrialResult<Value> {
    get_json_with_timeout(port, token, path, HEALTH_TIMEOUT).await
}

async fn get_json_with_timeout(
    port: u16,
    token: &str,
    path: &str,
    timeout: Duration,
) -> TrialResult<Value> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.len() > 512
        || path.bytes().any(|b| b <= 32 || b >= 127)
    {
        return Err(fail("TRIAL_HTTP_INVALID", "Invalid application probe path"));
    }
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header(header::HOST, format!("127.0.0.1:{port}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "application/json")
        .header(header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| fail("TRIAL_HTTP_INVALID", "Invalid application probe request"))?;

    tokio::time::timeout(timeout, async {
        // No environment proxy, DNS, redirect follower or connection pool. The
        // authentication stays on this one owned loopback connection.
        let socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| fail("TRIAL_HTTP_FAILED", "Application HTTP listener is unavailable"))?;
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_buf_size(HTTP_HEADER_LIMIT)
            .handshake(TokioIo::new(socket))
            .await
            .map_err(|_| fail("TRIAL_HTTP_FAILED", "Application HTTP connection failed"))?;
        let exchange = async {
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| fail("TRIAL_HTTP_INVALID", "Invalid application HTTP response"))?;
            if response.status() != StatusCode::OK {
                return Err(fail(
                    "TRIAL_HTTP_STATUS",
                    "Application health check did not return HTTP 200",
                )
                .detail("httpStatus", json!(response.status().as_u16())));
            }
            // Bound decoded bytes, including chunked bodies. Content-Length is
            // not trusted as a substitute for actually limiting the body.
            let body = Limited::new(response.into_body(), HTTP_BODY_LIMIT)
                .collect()
                .await
                .map_err(|error| {
                    if error.is::<LengthLimitError>() {
                        fail("TRIAL_HTTP_LIMIT", "Application HTTP body exceeded its budget")
                    } else {
                        fail("TRIAL_HTTP_INVALID", "Incomplete or invalid application HTTP body")
                    }
                })?
                .to_bytes();
            serde_json::from_slice(&body)
                .map_err(|_| fail("TRIAL_HTTP_INVALID", "Invalid application HTTP JSON"))
        };
        tokio::pin!(connection, exchange);
        tokio::select! {
            biased;
            result = &mut exchange => result,
            result = &mut connection => {
                result.map_err(|_| fail("TRIAL_HTTP_INVALID", "Invalid application HTTP connection"))?;
                // A close-delimited response can finish the driver first; all
                // its body data is then available to the pending exchange.
                exchange.await
            }
        }
        // Finishing the response never waits for peer EOF. Both futures and the
        // socket are dropped here, or synchronously when the timeout cancels us.
    })
    .await
    .unwrap_or_else(|_| Err(fail("TRIAL_HTTP_TIMEOUT", "Application HTTP probe timed out")))
}

#[cfg(test)]
#[path = "node_runtime_http_tests.rs"]
mod tests;

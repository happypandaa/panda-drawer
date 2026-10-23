//! API v1 boundary checks shared by platform hosts. No sockets or persistence.
use crate::ApiError;
use http::{header, HeaderMap};
use serde::Serialize;
use serde_json::{json, Value};

pub fn host(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    let values = headers.get_all(header::HOST);
    if values.iter().count() != 1
        || values.iter().next().and_then(|v| v.to_str().ok()) != Some(expected)
    {
        return Err(ApiError::new(
            "INVALID_HOST",
            "Use the discovered loopback address",
        ));
    }
    Ok(())
}

pub fn origin(headers: &HeaderMap, expected: &str, required: bool) -> Result<(), ApiError> {
    let values = headers.get_all(header::ORIGIN);
    let count = values.iter().count();
    if (required && count == 0)
        || count > 1
        || (count == 1 && values.iter().next().and_then(|v| v.to_str().ok()) != Some(expected))
    {
        return Err(ApiError::new(
            "INVALID_ORIGIN",
            "Request must originate from this platform instance",
        ));
    }
    Ok(())
}

pub fn cookie_name(id: &str) -> String {
    format!("lp_{id}")
}

pub fn session_cookie<'a>(headers: &'a HeaderMap, id: &str) -> Result<&'a str, ApiError> {
    let name = cookie_name(id);
    let mut found = None;
    for line in headers.get_all(header::COOKIE) {
        let line = line
            .to_str()
            .map_err(|_| ApiError::new("INVALID_SESSION", "Invalid project cookie"))?;
        for pair in line.split(';') {
            if let Some((key, value)) = pair.trim().split_once('=') {
                if key == name && found.replace(value).is_some() {
                    return Err(ApiError::new(
                        "INVALID_SESSION",
                        "Provide one project session",
                    ));
                }
            }
        }
    }
    found.filter(|v| !v.is_empty()).ok_or_else(|| {
        ApiError::new(
            "INVALID_SESSION",
            "Open this project from the platform to connect",
        )
    })
}

pub fn epoch<'a>(headers: &'a HeaderMap, query: Option<&'a str>) -> Result<&'a str, ApiError> {
    let values = headers.get_all("x-platform-document-epoch");
    if values.iter().count() > 1 {
        return Err(ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Provide one document epoch",
        ));
    }
    let value = values.iter().next().and_then(|v| v.to_str().ok());
    if value.is_some() && query.is_some() && value != query {
        return Err(ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Conflicting document epochs",
        ));
    }
    value.or(query).filter(|v| !v.is_empty()).ok_or_else(|| {
        ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Reload this project to establish its document epoch",
        )
    })
}

pub fn envelope<T: Serialize>(
    result: Result<T, ApiError>,
    project_id: Option<&str>,
    request_id: &str,
) -> Value {
    match result {
        Ok(data) => {
            json!({"apiVersion":1,"ok":true,"requestId":request_id,"projectId":project_id,"data":data})
        }
        Err(error) => {
            json!({"apiVersion":1,"ok":false,"requestId":request_id,"projectId":project_id,"error":error})
        }
    }
}

pub fn status(error: &ApiError) -> u16 {
    match error.code.as_str() {
        "UNAUTHENTICATED" | "INVALID_SESSION" | "INVALID_TICKET" => 401,
        "INVALID_HOST" | "INVALID_ORIGIN" | "BROWSER_MANAGEMENT_DENIED" | "FILE_ACCESS_DENIED" => {
            403
        }
        "PROJECT_NOT_FOUND" | "FILE_NOT_FOUND" | "NOT_FOUND" | "TRASH_NOT_FOUND" => 404,
        "FILE_EXISTS"
        | "METADATA_CHANGED"
        | "STORAGE_CURSOR_INVALID"
        | "PROJECT_RELOAD_REQUIRED"
        | "PROJECT_ID_CONFLICT"
        | "PROJECT_TRASHED"
        | "PROJECT_DELETED"
        | "PROJECT_DELETE_INCOMPLETE"
        | "PROJECT_DELETE_CONFIRMATION_CHANGED"
        | "PROJECT_REGISTRATION_PENDING"
        | "WORKSPACE_MIGRATING"
        | "SERVICE_INSTANCE_CHANGED"
        | "PROJECT_NOT_BUILT"
        | "DATABASE_MISSING"
        | "PROJECT_CHANGED"
        | "PROJECT_BUSY"
        | "PROJECT_MAINTENANCE"
        | "MAINTENANCE_ID_MISMATCH"
        | "RECOVERY_REQUIRED"
        | "BUILD_REQUIRED" => 409,
        "SERVICE_BUSY" | "SERVICE_STOPPING" | "SQL_QUEUE_FULL" | "SQL_BUSY" => 503,
        "REQUEST_TOO_LARGE" | "FILE_TOO_LARGE" => 413,
        "FILE_IO_ERROR" | "INTERNAL_ERROR" => 500,
        _ => 400,
    }
}

pub fn policy_fingerprint(requirements: &Value) -> String {
    use sha2::Digest;
    format!(
        "{:x}",
        sha2::Sha256::digest(serde_json::to_vec(requirements).unwrap_or_default())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_reject_ambiguous_or_foreign_identity() {
        let mut headers = HeaderMap::new();
        assert!(host(&headers, "127.0.0.1:1234").is_err());
        headers.insert(header::HOST, "127.0.0.1:1234".parse().unwrap());
        assert!(host(&headers, "127.0.0.1:1234").is_ok());
        headers.append(header::HOST, "localhost:1234".parse().unwrap());
        assert!(host(&headers, "127.0.0.1:1234").is_err());
        assert!(origin(&headers, "http://127.0.0.1:1234", true).is_err());
        headers.insert(header::ORIGIN, "http://127.0.0.1:1234".parse().unwrap());
        assert!(origin(&headers, "http://127.0.0.1:1234", true).is_ok());
        headers.append(header::ORIGIN, "null".parse().unwrap());
        assert!(origin(&headers, "http://127.0.0.1:1234", false).is_err());
        headers.insert(header::COOKIE, "lp_prj_a=token".parse().unwrap());
        assert_eq!(session_cookie(&headers, "prj_a").unwrap(), "token");
        headers.append(header::COOKIE, "lp_prj_a=other".parse().unwrap());
        assert!(session_cookie(&headers, "prj_a").is_err());
        assert!(epoch(&headers, None).is_err());
        headers.insert("x-platform-document-epoch", "one".parse().unwrap());
        assert_eq!(epoch(&headers, Some("one")).unwrap(), "one");
        assert!(epoch(&headers, Some("two")).is_err());
        headers.append("x-platform-document-epoch", "one".parse().unwrap());
        assert!(epoch(&headers, None).is_err());
    }
}

//! An instance router has no management routes and never accepts an app id.
use super::*;
use axum::{http::HeaderValue, routing::get};
#[derive(Clone)]
struct Bound {
    state: Arc<HttpState>,
    id: String,
}
pub(super) fn router(state: Arc<HttpState>, id: String) -> Router {
    Router::new()
        .route("/_platform/bootstrap", get(bootstrap))
        .route("/_platform/sdk.js", get(sdk))
        .route("/_platform/config.json", get(config))
        .route("/_platform/api/v1/{*tail}", any(api))
        .route("/_platform", any(reserved))
        .route("/_platform/{*tail}", any(reserved))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            interrupt_body,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), validate_host))
        .with_state(Bound { state, id })
}
async fn reserved() -> Response {
    failure(
        ApiError::new("NOT_FOUND", "Unknown application platform route"),
        None,
    )
}
async fn bootstrap(State(bound): State<Bound>, request: Request) -> Response {
    let outcome = (|| -> std::result::Result<String, ApiError> {
        origin(&bound.state, request.headers(), false)?;
        let parameters = query(&request)?;
        if parameters.len() != 1 {
            return Err(ApiError::new("INVALID_TICKET", "请从平台重新打开应用。"));
        }
        let token = parameters
            .get("token")
            .ok_or_else(|| ApiError::new("INVALID_TICKET", "缺少应用入口凭据。"))?;
        let ticket = bound
            .state
            .tickets
            .lock()
            .unwrap()
            .remove(token)
            .filter(|ticket| ticket.project_id == bound.id && ticket.expires > Instant::now())
            .ok_or_else(|| {
                ApiError::new(
                    "INVALID_TICKET",
                    "入口已过期或已使用，请从平台重新打开应用。",
                )
            })?;
        ticket.project.validate_epoch(Some(&ticket.epoch))?;
        let session_token = secret();
        bound
            .state
            .store
            .with_registered_epoch(&bound.id, &ticket.epoch, || {
                let mut sessions = bound.state.sessions.lock().unwrap();
                if sessions.len() >= 4096 {
                    return Err(ApiError::new(
                        "SERVICE_BUSY",
                        "应用会话过多，请关闭后重新打开。",
                    ));
                }
                sessions.insert(
                    session_token.clone(),
                    Session {
                        project: ticket.project,
                        project_id: bound.id.clone(),
                        document_epoch: ticket.epoch.clone(),
                    },
                );
                Ok(())
            })?
            .ok_or_else(|| ApiError::new("INVALID_TICKET", "应用登记已经变化，请重新打开。"))?;
        Ok(session_token)
    })();
    match outcome {
        Err(issue) => failure(issue, Some(&bound.id)),
        Ok(token) => {
            let mut response = StatusCode::SEE_OTHER.into_response();
            response
                .headers_mut()
                .insert(header::LOCATION, HeaderValue::from_static("/"));
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{}={token}; Path=/; HttpOnly; SameSite=Strict",
                    cookie_name(&bound.id)
                ))
                .unwrap(),
            );
            response
        }
    }
}
async fn sdk(State(bound): State<Bound>, headers: HeaderMap) -> Response {
    let data = (|| -> std::result::Result<Vec<u8>, ApiError> {
        session(&bound.state, &bound.id, &headers, false)?;
        bound
            .state
            .resources
            .as_ref()
            .ok_or_else(|| {
                ApiError::new(
                    "INSTALLATION_RESOURCES_MISSING",
                    "平台 SDK 资源不存在，请修复安装。",
                )
            })?
            .sdk_script()
    })();
    match data {
        Ok(bytes) => (
            [(
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            bytes,
        )
            .into_response(),
        Err(issue) => failure(issue, Some(&bound.id)),
    }
}
async fn config(State(bound): State<Bound>, headers: HeaderMap) -> Response {
    let data = (|| -> std::result::Result<_, ApiError> {
        let accepted = session(&bound.state, &bound.id, &headers, false)?;
        bound.state.store.web_configuration_values(&accepted)
    })();
    match data {
        Ok(values) => Json(values).into_response(),
        Err(issue) => failure(issue, Some(&bound.id)),
    }
}
async fn api(State(bound): State<Bound>, Path(tail): Path<String>, request: Request) -> Response {
    project_api(State(bound.state), Path((bound.id, tail)), request).await
}
async fn static_file(State(bound): State<Bound>, request: Request) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return failure(
            ApiError::new("NOT_FOUND", "Unknown application method"),
            Some(&bound.id),
        );
    }
    let path = request.uri().path();
    let decoded = match decode_path(path) {
        Ok(path) => path,
        Err(issue) => return failure(issue, Some(&bound.id)),
    };
    let relative = decoded.trim_matches('/');
    if relative == "_platform" || relative.starts_with("_platform/") {
        return reserved().await;
    }
    let project = match session(&bound.state, &bound.id, request.headers(), false) {
        Ok(project) => project,
        Err(issue) => return failure(issue, Some(&bound.id)),
    };
    let selected = if relative.is_empty() {
        None
    } else {
        match crate::web_files::lookup(&project.root, &project.manifest, relative) {
            Ok(crate::web_files::Lookup::Found(file)) => {
                drop(file);
                Some(relative.to_owned())
            }
            Ok(crate::web_files::Lookup::Missing) if page_request(&request, relative) => None,
            Ok(crate::web_files::Lookup::Missing) => {
                return failure(
                    ApiError::new("FILE_NOT_FOUND", "没有找到这个网页文件。"),
                    Some(&bound.id),
                )
            }
            Err(issue) => return failure(issue, Some(&bound.id)),
        }
    };
    let head = *request.method() == Method::HEAD;
    let mut response =
        serve_project(bound.state, bound.id, selected, request.headers().clone()).await;
    if head {
        *response.body_mut() = Body::empty();
    }
    response
}
fn page_request(request: &Request, path: &str) -> bool {
    std::path::Path::new(path).extension().is_none()
        && request
            .headers()
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().split(';').next() == Some("text/html"))
            })
        && request
            .headers()
            .get("sec-fetch-dest")
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| value == "document")
}

fn decode_path(path: &str) -> std::result::Result<String, ApiError> {
    let invalid = || ApiError::new("FILE_NOT_FOUND", "Invalid application path");
    let mut result = Vec::with_capacity(path.len());
    let mut bytes = path.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = (bytes.next().ok_or_else(invalid)? as char)
                .to_digit(16)
                .ok_or_else(invalid)?;
            let low = (bytes.next().ok_or_else(invalid)? as char)
                .to_digit(16)
                .ok_or_else(invalid)?;
            result.push(((high << 4) | low) as u8);
        } else {
            result.push(byte);
        }
    }
    String::from_utf8(result).map_err(|_| invalid())
}

async fn interrupt_body(
    State(state): State<Arc<HttpState>>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let stream = stream::unfold(
        (body.into_data_stream(), state.stop.subscribe(), false),
        |(mut body, mut stop, ended)| async move {
            if ended {
                return None;
            }
            if *stop.borrow() {
                return Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Application service stopped",
                    )),
                    (body, stop, true),
                ));
            }
            tokio::select! {
                biased;
                _ = stop.changed() => Some((Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Application service stopped")), (body, stop, true))),
                chunk = body.next() => chunk.map(|chunk| (chunk.map_err(std::io::Error::other), (body, stop, false))),
            }
        },
    );
    // Only the unread request body is interruptible. An accepted SQL operation
    // retains its actual future and database admission through normal finish.
    next.run(Request::from_parts(parts, Body::from_stream(stream)))
        .await
}

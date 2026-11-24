//! Local workspace access. Credentials are exchanged for bounded, memory-only sessions.
//! The application's outer origin/host guard and loopback binding remain mandatory.
use crate::error::{Error, Result};
use axum::{
    Json,
    extract::{Request, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

const COOKIE_NAME: &str = "relic_session";
const SESSION_SECONDS: u64 = 12 * 60 * 60;
const SESSION_LIMIT: usize = 32;
const TOKEN_BYTE_LIMIT: usize = 8192;

#[derive(Clone)]
pub struct Access(Arc<AccessState>);

// Deliberately no Debug/Serialize implementations for credentials or session state.
struct AccessState {
    token_hash: Option<[u8; 32]>,
    sessions: Mutex<Vec<Session>>,
}

struct Session {
    hash: [u8; 32],
    expires: Instant,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    token: String,
}

#[derive(Serialize)]
pub struct SessionStatus {
    authenticated: bool,
    mode: &'static str,
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn equal_digest(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn unauthorized() -> Error {
    Error {
        status: StatusCode::UNAUTHORIZED,
        code: "unauthorized",
        message: "Workspace authentication is required.".into(),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Accept only one well-formed session cookie, even when Cookie spans header fields.
fn cookie(headers: &HeaderMap) -> Option<&str> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for pair in header.to_str().ok()?.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == COOKIE_NAME {
                if found.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

impl Access {
    pub fn new(token: Option<String>) -> Self {
        let token_hash = token
            .filter(|value| !value.is_empty())
            .map(|value| digest(&value));
        Self(Arc::new(AccessState {
            token_hash,
            sessions: Mutex::new(Vec::with_capacity(SESSION_LIMIT)),
        }))
    }

    fn status(&self, headers: &HeaderMap) -> Result<SessionStatus> {
        if self.0.token_hash.is_none() {
            return Ok(SessionStatus {
                authenticated: true,
                mode: "local",
            });
        }
        let supplied = cookie(headers).map(digest);
        let mut sessions = self.0.sessions.lock().map_err(|_| Error::internal())?;
        sessions.retain(|session| session.expires > Instant::now());
        let authenticated = supplied.is_some_and(|hash| {
            sessions
                .iter()
                .any(|session| equal_digest(&session.hash, &hash))
        });
        Ok(SessionStatus {
            authenticated,
            mode: "token",
        })
    }

    fn establish(&self, headers: &HeaderMap, token: &str) -> Result<Option<String>> {
        let Some(expected) = &self.0.token_hash else {
            return Ok(None);
        };
        if token.len() > TOKEN_BYTE_LIMIT || !equal_digest(expected, &digest(token)) {
            return Err(unauthorized());
        }
        let replaced = cookie(headers).map(digest);
        let mut sessions = self.0.sessions.lock().map_err(|_| Error::internal())?;
        let now = Instant::now();
        sessions.retain(|session| {
            session.expires > now
                && !replaced.is_some_and(|hash| equal_digest(&session.hash, &hash))
        });
        if sessions.len() >= SESSION_LIMIT {
            return Err(Error {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "session_capacity",
                message: "The workspace has 32 active sessions. Sign out of an existing session or wait for expiry.".into(),
            });
        }
        // Two independent UUID v4 values supply a high-entropy, cookie-safe identifier.
        let identifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        sessions.push(Session {
            hash: digest(&identifier),
            expires: now + Duration::from_secs(SESSION_SECONDS),
        });
        Ok(Some(identifier))
    }

    fn revoke(&self, headers: &HeaderMap) -> Result<()> {
        let revoked = cookie(headers).map(digest);
        let mut sessions = self.0.sessions.lock().map_err(|_| Error::internal())?;
        let now = Instant::now();
        sessions.retain(|session| {
            session.expires > now && !revoked.is_some_and(|hash| equal_digest(&session.hash, &hash))
        });
        Ok(())
    }
}

/// Apply to protected API routes with `from_fn_with_state(access, require_access)`.
/// Leave session status/login/logout routes outside this middleware.
pub async fn require_access(
    State(access): State<Access>,
    request: Request,
    next: Next,
) -> Response {
    match access.status(request.headers()) {
        Ok(status) if status.authenticated => no_store(next.run(request).await),
        Ok(_) => no_store(unauthorized().into_response()),
        Err(error) => no_store(error.into_response()),
    }
}

pub async fn session_status(State(access): State<Access>, headers: HeaderMap) -> Response {
    match access.status(&headers) {
        Ok(status) => no_store(Json(status).into_response()),
        Err(error) => no_store(error.into_response()),
    }
}

pub async fn login(
    State(access): State<Access>,
    headers: HeaderMap,
    input: std::result::Result<Json<LoginRequest>, JsonRejection>,
) -> Response {
    let result = (|| {
        let Json(input) = input.map_err(|_| {
            Error::invalid("Expected a JSON object containing the workspace token.")
        })?;
        let identifier = access.establish(&headers, &input.token)?;
        let mut response = Json(SessionStatus {
            authenticated: true,
            mode: if identifier.is_some() {
                "token"
            } else {
                "local"
            },
        })
        .into_response();
        if let Some(identifier) = identifier {
            // This release binds HTTP to loopback. Secure is reserved for an HTTPS deployment.
            let value = format!(
                "{COOKIE_NAME}={identifier}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_SECONDS}"
            );
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&value).map_err(|_| Error::internal())?,
            );
        }
        Ok::<_, Error>(response)
    })();
    no_store(match result {
        Ok(response) => response,
        Err(error) => error.into_response(),
    })
}

pub async fn logout(State(access): State<Access>, headers: HeaderMap) -> Response {
    let result = access.revoke(&headers);
    let mut response = match result {
        Ok(()) => Json(SessionStatus {
            authenticated: access.0.token_hash.is_none(),
            mode: if access.0.token_hash.is_none() {
                "local"
            } else {
                "token"
            },
        })
        .into_response(),
        Err(error) => error.into_response(),
    };
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("relic_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
    );
    no_store(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        middleware,
        routing::{get, post},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    fn router(access: Access) -> Router {
        let protected = Router::new()
            .route("/private", get(|| async { "saved evidence" }))
            .route_layer(middleware::from_fn_with_state(
                access.clone(),
                require_access,
            ));
        Router::new()
            .route("/session", get(session_status))
            .route("/login", post(login))
            .route("/logout", post(logout))
            .with_state(access)
            .merge(protected)
    }

    fn headers(identifier: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("relic_session={identifier}")).unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn login_protects_data_and_logout_revokes_the_cookie() {
        let access = Access::new(Some("synthetic-workspace-token".into()));
        let app = router(access);
        let response = app
            .clone()
            .oneshot(Request::get("/private").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = app
            .clone()
            .oneshot(
                Request::post("/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"token":"synthetic-workspace-token"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(set_cookie.contains("HttpOnly; SameSite=Strict; Path=/; Max-Age=43200"));
        assert!(!set_cookie.contains("synthetic-workspace-token"));
        let value: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(value, json!({"authenticated":true,"mode":"token"}));
        let cookie = set_cookie.split(';').next().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::get("/private")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(
                Request::post("/logout")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        let response = app
            .oneshot(
                Request::get("/private")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_configuration_is_explicit_local_mode() {
        let app = router(Access::new(None));
        let response = app
            .clone()
            .oneshot(Request::get("/session").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body, json!({"authenticated":true,"mode":"local"}));
        let response = app
            .oneshot(Request::get("/private").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_login_is_structured_and_never_echoes_credentials() {
        let app = router(Access::new(Some("correct-test-only-token".into())));
        for input in [
            r#"{"token":"wrong-secret-value"}"#,
            r#"{"password":"wrong-secret-value"}"#,
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/login")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(input))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(response.status().is_client_error());
            assert!(response.headers().get(header::SET_COOKIE).is_none());
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(!body.contains("wrong-secret-value"));
            assert!(serde_json::from_str::<Value>(&body).unwrap()["error"]["code"].is_string());
        }
    }

    #[test]
    fn sessions_expire_and_capacity_is_bounded_without_evicting_live_work() {
        let access = Access::new(Some("test-token".into()));
        let first = access
            .establish(&HeaderMap::new(), "test-token")
            .unwrap()
            .unwrap();
        for _ in 1..SESSION_LIMIT {
            access.establish(&HeaderMap::new(), "test-token").unwrap();
        }
        assert_eq!(
            access
                .establish(&HeaderMap::new(), "test-token")
                .unwrap_err()
                .status,
            StatusCode::TOO_MANY_REQUESTS
        );
        let replaced = access
            .establish(&headers(&first), "test-token")
            .unwrap()
            .unwrap();
        assert!(!access.status(&headers(&first)).unwrap().authenticated);
        assert!(access.status(&headers(&replaced)).unwrap().authenticated);
        assert_eq!(access.0.sessions.lock().unwrap().len(), SESSION_LIMIT);
        for session in access.0.sessions.lock().unwrap().iter_mut() {
            session.expires = Instant::now() - Duration::from_secs(1);
        }
        assert!(!access.status(&headers(&replaced)).unwrap().authenticated);
        assert!(access.0.sessions.lock().unwrap().is_empty());
        assert!(
            access
                .establish(&HeaderMap::new(), "test-token")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn malformed_ambiguous_and_unknown_cookies_fail_closed() {
        let access = Access::new(Some("test-token".into()));
        let id = access
            .establish(&HeaderMap::new(), "test-token")
            .unwrap()
            .unwrap();
        let mut duplicate = headers(&id);
        duplicate.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("relic_session={id}")).unwrap(),
        );
        assert!(!access.status(&duplicate).unwrap().authenticated);
        assert!(!access.status(&headers("short")).unwrap().authenticated);
        assert!(
            !access
                .status(&headers(&"0".repeat(64)))
                .unwrap()
                .authenticated
        );
        assert!(access.status(&headers(&id)).unwrap().authenticated);
        assert!(
            access
                .establish(&HeaderMap::new(), &"x".repeat(TOKEN_BYTE_LIMIT + 1))
                .is_err()
        );
    }
}

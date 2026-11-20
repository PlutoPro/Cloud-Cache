use std::{sync::atomic::Ordering, time::Duration};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    auth::AuthError,
    identity::{Identity, OrgError},
    store::valid_key,
    AppState,
};

pub fn router(state: AppState) -> Router {
    let limit = state.max_value_bytes;
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/me", get(me))
        .route("/v1/kv", get(list_kv))
        .route("/v1/kv/{key}", get(get_kv).put(put_kv).delete(del_kv))
        .route("/v1/admin/usage", get(all_usage))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(state)
}

/// Header naming the organization to act on (id or alias). Only needed when the
/// caller belongs to more than one.
pub const ORG_HEADER: &str = "x-org-id";

pub enum ApiError {
    Unauthorized(String),
    Forbidden,
    Org(OrgError),
    NotFound,
    BadRequest(&'static str),
    Unavailable(String),
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::Jwks(m) => ApiError::Unavailable(m),
            other => ApiError::Unauthorized(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::Unauthorized(m) => {
                tracing::debug!(reason = %m, "rejected token");
                let body = Json(json!({ "error": "unauthorized" }));
                return (
                    StatusCode::UNAUTHORIZED,
                    [(header::WWW_AUTHENTICATE, "Bearer")],
                    body,
                )
                    .into_response();
            }
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            ApiError::Org(e @ OrgError::Ambiguous) => (
                StatusCode::BAD_REQUEST,
                format!("{e}: set the X-Org-Id header"),
            ),
            ApiError::Org(e) => (StatusCode::FORBIDDEN, e.to_string()),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.to_string()),
            ApiError::Unavailable(m) => {
                tracing::warn!(error = %m, "auth backend unavailable");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "auth unavailable".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

/// Extractor: a verified Keycloak identity from `Authorization: Bearer <token>`.
pub struct Authed(pub Identity);

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let raw = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthorized(
                "missing Authorization header".into(),
            ))?;
        let token = raw
            .strip_prefix("Bearer ")
            .or_else(|| raw.strip_prefix("bearer "))
            .ok_or(ApiError::Unauthorized("not a bearer token".into()))?;
        Ok(Authed(state.verifier.verify(token).await?))
    }
}

/// Liveness: the process is up. LBs/orchestrators restart on failure.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness: should the LB send traffic here? False until signing keys load, and during drain.
async fn readyz(State(st): State<AppState>) -> Response {
    let draining = !st.ready.load(Ordering::Relaxed);
    let keys = st.verifier.has_keys().await;
    let ok = !draining && keys;
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "ready": ok,
            "draining": draining,
            "jwks_loaded": keys,
            "keys": st.store.len(),
        })),
    )
        .into_response()
}

fn check_key(key: &str) -> Result<(), ApiError> {
    if valid_key(key) {
        Ok(())
    } else {
        Err(ApiError::BadRequest("invalid key"))
    }
}

/// Resolve the org this request acts on.
fn org_of<'a>(id: &'a Identity, headers: &HeaderMap) -> Result<&'a str, ApiError> {
    let requested = headers.get(ORG_HEADER).and_then(|v| v.to_str().ok());
    id.org(requested)
        .map(|o| o.id.as_str())
        .map_err(ApiError::Org)
}

async fn get_kv(
    State(st): State<AppState>,
    Authed(id): Authed,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Response, ApiError> {
    if !id.perms.read {
        return Err(ApiError::Forbidden);
    }
    let org = org_of(&id, &headers)?;
    check_key(&key)?;
    let v = st.store.get(org, &key).ok_or(ApiError::NotFound)?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], v).into_response())
}

#[derive(Deserialize)]
struct PutParams {
    /// Optional time-to-live in seconds.
    ttl: Option<u64>,
}

async fn put_kv(
    State(st): State<AppState>,
    Authed(id): Authed,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(p): Query<PutParams>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    if !id.perms.write {
        return Err(ApiError::Forbidden);
    }
    let org = org_of(&id, &headers)?;
    check_key(&key)?;
    if p.ttl == Some(0) {
        return Err(ApiError::BadRequest("ttl must be > 0"));
    }
    tracing::debug!(user = %id.principal, %org, %key, "http set");
    st.store
        .set(org, &key, body, p.ttl.map(Duration::from_secs));
    Ok(StatusCode::NO_CONTENT)
}

async fn del_kv(
    State(st): State<AppState>,
    Authed(id): Authed,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !id.perms.write {
        return Err(ApiError::Forbidden);
    }
    let org = org_of(&id, &headers)?;
    check_key(&key)?;
    if st.store.del(org, &key) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Who am I: identity, permissions and organizations (with admin flags). Any valid token.
async fn me(Authed(id): Authed) -> Json<serde_json::Value> {
    Json(json!({
        "user": id.principal,
        "sub": id.sub,
        "permissions": { "read": id.perms.read, "write": id.perms.write, "usage": id.perms.usage },
        "organizations": id.orgs.iter()
            .map(|o| json!({ "id": o.id, "alias": o.alias, "admin": o.admin }))
            .collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct ListParams {
    #[serde(default)]
    prefix: String,
    /// Cursor: the `next` value from the previous page.
    after: Option<String>,
    limit: Option<usize>,
}

/// List keys in the caller's org (this node only; see README on multi-node listing).
async fn list_kv(
    State(st): State<AppState>,
    Authed(id): Authed,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Result<Json<crate::store::KeyPage>, ApiError> {
    if !id.perms.read {
        return Err(ApiError::Forbidden);
    }
    let org = org_of(&id, &headers)?;
    Ok(Json(st.store.list(
        org,
        &p.prefix,
        p.after.as_deref(),
        p.limit.unwrap_or(100),
    )))
}

#[derive(Deserialize)]
struct UsageParams {
    org: Option<String>,
}

/// Per-org usage on THIS node (role `kv-usage`), e.g. for the dashboard or a metrics collector.
/// `reads`/`writes` only increase since `started_at`; a changed `started_at` means the node
/// restarted and its counters began again from zero.
async fn all_usage(
    State(st): State<AppState>,
    Authed(id): Authed,
    Query(p): Query<UsageParams>,
) -> Result<Response, ApiError> {
    if !id.perms.usage {
        return Err(ApiError::Forbidden);
    }
    let orgs = match p.org {
        Some(org) => vec![st.store.usage(&org)],
        None => st.store.all_usage(),
    };
    Ok(Json(json!({
        "node": &*st.node_id,
        "started_at": st.started_at,
        "orgs": orgs,
    }))
    .into_response())
}

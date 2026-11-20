//! OIDC access-token verification: signature (against the provider's JWKS), issuer, audience,
//! expiry. Provider-neutral. Turning claims into roles and orgs is `identity.rs`'s job.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::{
    decode, decode_header,
    jwk::{JwkSet, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::{Mutex, RwLock};

use crate::{
    config::Config,
    identity::{ClaimMapper, Identity},
};

/// Asymmetric algorithms only. HS* and `none` are rejected outright, which closes
/// the classic "sign with the public key as an HMAC secret" confusion attack.
const ALLOWED_ALGS: &[Algorithm] = &[
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
];

/// Minimum gap between JWKS fetches triggered by an unknown `kid`,
/// so a flood of garbage tokens can't hammer the identity provider.
const MIN_REFETCH: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing bearer token")]
    Missing,
    #[error("invalid token: {0}")]
    Invalid(String),
    #[error("token signed with an unknown key")]
    UnknownKey,
    #[error("could not fetch signing keys: {0}")]
    Jwks(String),
}

/// The registered claims we check ourselves; everything else goes to the mapper.
#[derive(Deserialize)]
struct Registered {
    exp: u64,
    #[serde(default)]
    sub: Option<String>,
}

pub struct Verifier {
    issuer: String,
    audience: Option<String>,
    /// Set explicitly, or discovered from `{issuer}/.well-known/openid-configuration`.
    jwks_url: RwLock<Option<String>>,
    discovery_url: String,
    mapper: Arc<dyn ClaimMapper>,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
    last_fetch: Mutex<Option<Instant>>,
}

impl Verifier {
    pub fn new(cfg: &Config, mapper: Arc<dyn ClaimMapper>) -> anyhow::Result<Self> {
        Ok(Self {
            issuer: cfg.issuer.clone(),
            audience: cfg.audience.clone(),
            jwks_url: RwLock::new(cfg.jwks_url.clone()),
            discovery_url: cfg.discovery_url.clone(),
            mapper,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?,
            keys: RwLock::new(HashMap::new()),
            last_fetch: Mutex::new(None),
        })
    }

    /// True once at least one signing key is loaded. Used by /readyz.
    pub async fn has_keys(&self) -> bool {
        !self.keys.read().await.is_empty()
    }

    /// Where keys are fetched from (for logs).
    pub async fn jwks_url(&self) -> String {
        self.jwks_url
            .read()
            .await
            .clone()
            .unwrap_or_else(|| format!("(discover via {})", self.discovery_url))
    }

    async fn resolve_jwks_url(&self) -> Result<String, AuthError> {
        if let Some(u) = self.jwks_url.read().await.clone() {
            return Ok(u);
        }
        #[derive(Deserialize)]
        struct Discovery {
            issuer: String,
            jwks_uri: String,
        }
        let d: Discovery = self
            .http
            .get(&self.discovery_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| AuthError::Jwks(format!("discovery: {e}")))?
            .json()
            .await
            .map_err(|e| AuthError::Jwks(format!("discovery: {e}")))?;
        if d.issuer != self.issuer {
            tracing::warn!(
                expected = %self.issuer, discovered = %d.issuer,
                "discovery document's issuer differs from OIDC_ISSUER; tokens are checked against OIDC_ISSUER"
            );
        }
        *self.jwks_url.write().await = Some(d.jwks_uri.clone());
        Ok(d.jwks_uri)
    }

    /// Fetch the JWKS. `force` bypasses the refetch rate limit (used by the periodic refresher).
    pub async fn refresh(&self, force: bool) -> Result<usize, AuthError> {
        let mut last = self.last_fetch.lock().await;
        if !force {
            if let Some(t) = *last {
                if t.elapsed() < MIN_REFETCH {
                    return Ok(self.keys.read().await.len());
                }
            }
        }
        *last = Some(Instant::now());

        let url = self.resolve_jwks_url().await?;
        let set: JwkSet = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| AuthError::Jwks(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Jwks(e.to_string()))?;

        let mut map = HashMap::new();
        for jwk in &set.keys {
            // Some providers (Keycloak) also publish encryption keys; never accept those for signatures.
            if matches!(jwk.common.public_key_use, Some(PublicKeyUse::Encryption)) {
                continue;
            }
            let Some(kid) = jwk.common.key_id.clone() else {
                continue;
            };
            match DecodingKey::from_jwk(jwk) {
                Ok(k) => {
                    map.insert(kid, k);
                }
                Err(e) => tracing::warn!(%kid, error = %e, "skipping unusable JWK"),
            }
        }
        if map.is_empty() {
            return Err(AuthError::Jwks(
                "JWKS contained no usable signing keys".into(),
            ));
        }
        let n = map.len();
        *self.keys.write().await = map;
        Ok(n)
    }

    async fn key_for(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(k) = self.keys.read().await.get(kid) {
            return Ok(k.clone());
        }
        // Possibly a key rotation at the provider: refetch (rate-limited) and retry once.
        self.refresh(false).await?;
        self.keys
            .read()
            .await
            .get(kid)
            .cloned()
            .ok_or(AuthError::UnknownKey)
    }

    pub async fn verify(&self, token: &str) -> Result<Identity, AuthError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AuthError::Missing);
        }
        let header = decode_header(token).map_err(|e| AuthError::Invalid(e.to_string()))?;
        if !ALLOWED_ALGS.contains(&header.alg) {
            return Err(AuthError::Invalid(format!(
                "algorithm {:?} not allowed",
                header.alg
            )));
        }
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Invalid("token has no kid".into()))?;
        let key = self.key_for(&kid).await?;

        let mut v = Validation::new(header.alg);
        v.leeway = 30;
        // Exact match, trailing slash included (Auth0 issuers end in `/`).
        v.set_issuer(&[&self.issuer]);
        v.set_required_spec_claims(&["exp", "iss"]);
        match &self.audience {
            Some(aud) => v.set_audience(&[aud]),
            None => v.validate_aud = false,
        }

        let claims: Map<String, Value> = decode(token, &key, &v)
            .map_err(|e| AuthError::Invalid(e.to_string()))?
            .claims;
        let reg: Registered = serde_json::from_value(Value::Object(claims.clone()))
            .map_err(|e| AuthError::Invalid(e.to_string()))?;
        let mapped = self.mapper.map(&claims).map_err(|e| {
            tracing::warn!(error = %e, "rejecting token: claims don't fit the configured mapping");
            AuthError::Invalid(e)
        })?;
        Ok(Identity {
            principal: mapped.principal,
            sub: reg.sub,
            exp: reg.exp,
            perms: mapped.perms,
            orgs: mapped.orgs,
        })
    }
}

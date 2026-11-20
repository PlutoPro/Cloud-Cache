use std::{env, net::SocketAddr, time::Duration};

use anyhow::Context;

use crate::identity::{mapping_from_env, MappingConfig};

#[derive(Clone, Debug)]
pub struct Config {
    /// HTTP API listen address (also serves /healthz and /readyz for the load balancer).
    pub http_addr: SocketAddr,
    /// TCP line-protocol listen address.
    pub tcp_addr: SocketAddr,
    /// Expected `iss` claim, exactly as the provider writes it (Auth0's ends in `/`).
    pub issuer: String,
    /// Signing keys URL. Unset = read `jwks_uri` from the discovery document.
    pub jwks_url: Option<String>,
    /// OIDC discovery document. Defaults to `{issuer}/.well-known/openid-configuration`;
    /// set it when this service reaches the provider on an internal hostname.
    pub discovery_url: String,
    /// Expected `aud` claim (this API's identifier at the provider). Unset = not checked.
    pub audience: Option<String>,
    /// How claims become roles and orgs (`AUTH_PRESET` + overrides; see identity.rs).
    pub mapping: MappingConfig,
    /// Name this node reports in usage snapshots, so callers can tell nodes apart.
    pub node_id: String,
    /// How long to report not-ready before closing listeners on SIGTERM, so the LB drains us first.
    pub drain: Duration,
    pub max_value_bytes: usize,
    pub tcp_idle_timeout: Duration,
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn parse<T: std::str::FromStr>(name: &str, default: T) -> anyhow::Result<T>
where
    T::Err: std::fmt::Display,
{
    match var(name) {
        Some(v) => v
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid {name}={v}: {e}")),
        None => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let issuer = var("OIDC_ISSUER")
            .context("OIDC_ISSUER is required (e.g. https://sso.example.com/realms/kv)")?;
        let discovery_url = var("OIDC_DISCOVERY_URL").unwrap_or_else(|| {
            format!(
                "{}/.well-known/openid-configuration",
                issuer.trim_end_matches('/')
            )
        });
        let audience = var("OIDC_AUDIENCE");
        let mapping = mapping_from_env(var, audience.clone()).map_err(anyhow::Error::msg)?;
        Ok(Self {
            http_addr: parse("HTTP_ADDR", "0.0.0.0:8080".parse()?)?,
            tcp_addr: parse("TCP_ADDR", "0.0.0.0:7379".parse()?)?,
            issuer,
            jwks_url: var("OIDC_JWKS_URL"),
            discovery_url,
            audience,
            mapping,
            node_id: var("NODE_ID")
                .or_else(|| var("HOSTNAME"))
                .unwrap_or_else(|| "cloud-cache".into()),
            drain: Duration::from_secs(parse("DRAIN_SECONDS", 5u64)?),
            max_value_bytes: parse("MAX_VALUE_BYTES", 1024 * 1024usize)?,
            tcp_idle_timeout: Duration::from_secs(parse("TCP_IDLE_SECONDS", 300u64)?),
        })
    }
}

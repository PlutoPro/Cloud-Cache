//! What a verified token *means* to Cloud Cache: who the caller is, what they may do, and which
//! organizations they belong to (and administer).
//!
//! Every identity provider puts this in different claims, so the translation lives here,
//! separate from signature checking (`auth.rs`). Two ways to plug in a provider:
//!
//! - **Configuration:** pick a preset (`AUTH_PRESET=keycloak|auth0|entra|zitadel|single`) and
//!   override individual claim paths with env vars. Covers most providers.
//! - **Code:** implement [`ClaimMapper`] and pass it to [`crate::run_with`].

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Turns the claims of an already-verified token into an [`Identity`].
pub trait ClaimMapper: Send + Sync {
    fn map(&self, claims: &Map<String, Value>) -> Result<Mapped, String>;
}

/// Output of a [`ClaimMapper`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    /// Human-readable caller name for logs and the TCP `AUTH` reply.
    pub principal: String,
    pub perms: Perms,
    pub orgs: Vec<Org>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Perms {
    pub read: bool,
    pub write: bool,
    /// May read every org's usage counters (`/v1/admin/usage`), e.g. the dashboard's service account.
    pub usage: bool,
}

/// An organization (tenant) the caller belongs to. Keys are partitioned by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Org {
    pub id: String,
    /// Display name / short name. Equals `id` when the provider doesn't send one.
    pub alias: String,
    /// Caller administers this org.
    pub admin: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrgError {
    #[error("not a member of any organization")]
    NoOrg,
    #[error("member of several organizations; choose one")]
    Ambiguous,
    #[error("not a member of that organization")]
    NotMember,
}

/// A verified, mapped caller.
#[derive(Debug, Clone)]
pub struct Identity {
    pub principal: String,
    pub sub: Option<String>,
    /// Token expiry, unix seconds.
    pub exp: u64,
    pub perms: Perms,
    pub orgs: Vec<Org>,
}

impl Identity {
    pub fn expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.exp <= now
    }

    /// Pick the org a request acts on. With one membership it's implicit; with several the
    /// caller must name one (by id or alias).
    pub fn org(&self, requested: Option<&str>) -> Result<&Org, OrgError> {
        match requested.map(str::trim).filter(|s| !s.is_empty()) {
            Some(r) => self
                .orgs
                .iter()
                .find(|o| o.id == r || o.alias == r)
                .ok_or(OrgError::NotMember),
            None => match self.orgs.as_slice() {
                [one] => Ok(one),
                [] => Err(OrgError::NoOrg),
                _ => Err(OrgError::Ambiguous),
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Configurable mapper
// ---------------------------------------------------------------------------------------------

/// Where organization membership comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgSource {
    /// Keycloak 26 Organizations: `{"alias": {"id": "…"}}` (or Keycloak 26.0's merged list form).
    Keycloak { claim: String },
    /// One org per token: an id claim plus an optional display-name claim
    /// (Auth0 `org_id`/`org_name`, Entra `tid`, Zitadel resource owner).
    Claim { id: String, name: Option<String> },
    /// A list of org ids (custom claims, groups used as tenants, …).
    List { claim: String },
    /// Single-tenant: everyone who passes auth shares one keyspace.
    Fixed { id: String },
}

/// How to tell that the caller administers an org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminRule {
    None,
    /// A value in `claim` (array, space-separated string, or object keys) must equal `pattern`,
    /// after substituting `{id}` / `{alias}`. A pattern with no placeholder makes the caller
    /// admin of all their orgs (fits providers whose tokens are scoped to one org).
    Match {
        claim: String,
        pattern: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingConfig {
    /// Claims holding role names, read in order and merged. `{aud}` is replaced by the audience.
    pub role_claims: Vec<String>,
    pub read_role: String,
    pub write_role: String,
    pub usage_role: String,
    pub orgs: OrgSource,
    pub admin: AdminRule,
    /// Claims tried in order for the display name.
    pub principal_claims: Vec<String>,
    /// Used to expand `{aud}` in claim paths.
    pub audience: Option<String>,
}

/// Built-in presets. Each is a starting point; any field can be overridden by env vars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Keycloak,
    Auth0,
    Entra,
    Zitadel,
    Single,
}

impl std::str::FromStr for Preset {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "keycloak" => Preset::Keycloak,
            "auth0" => Preset::Auth0,
            "entra" | "azure" | "entra-id" => Preset::Entra,
            "zitadel" => Preset::Zitadel,
            "single" | "single-tenant" | "generic" => Preset::Single,
            other => {
                return Err(format!(
                    "unknown AUTH_PRESET {other:?} (keycloak, auth0, entra, zitadel, single)"
                ))
            }
        })
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Preset {
    pub fn config(self, audience: Option<String>) -> MappingConfig {
        let std_principal = vec![s("preferred_username"), s("email"), s("sub"), s("azp")];
        match self {
            // Realm roles + this service's client roles; Organizations with ids; admins via group
            // `/org-admins/<alias>` (Keycloak 26.0 orgs have no roles of their own).
            Preset::Keycloak => MappingConfig {
                role_claims: vec![s("realm_access.roles"), s("resource_access.{aud}.roles")],
                read_role: s("kv-read"),
                write_role: s("kv-write"),
                usage_role: s("kv-usage"),
                orgs: OrgSource::Keycloak {
                    claim: s("organization"),
                },
                admin: AdminRule::Match {
                    claim: s("groups"),
                    pattern: s("/org-admins/{alias}"),
                },
                principal_claims: std_principal,
                audience,
            },
            // RBAC permissions on the API; Organizations put one org per token.
            Preset::Auth0 => MappingConfig {
                role_claims: vec![s("permissions")],
                read_role: s("read:kv"),
                write_role: s("write:kv"),
                usage_role: s("read:usage"),
                orgs: OrgSource::Claim {
                    id: s("org_id"),
                    name: Some(s("org_name")),
                },
                admin: AdminRule::Match {
                    claim: s("permissions"),
                    pattern: s("admin:kv"),
                },
                principal_claims: vec![s("email"), s("nickname"), s("sub")],
                audience,
            },
            // App roles; the tenant is the organization.
            Preset::Entra => MappingConfig {
                role_claims: vec![s("roles")],
                read_role: s("kv-read"),
                write_role: s("kv-write"),
                usage_role: s("kv-usage"),
                orgs: OrgSource::Claim {
                    id: s("tid"),
                    name: None,
                },
                admin: AdminRule::Match {
                    claim: s("roles"),
                    pattern: s("kv-admin"),
                },
                principal_claims: vec![
                    s("preferred_username"),
                    s("name"),
                    s("oid"),
                    s("sub"),
                    s("azp"),
                ],
                audience,
            },
            // Project roles (object keys); the user's own org (scope urn:zitadel:iam:user:resourceowner).
            Preset::Zitadel => MappingConfig {
                role_claims: vec![s("urn:zitadel:iam:org:project:roles")],
                read_role: s("kv-read"),
                write_role: s("kv-write"),
                usage_role: s("kv-usage"),
                orgs: OrgSource::Claim {
                    id: s("urn:zitadel:iam:user:resourceowner:id"),
                    name: Some(s("urn:zitadel:iam:user:resourceowner:name")),
                },
                admin: AdminRule::Match {
                    claim: s("urn:zitadel:iam:org:project:roles"),
                    pattern: s("kv-admin"),
                },
                principal_claims: std_principal,
                audience,
            },
            // No orgs: one shared keyspace. Roles from a plain `roles` claim.
            Preset::Single => MappingConfig {
                role_claims: vec![s("roles")],
                read_role: s("kv-read"),
                write_role: s("kv-write"),
                usage_role: s("kv-usage"),
                orgs: OrgSource::Fixed { id: s("default") },
                admin: AdminRule::Match {
                    claim: s("roles"),
                    pattern: s("kv-admin"),
                },
                principal_claims: std_principal,
                audience,
            },
        }
    }
}

/// Look up a claim by path. An exact top-level key wins (claim names like
/// `https://example.com/roles` or `urn:zitadel:…` contain dots and colons); otherwise the path
/// is split on `.` and walked through nested objects.
pub fn lookup<'a>(claims: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    if let Some(v) = claims.get(path) {
        return Some(v);
    }
    let mut parts = path.split('.');
    let mut cur = claims.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_object()?.get(p)?;
    }
    Some(cur)
}

/// Strings in a claim value: array items, space-separated words (OAuth `scope`), or object keys
/// (Zitadel roles).
fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
        Value::Object(m) => m.keys().cloned().collect(),
        _ => vec![],
    }
}

fn valid_org_id(id: &str) -> Result<(), String> {
    if crate::store::valid_org_id(id) {
        Ok(())
    } else {
        Err(format!("invalid organization id {id:?}"))
    }
}

/// Parse Keycloak's organization claim. Shapes seen in the wild:
/// - `{"acme": {"id": "<uuid>"}}`: Organization Membership mapper with "Add organization id" on
/// - `["acme"]`: aliases only (default mapper settings)
/// - `[{"acme": {"id": ...}}, "acme"]`: Keycloak 26.0 merging two mappers into one claim
///
/// Tenancy is keyed by id, so every org must come with one.
pub fn parse_keycloak_orgs(claim: Option<&Value>) -> Result<Vec<Org>, String> {
    const NEED_IDS: &str = "organization claim has no ids; turn on \"Add organization id\" \
                            on the Organization Membership mapper";

    fn from_map(map: &Map<String, Value>, out: &mut Vec<Org>) -> Result<(), String> {
        for (alias, v) in map {
            let id = v
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| NEED_IDS.to_string())?;
            valid_org_id(id)?;
            out.push(Org {
                id: id.into(),
                alias: alias.clone(),
                admin: false,
            });
        }
        Ok(())
    }

    let mut out = Vec::new();
    match claim {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => from_map(map, &mut out)?,
        Some(Value::Array(items)) => {
            let mut bare = Vec::new();
            for item in items {
                match item {
                    Value::Object(map) => from_map(map, &mut out)?,
                    Value::String(alias) => bare.push(alias),
                    other => return Err(format!("unexpected organization claim item: {other}")),
                }
            }
            if bare.iter().any(|a| !out.iter().any(|o| &o.alias == *a)) {
                return Err(NEED_IDS.to_string());
            }
        }
        Some(other) => return Err(format!("unexpected organization claim: {other}")),
    }
    Ok(out)
}

/// The standard, configuration-driven [`ClaimMapper`].
pub struct ConfiguredMapper {
    cfg: MappingConfig,
}

impl ConfiguredMapper {
    pub fn new(cfg: MappingConfig) -> Self {
        Self { cfg }
    }

    pub fn config(&self) -> &MappingConfig {
        &self.cfg
    }

    fn expand(&self, path: &str) -> String {
        match &self.cfg.audience {
            Some(aud) => path.replace("{aud}", aud),
            None => path.to_string(),
        }
    }

    fn values(&self, claims: &Map<String, Value>, path: &str) -> Vec<String> {
        lookup(claims, &self.expand(path))
            .map(strings)
            .unwrap_or_default()
    }

    fn orgs(&self, claims: &Map<String, Value>) -> Result<Vec<Org>, String> {
        let mut orgs = match &self.cfg.orgs {
            OrgSource::Keycloak { claim } => parse_keycloak_orgs(lookup(claims, claim))?,
            OrgSource::Claim { id, name } => match lookup(claims, id).and_then(Value::as_str) {
                None => vec![],
                Some(id) => {
                    valid_org_id(id)?;
                    let alias = name
                        .as_deref()
                        .and_then(|n| lookup(claims, n))
                        .and_then(Value::as_str)
                        .unwrap_or(id);
                    vec![Org {
                        id: id.into(),
                        alias: alias.into(),
                        admin: false,
                    }]
                }
            },
            OrgSource::List { claim } => {
                let ids = lookup(claims, claim).map(strings).unwrap_or_default();
                ids.into_iter()
                    .map(|id| {
                        valid_org_id(&id)?;
                        Ok(Org {
                            alias: id.clone(),
                            id,
                            admin: false,
                        })
                    })
                    .collect::<Result<_, String>>()?
            }
            OrgSource::Fixed { id } => vec![Org {
                id: id.clone(),
                alias: id.clone(),
                admin: false,
            }],
        };
        orgs.sort_by(|a, b| a.id.cmp(&b.id));
        orgs.dedup_by(|a, b| a.id == b.id);

        if let AdminRule::Match { claim, pattern } = &self.cfg.admin {
            let have = self.values(claims, claim);
            for org in orgs.iter_mut() {
                let want = pattern
                    .replace("{id}", &org.id)
                    .replace("{alias}", &org.alias);
                org.admin = have.contains(&want);
            }
        }
        Ok(orgs)
    }
}

impl ClaimMapper for ConfiguredMapper {
    fn map(&self, claims: &Map<String, Value>) -> Result<Mapped, String> {
        let roles: Vec<String> = self
            .cfg
            .role_claims
            .iter()
            .flat_map(|p| self.values(claims, p))
            .collect();
        let has = |r: &str| roles.iter().any(|x| x == r);
        let write = has(&self.cfg.write_role);
        let perms = Perms {
            read: write || has(&self.cfg.read_role),
            write,
            usage: has(&self.cfg.usage_role),
        };
        let principal = self
            .cfg
            .principal_claims
            .iter()
            .find_map(|p| lookup(claims, p).and_then(Value::as_str))
            .unwrap_or("unknown")
            .to_string();
        Ok(Mapped {
            principal,
            perms,
            orgs: self.orgs(claims)?,
        })
    }
}

/// Build the mapping from env: `AUTH_PRESET` first, then per-field overrides.
///
/// | Variable | Meaning |
/// |---|---|
/// | `CLAIM_ROLES` | comma-separated claim paths holding roles |
/// | `KV_READ_ROLE` / `KV_WRITE_ROLE` / `KV_USAGE_ROLE` | role names |
/// | `ORG_MODE` | `keycloak`, `claim`, `list` or `fixed` |
/// | `CLAIM_ORG` | org claim (id claim for `claim` mode) |
/// | `CLAIM_ORG_NAME` | display-name claim (`claim` mode) |
/// | `ORG_FIXED_ID` | org id for `fixed` mode |
/// | `CLAIM_ADMIN` / `ADMIN_MATCH` | admin claim and pattern (`{id}`, `{alias}`); `ADMIN_MATCH=none` disables |
/// | `CLAIM_PRINCIPAL` | comma-separated display-name claims |
pub fn mapping_from_env(
    get: impl Fn(&str) -> Option<String>,
    audience: Option<String>,
) -> Result<MappingConfig, String> {
    let preset: Preset = get("AUTH_PRESET")
        .unwrap_or_else(|| "keycloak".into())
        .parse()?;
    let mut c = preset.config(audience);
    let list = |v: String| {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    };

    if let Some(v) = get("CLAIM_ROLES") {
        c.role_claims = list(v);
    }
    if let Some(v) = get("KV_READ_ROLE") {
        c.read_role = v;
    }
    if let Some(v) = get("KV_WRITE_ROLE") {
        c.write_role = v;
    }
    if let Some(v) = get("KV_USAGE_ROLE") {
        c.usage_role = v;
    }
    if let Some(v) = get("CLAIM_PRINCIPAL") {
        c.principal_claims = list(v);
    }

    let org_claim = get("CLAIM_ORG");
    let mode = get("ORG_MODE");
    if mode.is_some() || org_claim.is_some() || get("ORG_FIXED_ID").is_some() {
        let current_claim = match &c.orgs {
            OrgSource::Keycloak { claim } | OrgSource::List { claim } => Some(claim.clone()),
            OrgSource::Claim { id, .. } => Some(id.clone()),
            OrgSource::Fixed { .. } => None,
        };
        let current_mode = match &c.orgs {
            OrgSource::Keycloak { .. } => "keycloak",
            OrgSource::Claim { .. } => "claim",
            OrgSource::List { .. } => "list",
            OrgSource::Fixed { .. } => "fixed",
        };
        let claim = || {
            org_claim
                .clone()
                .or(current_claim.clone())
                .ok_or_else(|| "CLAIM_ORG is required for this ORG_MODE".to_string())
        };
        c.orgs = match mode.as_deref().unwrap_or(current_mode) {
            "keycloak" => OrgSource::Keycloak { claim: claim()? },
            "claim" => OrgSource::Claim {
                id: claim()?,
                name: get("CLAIM_ORG_NAME").or(match &c.orgs {
                    OrgSource::Claim { name, .. } => name.clone(),
                    _ => None,
                }),
            },
            "list" => OrgSource::List { claim: claim()? },
            "fixed" => OrgSource::Fixed {
                id: get("ORG_FIXED_ID").unwrap_or_else(|| "default".into()),
            },
            other => {
                return Err(format!(
                    "unknown ORG_MODE {other:?} (keycloak, claim, list, fixed)"
                ))
            }
        };
    } else if let Some(name) = get("CLAIM_ORG_NAME") {
        if let OrgSource::Claim { name: n, .. } = &mut c.orgs {
            *n = Some(name);
        }
    }

    match (get("CLAIM_ADMIN"), get("ADMIN_MATCH")) {
        (_, Some(m)) if m.eq_ignore_ascii_case("none") => c.admin = AdminRule::None,
        (claim, pattern) if claim.is_some() || pattern.is_some() => {
            let (cur_claim, cur_pattern) = match &c.admin {
                AdminRule::Match { claim, pattern } => (Some(claim.clone()), Some(pattern.clone())),
                AdminRule::None => (None, None),
            };
            c.admin = AdminRule::Match {
                claim: claim
                    .or(cur_claim)
                    .ok_or("CLAIM_ADMIN is required with ADMIN_MATCH")?,
                pattern: pattern
                    .or(cur_pattern)
                    .ok_or("ADMIN_MATCH is required with CLAIM_ADMIN")?,
            };
        }
        _ => {}
    }
    Ok(c)
}

/// Human-readable summary for the startup log.
pub fn describe(c: &MappingConfig) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert("roles", c.role_claims.join(","));
    m.insert(
        "read/write/usage",
        format!("{}/{}/{}", c.read_role, c.write_role, c.usage_role),
    );
    m.insert(
        "orgs",
        match &c.orgs {
            OrgSource::Keycloak { claim } => format!("keycloak:{claim}"),
            OrgSource::Claim { id, name } => format!(
                "claim:{id}{}",
                name.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()
            ),
            OrgSource::List { claim } => format!("list:{claim}"),
            OrgSource::Fixed { id } => format!("fixed:{id}"),
        },
    );
    m.insert(
        "admin",
        match &c.admin {
            AdminRule::None => "none".into(),
            AdminRule::Match { claim, pattern } => format!("{claim} = {pattern}"),
        },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map_with(preset: Preset, aud: Option<&str>, claims: Value) -> Result<Mapped, String> {
        let m = ConfiguredMapper::new(preset.config(aud.map(str::to_string)));
        m.map(claims.as_object().unwrap())
    }

    #[test]
    fn keycloak_preset() {
        let m = map_with(
            Preset::Keycloak,
            Some("kv-service"),
            json!({
                "preferred_username": "alice",
                "realm_access": {"roles": ["kv-read"]},
                "resource_access": {"kv-service": {"roles": ["kv-write"]}, "other": {"roles": ["kv-usage"]}},
                "organization": [{"acme": {"id": "id-a"}, "globex": {"id": "id-g"}}, "acme", "globex"],
                "groups": ["/org-admins/globex"],
            }),
        )
        .unwrap();
        assert_eq!(m.principal, "alice");
        // kv-usage on another client's roles must not count.
        assert_eq!(
            m.perms,
            Perms {
                read: true,
                write: true,
                usage: false
            }
        );
        assert_eq!(
            m.orgs,
            [
                Org {
                    id: "id-a".into(),
                    alias: "acme".into(),
                    admin: false
                },
                Org {
                    id: "id-g".into(),
                    alias: "globex".into(),
                    admin: true
                },
            ]
        );
        let err = map_with(Preset::Keycloak, None, json!({"organization": ["acme"]})).unwrap_err();
        assert!(err.contains("Add organization id"));
    }

    #[test]
    fn auth0_preset() {
        // Shape from Auth0's Organizations docs.
        let m = map_with(
            Preset::Auth0,
            None,
            json!({
                "iss": "https://exampleco.auth0.com/",
                "sub": "auth0|602c0dcab993d10073daf680",
                "org_id": "org_9ybsU1dN2dKfDkBi",
                "org_name": "acme",
                "permissions": ["read:kv", "write:kv", "admin:kv"],
            }),
        )
        .unwrap();
        assert_eq!(
            m.perms,
            Perms {
                read: true,
                write: true,
                usage: false
            }
        );
        assert_eq!(
            m.orgs,
            [Org {
                id: "org_9ybsU1dN2dKfDkBi".into(),
                alias: "acme".into(),
                admin: true
            }]
        );
        assert_eq!(m.principal, "auth0|602c0dcab993d10073daf680");

        // Machine-to-machine token, org name feature off: alias falls back to the id.
        let m = map_with(
            Preset::Auth0,
            None,
            json!({"org_id": "org_x", "permissions": ["read:kv"]}),
        )
        .unwrap();
        assert_eq!(m.orgs[0].alias, "org_x");
        assert!(!m.orgs[0].admin);
    }

    #[test]
    fn entra_preset() {
        let m = map_with(
            Preset::Entra,
            None,
            json!({"tid": "6233c3e1-0000-4000-8000-000000000001", "roles": ["kv-read"], "preferred_username": "ada@contoso.com"}),
        )
        .unwrap();
        assert_eq!(
            m.perms,
            Perms {
                read: true,
                write: false,
                usage: false
            }
        );
        assert_eq!(m.orgs[0].id, "6233c3e1-0000-4000-8000-000000000001");
        assert_eq!(m.principal, "ada@contoso.com");
    }

    #[test]
    fn zitadel_preset() {
        // Roles are object keys mapping org ids to domains.
        let m = map_with(
            Preset::Zitadel,
            None,
            json!({
                "urn:zitadel:iam:org:project:roles": {
                    "kv-write": {"223279178798072065": "acme.zitadel.cloud"},
                    "kv-admin": {"223279178798072065": "acme.zitadel.cloud"},
                },
                "urn:zitadel:iam:user:resourceowner:id": "223279178798072065",
                "urn:zitadel:iam:user:resourceowner:name": "acme",
                "preferred_username": "ada",
            }),
        )
        .unwrap();
        assert_eq!(
            m.perms,
            Perms {
                read: true,
                write: true,
                usage: false
            }
        );
        assert_eq!(
            m.orgs,
            [Org {
                id: "223279178798072065".into(),
                alias: "acme".into(),
                admin: true
            }]
        );
    }

    #[test]
    fn single_tenant_preset() {
        let m = map_with(
            Preset::Single,
            None,
            json!({"sub": "svc", "roles": ["kv-write"]}),
        )
        .unwrap();
        assert_eq!(
            m.orgs,
            [Org {
                id: "default".into(),
                alias: "default".into(),
                admin: false
            }]
        );
        assert!(m.perms.write);
    }

    #[test]
    fn lookup_rules() {
        let c = json!({"https://example.com/roles": ["a"], "a": {"b": {"c": 1}}, "urn:x:y": "z"});
        let c = c.as_object().unwrap();
        assert_eq!(lookup(c, "https://example.com/roles"), Some(&json!(["a"])));
        assert_eq!(lookup(c, "a.b.c"), Some(&json!(1)));
        assert_eq!(lookup(c, "urn:x:y"), Some(&json!("z")));
        assert_eq!(lookup(c, "a.nope"), None);
        // Space-separated scope strings count as roles too.
        assert_eq!(strings(&json!("read:kv write:kv")), ["read:kv", "write:kv"]);
    }

    #[test]
    fn env_overrides() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        // Generic provider with roles and tenants in custom namespaced claims.
        let c = mapping_from_env(
            env(&[
                ("AUTH_PRESET", "single"),
                ("CLAIM_ROLES", "https://cc.example/roles"),
                ("ORG_MODE", "list"),
                ("CLAIM_ORG", "https://cc.example/tenants"),
                ("CLAIM_ADMIN", "https://cc.example/admin_of"),
                ("ADMIN_MATCH", "{id}"),
            ]),
            None,
        )
        .unwrap();
        let m = ConfiguredMapper::new(c)
            .map(
                json!({
                    "sub": "u1",
                    "https://cc.example/roles": ["kv-read"],
                    "https://cc.example/tenants": ["t1", "t2"],
                    "https://cc.example/admin_of": ["t2"],
                })
                .as_object()
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            m.orgs
                .iter()
                .map(|o| (o.id.as_str(), o.admin))
                .collect::<Vec<_>>(),
            [("t1", false), ("t2", true)]
        );

        // Keeping a preset but renaming roles and turning admin off.
        let c = mapping_from_env(
            env(&[
                ("AUTH_PRESET", "entra"),
                ("KV_WRITE_ROLE", "Writer"),
                ("ADMIN_MATCH", "none"),
            ]),
            None,
        )
        .unwrap();
        assert_eq!(c.write_role, "Writer");
        assert_eq!(c.admin, AdminRule::None);
        assert!(matches!(c.orgs, OrgSource::Claim { .. }));

        assert!(mapping_from_env(env(&[("AUTH_PRESET", "okta-ish")]), None).is_err());
        assert!(mapping_from_env(env(&[("ORG_MODE", "weird")]), None).is_err());
    }

    #[test]
    fn org_selection() {
        let id = |orgs: &[(&str, &str)]| Identity {
            principal: "p".into(),
            sub: None,
            exp: 0,
            perms: Perms::default(),
            orgs: orgs
                .iter()
                .map(|(a, i)| Org {
                    alias: a.to_string(),
                    id: i.to_string(),
                    admin: false,
                })
                .collect(),
        };
        let one = id(&[("acme", "id-a")]);
        assert_eq!(one.org(None).unwrap().id, "id-a");
        assert_eq!(one.org(Some("acme")).unwrap().id, "id-a");
        assert_eq!(one.org(Some("id-other")), Err(OrgError::NotMember));
        let two = id(&[("acme", "id-a"), ("beta", "id-b")]);
        assert_eq!(two.org(None), Err(OrgError::Ambiguous));
        assert_eq!(two.org(Some("id-b")).unwrap().alias, "beta");
        assert_eq!(id(&[]).org(None), Err(OrgError::NoOrg));
    }
}

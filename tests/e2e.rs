//! End-to-end: real HTTP + TCP listeners, tokens signed by a throwaway RSA key
//! served from a mock JWKS endpoint shaped like Keycloak's.

use std::{net::SocketAddr, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{encode, EncodingKey, Header};
use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

const ISS: &str = "http://localhost:8081/realms/kv";
const AUD: &str = "kv-service";
const ACME: &str = "6f1d2c3b-aaaa-4bbb-8ccc-000000000001";
const GLOBEX: &str = "6f1d2c3b-aaaa-4bbb-8ccc-000000000002";

fn acme() -> serde_json::Value {
    json!({ "acme": { "id": ACME } })
}
fn globex() -> serde_json::Value {
    json!({ "globex": { "id": GLOBEX } })
}
fn both() -> serde_json::Value {
    json!({ "acme": { "id": ACME }, "globex": { "id": GLOBEX } })
}

struct Harness {
    /// `groups` claim put into tokens minted from now on.
    groups: std::sync::Mutex<Vec<String>>,
    http: SocketAddr,
    tcp: SocketAddr,
    key: EncodingKey,
    shutdown: CancellationToken,
}

/// How to start the service under test.
struct Opts {
    issuer: String,
    audience: String,
    mapping: cloud_cache::identity::MappingConfig,
    /// Find the JWKS through the discovery document instead of a configured URL.
    discovery: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            issuer: ISS.into(),
            audience: AUD.into(),
            mapping: cloud_cache::identity::Preset::Keycloak.config(Some(AUD.into())),
            discovery: false,
        }
    }
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(Opts::default()).await
    }

    async fn start_with(opts: Opts) -> Self {
        let mut rng = rand::thread_rng();
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pub_key = priv_key.to_public_key();
        let jwks = json!({ "keys": [
            // Keycloak also publishes an encryption key; it must be ignored.
            { "kty": "RSA", "kid": "enc-1", "use": "enc", "alg": "RSA-OAEP",
              "n": URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be()), "e": "AQAB" },
            { "kty": "RSA", "kid": "sig-1", "use": "sig", "alg": "RS256",
              "n": URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be()),
              "e": URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be()) },
        ]});
        let jwks_l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let jwks_addr = jwks_l.local_addr().unwrap();
        let discovery =
            json!({ "issuer": opts.issuer, "jwks_uri": format!("http://{jwks_addr}/certs") });
        let app = axum::Router::new()
            .route(
                "/certs",
                axum::routing::get(move || {
                    let j = jwks.clone();
                    async move { axum::Json(j) }
                }),
            )
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get(move || {
                    let d = discovery.clone();
                    async move { axum::Json(d) }
                }),
            );
        tokio::spawn(async move { axum::serve(jwks_l, app).await.unwrap() });

        let http_l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = http_l.local_addr().unwrap();
        let tcp = tcp_l.local_addr().unwrap();
        let cfg = cloud_cache::Config {
            http_addr: http,
            tcp_addr: tcp,
            issuer: opts.issuer.clone(),
            jwks_url: (!opts.discovery).then(|| format!("http://{jwks_addr}/certs")),
            discovery_url: format!("http://{jwks_addr}/.well-known/openid-configuration"),
            audience: Some(opts.audience.clone()),
            mapping: opts.mapping,
            node_id: "test-node".into(),
            drain: Duration::from_millis(200),
            max_value_bytes: 64 * 1024,
            tcp_idle_timeout: Duration::from_secs(30),
        };
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            cloud_cache::run_with_listeners(cfg, http_l, tcp_l, sd)
                .await
                .unwrap()
        });

        let pem = priv_key.to_pkcs1_pem(Default::default()).unwrap();
        let h = Harness {
            groups: Default::default(),
            http,
            tcp,
            key: EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
            shutdown,
        };
        // Wait for readiness (JWKS loaded).
        for _ in 0..100 {
            if let Ok(r) = reqwest::get(h.url("/readyz")).await {
                if r.status() == 200 {
                    return h;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("service never became ready");
    }

    /// Sign arbitrary claims with the test key (for non-Keycloak token shapes).
    fn sign(&self, claims: serde_json::Value) -> String {
        let mut h = Header::new(jsonwebtoken::Algorithm::RS256);
        h.kid = Some("sig-1".into());
        encode(&h, &claims, &self.key).unwrap()
    }

    fn url(&self, p: &str) -> String {
        format!("http://{}{}", self.http, p)
    }

    fn token_with(
        &self,
        iss: &str,
        aud: &str,
        realm_roles: &[&str],
        client_roles: &[&str],
        exp_in: i64,
    ) -> String {
        self.mint(iss, aud, realm_roles, client_roles, exp_in, acme())
    }

    /// Token for a member of the given orgs (Keycloak's "rich" organization claim).
    fn org_token(&self, roles: &[&str], orgs: serde_json::Value) -> String {
        self.mint(ISS, AUD, roles, &[], 300, orgs)
    }

    fn mint(
        &self,
        iss: &str,
        aud: &str,
        realm_roles: &[&str],
        client_roles: &[&str],
        exp_in: i64,
        orgs: serde_json::Value,
    ) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = json!({
            "iss": iss, "aud": aud, "sub": "user-123", "azp": "kv-cli",
            "preferred_username": "alice",
            "iat": now, "exp": now + exp_in,
            "realm_access": { "roles": realm_roles },
            "resource_access": { AUD: { "roles": client_roles } },
            "organization": orgs,
            "groups": self.groups.lock().unwrap().clone(),
        });
        let mut h = Header::new(jsonwebtoken::Algorithm::RS256);
        h.kid = Some("sig-1".into());
        encode(&h, &claims, &self.key).unwrap()
    }

    fn token(&self, roles: &[&str]) -> String {
        self.token_with(ISS, AUD, roles, &[], 300)
    }
}

struct Tcp {
    r: BufReader<tokio::net::tcp::OwnedReadHalf>,
    w: tokio::net::tcp::OwnedWriteHalf,
}

impl Tcp {
    async fn connect(addr: SocketAddr) -> Self {
        let (r, w) = TcpStream::connect(addr).await.unwrap().into_split();
        Tcp {
            r: BufReader::new(r),
            w,
        }
    }
    async fn cmd(&mut self, line: &str) -> String {
        self.w
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        self.r.read_line(&mut out).await.unwrap();
        out.trim_end().to_string()
    }
}

#[tokio::test]
async fn http_api_with_roles() {
    let h = Harness::start().await;
    let c = reqwest::Client::new();
    let writer = h.token(&["kv-write"]);
    let reader = h.token(&["kv-read"]);
    let nobody = h.token(&["offline_access"]);

    assert_eq!(reqwest::get(h.url("/healthz")).await.unwrap().status(), 200);

    let r = c
        .put(h.url("/v1/kv/greeting"))
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers()["www-authenticate"], "Bearer");

    let r = c
        .put(h.url("/v1/kv/greeting"))
        .bearer_auth(&reader)
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = c
        .get(h.url("/v1/kv/greeting"))
        .bearer_auth(&nobody)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    let r = c
        .put(h.url("/v1/kv/greeting"))
        .bearer_auth(&writer)
        .body("hello world")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = c
        .get(h.url("/v1/kv/greeting"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello world");

    // Client role on the service's own client also grants access.
    let client_role = h.token_with(ISS, AUD, &[], &["kv-read"], 300);
    let r = c
        .get(h.url("/v1/kv/greeting"))
        .bearer_auth(&client_role)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // TTL
    let r = c
        .put(h.url("/v1/kv/temp?ttl=1"))
        .bearer_auth(&writer)
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let r = c
        .get(h.url("/v1/kv/temp"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    let r = c
        .delete(h.url("/v1/kv/greeting"))
        .bearer_auth(&writer)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = c
        .delete(h.url("/v1/kv/greeting"))
        .bearer_auth(&writer)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn rejects_bad_tokens() {
    let h = Harness::start().await;
    let c = reqwest::Client::new();
    let bad = [
        h.token_with("http://evil/realms/kv", AUD, &["kv-write"], &[], 300), // wrong issuer
        h.token_with(ISS, "account", &["kv-write"], &[], 300),               // wrong audience
        h.token_with(ISS, AUD, &["kv-write"], &[], -120),                    // expired
        // HS256 signed with a guessable secret must never be accepted.
        {
            let mut hd = Header::new(jsonwebtoken::Algorithm::HS256);
            hd.kid = Some("sig-1".into());
            encode(
                &hd,
                &json!({"iss": ISS, "aud": AUD, "exp": 9999999999u64,
                "realm_access": {"roles": ["kv-write"]}}),
                &EncodingKey::from_secret(b"x"),
            )
            .unwrap()
        },
        "not.a.jwt".to_string(),
    ];
    for t in bad {
        let r = c
            .put(h.url("/v1/kv/k"))
            .bearer_auth(&t)
            .body("v")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "token should be rejected: {t}");
    }
}

#[tokio::test]
async fn tcp_api_and_cross_protocol() {
    let h = Harness::start().await;
    let mut t = Tcp::connect(h.tcp).await;

    assert_eq!(t.cmd("PING").await, "PONG");
    assert!(t.cmd("GET a").await.starts_with("ERR not authenticated"));
    assert_eq!(t.cmd("AUTH garbage").await, "ERR unauthorized");

    assert_eq!(
        t.cmd(&format!("AUTH {}", h.token(&["kv-read"]))).await,
        format!("OK alice {ACME}")
    );
    assert_eq!(t.cmd("SET a b").await, "ERR forbidden");

    assert_eq!(
        t.cmd(&format!("AUTH {}", h.token(&["kv-write"]))).await,
        format!("OK alice {ACME}")
    );
    assert_eq!(t.cmd("SET session:1 hello there world").await, "OK");
    assert_eq!(t.cmd("get session:1").await, "VALUE hello there world");
    assert_eq!(t.cmd("SETEX tmp 1 soon gone").await, "OK");
    assert_eq!(t.cmd("GET tmp").await, "VALUE soon gone");
    assert_eq!(t.cmd("GET missing").await, "NIL");
    assert_eq!(
        t.cmd("SETEX tmp zero x").await,
        "ERR ttl must be a positive integer"
    );
    assert_eq!(t.cmd("FLY away").await, "ERR unknown command");

    // Written over TCP, read over HTTP.
    let r = reqwest::Client::new()
        .get(h.url("/v1/kv/session:1"))
        .bearer_auth(h.token(&["kv-read"]))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "hello there world");

    // Binary over HTTP isn't representable on the line protocol.
    reqwest::Client::new()
        .put(h.url("/v1/kv/bin"))
        .bearer_auth(h.token(&["kv-write"]))
        .body(vec![0xffu8, 0x00, 0x0a])
        .send()
        .await
        .unwrap();
    assert_eq!(t.cmd("GET bin").await, "ERR binary value; use HTTP");

    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(t.cmd("GET tmp").await, "NIL");
    assert_eq!(t.cmd("DEL session:1").await, "OK");
    assert_eq!(t.cmd("DEL session:1").await, "NIL");
    assert_eq!(t.cmd("QUIT").await, "BYE");
}

#[tokio::test]
async fn tcp_session_expires_with_token() {
    let h = Harness::start().await;
    let mut t = Tcp::connect(h.tcp).await;
    // Valid for 1s beyond now, but within the 30s leeway on initial verify; session check is strict.
    let short = h.token_with(ISS, AUD, &["kv-read"], &[], 1);
    assert_eq!(
        t.cmd(&format!("AUTH {short}")).await,
        format!("OK alice {ACME}")
    );
    assert_eq!(t.cmd("GET x").await, "NIL");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(t.cmd("GET x").await.starts_with("ERR token expired"));
}

#[tokio::test]
async fn drains_on_shutdown() {
    let h = Harness::start().await;
    let mut t = Tcp::connect(h.tcp).await;
    h.shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // During drain: still serving, but readiness fails so the LB stops routing new traffic.
    let r = reqwest::get(h.url("/readyz")).await.unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(t.cmd("PING").await, "PONG");
    // After drain the TCP session is told to go away.
    let mut line = String::new();
    t.r.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), "BYE shutting down");
}

#[tokio::test]
async fn organizations_are_isolated_over_http() {
    let h = Harness::start().await;
    let c = reqwest::Client::new();
    let acme_w = h.org_token(&["kv-write"], acme());
    let globex_w = h.org_token(&["kv-write"], globex());
    let multi = h.org_token(&["kv-write"], both());

    for (tok, val) in [(&acme_w, "acme-config"), (&globex_w, "globex-config")] {
        let r = c
            .put(h.url("/v1/kv/config"))
            .bearer_auth(tok)
            .body(val)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 204);
    }
    // Same key, different orgs, different values.
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&acme_w)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "acme-config");
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&globex_w)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "globex-config");

    // A user in two orgs must pick one, by id or alias.
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&multi)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&multi)
        .header("X-Org-Id", GLOBEX)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "globex-config");
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&multi)
        .header("X-Org-Id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "acme-config");

    // Can't reach into an org you don't belong to.
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&acme_w)
        .header("X-Org-Id", GLOBEX)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // No membership at all -> 403; alias-only claim (no ids) -> 401.
    let none = h.org_token(&["kv-write"], json!(null));
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&none)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let aliases = h.org_token(&["kv-write"], json!(["acme"]));
    let r = c
        .get(h.url("/v1/kv/config"))
        .bearer_auth(&aliases)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn organizations_over_tcp() {
    let h = Harness::start().await;
    let multi = h.org_token(&["kv-write"], both());
    let mut t = Tcp::connect(h.tcp).await;
    assert!(t
        .cmd(&format!("AUTH {multi}"))
        .await
        .starts_with("ERR member of several organizations"));
    assert_eq!(
        t.cmd(&format!("AUTH {multi} globex")).await,
        format!("OK alice {GLOBEX}")
    );
    assert_eq!(t.cmd("SET plan pro").await, "OK");
    assert!(t
        .cmd(&format!("AUTH {multi} initech"))
        .await
        .starts_with("ERR not a member"));
    assert!(t.cmd("GET plan").await.starts_with("ERR not authenticated"));
    assert_eq!(
        t.cmd(&format!("AUTH {multi} {ACME}")).await,
        format!("OK alice {ACME}")
    );
    assert_eq!(t.cmd("GET plan").await, "NIL");

    // Written over TCP into globex, visible over HTTP for globex only.
    let r = reqwest::Client::new()
        .get(h.url("/v1/kv/plan"))
        .bearer_auth(h.org_token(&["kv-read"], globex()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "pro");
}

#[tokio::test]
async fn usage_endpoint() {
    let h = Harness::start().await;
    let c = reqwest::Client::new();
    let acme_w = h.org_token(&["kv-write"], acme());
    c.put(h.url("/v1/kv/k1"))
        .bearer_auth(&acme_w)
        .body("12345")
        .send()
        .await
        .unwrap();
    c.put(h.url("/v1/kv/k2?ttl=60"))
        .bearer_auth(&acme_w)
        .body("x")
        .send()
        .await
        .unwrap();
    c.get(h.url("/v1/kv/k1"))
        .bearer_auth(&acme_w)
        .send()
        .await
        .unwrap();
    c.put(h.url("/v1/kv/g"))
        .bearer_auth(h.org_token(&["kv-write"], globex()))
        .body("y")
        .send()
        .await
        .unwrap();

    // Ordinary writers can't see everyone's usage.
    let r = c
        .get(h.url("/v1/admin/usage"))
        .bearer_auth(&acme_w)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // Usage service account: no org membership needed.
    let biller = h.org_token(&["kv-usage"], json!(null));
    let r: serde_json::Value = c
        .get(h.url("/v1/admin/usage"))
        .bearer_auth(&biller)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["node"], "test-node");
    assert!(r["started_at"].as_u64().unwrap() > 0);
    let orgs = r["orgs"].as_array().unwrap();
    let a = orgs.iter().find(|o| o["org_id"] == ACME).unwrap();
    assert_eq!(a["keys"], 2);
    assert_eq!(a["bytes"], 2 + 5 + 2 + 1);
    assert_eq!(a["writes"], 2);
    assert_eq!(a["reads"], 1);
    let g = orgs.iter().find(|o| o["org_id"] == GLOBEX).unwrap();
    assert_eq!(g["keys"], 1);
}

#[tokio::test]
async fn me_reports_orgs_and_admin_flags() {
    let h = Harness::start().await;
    *h.groups.lock().unwrap() = vec!["/org-admins/globex".into()];
    let tok = h.org_token(&["kv-read"], both());
    let me: serde_json::Value = reqwest::Client::new()
        .get(h.url("/v1/me"))
        .bearer_auth(tok)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"], "alice");
    assert_eq!(
        me["permissions"],
        json!({"read": true, "write": false, "usage": false})
    );
    assert_eq!(
        me["organizations"],
        json!([
            {"id": ACME, "alias": "acme", "admin": false},
            {"id": GLOBEX, "alias": "globex", "admin": true},
        ])
    );
}

#[tokio::test]
async fn list_keys_paginates_per_org() {
    let h = Harness::start().await;
    let c = reqwest::Client::new();
    let acme_w = h.org_token(&["kv-write"], acme());
    for i in 0..7 {
        c.put(h.url(&format!("/v1/kv/user:{i}")))
            .bearer_auth(&acme_w)
            .body("v")
            .send()
            .await
            .unwrap();
    }
    c.put(h.url("/v1/kv/config?ttl=300"))
        .bearer_auth(&acme_w)
        .body("cfg")
        .send()
        .await
        .unwrap();
    c.put(h.url("/v1/kv/user:globex"))
        .bearer_auth(h.org_token(&["kv-write"], globex()))
        .body("x")
        .send()
        .await
        .unwrap();

    let reader = h.org_token(&["kv-read"], acme());
    let page: serde_json::Value = c
        .get(h.url("/v1/kv?prefix=user:&limit=5"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let keys: Vec<_> = page["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, ["user:0", "user:1", "user:2", "user:3", "user:4"]);
    assert_eq!(page["next"], "user:4");

    let page2: serde_json::Value = c
        .get(h.url("/v1/kv?prefix=user:&limit=5&after=user:4"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page2["keys"].as_array().unwrap().len(), 2); // globex's key is not visible
    assert!(page2["next"].is_null());

    let all: serde_json::Value = c
        .get(h.url("/v1/kv"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        all["keys"][0],
        json!({"key": "config", "size": 3, "ttl": 299})
    );

    // Listing needs read permission and an org.
    let none = h.org_token(&["kv-read"], json!(null));
    assert_eq!(
        c.get(h.url("/v1/kv"))
            .bearer_auth(&none)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn keys_found_through_discovery() {
    let h = Harness::start_with(Opts {
        discovery: true,
        ..Opts::default()
    })
    .await;
    let r = reqwest::Client::new()
        .put(h.url("/v1/kv/k"))
        .bearer_auth(h.token(&["kv-write"]))
        .body("v")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
}

#[tokio::test]
async fn auth0_style_tokens() {
    // Auth0: issuer ends in "/", one org per token (org_id/org_name), RBAC `permissions`.
    let iss = "https://cloud-cache-test.us.auth0.com/";
    let aud = "https://api.cloud-cache.example";
    let h = Harness::start_with(Opts {
        issuer: iss.into(),
        audience: aud.into(),
        mapping: cloud_cache::identity::Preset::Auth0.config(Some(aud.into())),
        discovery: true,
    })
    .await;
    let c = reqwest::Client::new();
    let tok = |perms: &[&str], issuer: &str| {
        h.sign(json!({
            "iss": issuer, "aud": [aud, format!("{}userinfo", iss)], "sub": "auth0|abc",
            "exp": now() + 300, "org_id": "org_9ybsU1dN2dKfDkBi", "org_name": "acme",
            "permissions": perms, "scope": "openid profile",
        }))
    };
    let writer = tok(&["read:kv", "write:kv", "admin:kv"], iss);
    let r = c
        .put(h.url("/v1/kv/greeting"))
        .bearer_auth(&writer)
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let reader = tok(&["read:kv"], iss);
    assert_eq!(
        c.get(h.url("/v1/kv/greeting"))
            .bearer_auth(&reader)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "hi"
    );
    assert_eq!(
        c.put(h.url("/v1/kv/greeting"))
            .bearer_auth(&reader)
            .body("x")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );

    let me: serde_json::Value = c
        .get(h.url("/v1/me"))
        .bearer_auth(&writer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        me["organizations"],
        json!([{"id": "org_9ybsU1dN2dKfDkBi", "alias": "acme", "admin": true}])
    );

    // The issuer must match exactly, trailing slash included.
    let no_slash = tok(&["read:kv"], iss.trim_end_matches('/'));
    assert_eq!(
        c.get(h.url("/v1/kv/greeting"))
            .bearer_auth(&no_slash)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    // TCP works the same way.
    let mut t = Tcp::connect(h.tcp).await;
    assert_eq!(
        t.cmd(&format!("AUTH {writer}")).await,
        "OK auth0|abc org_9ybsU1dN2dKfDkBi"
    );
    assert_eq!(t.cmd("GET greeting").await, "VALUE hi");
}

#[tokio::test]
async fn single_tenant_mode() {
    let h = Harness::start_with(Opts {
        mapping: cloud_cache::identity::Preset::Single.config(None),
        ..Opts::default()
    })
    .await;
    let c = reqwest::Client::new();
    let tok = |roles: &[&str], sub: &str| {
        h.sign(json!({"iss": ISS, "aud": AUD, "sub": sub, "exp": now() + 300, "roles": roles}))
    };
    // Two different users, no org claims at all, share one keyspace.
    c.put(h.url("/v1/kv/shared"))
        .bearer_auth(tok(&["kv-write"], "u1"))
        .body("one")
        .send()
        .await
        .unwrap();
    let r = c
        .get(h.url("/v1/kv/shared"))
        .bearer_auth(tok(&["kv-read"], "u2"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "one");
}

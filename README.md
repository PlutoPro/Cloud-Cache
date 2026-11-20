# Cloud Cache

A multi-tenant key-value store in Rust with two APIs over one shared store:

- **HTTP API** (`:8080`): REST, binary-safe values, plus `/healthz` and `/readyz` for load balancers
- **TCP API** (`:7379`): a newline-delimited text protocol for low-overhead clients

Both APIs authenticate with **OpenID Connect access tokens** from the identity provider you already run: Keycloak, Auth0, Microsoft Entra ID, Zitadel, or any other provider that issues JWTs. Tokens are verified locally against the provider's signing keys, so there's no call to the provider per request.

Data is partitioned by **organization**: every key belongs to the caller's org, and users in the same org share its keyspace. Or run it **single-tenant**, with one keyspace for everyone.

The repo has two parts:
- the Rust service (this folder)
- a Next.js web dashboard in [`dashboard/`](dashboard/README.md) for browsing keys, viewing usage and managing members

## Quick start

The demo stack uses Keycloak as the identity provider:

```bash
cargo test                       # unit + end-to-end tests (mock provider, real listeners)

cd deploy && docker compose up --build
# Dashboard:     http://localhost:3100  (alice/alice)
# HTTP API:      http://localhost:18080
# TCP API:       localhost:17379
# Keycloak:      http://localhost:18081 (admin/admin)
# HAProxy stats: http://localhost:18404/stats
../scripts/demo.sh               # gets tokens, exercises HTTP and TCP through the load balancer
```

Host ports are deliberately uncommon so the stack can run next to others. If one is still taken, copy `deploy/.env.example` to `deploy/.env` and change it. Keycloak's issuer URL and the dashboard's sign-in redirect follow automatically. Everything binds to `127.0.0.1` by default.

Run a single node against your own provider:

```bash
AUTH_PRESET=auth0 OIDC_ISSUER=https://your-tenant.us.auth0.com/ \
OIDC_AUDIENCE=https://api.your-app.example cargo run --release
```

## Identity providers

Authentication is split into two independent parts:

1. **Checking the token** (`src/auth.rs`) is standard OIDC and the same for every provider: signature against the provider's published keys, then `iss`, `aud` and `exp`.
2. **Reading the claims** (`src/identity.rs`) turns the token into roles, organizations and org-admin rights. Providers differ here, so this part is configurable.

### Connection settings

| Setting | Purpose |
|---|---|
| `OIDC_ISSUER` (required) | Expected `iss`, **exactly** as the provider writes it. Auth0's ends with `/`. |
| `OIDC_AUDIENCE` | Expected `aud`: your API's identifier or client ID at the provider. Unset = not checked (not recommended). |
| `OIDC_DISCOVERY_URL` | Where to read the provider's configuration. Default: `{issuer}/.well-known/openid-configuration`. |
| `OIDC_JWKS_URL` | Signing-keys URL. Default: read from discovery. Set it when nodes reach the provider on an internal hostname. |

### Presets

`AUTH_PRESET` picks where roles, orgs and admin rights come from:

| Preset | Roles from | Default role names (read / write / usage) | Organization | Org admin |
|---|---|---|---|---|
| `keycloak` (default) | `realm_access.roles` + `resource_access.<audience>.roles` | `kv-read` / `kv-write` / `kv-usage` | Keycloak Organizations (`organization` claim with ids) | in group `/org-admins/<alias>` |
| `auth0` | `permissions` (RBAC on your API) | `read:kv` / `write:kv` / `read:usage` | `org_id`, named by `org_name` | has permission `admin:kv` |
| `entra` | `roles` (app roles) | `kv-read` / `kv-write` / `kv-usage` | tenant (`tid`) | has app role `kv-admin` |
| `zitadel` | `urn:zitadel:iam:org:project:roles` | `kv-read` / `kv-write` / `kv-usage` | the user's organization (`urn:zitadel:iam:user:resourceowner:id`) | has role `kv-admin` |
| `single` | `roles` | `kv-read` / `kv-write` / `kv-usage` | none: one shared keyspace (`default`) | has role `kv-admin` |

The Keycloak preset is tested against a live Keycloak 26. The others are built from each provider's documented token formats and tested against tokens of that shape, not against the live services. Check them against a real token from your tenant before relying on them.

Provider notes:
- **Auth0:** turn on RBAC and "Add Permissions in the Access Token" for your API. Clients must request your API's `audience`.
- **Entra ID:** use v2.0 tokens. The issuer is `https://login.microsoftonline.com/<tenant-id>/v2.0`.
- **Zitadel:** request the scope `urn:zitadel:iam:user:resourceowner` so tokens carry the user's org, and enable roles in the access token.

### Overriding a preset

Any part of a preset can be overridden. That covers most other providers (Okta, Authentik, Ory, Cognito and so on) with no code:

| Variable | Meaning |
|---|---|
| `CLAIM_ROLES` | Comma-separated claims holding roles. A claim can be an array, a space-separated string (like `scope`), or an object whose keys are roles. `{aud}` is replaced by the audience. |
| `KV_READ_ROLE` / `KV_WRITE_ROLE` / `KV_USAGE_ROLE` | Role names. Write implies read. |
| `ORG_MODE` | `keycloak`, `claim` (one org id per token), `list` (array of org ids) or `fixed` (single-tenant) |
| `CLAIM_ORG` / `CLAIM_ORG_NAME` | Org id claim, and an optional display-name claim |
| `ORG_FIXED_ID` | Keyspace name in `fixed` mode (default `default`) |
| `CLAIM_ADMIN` / `ADMIN_MATCH` | Claim and value that mark an org admin. `{id}` and `{alias}` are substituted. A value with neither makes the caller admin of all their orgs. `ADMIN_MATCH=none` turns admin rights off. |
| `CLAIM_PRINCIPAL` | Comma-separated claims tried for the display name |

Claim paths follow nested objects with dots (`realm_access.roles`). A claim whose own name contains dots or colons, such as `https://example.com/roles` or `urn:zitadel:…`, is matched by its exact name first.

For example, a provider that puts roles and tenants in custom namespaced claims:

```bash
AUTH_PRESET=single
CLAIM_ROLES=https://cloud-cache.example/roles
ORG_MODE=list
CLAIM_ORG=https://cloud-cache.example/tenants
CLAIM_ADMIN=https://cloud-cache.example/admin_of
ADMIN_MATCH={id}
```

The node logs its active mapping at startup.

### In code

If configuration can't describe your provider, implement the `ClaimMapper` trait and start the server with `cloud_cache::run_with(config, Arc::new(MyMapper), …)`. The trait has one method: it receives the verified token's claims and returns the caller's name, permissions and orgs.

```rust
impl ClaimMapper for MyMapper {
    fn map(&self, claims: &serde_json::Map<String, Value>) -> Result<Mapped, String> { … }
}
```

## Organizations

- A user in one org doesn't need to do anything.
- A user in several orgs picks one per request: the HTTP header `X-Org-Id: <id or alias>`, or `AUTH <token> <id or alias>` on TCP. If they don't pick one, the request is refused (`400` / `ERR member of several organizations`).
- Naming an org you're not in gets `403`. A token with no org membership can't touch any data (`403`).
- Roles apply in whichever org is selected.
- `/v1/me` reports the caller's orgs and which ones they administer. The dashboard uses this for member management.

Keycloak specifics (already set up in `deploy/keycloak/kv-realm.json`):
1. Enable Organizations in the realm, create orgs and add members.
2. On the client users sign in with:
   - add an **Organization Membership** mapper with **Add organization id = ON**
   - add a **Group Membership** mapper (full path, claim `groups`) for admin groups
   - make the `organization` client scope a default scope

   Users in several orgs need the scope `organization:*` so their token lists all of them.
3. Machine clients (client credentials) aren't org members. Give each one a *Hardcoded claim* mapper: claim `organization`, JSON type, value `{"acme":{"id":"<org-id>"}}`. See `kv-worker`.
4. Keycloak 26.0 writes its built-in alias mapper and the id mapper into one list, like `[{"acme":{"id":"…"}}, "acme"]`. Cloud Cache accepts that, and the plain map that newer versions send.

The demo realm has two orgs, with fixed ids `6f1d2c3b-aaaa-4bbb-8ccc-00000000000{1,2}`:
- `acme`: alice (write, admin), bob (read) and dave
- `globex`: carol (write, admin) and dave

## Usage stats

`GET /v1/admin/usage` (needs the usage role) returns this node's counters per org:

```json
{"node":"kv1","started_at":1795129843,
 "orgs":[{"org_id":"6f1d…0001","bytes":45,"keys":3,"reads":2,"writes":3}]}
```

- `bytes` (key plus value bytes) and `keys` are current levels.
- `reads` and `writes` only increase from `started_at`. A changed `started_at` means the node restarted.
- Counters are **per node**: ask each node directly, not through the load balancer, and add them up. The dashboard does this.

## HTTP API

| Method | Path | Needs | Result |
|---|---|---|---|
| `GET` | `/v1/me` | any valid token | caller, permissions, orgs with `admin` flags |
| `GET` | `/v1/kv?prefix=&after=&limit=` | read | `{"keys":[{"key","size","ttl"}],"next"}`, sorted, on this node |
| `GET` | `/v1/kv/{key}` | read | `200` with the value bytes, or `404` |
| `PUT` | `/v1/kv/{key}?ttl=<secs>` | write | `204` (body = value, `ttl` optional) |
| `DELETE` | `/v1/kv/{key}` | write | `204` or `404` |
| `GET` | `/v1/admin/usage[?org=<id>]` | usage | per-org usage on this node |
| `GET` | `/healthz` | nothing | `200` while the process is alive (liveness) |
| `GET` | `/readyz` | nothing | `200` once signing keys are loaded and while not draining, else `503` |

Send `Authorization: Bearer <access_token>`, plus `X-Org-Id` if the user is in several orgs. A missing or invalid token gets `401` with `WWW-Authenticate: Bearer`. A valid token without the needed role gets `403`.

## TCP API

```text
PING                      -> PONG                (no auth)
AUTH <token> [org]        -> OK <user> <org_id> | ERR ...
GET <key>                 -> VALUE <value> | NIL
SET <key> <value>         -> OK
SETEX <key> <ttl> <value> -> OK
DEL <key>                 -> OK | NIL
QUIT                      -> BYE
```

- Values are the rest of the line, so spaces are fine but newlines aren't. A binary value written over HTTP returns `ERR binary value; use HTTP`.
- A session lasts as long as its token. After the token expires, commands return `ERR token expired` until the client sends `AUTH` again with a fresh one.
- All keys in a session belong to the org chosen at `AUTH`. To switch orgs, send `AUTH` again.
- Commands are case-insensitive. Keys can't contain whitespace and are at most 512 bytes (the same rule applies on HTTP).

```bash
TOKEN=$(curl -s localhost:18081/realms/kv/protocol/openid-connect/token \
  -d grant_type=password -d client_id=kv-cli -d username=alice -d password=alice | jq -r .access_token)
printf 'AUTH %s\nSET greeting hi there\nGET greeting\nQUIT\n' "$TOKEN" | nc localhost 17379
```

## Security

- Only asymmetric signing algorithms are accepted. `HS*` and `none` are rejected, which blocks algorithm-confusion attacks. Encryption keys a provider publishes (`use: enc`) are never used to check signatures.
- `iss`, `aud` and `exp` are checked, with 30 seconds of clock leeway.
- Signing keys refresh every 5 minutes. A token with an unknown key ID triggers an immediate refetch, rate-limited to once per 10 seconds, so junk tokens can't flood your provider.
- Keys are isolated per org. A token for one org can't read or list another org's keys.
- The demo realm's client secrets (`change-me-*`) and `admin/admin` are placeholders for local use only.

## Load balancer integration

- **Health:** point load-balancer checks at `GET /readyz`. A node reports ready only after it has loaded signing keys, so it never takes traffic it would reject.
- **Graceful drain:** on `SIGTERM`, `/readyz` returns `503` for `DRAIN_SECONDS` (default 5) while the node keeps serving. The load balancer marks it down, then the listeners close and TCP clients get `BYE shutting down`. Keep the check interval × fall count below the drain window.
- **Kubernetes:** use `/readyz` as the readiness probe and `/healthz` for liveness, with `terminationGracePeriodSeconds` greater than `DRAIN_SECONDS`.
- **HAProxy** (`deploy/haproxy/haproxy.cfg`):
  - HTTP is routed by a **consistent hash of the URL path**, so each key always lands on the same node.
  - TCP is sticky by **client source address**, because one connection carries many keys.
  - The TCP backend's health check uses the node's HTTP `/readyz`, so draining applies to both APIs.
  - Node names are re-resolved through Docker's DNS, so a restarted container with a new IP is picked up.
- It also works behind an AWS NLB/ALB, Envoy or nginx: use a readiness check on `/readyz`, an L7 target for `:8080` and an L4 target for `:7379`.

## Other settings

| Setting | Default |
|---|---|
| `NODE_ID` | `HOSTNAME` |
| `HTTP_ADDR` | `0.0.0.0:8080` |
| `TCP_ADDR` | `0.0.0.0:7379` |
| `MAX_VALUE_BYTES` | 1 MiB |
| `TCP_IDLE_SECONDS` | 300 |
| `DRAIN_SECONDS` | 5 |
| `RUST_LOG` | `info` |

## Known limits

- **Each node holds its own data in memory.** With several nodes, HTTP key-hashing keeps HTTP-only use consistent. A TCP client pinned to node B, though, won't see a key that HTTP put on node A. Fixes, roughly from least to most work:
  1. Run one active node with the others as hot standbys (`backup` in HAProxy).
  2. Have nodes forward a request to the key's owner using the same hash ring.
  3. Add replication, for example Raft via `openraft`.
- **Listing is per node.** The dashboard asks every node and merges the results; an API client listing through the load balancer only sees one node's keys.
- **No persistence yet.** Restarting a node loses its keys. An append-only log or snapshots would be the natural next step.
- **No per-org quotas yet.** The usage counters exist to build on.
- **TLS** is expected to end at the load balancer. For end-to-end encryption, add `rustls` on the listeners.

Contributions toward any of these are welcome.

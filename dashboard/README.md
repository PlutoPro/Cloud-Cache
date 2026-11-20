# Cloud Cache dashboard

The web console for Cloud Cache, built with Next.js 16. People sign in with your OpenID Connect provider and can:
- browse, create, edit and delete their organization's keys
- see usage (storage, keys, reads, writes) summed across every node
- get connection details for the HTTP and TCP APIs
- switch between organizations, if they belong to more than one
- if they're org admins, and member management is on: add or remove members and promote admins

## Run it

The easiest way is the full stack: `cd ../deploy && docker compose up --build`, then open http://localhost:3100. The port is set by `CC_DASHBOARD_PORT` in `deploy/.env`. Sign in as:
- `alice` / `alice`: admin of acme
- `bob`: read-only
- `carol`: admin of globex
- `dave`: in both orgs

On its own, with your provider and Cloud Cache already running:

```bash
cp .env.example .env.local   # fill in SESSION_SECRET, your provider and the URLs
npm install
npm run dev                  # or: npm run build && node .next/standalone/server.js
```

## Using your identity provider

The dashboard needs one **confidential client** at your provider, with:
- the authorization-code grant (with PKCE), for users signing in
- the client-credentials grant, so the dashboard's own service account can read usage. Give that service account the usage role (`kv-usage` by default, `read:usage` with the Auth0 preset).
- `APP_URL/auth/callback` registered as a redirect URI, and `APP_URL/` as the sign-out redirect

| Setting | Purpose |
|---|---|
| `OIDC_ISSUER` | Issuer exactly as it appears in tokens |
| `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET` | The dashboard's client |
| `OIDC_SCOPES` | Sign-in scopes. Default `openid`. Keycloak with Organizations: `openid organization:*`. Zitadel: add `urn:zitadel:iam:user:resourceowner`. |
| `OIDC_AUDIENCE` | Sent as `audience` (Auth0 needs it to issue tokens for your API) |
| `OIDC_INTERNAL_URL` | How this server reaches the provider, if that differs from the public URL (Docker) |
| `MEMBERS_PROVIDER` | `keycloak` to manage members here, or `none` (default) to manage them in your provider's console |

Endpoints come from the provider's discovery document. If the provider doesn't advertise a logout endpoint, signing out clears the dashboard session only.

## How it works

**Sign-in** uses the authorization-code flow with PKCE (`openid-client`).
- Tokens are kept in an encrypted, HTTP-only cookie, split across several cookies if large. The browser never sees them in readable form.
- `proxy.ts` refreshes the access token 30 seconds before it expires.

**Organization:** the selected org is stored in a cookie and checked against the user's orgs from Cloud Cache's `/v1/me` on every request. Requests send it to Cloud Cache as `X-Org-Id`.

**Data calls** run on the server (server components and server actions) with the user's own token, so Cloud Cache enforces the same roles and org isolation as for API clients:
- Reading or writing a single key goes through the load balancer.
- **Listing keys** asks every node in `CLOUD_CACHE_NODES` and merges the pages, because each node holds its own keys.

**Usage** is read with the dashboard's service account from every node, filtered to the current org, and summed. If that account can't get a token, or lacks the usage role, the page says which.

**Members** go through an adapter (`lib/members/`), so the page works with any provider that has one:
- `keycloak` uses Keycloak's admin API. Org admins are members of the group `/org-admins/<org alias>`, which is created on first use.
- Keycloak 26.0 requires `realm-management` → `manage-realm` for organization endpoints, so **treat `OIDC_CLIENT_SECRET` as an admin credential** with this adapter. Keep it server-side only.
- Before any change, the server re-checks that the signed-in user is an admin of the current org and that the target user is a member of it. An org can't lose its last admin.
- To add a provider, implement `MembersAdapter` (`list`, `addByEmail`, `remove`, `setAdmin`) against its management API and register it in `lib/members/index.ts`.

## Testing

`e2e/ui_test.py` drives Chrome through the demo realm. It:
- signs in through Keycloak
- creates, edits, filters and deletes keys
- checks usage
- adds, promotes, demotes and removes a member
- checks that a read-only user can't edit, and a non-admin can't open Members
- switches orgs as a two-org user
- renders the mobile layout
- signs out

It saves a screenshot of each page.

## Ideas for contributors

- Members adapters for Auth0 (Management API), Entra ID (Microsoft Graph) and Zitadel
- Creating API clients (client ID and secret) per org from the dashboard
- Inviting people who don't have an account yet

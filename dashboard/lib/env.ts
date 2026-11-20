import "server-only";

function required(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is not set (see .env.example)`);
  return v;
}

const list = (v: string | undefined, sep = ",") =>
  (v ?? "")
    .split(sep)
    .map((s) => s.trim())
    .filter(Boolean);

export type MembersProvider = "keycloak" | "none";

export const env = {
  appUrl: () => (process.env.APP_URL ?? "http://localhost:3100").replace(/\/$/, ""),
  sessionSecret: () => required("SESSION_SECRET"),

  // --- OpenID Connect provider (any: Keycloak, Auth0, Entra ID, Zitadel, Authentik, …) ---
  /** Issuer exactly as it appears in tokens (Auth0's ends with "/"). */
  issuer: () => required("OIDC_ISSUER"),
  /** Optional: how this server reaches the provider when that differs from the public URL (Docker). */
  oidcInternal: () => process.env.OIDC_INTERNAL_URL?.replace(/\/$/, "") || undefined,
  clientId: () => required("OIDC_CLIENT_ID"),
  clientSecret: () => required("OIDC_CLIENT_SECRET"),
  /** Scopes for user sign-in. Keycloak with Organizations: "openid organization:*". */
  scopes: () => process.env.OIDC_SCOPES?.trim() || "openid",
  /** Sent as `audience` on sign-in and service-token requests (Auth0 needs it to issue API tokens). */
  audience: () => process.env.OIDC_AUDIENCE || undefined,

  // --- Member management (optional) ---
  /** `keycloak` manages org members through Keycloak's admin API; `none` hides the Members page. */
  membersProvider: (): MembersProvider => {
    const v = (process.env.MEMBERS_PROVIDER ?? "none").toLowerCase();
    if (v !== "keycloak" && v !== "none") throw new Error(`MEMBERS_PROVIDER must be keycloak or none, not ${v}`);
    return v;
  },
  /** Keycloak group whose subgroups mark org admins (must match Cloud Cache's ADMIN_MATCH). */
  keycloakAdminGroup: () => process.env.KEYCLOAK_ADMIN_GROUP ?? "org-admins",

  // --- Cloud Cache ---
  apiUrl: () => required("CLOUD_CACHE_URL").replace(/\/$/, ""),
  nodes: () => {
    const n = list(process.env.CLOUD_CACHE_NODES).map((s) => s.replace(/\/$/, ""));
    return n.length ? n : [env.apiUrl()];
  },
  publicHttp: () => process.env.PUBLIC_HTTP_ENDPOINT ?? "http://localhost:18080",
  publicTcp: () => process.env.PUBLIC_TCP_ENDPOINT ?? "localhost:17379",
  secureCookies: () => env.appUrl().startsWith("https://"),
};

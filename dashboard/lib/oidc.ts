import "server-only";
import * as client from "openid-client";
import { env } from "./env";
import type { Session } from "./session";

/**
 * Fetch that reaches the identity provider on its internal address when this server can't use
 * the public one (e.g. Docker: browsers use localhost:18081, this server uses keycloak:8080).
 * URLs and the token `iss` stay public; only the network hop changes.
 */
export function providerFetch(url: string | URL, init?: RequestInit): Promise<Response> {
  const s = url.toString();
  const internal = env.oidcInternal();
  const publicOrigin = new URL(env.issuer()).origin;
  const target = internal && s.startsWith(publicOrigin) ? internal + s.slice(publicOrigin.length) : s;
  return fetch(target, { ...init, cache: "no-store" });
}

/** Browser-facing URLs must use the public origin, even if discovery returned internal ones. */
export function publicUrl(url: URL): URL {
  const internal = env.oidcInternal();
  if (internal && url.href.startsWith(internal)) {
    return new URL(new URL(env.issuer()).origin + url.href.slice(internal.length));
  }
  return url;
}

let configPromise: Promise<client.Configuration> | null = null;

export function oidc(): Promise<client.Configuration> {
  if (!configPromise) {
    configPromise = client
      .discovery(new URL(env.issuer()), env.clientId(), env.clientSecret(), undefined, {
        execute: env.issuer().startsWith("http://") ? [client.allowInsecureRequests] : [],
        [client.customFetch]: (url, options) => providerFetch(url, options as RequestInit),
      })
      .catch((e) => {
        configPromise = null; // retry discovery on the next request
        throw e;
      });
  }
  return configPromise;
}

/** Extra parameters for authorization and token requests (Auth0's `audience`). */
export function audienceParams(): Record<string, string> {
  const aud = env.audience();
  return aud ? { audience: aud } : {};
}

export function toSession(t: client.TokenEndpointResponse, previous?: Session): Session {
  return {
    at: t.access_token,
    rt: t.refresh_token ?? previous?.rt,
    atExp: Math.floor(Date.now() / 1000) + (t.expires_in ?? 60),
    idt: t.id_token ?? previous?.idt,
  };
}

export async function refresh(s: Session): Promise<Session | null> {
  if (!s.rt) return null;
  try {
    const t = await client.refreshTokenGrant(await oidc(), s.rt, { scope: env.scopes() });
    return toSession(t, s);
  } catch {
    return null; // refresh token expired or revoked: sign in again
  }
}

let serviceToken: { at: string; exp: number } | null = null;

/** The dashboard's own service-account token (usage across orgs, Keycloak admin API). */
export async function serviceAccessToken(): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  if (serviceToken && serviceToken.exp - 30 > now) return serviceToken.at;
  const t = await client.clientCredentialsGrant(await oidc(), audienceParams());
  serviceToken = { at: t.access_token, exp: now + (t.expires_in ?? 60) };
  return serviceToken.at;
}

import "server-only";
import { createHash } from "node:crypto";
import { EncryptJWT, jwtDecrypt } from "jose";
import { env } from "./env";

/** What we keep about a signed-in user. Tokens never reach the browser in readable form. */
export type Session = {
  /** Keycloak access token, forwarded to Cloud Cache. */
  at: string;
  /** Refresh token, used by proxy.ts before the access token expires. */
  rt?: string;
  /** Access token expiry, unix seconds. (Not `exp`: that's the sealed cookie's own JWT claim.) */
  atExp: number;
  /** ID token, sent as id_token_hint so Keycloak signs out without a confirmation page. */
  idt?: string;
};

/** Anything with Next's cookie get/set/delete shape (next/headers cookies(), NextResponse.cookies). */
export type CookieJar = {
  get(name: string): { value: string } | undefined;
  set(name: string, value: string, options?: Record<string, unknown>): unknown;
  delete(name: string): unknown;
};

const SESSION = "cc_session";
const CHUNK = 3800; // stay under the ~4 KB per-cookie browser limit
const MAX_CHUNKS = 6;
const SESSION_TTL = 12 * 60 * 60;

export const ORG_COOKIE = "cc_org";
export const LOGIN_COOKIE = "cc_login";

const key = () => createHash("sha256").update(env.sessionSecret()).digest();

export const cookieOptions = (maxAge: number) => ({
  httpOnly: true,
  sameSite: "lax" as const,
  secure: env.secureCookies(),
  path: "/",
  maxAge,
});

export async function seal(payload: Record<string, unknown>, ttlSeconds: number): Promise<string> {
  return new EncryptJWT(payload)
    .setProtectedHeader({ alg: "dir", enc: "A256GCM" })
    .setIssuedAt()
    .setExpirationTime(`${ttlSeconds}s`)
    .encrypt(key());
}

export async function unseal<T>(value: string | undefined): Promise<T | null> {
  if (!value) return null;
  try {
    const { payload } = await jwtDecrypt(value, key());
    return payload as T;
  } catch {
    return null;
  }
}

export async function readSession(jar: Pick<CookieJar, "get">): Promise<Session | null> {
  let joined = "";
  for (let i = 0; i < MAX_CHUNKS; i++) {
    const part = jar.get(`${SESSION}.${i}`)?.value;
    if (!part) break;
    joined += part;
  }
  const s = await unseal<Session>(joined);
  return s?.at && s.atExp ? { at: s.at, rt: s.rt, atExp: s.atExp, idt: s.idt } : null;
}

export async function writeSession(jar: CookieJar, s: Session | null): Promise<void> {
  const parts: string[] = [];
  if (s) {
    const sealed = await seal(s, SESSION_TTL);
    for (let i = 0; i < sealed.length; i += CHUNK) parts.push(sealed.slice(i, i + CHUNK));
    if (parts.length > MAX_CHUNKS) throw new Error("session too large for cookies");
  }
  for (let i = 0; i < MAX_CHUNKS; i++) {
    if (i < parts.length) jar.set(`${SESSION}.${i}`, parts[i], cookieOptions(SESSION_TTL));
    // Delete unconditionally: a response jar can't see what the browser holds.
    else jar.delete(`${SESSION}.${i}`);
  }
}

/** Cookie names this module manages (for building request Cookie headers in proxy.ts). */
export const sessionCookieNames = Array.from({ length: MAX_CHUNKS }, (_, i) => `${SESSION}.${i}`);

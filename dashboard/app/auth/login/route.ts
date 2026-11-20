import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import * as client from "openid-client";
import { env } from "@/lib/env";
import { audienceParams, oidc, publicUrl } from "@/lib/oidc";
import { LOGIN_COOKIE, cookieOptions, seal } from "@/lib/session";

/** Start the authorization-code + PKCE flow. */
export async function GET() {
  const config = await oidc();
  const verifier = client.randomPKCECodeVerifier();
  const state = client.randomState();
  const jar = await cookies();
  // Short-lived, encrypted: binds the callback to this browser.
  jar.set(LOGIN_COOKIE, await seal({ verifier, state }, 600), cookieOptions(600));
  const url = client.buildAuthorizationUrl(config, {
    redirect_uri: `${env.appUrl()}/auth/callback`,
    scope: env.scopes(),
    ...audienceParams(),
    state,
    code_challenge: await client.calculatePKCECodeChallenge(verifier),
    code_challenge_method: "S256",
  });
  redirect(publicUrl(url).href);
}

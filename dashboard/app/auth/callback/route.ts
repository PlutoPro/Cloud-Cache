import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import type { NextRequest } from "next/server";
import * as client from "openid-client";
import { env } from "@/lib/env";
import { oidc, toSession } from "@/lib/oidc";
import { LOGIN_COOKIE, unseal, writeSession } from "@/lib/session";

export async function GET(request: NextRequest) {
  const jar = await cookies();
  const login = await unseal<{ verifier: string; state: string }>(jar.get(LOGIN_COOKIE)?.value);
  jar.delete(LOGIN_COOKIE);
  if (!login) redirect("/?error=expired");

  // Rebuild the URL on the public origin so it matches the registered redirect URI,
  // even when this server sits behind a proxy.
  const current = new URL(`${env.appUrl()}/auth/callback${request.nextUrl.search}`);
  let tokens: client.TokenEndpointResponse;
  try {
    tokens = await client.authorizationCodeGrant(await oidc(), current, {
      pkceCodeVerifier: login.verifier,
      expectedState: login.state,
    });
  } catch (e) {
    console.error("login callback failed", e);
    redirect("/?error=login");
  }
  await writeSession(jar, toSession(tokens));
  redirect("/keys");
}

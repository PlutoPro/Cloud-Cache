import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import * as client from "openid-client";
import { env } from "@/lib/env";
import { oidc, publicUrl } from "@/lib/oidc";
import { ORG_COOKIE, readSession, writeSession } from "@/lib/session";

/** Clear our session, then end the Keycloak session too. POST so links can't sign people out. */
export async function POST() {
  const jar = await cookies();
  const session = await readSession(jar);
  await writeSession(jar, null);
  jar.delete(ORG_COOKIE);
  const config = await oidc();
  // Not every provider advertises a logout endpoint (Auth0 only when enabled); then just
  // clearing our session is all we can do.
  if (!config.serverMetadata().end_session_endpoint) redirect("/");
  const url = client.buildEndSessionUrl(config, {
    client_id: env.clientId(),
    post_logout_redirect_uri: `${env.appUrl()}/`,
    // With the ID token Keycloak ends the session straight away; without it, it asks first.
    ...(session?.idt ? { id_token_hint: session.idt } : {}),
  });
  redirect(publicUrl(url).href);
}

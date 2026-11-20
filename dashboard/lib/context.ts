import "server-only";
import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { cache } from "react";
import { CloudCacheError, me, type Me, type Org } from "./cloudcache";
import { ORG_COOKIE, readSession } from "./session";

export type Context = { token: string; me: Me; org: Org | null };

/**
 * The signed-in user, their organizations and the one they're working in.
 * Cached per request; sends people to sign in when there's no usable session.
 */
export const getContext = cache(async (): Promise<Context> => {
  const jar = await cookies();
  const session = await readSession(jar);
  if (!session) redirect("/auth/login");
  let who: Me;
  try {
    who = await me(session.at);
  } catch (e) {
    if (e instanceof CloudCacheError && e.status === 401) redirect("/auth/login");
    throw e;
  }
  const wanted = jar.get(ORG_COOKIE)?.value;
  const org = who.organizations.find((o) => o.id === wanted) ?? who.organizations[0] ?? null;
  return { token: session.at, me: who, org };
});

/** Same, but requires an org (and optionally admin rights in it). */
export async function requireOrg(opts: { admin?: boolean } = {}): Promise<Context & { org: Org }> {
  const ctx = await getContext();
  if (!ctx.org) redirect("/keys");
  if (opts.admin && !ctx.org.admin) redirect("/keys");
  return ctx as Context & { org: Org };
}

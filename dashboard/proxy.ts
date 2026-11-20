import { NextResponse, type NextRequest } from "next/server";
import { refresh } from "@/lib/oidc";
import { readSession, sessionCookieNames, writeSession, type CookieJar } from "@/lib/session";

/**
 * Refresh the access token shortly before it expires. Server components can't set cookies,
 * so this runs first: it rewrites the session cookies on the response (for the browser) and
 * on the forwarded request (so this very render sees the fresh token).
 */
export async function proxy(request: NextRequest) {
  const session = await readSession(request.cookies);
  if (!session || session.atExp - 30 > Date.now() / 1000) return NextResponse.next();

  const fresh = await refresh(session);
  const pending = new Map<string, string | null>();
  const jar: CookieJar = {
    get: (n) => request.cookies.get(n),
    set: (n, v) => void pending.set(n, v),
    delete: (n) => void pending.set(n, null),
  };
  await writeSession(jar, fresh);

  // Forward the request with the new cookie values.
  const forwarded = new Headers(request.headers);
  const cookies = request.cookies
    .getAll()
    .filter((c) => !sessionCookieNames.includes(c.name))
    .map((c) => `${c.name}=${c.value}`);
  for (const [n, v] of pending) if (v) cookies.push(`${n}=${v}`);
  forwarded.set("cookie", cookies.join("; "));

  const res = NextResponse.next({ request: { headers: forwarded } });
  await writeSession(res.cookies, fresh);
  return res;
}

export const config = {
  matcher: ["/((?!auth/|_next/|favicon.ico).*)"],
};

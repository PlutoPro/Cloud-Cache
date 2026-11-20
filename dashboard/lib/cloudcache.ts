import "server-only";
import { env } from "./env";
import { serviceAccessToken } from "./oidc";

export type Org = { id: string; alias: string; admin: boolean };
export type Me = {
  user: string;
  permissions: { read: boolean; write: boolean; usage: boolean };
  organizations: Org[];
};
export type KeyInfo = { key: string; size: number; ttl: number | null };
export type KeyPage = { keys: KeyInfo[]; next: string | null };
export type Usage = { bytes: number; keys: number; reads: number; writes: number };
export type NodeUsage = Usage & { node: string; startedAt: number; reachable: boolean };

export class CloudCacheError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

async function call(
  base: string,
  path: string,
  token: string,
  init: RequestInit & { org?: string } = {},
): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set("authorization", `Bearer ${token}`);
  if (init.org) headers.set("x-org-id", init.org);
  let res: Response;
  try {
    res = await fetch(`${base}${path}`, { ...init, headers, cache: "no-store" });
  } catch {
    throw new CloudCacheError(503, `Can't reach Cloud Cache at ${base}`);
  }
  if (!res.ok && res.status !== 404) {
    const body = await res.json().catch(() => ({}) as { error?: string });
    throw new CloudCacheError(res.status, body.error ?? res.statusText);
  }
  return res;
}

export async function me(token: string): Promise<Me> {
  return (await call(env.apiUrl(), "/v1/me", token)).json();
}

/**
 * Each node holds its own keys, so ask every node for a page and merge.
 * Single-key reads and writes go through the load balancer, which routes by key.
 */
export async function listKeys(
  token: string,
  org: string,
  prefix: string,
  after: string | undefined,
  limit = 50,
): Promise<KeyPage> {
  const q = new URLSearchParams({ prefix, limit: String(limit) });
  if (after) q.set("after", after);
  const pages: KeyPage[] = await Promise.all(
    env.nodes().map(async (node) => (await call(node, `/v1/kv?${q}`, token, { org })).json()),
  );
  const merged = new Map<string, KeyInfo>();
  for (const p of pages) for (const k of p.keys) merged.set(k.key, k);
  const sorted = [...merged.values()].sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  const keys = sorted.slice(0, limit);
  const more = sorted.length > limit || pages.some((p) => p.next);
  return { keys, next: more && keys.length ? keys[keys.length - 1].key : null };
}

export async function getValue(token: string, org: string, key: string): Promise<Uint8Array | null> {
  const res = await call(env.apiUrl(), `/v1/kv/${encodeURIComponent(key)}`, token, { org });
  if (res.status === 404) return null;
  return new Uint8Array(await res.arrayBuffer());
}

export async function putValue(
  token: string,
  org: string,
  key: string,
  value: string,
  ttl?: number,
): Promise<void> {
  const q = ttl ? `?ttl=${ttl}` : "";
  await call(env.apiUrl(), `/v1/kv/${encodeURIComponent(key)}${q}`, token, {
    org,
    method: "PUT",
    body: value,
  });
}

export async function deleteValue(token: string, org: string, key: string): Promise<void> {
  await call(env.apiUrl(), `/v1/kv/${encodeURIComponent(key)}`, token, { org, method: "DELETE" });
}

/** Current usage for one org, per node and summed, read with the dashboard's service account (role `kv-usage`). */
export class UsageUnavailable extends Error {}

export async function orgUsage(org: string): Promise<{ total: Usage; nodes: NodeUsage[] }> {
  let token: string;
  try {
    token = await serviceAccessToken();
  } catch (e) {
    throw new UsageUnavailable(
      `The dashboard couldn't get a service token from the identity provider (${(e as Error).message}). ` +
        "Check that its client allows the client-credentials grant.",
    );
  }
  const nodes = await Promise.all(
    env.nodes().map(async (base): Promise<NodeUsage> => {
      try {
        const r = await call(base, `/v1/admin/usage?org=${encodeURIComponent(org)}`, token);
        const body = await r.json();
        const u = body.orgs[0] ?? { bytes: 0, keys: 0, reads: 0, writes: 0 };
        return { ...u, node: body.node, startedAt: body.started_at, reachable: true };
      } catch (e) {
        if (e instanceof CloudCacheError && (e.status === 401 || e.status === 403)) {
          throw new UsageUnavailable(
            "The dashboard's service account isn't allowed to read usage. Give it the usage role " +
              "(KV_USAGE_ROLE on Cloud Cache, kv-usage by default).",
          );
        }
        return { bytes: 0, keys: 0, reads: 0, writes: 0, node: base, startedAt: 0, reachable: false };
      }
    }),
  );
  const total = nodes.reduce(
    (t, n) => ({
      bytes: t.bytes + n.bytes,
      keys: t.keys + n.keys,
      reads: t.reads + n.reads,
      writes: t.writes + n.writes,
    }),
    { bytes: 0, keys: 0, reads: 0, writes: 0 },
  );
  return { total, nodes };
}

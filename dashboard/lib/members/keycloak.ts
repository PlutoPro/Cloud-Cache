import "server-only";
import { env } from "../env";
import { providerFetch, serviceAccessToken } from "../oidc";
import type { Member, MembersAdapter, Org } from "./index";

/**
 * Members via Keycloak 26 Organizations and the admin REST API, called as the dashboard's
 * service account.
 *
 * - Membership is the Keycloak organization's member list.
 * - Admins are members of the group `/<KEYCLOAK_ADMIN_GROUP>/<org alias>` (default
 *   `/org-admins/<alias>`), created on first use. Cloud Cache reads the same groups from the token.
 *
 * Keycloak 26.0 requires realm-management `manage-realm` for organization endpoints, so the
 * dashboard's client secret is a powerful credential: keep it server-side only.
 */

function adminBase(): string {
  const issuer = new URL(env.issuer());
  const realm = issuer.pathname.split("/realms/")[1]?.replace(/\/$/, "");
  if (!realm) throw new Error("MEMBERS_PROVIDER=keycloak needs OIDC_ISSUER like https://host/realms/<realm>");
  return `${issuer.origin}/admin/realms/${realm}`;
}

async function admin(path: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set("authorization", `Bearer ${await serviceAccessToken()}`);
  if (init.body) headers.set("content-type", "application/json");
  const res = await providerFetch(`${adminBase()}${path}`, { ...init, headers });
  if (!res.ok && res.status !== 404 && res.status !== 409) {
    throw new Error(`Keycloak ${init.method ?? "GET"} ${path}: ${res.status}`);
  }
  return res;
}

type KcUser = { id: string; username: string; email?: string; firstName?: string; lastName?: string };

async function adminGroupId(alias: string, create: boolean): Promise<string | null> {
  const root = env.keycloakAdminGroup();
  const res = await admin(`/group-by-path/${root}/${encodeURIComponent(alias)}`);
  if (res.ok) return (await res.json()).id;
  if (!create) return null;
  let parent = await admin(`/group-by-path/${root}`);
  if (!parent.ok) {
    await admin(`/groups`, { method: "POST", body: JSON.stringify({ name: root }) });
    parent = await admin(`/group-by-path/${root}`);
  }
  const parentId = (await parent.json()).id;
  await admin(`/groups/${parentId}/children`, { method: "POST", body: JSON.stringify({ name: alias }) });
  return adminGroupId(alias, false);
}

async function setAdmin(org: Org, userId: string, makeAdmin: boolean): Promise<void> {
  const groupId = await adminGroupId(org.alias, makeAdmin);
  if (!groupId) return;
  await admin(`/users/${userId}/groups/${groupId}`, { method: makeAdmin ? "PUT" : "DELETE" });
}

export const keycloakMembers: MembersAdapter = {
  async list(org) {
    const [members, groupId] = await Promise.all([
      admin(`/organizations/${org.id}/members?first=0&max=500`).then((r) => r.json() as Promise<KcUser[]>),
      adminGroupId(org.alias, false),
    ]);
    const admins = new Set<string>(
      groupId
        ? ((await (await admin(`/groups/${groupId}/members?max=500`)).json()) as KcUser[]).map((u) => u.id)
        : [],
    );
    return members
      .map(
        (u): Member => ({
          id: u.id,
          username: u.username,
          email: u.email,
          name: [u.firstName, u.lastName].filter(Boolean).join(" ") || u.username,
          admin: admins.has(u.id),
        }),
      )
      .sort((a, b) => a.name.localeCompare(b.name));
  },

  async addByEmail(org, email) {
    const users: KcUser[] = await (await admin(`/users?email=${encodeURIComponent(email)}&exact=true`)).json();
    if (!users.length) return "not-found";
    const res = await admin(`/organizations/${org.id}/members`, {
      method: "POST",
      body: JSON.stringify(users[0].id),
    });
    return res.status === 409 ? "already" : "added";
  },

  async remove(org, userId) {
    await setAdmin(org, userId, false);
    await admin(`/organizations/${org.id}/members/${userId}`, { method: "DELETE" });
  },

  setAdmin,
};

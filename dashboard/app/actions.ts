"use server";

import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { revalidatePath } from "next/cache";
import { deleteValue, putValue } from "@/lib/cloudcache";
import { getContext, requireOrg } from "@/lib/context";
import { members as membersAdapter, type MembersAdapter } from "@/lib/members";
import { ORG_COOKIE, cookieOptions } from "@/lib/session";

export type FormState = { ok?: string; error?: string } | null;

const KEY_RE = /^[^\s\x00-\x1f\x7f]{1,512}$/;

export async function switchOrg(formData: FormData) {
  const id = String(formData.get("org") ?? "");
  const ctx = await getContext();
  if (!ctx.me.organizations.some((o) => o.id === id)) return;
  (await cookies()).set(ORG_COOKIE, id, cookieOptions(365 * 24 * 3600));
  redirect(String(formData.get("return") || "/keys"));
}

export async function saveKey(_: FormState, formData: FormData): Promise<FormState> {
  const { token, org, me } = await requireOrg();
  if (!me.permissions.write) return { error: "You have read-only access to this organization." };
  const key = String(formData.get("key") ?? "").trim();
  const value = String(formData.get("value") ?? "");
  const ttlRaw = String(formData.get("ttl") ?? "").trim();
  if (!KEY_RE.test(key)) return { error: "Keys are 1–512 characters with no spaces." };
  const ttl = ttlRaw ? Number(ttlRaw) : undefined;
  if (ttl !== undefined && (!Number.isInteger(ttl) || ttl <= 0)) {
    return { error: "Expiry must be a whole number of seconds, or empty to keep the key." };
  }
  try {
    await putValue(token, org.id, key, value, ttl);
  } catch (e) {
    return { error: `Couldn't save ${key}: ${(e as Error).message}` };
  }
  revalidatePath("/keys");
  return { ok: `Saved ${key}` };
}

export async function removeKey(formData: FormData) {
  const { token, org, me } = await requireOrg();
  if (!me.permissions.write) return;
  const key = String(formData.get("key") ?? "");
  await deleteValue(token, org.id, key);
  revalidatePath("/keys");
  redirect(`/keys?deleted=${encodeURIComponent(key)}`);
}

/** Member management is optional; refuse cleanly when no adapter is configured. */
function adapter(): MembersAdapter {
  const a = membersAdapter();
  if (!a) throw new Error("Member management isn't enabled (MEMBERS_PROVIDER=none).");
  return a;
}

export async function addMember(_: FormState, formData: FormData): Promise<FormState> {
  const { org } = await requireOrg({ admin: true });
  const a = membersAdapter();
  if (!a) return { error: "Member management isn't enabled on this server." };
  const email = String(formData.get("email") ?? "").trim().toLowerCase();
  if (!email.includes("@")) return { error: "Enter the person's email address." };
  const result = await a.addByEmail(org, email);
  revalidatePath("/members");
  if (result === "not-found") {
    return { error: `No account uses ${email}. Ask them to sign up first, then add them.` };
  }
  if (result === "already") return { ok: `${email} is already a member.` };
  return { ok: `Added ${email} to ${org.alias}.` };
}

/** Only act on people who are actually in the admin's current org. */
async function memberOfCurrentOrg(formData: FormData) {
  const ctx = await requireOrg({ admin: true });
  const userId = String(formData.get("user") ?? "");
  const members = await adapter().list(ctx.org);
  const member = members.find((m) => m.id === userId);
  if (!member) throw new Error("That person isn't a member of this organization.");
  return { ...ctx, member, members };
}

export async function changeAdmin(formData: FormData) {
  const { org, member, members } = await memberOfCurrentOrg(formData);
  const makeAdmin = formData.get("admin") === "true";
  // Never leave an org without an admin.
  if (!makeAdmin && member.admin && members.filter((m) => m.admin).length === 1) return;
  await adapter().setAdmin(org, member.id, makeAdmin);
  revalidatePath("/members");
}

export async function dropMember(formData: FormData) {
  const { org, member, members } = await memberOfCurrentOrg(formData);
  if (member.admin && members.filter((m) => m.admin).length === 1) return;
  await adapter().remove(org, member.id);
  revalidatePath("/members");
}

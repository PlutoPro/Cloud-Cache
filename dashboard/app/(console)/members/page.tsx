import { AddMember } from "@/components/AddMember";
import { PageHeader } from "@/components/PageHeader";
import { changeAdmin, dropMember } from "@/app/actions";
import { requireOrg } from "@/lib/context";
import { redirect } from "next/navigation";
import { members as membersAdapter } from "@/lib/members";

export const metadata = { title: "Members" };

export default async function MembersPage() {
  const { org } = await requireOrg({ admin: true });
  const adapter = membersAdapter();
  if (!adapter) redirect("/keys");
  const members = await adapter.list(org);
  const adminCount = members.filter((m) => m.admin).length;

  return (
    <>
      <PageHeader title="Members">
        People in <strong className="text-ink">{org.alias}</strong> share its keys. Admins can add
        and remove members. Changes apply the next time someone signs in.
      </PageHeader>

      <section className="max-w-3xl rounded border border-rule bg-paper p-5">
        <h2 className="text-lg font-extrabold mb-3">Add someone</h2>
        <AddMember />
      </section>

      <section className="mt-10 max-w-3xl">
        <h2 className="text-lg font-extrabold mb-3">
          {members.length} {members.length === 1 ? "member" : "members"}
        </h2>
        <ul className="rounded border border-rule bg-paper divide-y divide-rule">
          {members.map((m) => {
            const lastAdmin = m.admin && adminCount === 1;
            return (
              <li key={m.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
                <div className="grow min-w-48">
                  <p className="font-semibold">
                    {m.name}
                    {m.admin && (
                      <span className="ml-2 rounded bg-amber-soft px-1.5 py-0.5 text-xs font-semibold">Admin</span>
                    )}
                  </p>
                  <p className="text-sm text-slate">{m.email ?? m.username}</p>
                </div>
                <form action={changeAdmin}>
                  <input type="hidden" name="user" value={m.id} />
                  <input type="hidden" name="admin" value={m.admin ? "false" : "true"} />
                  <button
                    className="btn btn-quiet text-sm"
                    disabled={lastAdmin}
                    title={lastAdmin ? "Every organization needs at least one admin" : undefined}
                  >
                    {m.admin ? "Remove admin" : "Make admin"}
                  </button>
                </form>
                <form action={dropMember}>
                  <input type="hidden" name="user" value={m.id} />
                  <button
                    className="btn btn-danger text-sm"
                    disabled={lastAdmin}
                    title={lastAdmin ? "Make someone else an admin first" : undefined}
                  >
                    Remove member
                  </button>
                </form>
              </li>
            );
          })}
        </ul>
      </section>
    </>
  );
}

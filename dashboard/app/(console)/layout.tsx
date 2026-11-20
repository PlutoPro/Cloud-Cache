import { NavLink } from "@/components/NavLink";
import { OrgSwitcher } from "@/components/OrgSwitcher";
import { getContext } from "@/lib/context";
import { members } from "@/lib/members";

export default async function ConsoleLayout({ children }: { children: React.ReactNode }) {
  const { me, org } = await getContext();
  const membersEnabled = members() !== null;
  return (
    <div className="min-h-dvh md:grid md:grid-cols-[16rem_minmax(0,1fr)]">
      <aside className="bg-ink text-fog px-5 py-6 flex flex-col gap-8 md:min-h-dvh md:sticky md:top-0 md:max-h-dvh">
        <a href="/keys" className="font-extrabold tracking-tight no-underline">
          Cloud Cache
        </a>
        {org ? (
          <OrgSwitcher orgs={me.organizations} current={org} />
        ) : (
          <p className="text-sm text-fog/70">No organization yet</p>
        )}
        <nav aria-label="Main" className="flex flex-wrap md:flex-col gap-1">
          <NavLink href="/keys">Keys</NavLink>
          <NavLink href="/usage">Usage</NavLink>
          <NavLink href="/connect">Connect</NavLink>
          {org?.admin && membersEnabled && <NavLink href="/members">Members</NavLink>}
        </nav>
        <div className="md:mt-auto text-sm">
          <p className="text-fog/70">Signed in as</p>
          <p className="font-semibold break-words">{me.user}</p>
          <form action="/auth/logout" method="post" className="mt-2">
            <button className="underline text-fog/80 hover:text-fog">Sign out</button>
          </form>
        </div>
      </aside>
      <main className="px-5 py-8 md:px-12 md:py-10 max-w-6xl">
        {org ? (
          children
        ) : (
          <section className="max-w-prose">
            <h1 className="text-2xl font-extrabold">You&apos;re not in an organization yet</h1>
            <p className="mt-3 text-slate leading-relaxed">
              Keys belong to an organization. Ask an admin of your team&apos;s organization to
              add <strong>{me.user}</strong>, then sign out and back in.
            </p>
          </section>
        )}
      </main>
    </div>
  );
}

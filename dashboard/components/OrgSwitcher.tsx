"use client";

import { usePathname } from "next/navigation";
import { useRef } from "react";
import { switchOrg } from "@/app/actions";

type Org = { id: string; alias: string; admin: boolean };

export function OrgSwitcher({ orgs, current }: { orgs: Org[]; current: Org }) {
  const form = useRef<HTMLFormElement>(null);
  const path = usePathname();
  return (
    <div className="border-l-4 border-amber pl-3">
      <p className="text-xs text-fog/60">Organization</p>
      <p className="text-2xl font-extrabold leading-tight break-words">{current.alias}</p>
      <p className="text-xs text-fog/60 mt-0.5">{current.admin ? "You're an admin" : "Member"}</p>
      {orgs.length > 1 && (
        <form ref={form} action={switchOrg} className="mt-3">
          <input type="hidden" name="return" value={path} />
          <label className="sr-only" htmlFor="org-switch">
            Switch organization
          </label>
          <select
            id="org-switch"
            name="org"
            defaultValue={current.id}
            onChange={() => form.current?.requestSubmit()}
            className="w-full rounded bg-ink-soft text-fog border border-fog/20 px-2 py-1.5 text-sm"
          >
            {orgs.map((o) => (
              <option key={o.id} value={o.id}>
                {o.alias}
              </option>
            ))}
          </select>
          <noscript>
            <button className="btn btn-quiet mt-2 text-fog">Switch</button>
          </noscript>
        </form>
      )}
    </div>
  );
}

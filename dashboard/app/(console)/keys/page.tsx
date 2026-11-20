import Link from "next/link";
import { DeleteKey } from "@/components/DeleteKey";
import { KeyEditor } from "@/components/KeyEditor";
import { PageHeader, duration, format } from "@/components/PageHeader";
import { CloudCacheError, getValue, listKeys, type KeyPage } from "@/lib/cloudcache";
import { requireOrg } from "@/lib/context";

export const metadata = { title: "Keys" };

type Search = { prefix?: string; after?: string; key?: string; new?: string; deleted?: string };

function qs(base: Search, patch: Partial<Search>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries({ ...base, ...patch })) if (v) q.set(k, v);
  const s = q.toString();
  return s ? `/keys?${s}` : "/keys";
}

export default async function KeysPage({ searchParams }: { searchParams: Promise<Search> }) {
  const sp = await searchParams;
  const { token, org, me } = await requireOrg();
  const prefix = sp.prefix ?? "";

  let page: KeyPage = { keys: [], next: null };
  let error: string | null = null;
  try {
    page = await listKeys(token, org.id, prefix, sp.after);
  } catch (e) {
    error = e instanceof CloudCacheError ? e.message : "Couldn't load keys.";
  }

  const selected = sp.key;
  let value: string | null = null;
  let binary = 0;
  let missing = false;
  if (selected) {
    const raw = await getValue(token, org.id, selected).catch(() => null);
    if (!raw) missing = true;
    else {
      try {
        value = new TextDecoder("utf-8", { fatal: true }).decode(raw);
      } catch {
        binary = raw.length;
      }
    }
  }
  const panelOpen = Boolean(selected) || sp.new === "1";

  return (
    <>
      <PageHeader title="Keys">
        Everything stored for <strong className="text-ink">{org.alias}</strong>. Keys written over
        HTTP or TCP show up here.
      </PageHeader>

      {sp.deleted && (
        <p role="status" className="mb-6 rounded border border-rule bg-paper px-3 py-2">
          Deleted <span className="font-mono">{sp.deleted}</span>.
        </p>
      )}

      <div className={`grid gap-8 ${panelOpen ? "lg:grid-cols-[minmax(0,1fr)_minmax(0,26rem)]" : ""}`}>
        <section aria-label="Key list">
          <div className="flex flex-wrap items-end gap-3 mb-4">
            <form className="flex items-end gap-2 grow max-w-md" action="/keys">
              <div className="grow">
                <label htmlFor="prefix" className="block text-sm font-semibold mb-1">
                  Keys starting with
                </label>
                <input id="prefix" name="prefix" defaultValue={prefix} placeholder="user:" className="field font-mono" />
              </div>
              <button className="btn btn-quiet">Filter</button>
            </form>
            {me.permissions.write && (
              <Link href={qs({ prefix }, { new: "1" })} className="btn btn-primary no-underline">
                New key
              </Link>
            )}
          </div>

          {error ? (
            <p role="alert" className="rounded border border-alert/30 bg-alert-soft px-3 py-2 text-alert">
              {error}
            </p>
          ) : page.keys.length === 0 ? (
            <div className="rounded border border-dashed border-rule px-5 py-8 text-slate">
              {prefix || sp.after ? (
                <>
                  No keys start with <span className="font-mono text-ink">{prefix}</span>.{" "}
                  <Link href="/keys" className="text-ink underline">
                    Show all keys
                  </Link>
                </>
              ) : (
                <>
                  No keys yet. Create one here, or write one from your app using the endpoints on{" "}
                  <Link href="/connect" className="text-ink underline">
                    Connect
                  </Link>
                  .
                </>
              )}
            </div>
          ) : (
            <div className="overflow-x-auto rounded border border-rule bg-paper">
              <table className="w-full text-left">
                <thead className="text-sm text-slate border-b border-rule">
                  <tr>
                    <th className="px-4 py-2 font-semibold">Key</th>
                    <th className="px-4 py-2 font-semibold text-right">Size</th>
                    <th className="px-4 py-2 font-semibold text-right">Expires in</th>
                  </tr>
                </thead>
                <tbody>
                  {page.keys.map((k) => {
                    const active = k.key === selected;
                    return (
                      <tr key={k.key} className={`border-b border-rule last:border-0 ${active ? "bg-amber-soft" : "hover:bg-fog/60"}`}>
                        <td className="px-4 py-2 font-mono text-sm break-all">
                          <Link
                            href={qs({ prefix, after: sp.after }, { key: k.key })}
                            aria-current={active ? "true" : undefined}
                            className="no-underline hover:underline"
                          >
                            {k.key}
                          </Link>
                        </td>
                        <td className="px-4 py-2 text-right text-sm whitespace-nowrap">{format(k.size)}</td>
                        <td className="px-4 py-2 text-right text-sm text-slate whitespace-nowrap">
                          {k.ttl === null ? "Never" : duration(k.ttl)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}

          <nav aria-label="Pages" className="mt-4 flex gap-3">
            {sp.after && (
              <Link href={qs({ prefix }, {})} className="btn btn-quiet no-underline">
                First page
              </Link>
            )}
            {page.next && (
              <Link href={qs({ prefix }, { after: page.next })} className="btn btn-quiet no-underline">
                Next page
              </Link>
            )}
          </nav>
        </section>

        {panelOpen && (
          <aside aria-label="Key details" className="rounded border border-rule bg-paper p-5 self-start lg:sticky lg:top-6">
            <div className="flex items-start justify-between gap-4 mb-4">
              <h2 className="text-lg font-extrabold">{selected ? "Edit key" : "New key"}</h2>
              <Link href={qs({ prefix, after: sp.after }, {})} className="text-sm text-slate underline">
                Close
              </Link>
            </div>
            {selected && missing ? (
              <p className="text-slate">
                <span className="font-mono text-ink">{selected}</span> no longer exists. It may have
                expired or been deleted.
              </p>
            ) : binary ? (
              <p className="text-slate">
                <span className="font-mono text-ink">{selected}</span> holds {format(binary)} of binary
                data, which can&apos;t be edited as text. Read it with the HTTP API.
              </p>
            ) : (
              <KeyEditor
                key={selected ?? "new"}
                initialKey={selected ?? prefix}
                value={value}
                isNew={!selected}
                canWrite={me.permissions.write}
              />
            )}
            {selected && !missing && me.permissions.write && (
              <div className="mt-6 pt-4 border-t border-rule">
                <DeleteKey keyName={selected} />
              </div>
            )}
          </aside>
        )}
      </div>
    </>
  );
}

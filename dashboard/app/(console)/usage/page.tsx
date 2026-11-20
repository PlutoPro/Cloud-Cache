import { PageHeader, format } from "@/components/PageHeader";
import { requireOrg } from "@/lib/context";
import { UsageUnavailable, orgUsage } from "@/lib/cloudcache";

export const metadata = { title: "Usage" };

function Readout({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div>
      <p className="text-sm font-semibold mb-1.5">{label}</p>
      <p className="readout rounded px-4 py-3 text-2xl md:text-3xl leading-none text-right">{value}</p>
      <p className="text-xs text-slate mt-1.5">{note}</p>
    </div>
  );
}

const n = new Intl.NumberFormat("en-US");

export default async function UsagePage() {
  const { org } = await requireOrg();
  let usage: Awaited<ReturnType<typeof orgUsage>>;
  try {
    usage = await orgUsage(org.id);
  } catch (e) {
    if (!(e instanceof UsageUnavailable)) throw e;
    return (
      <>
        <PageHeader title="Usage" />
        <p role="alert" className="max-w-prose rounded border border-alert/30 bg-alert-soft px-3 py-2 text-alert">
          {e.message}
        </p>
      </>
    );
  }
  const { total, nodes } = usage;
  const down = nodes.filter((x) => !x.reachable);

  return (
    <>
      <PageHeader title="Usage">
        What <strong className="text-ink">{org.alias}</strong> is using right now, added up across
        every server.
      </PageHeader>

      {down.length > 0 && (
        <p role="alert" className="mb-6 rounded border border-alert/30 bg-alert-soft px-3 py-2 text-alert">
          {down.length} of {nodes.length} servers didn&apos;t answer, so these numbers are low.
        </p>
      )}

      <div className="grid gap-6 sm:grid-cols-2 xl:grid-cols-4 max-w-5xl">
        <Readout label="Stored" value={format(total.bytes)} note="Keys and values, right now" />
        <Readout label="Keys" value={n.format(total.keys)} note="Including keys set to expire" />
        <Readout label="Reads" value={n.format(total.reads)} note="GET commands, any outcome" />
        <Readout label="Writes" value={n.format(total.writes)} note="SET, SETEX, PUT and DELETE" />
      </div>

      <section className="mt-12 max-w-3xl">
        <h2 className="text-lg font-extrabold">By server</h2>
        <p className="text-sm text-slate mt-1 mb-3 max-w-[65ch]">
          Read and write counts start over when a server restarts, so totals cover the time since
          the most recent restart shown here.
        </p>
        <div className="overflow-x-auto rounded border border-rule bg-paper">
          <table className="w-full text-left text-sm">
            <thead className="text-slate border-b border-rule">
              <tr>
                <th className="px-4 py-2 font-semibold">Server</th>
                <th className="px-4 py-2 font-semibold text-right">Stored</th>
                <th className="px-4 py-2 font-semibold text-right">Keys</th>
                <th className="px-4 py-2 font-semibold text-right">Reads</th>
                <th className="px-4 py-2 font-semibold text-right">Writes</th>
                <th className="px-4 py-2 font-semibold">Counting since</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((x) => (
                <tr key={x.node} className="border-b border-rule last:border-0">
                  <td className="px-4 py-2 font-mono">{x.node}</td>
                  {x.reachable ? (
                    <>
                      <td className="px-4 py-2 text-right">{format(x.bytes)}</td>
                      <td className="px-4 py-2 text-right">{n.format(x.keys)}</td>
                      <td className="px-4 py-2 text-right">{n.format(x.reads)}</td>
                      <td className="px-4 py-2 text-right">{n.format(x.writes)}</td>
                      <td className="px-4 py-2 text-slate whitespace-nowrap">
                        {new Date(x.startedAt * 1000).toLocaleString("en-US", { dateStyle: "medium", timeStyle: "short" })}
                      </td>
                    </>
                  ) : (
                    <td colSpan={5} className="px-4 py-2 text-alert">Not responding</td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </>
  );
}

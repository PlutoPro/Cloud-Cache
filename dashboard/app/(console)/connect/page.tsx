import { PageHeader } from "@/components/PageHeader";
import { requireOrg } from "@/lib/context";
import { env } from "@/lib/env";
import { oidc } from "@/lib/oidc";

export const metadata = { title: "Connect" };

function Code({ label, children }: { label: string; children: string }) {
  return (
    <figure className="mt-4">
      <figcaption className="text-sm font-semibold mb-1.5">{label}</figcaption>
      <pre className="overflow-x-auto rounded bg-ink text-fog px-4 py-3 text-sm leading-relaxed font-mono">
        {children}
      </pre>
    </figure>
  );
}

export default async function ConnectPage() {
  const { org, me } = await requireOrg();
  const http = env.publicHttp();
  const tcp = env.publicTcp();
  const [host, port] = tcp.split(":");
  const tokenUrl = (await oidc()).serverMetadata().token_endpoint ?? "(see your provider's discovery document)";
  const multi = me.organizations.length > 1;

  return (
    <>
      <PageHeader title="Connect">
        Point your app at these endpoints. Every request carries an access token from your
        organization&apos;s sign-in, and keys always land in <strong className="text-ink">{org.alias}</strong>.
      </PageHeader>

      <dl className="grid sm:grid-cols-[10rem_minmax(0,1fr)] gap-x-6 gap-y-3 max-w-3xl rounded border border-rule bg-paper p-5">
        <dt className="font-semibold">HTTP API</dt>
        <dd className="font-mono break-all">{http}</dd>
        <dt className="font-semibold">TCP API</dt>
        <dd className="font-mono break-all">{tcp}</dd>
        <dt className="font-semibold">Organization ID</dt>
        <dd className="font-mono break-all">{org.id}</dd>
        <dt className="font-semibold">Token endpoint</dt>
        <dd className="font-mono break-all">{tokenUrl}</dd>
      </dl>

      <section className="mt-10 max-w-3xl">
        <h2 className="text-lg font-extrabold">Get a token</h2>
        <p className="text-slate mt-1 max-w-[65ch] leading-relaxed">
          Services use a client ID and secret issued for {org.alias} by your admin. Tokens last a few
          minutes, so request a new one when it expires.
        </p>
        <Code label="Client credentials">{`TOKEN=$(curl -s ${tokenUrl} \\
  -d grant_type=client_credentials \\
  -d client_id=YOUR_CLIENT_ID -d client_secret=YOUR_SECRET \\${
    env.audience() ? `\n  -d audience=${env.audience()} \\` : ""
  }
  | jq -r .access_token)`}</Code>
      </section>

      <section className="mt-10 max-w-3xl">
        <h2 className="text-lg font-extrabold">HTTP</h2>
        <Code label="Store, read and delete a key">{`curl -X PUT "${http}/v1/kv/session:42?ttl=3600" \\
  -H "Authorization: Bearer $TOKEN"${multi ? ` -H "X-Org-Id: ${org.alias}"` : ""} \\
  --data 'signed in'

curl "${http}/v1/kv/session:42" -H "Authorization: Bearer $TOKEN"${multi ? ` -H "X-Org-Id: ${org.alias}"` : ""}

curl -X DELETE "${http}/v1/kv/session:42" -H "Authorization: Bearer $TOKEN"${multi ? ` -H "X-Org-Id: ${org.alias}"` : ""}`}</Code>
        {multi && (
          <p className="text-sm text-slate mt-2">
            You belong to more than one organization, so requests name one with <span className="font-mono text-ink">X-Org-Id</span>.
            Service tokens issued for a single organization don&apos;t need it.
          </p>
        )}
      </section>

      <section className="mt-10 max-w-3xl">
        <h2 className="text-lg font-extrabold">TCP</h2>
        <p className="text-slate mt-1 max-w-[65ch] leading-relaxed">
          One command per line. Sign in once per connection, then run commands until the token expires.
        </p>
        <Code label="Commands">{`AUTH <token>${multi ? ` ${org.alias}` : ""}
SET <key> <value>
SETEX <key> <seconds> <value>
GET <key>
DEL <key>
PING
QUIT`}</Code>
        <Code label="Try it with netcat">{`printf 'AUTH %s${multi ? ` ${org.alias}` : ""}\\nSET greeting hello\\nGET greeting\\nQUIT\\n' "$TOKEN" \\
  | nc ${host} ${port ?? "7379"}`}</Code>
      </section>
    </>
  );
}

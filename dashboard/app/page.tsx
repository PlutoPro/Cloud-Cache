import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { readSession } from "@/lib/session";

const errors: Record<string, string> = {
  expired: "Your sign-in took too long. Try again.",
  login: "Sign-in didn't complete. Try again, and contact your admin if it keeps failing.",
};

export default async function Home({ searchParams }: { searchParams: Promise<{ error?: string }> }) {
  if (await readSession(await cookies())) redirect("/keys");
  const { error } = await searchParams;
  return (
    <main className="min-h-dvh grid md:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)]">
      <section className="bg-ink text-fog px-8 py-12 md:px-14 flex flex-col justify-between gap-16">
        <p className="font-extrabold text-lg tracking-tight">Cloud Cache</p>
        <div>
          <pre aria-label="Example TCP session" className="readout inline-block rounded px-5 py-4 text-base md:text-xl leading-relaxed">
{`AUTH eyJhbGci… acme
OK alice acme
SET session:42 signed in
OK
GET session:42
VALUE signed in`}
          </pre>
          <h1 className="mt-8 text-2xl md:text-3xl font-extrabold leading-tight max-w-[18ch]">
            Your team&apos;s key-value store, over HTTP and TCP.
          </h1>
          <p className="mt-4 text-fog/75 max-w-[46ch] leading-relaxed">
            Sign in with your organization account to browse keys, see what you&apos;re using,
            and manage who has access.
          </p>
        </div>
        <p className="text-sm text-fog/60">Open source. Runs on your own servers.</p>
      </section>
      <section className="flex items-center px-8 py-12 md:px-16">
        <div className="max-w-sm w-full">
          <h2 className="text-xl font-extrabold">Sign in</h2>
          <p className="mt-2 text-slate">You&apos;ll continue to your organization&apos;s sign-in page.</p>
          {error && (
            <p role="alert" className="mt-5 rounded border border-alert/30 bg-alert-soft px-3 py-2 text-alert">
              {errors[error] ?? errors.login}
            </p>
          )}
          <a href="/auth/login" className="btn btn-primary mt-6 w-full justify-center py-3 text-lg">
            Sign in to Cloud Cache
          </a>
        </div>
      </section>
    </main>
  );
}

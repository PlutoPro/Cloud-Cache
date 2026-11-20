"use client";

import { useActionState } from "react";
import { saveKey, type FormState } from "@/app/actions";

export function KeyEditor({
  initialKey,
  value,
  isNew,
  canWrite,
}: {
  initialKey: string;
  value: string | null;
  isNew: boolean;
  canWrite: boolean;
}) {
  const [state, action, pending] = useActionState<FormState, FormData>(saveKey, null);
  return (
    <form action={action} className="flex flex-col gap-4">
      <div>
        <label htmlFor="key" className="block text-sm font-semibold mb-1">
          Key
        </label>
        <input
          id="key"
          name="key"
          defaultValue={initialKey}
          readOnly={!isNew}
          required
          maxLength={512}
          pattern="[^\s]+"
          title="No spaces"
          placeholder="session:42"
          className={`field font-mono ${isNew ? "" : "bg-fog"}`}
        />
      </div>
      <div>
        <label htmlFor="value" className="block text-sm font-semibold mb-1">
          Value
        </label>
        <textarea
          id="value"
          name="value"
          defaultValue={value ?? ""}
          rows={10}
          readOnly={!canWrite}
          className="field font-mono text-sm leading-relaxed"
        />
      </div>
      <div>
        <label htmlFor="ttl" className="block text-sm font-semibold mb-1">
          Expires after (seconds)
        </label>
        <input
          id="ttl"
          name="ttl"
          inputMode="numeric"
          pattern="[0-9]*"
          placeholder="Never"
          className="field w-40"
          aria-describedby="ttl-help"
        />
        <p id="ttl-help" className="text-xs text-slate mt-1">
          Saving resets the expiry. Leave empty to keep the key until it&apos;s deleted.
        </p>
      </div>
      {canWrite ? (
        <div className="flex items-center gap-3">
          <button className="btn btn-primary" disabled={pending}>
            {pending ? "Saving…" : isNew ? "Create key" : "Save changes"}
          </button>
          <p aria-live="polite" className={state?.error ? "text-alert text-sm" : "text-ok text-sm"}>
            {state?.error ?? state?.ok}
          </p>
        </div>
      ) : (
        <p className="text-sm text-slate">You have read-only access, so you can view keys but not change them.</p>
      )}
    </form>
  );
}

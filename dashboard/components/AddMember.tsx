"use client";

import { useActionState } from "react";
import { addMember, type FormState } from "@/app/actions";

export function AddMember() {
  const [state, action, pending] = useActionState<FormState, FormData>(addMember, null);
  return (
    <form action={action} className="flex flex-wrap items-end gap-3">
      <div className="grow max-w-sm">
        <label htmlFor="email" className="block text-sm font-semibold mb-1">
          Email address
        </label>
        <input id="email" name="email" type="email" required placeholder="name@company.com" className="field" />
      </div>
      <button className="btn btn-primary" disabled={pending}>
        {pending ? "Adding…" : "Add member"}
      </button>
      <p aria-live="polite" className={`basis-full text-sm ${state?.error ? "text-alert" : "text-ok"}`}>
        {state?.error ?? state?.ok}
      </p>
    </form>
  );
}

"use client";

import { removeKey } from "@/app/actions";

export function DeleteKey({ keyName }: { keyName: string }) {
  return (
    <form
      action={removeKey}
      onSubmit={(e) => {
        if (!confirm(`Delete ${keyName}? This can't be undone.`)) e.preventDefault();
      }}
    >
      <input type="hidden" name="key" value={keyName} />
      <button className="btn btn-danger">Delete key</button>
    </form>
  );
}

"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

export function NavLink({ href, children }: { href: string; children: React.ReactNode }) {
  const active = usePathname().startsWith(href);
  return (
    <Link
      href={href}
      aria-current={active ? "page" : undefined}
      className={`block rounded px-3 py-2 font-semibold no-underline ${
        active ? "bg-fog text-ink" : "text-fog/80 hover:text-fog hover:bg-ink-soft"
      }`}
    >
      {children}
    </Link>
  );
}

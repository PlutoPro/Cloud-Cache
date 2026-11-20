import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: { default: "Cloud Cache", template: "%s · Cloud Cache" },
  description: "Manage your Cloud Cache keys and team.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body className="min-h-dvh">{children}</body>
    </html>
  );
}

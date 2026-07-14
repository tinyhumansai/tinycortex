import type { Metadata } from "next";
import "./globals.css";
import { Nav } from "./nav";
import { listChunks, listEntities, listSkillDocs, workspacePath } from "@/lib/memory";

export const metadata: Metadata = {
  title: "TinyCortex Memory Viewer",
  description: "Local debug viewer for a TinyCortex memory workspace",
};

// Always read fresh from disk — this is a live debug tool over local files.
export const dynamic = "force-dynamic";

export default function RootLayout({ children }: { children: React.ReactNode }) {
  const ws = workspacePath();
  const docs = ws ? listSkillDocs(ws) : [];
  const chunks = ws ? listChunks(ws, 1) : { total: 0 };
  const entities = ws ? listEntities(ws) : [];

  return (
    <html lang="en">
      <body>
        <div className="shell">
          <aside className="sidebar">
            <div className="brand">
              TinyCortex
              <small>memory viewer</small>
            </div>
            <Nav
              items={[
                { href: "/", label: "Overview" },
                { href: "/docs", label: "Skill docs", count: docs.length },
                { href: "/tree", label: "Memory tree", count: chunks.total },
                { href: "/entities", label: "Entities", count: entities.length },
                { href: "/runs", label: "Sync runs" },
              ]}
            />
          </aside>
          <main className="main">{children}</main>
        </div>
      </body>
    </html>
  );
}

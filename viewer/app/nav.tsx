"use client";

import { usePathname } from "next/navigation";
import Link from "next/link";

type Item = { href: string; label: string; count?: number };

export function Nav({ items }: { items: Item[] }) {
  const pathname = usePathname();
  return (
    <nav className="nav">
      {items.map((item) => {
        const active =
          item.href === "/" ? pathname === "/" : pathname.startsWith(item.href);
        return (
          <Link key={item.href} href={item.href} className={active ? "active" : ""}>
            <span>{item.label}</span>
            {typeof item.count === "number" && <span className="pill">{item.count}</span>}
          </Link>
        );
      })}
    </nav>
  );
}

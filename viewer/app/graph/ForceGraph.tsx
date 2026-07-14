"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  type Simulation,
} from "d3-force";
import type { GraphEdge, GraphNode, GraphNodeKind } from "@/lib/memory";

type SimNode = GraphNode & {
  x: number;
  y: number;
  vx?: number;
  vy?: number;
  fx?: number | null;
  fy?: number | null;
};
type SimLink = { source: SimNode | string; target: SimNode | string };

const LEVEL_PALETTE = [
  "#7C3AED",
  "#4A83DD",
  "#1FB6C7",
  "#34C77B",
  "#E8A653",
  "#E0654A",
  "#C026D3",
];

function color(node: GraphNode): string {
  switch (node.kind) {
    case "root":
      return "#8B5CF6";
    case "source":
      return "#F97316";
    case "chunk":
      return "#94A3B8";
    case "summary":
      return LEVEL_PALETTE[(node.level ?? 0) % LEVEL_PALETTE.length];
  }
}

function radius(node: GraphNode): number {
  switch (node.kind) {
    case "root":
      return 18;
    case "source":
      return 14;
    case "chunk":
      return 3.5;
    case "summary":
      return Math.min(5 + (node.level ?? 0) * 2.5, 13);
  }
}

const LABEL_KINDS: Set<GraphNodeKind> = new Set(["root", "source"]);

export function ForceGraph({ nodes, edges }: { nodes: GraphNode[]; edges: GraphEdge[] }) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 900, h: 620 });
  const [, setTick] = useState(0);
  const [view, setView] = useState({ x: 0, y: 0, k: 0.85 });
  const [hover, setHover] = useState<string | null>(null);
  const [mounted, setMounted] = useState(false);
  const simRef = useRef<Simulation<SimNode, undefined> | null>(null);
  const dragRef = useRef<{ id: string } | null>(null);
  const panRef = useRef<{ x: number; y: number; vx: number; vy: number } | null>(null);
  const sizeRef = useRef(size);
  const fittedRef = useRef(false);
  const fitRef = useRef<() => void>(() => {});

  // Client-only render gate: node positions are computed by the simulation, so
  // rendering them during SSR would hydration-mismatch on float precision.
  useEffect(() => setMounted(true), []);

  // Build simulation nodes/links once per data set. Seed positions on a ring so
  // the initial layout is deterministic (no SSR/hydration mismatch).
  const { simNodes, simLinks, byId } = useMemo(() => {
    const simNodes: SimNode[] = nodes.map((n, i) => {
      const a = (i / Math.max(nodes.length, 1)) * Math.PI * 2;
      const r = n.kind === "root" ? 0 : 60 + (i % 7) * 22;
      return { ...n, x: Math.cos(a) * r, y: Math.sin(a) * r };
    });
    const byId = new Map(simNodes.map((n) => [n.id, n]));
    const simLinks: SimLink[] = edges
      .filter((e) => byId.has(e.from) && byId.has(e.to))
      .map((e) => ({ source: e.from, target: e.to }));
    return { simNodes, simLinks, byId };
  }, [nodes, edges]);

  // Fit the whole graph into view (framed with padding). Kept in a ref so the
  // simulation's tick handler can call the latest version.
  fitRef.current = () => {
    const ns = simNodes;
    if (ns.length === 0) return;
    let minX = Infinity,
      minY = Infinity,
      maxX = -Infinity,
      maxY = -Infinity;
    for (const n of ns) {
      if (n.x < minX) minX = n.x;
      if (n.x > maxX) maxX = n.x;
      if (n.y < minY) minY = n.y;
      if (n.y > maxY) maxY = n.y;
    }
    const { w, h } = sizeRef.current;
    const pad = 70;
    const gw = Math.max(maxX - minX, 1);
    const gh = Math.max(maxY - minY, 1);
    const k = Math.min(2.5, Math.max(0.12, Math.min((w - pad) / gw, (h - pad) / gh)));
    setView({ k, x: w / 2 - (k * (minX + maxX)) / 2, y: h / 2 - (k * (minY + maxY)) / 2 });
  };

  // Track container size.
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const apply = () => {
      const s = { w: el.clientWidth, h: el.clientHeight };
      sizeRef.current = s;
      setSize(s);
    };
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    apply();
    return () => ro.disconnect();
  }, []);

  // Run the force simulation.
  useEffect(() => {
    fittedRef.current = false;
    const sim = forceSimulation(simNodes)
      .force(
        "charge",
        forceManyBody<SimNode>().strength((d) =>
          d.kind === "root" ? -600 : d.kind === "source" ? -300 : d.kind === "summary" ? -140 : -50,
        ),
      )
      .force(
        "link",
        forceLink<SimNode, SimLink>(simLinks)
          .id((d) => d.id)
          .distance((l) => {
            const t = (l.source as SimNode).kind;
            return t === "root" ? 90 : t === "source" ? 46 : 26;
          })
          .strength(0.75),
      )
      .force("center", forceCenter(0, 0).strength(0.06))
      .force("collide", forceCollide<SimNode>().radius((d) => radius(d) + 3))
      .on("tick", () => {
        setTick((t) => (t + 1) % 1_000_000);
        // Auto-fit once the layout has cooled enough to be stable.
        if (!fittedRef.current && sim.alpha() < 0.12) {
          fittedRef.current = true;
          fitRef.current();
        }
      });
    simRef.current = sim;
    return () => {
      sim.stop();
    };
  }, [simNodes, simLinks]);

  // ── Pan / zoom ─────────────────────────────────────────────────────────────
  function onWheel(e: React.WheelEvent) {
    e.preventDefault();
    const rect = wrapRef.current?.getBoundingClientRect();
    if (!rect) return;
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    setView((v) => {
      const k = Math.min(4, Math.max(0.12, v.k * (e.deltaY < 0 ? 1.1 : 0.9)));
      // Keep the point under the cursor fixed.
      const gx = (mx - v.x) / v.k;
      const gy = (my - v.y) / v.k;
      return { k, x: mx - gx * k, y: my - gy * k };
    });
  }

  function toGraph(clientX: number, clientY: number) {
    const rect = wrapRef.current!.getBoundingClientRect();
    return {
      x: (clientX - rect.left - view.x) / view.k,
      y: (clientY - rect.top - view.y) / view.k,
    };
  }

  function onPointerDownNode(e: React.PointerEvent, id: string) {
    e.stopPropagation();
    (e.target as Element).setPointerCapture(e.pointerId);
    dragRef.current = { id };
    const node = byId.get(id);
    if (node) {
      node.fx = node.x;
      node.fy = node.y;
    }
    simRef.current?.alphaTarget(0.3).restart();
  }

  function onPointerDownBg(e: React.PointerEvent) {
    (e.target as Element).setPointerCapture(e.pointerId);
    panRef.current = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y };
  }

  function onPointerMove(e: React.PointerEvent) {
    if (dragRef.current) {
      const node = byId.get(dragRef.current.id);
      if (node) {
        const g = toGraph(e.clientX, e.clientY);
        node.fx = g.x;
        node.fy = g.y;
      }
    } else if (panRef.current) {
      const p = panRef.current;
      setView((v) => ({ ...v, x: p.vx + (e.clientX - p.x), y: p.vy + (e.clientY - p.y) }));
    }
  }

  function onPointerUp() {
    if (dragRef.current) {
      const node = byId.get(dragRef.current.id);
      if (node) {
        node.fx = null;
        node.fy = null;
      }
      simRef.current?.alphaTarget(0);
      dragRef.current = null;
    }
    panRef.current = null;
  }

  const hovered = hover ? byId.get(hover) : null;

  return (
    <div
      ref={wrapRef}
      className="graphwrap"
      onWheel={onWheel}
      onPointerDown={onPointerDownBg}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerLeave={onPointerUp}
    >
      <svg width={size.w} height={size.h}>
        <g transform={`translate(${view.x},${view.y}) scale(${view.k})`}>
          {mounted &&
          simLinks.map((l, i) => {
            const s = l.source as SimNode;
            const t = l.target as SimNode;
            if (!s || !t || s.x == null || t.x == null) return null;
            return (
              <line
                key={i}
                x1={s.x}
                y1={s.y}
                x2={t.x}
                y2={t.y}
                stroke="var(--graph-edge)"
                strokeWidth={0.6 / view.k + 0.4}
              />
            );
          })}
          {mounted &&
          simNodes.map((n) => {
            const r = radius(n) + (hover === n.id ? 2 : 0);
            const isChunk = n.kind === "chunk";
            const showLabel = LABEL_KINDS.has(n.kind) || hover === n.id;
            return (
              <g key={n.id} transform={`translate(${n.x},${n.y})`}>
                <circle
                  r={r}
                  fill={color(n)}
                  stroke="var(--panel)"
                  strokeWidth={0.8}
                  style={{ cursor: "grab", filter: isChunk ? undefined : "url(#glow)" }}
                  onPointerDown={(e) => onPointerDownNode(e, n.id)}
                  onPointerEnter={() => setHover(n.id)}
                  onPointerLeave={() => setHover((h) => (h === n.id ? null : h))}
                />
                {showLabel && (
                  <text
                    x={r + 3}
                    y={3}
                    fontSize={Math.max(9, 11 / view.k)}
                    fill="var(--text)"
                    style={{ pointerEvents: "none", paintOrder: "stroke" }}
                    stroke="var(--panel)"
                    strokeWidth={3 / view.k}
                  >
                    {n.label}
                  </text>
                )}
              </g>
            );
          })}
        </g>
        <defs>
          <filter id="glow" x="-60%" y="-60%" width="220%" height="220%">
            <feGaussianBlur stdDeviation="1.6" result="b" />
            <feMerge>
              <feMergeNode in="b" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>
      </svg>

      {hovered && (
        <div className="graphtip">
          <div className="graphtip-kind">
            {hovered.kind}
            {hovered.kind === "summary" && hovered.level != null ? ` · L${hovered.level}` : ""}
            {hovered.childCount != null ? ` · ${hovered.childCount} children` : ""}
          </div>
          <div className="graphtip-label">{hovered.detail ?? hovered.label}</div>
        </div>
      )}

      <button type="button" className="graphfit" onClick={() => fitRef.current()}>
        Fit
      </button>

      <div className="graphlegend">
        <span>
          <i style={{ background: "#8B5CF6" }} /> root
        </span>
        <span>
          <i style={{ background: "#F97316" }} /> source
        </span>
        <span>
          <i style={{ background: LEVEL_PALETTE[1] }} /> summary
        </span>
        <span>
          <i style={{ background: "#94A3B8" }} /> chunk
        </span>
      </div>
    </div>
  );
}

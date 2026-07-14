import { memoryGraph, workspacePath } from "@/lib/memory";
import { Empty, NoWorkspace } from "@/lib/ui";
import { ForceGraph } from "./ForceGraph";

export default function GraphPage() {
  const ws = workspacePath();
  if (!ws) {
    return (
      <>
        <h1>Memory graph</h1>
        <NoWorkspace />
      </>
    );
  }

  const graph = memoryGraph(ws);
  if (!graph.available || graph.nodes.length === 0) {
    return (
      <>
        <h1>Memory graph</h1>
        <p className="subtitle">Force-directed view of the summary tree.</p>
        <Empty>
          No memory tree in <code>memory_tree/chunks.db</code> yet. Seed one with{" "}
          <code>cargo run --example seed_memory --features sync</code> or run a full ingest.
        </Empty>
      </>
    );
  }

  return (
    <>
      <h1>Memory graph</h1>
      <p className="subtitle">
        {graph.treeCount} tree{graph.treeCount === 1 ? "" : "s"} · {graph.summaryCount} summaries ·{" "}
        {graph.chunkCount} chunks. Scroll to zoom, drag to pan, drag a node to pull it.
      </p>
      <ForceGraph nodes={graph.nodes} edges={graph.edges} />
    </>
  );
}

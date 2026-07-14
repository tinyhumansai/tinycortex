import { listChunks, workspacePath } from "@/lib/memory";
import { Empty, NoWorkspace, ts } from "@/lib/ui";

export default function TreePage() {
  const ws = workspacePath();
  if (!ws) {
    return (
      <>
        <h1>Memory tree</h1>
        <NoWorkspace />
      </>
    );
  }

  const { available, chunks, total } = listChunks(ws, 300);

  if (!available) {
    return (
      <>
        <h1>Memory tree</h1>
        <p className="subtitle">Ingested chunks from the summary tree.</p>
        <Empty>
          No <code>memory_tree/chunks.db</code> in this workspace yet. Skill docs are
          persisted on sync; the chunk/summary tree is produced by a full ingest pass.
        </Empty>
      </>
    );
  }

  return (
    <>
      <h1>Memory tree</h1>
      <p className="subtitle">
        {chunks.length} of {total} chunks (most recent first).
      </p>
      <div className="panel">
        <div className="tablewrap">
          <table>
            <thead>
              <tr>
                <th>Source</th>
                <th>Preview</th>
                <th>Tags</th>
                <th>Tokens</th>
                <th>Timestamp</th>
              </tr>
            </thead>
            <tbody>
              {chunks.map((c) => (
                <tr key={c.id}>
                  <td>
                    <div className="mono">{c.sourceKind ?? "—"}</div>
                    <div className="mono muted">{c.sourceId ?? c.id}</div>
                  </td>
                  <td style={{ maxWidth: 460 }}>{c.preview}</td>
                  <td>
                    {c.tags.length === 0
                      ? "—"
                      : c.tags.map((t) => (
                          <span key={t} className="tag">
                            {t}
                          </span>
                        ))}
                  </td>
                  <td className="mono muted">{c.tokenCount ?? "—"}</td>
                  <td className="mono muted">{ts(c.timestampMs)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  );
}

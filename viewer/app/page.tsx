import Link from "next/link";
import { overview, workspacePath } from "@/lib/memory";
import { Bool, NoWorkspace } from "@/lib/ui";

export default function OverviewPage() {
  const ws = workspacePath();
  if (!ws) {
    return (
      <>
        <h1>Overview</h1>
        <p className="subtitle">Inspect a local TinyCortex memory workspace.</p>
        <NoWorkspace />
      </>
    );
  }

  const o = overview(ws);
  return (
    <>
      <h1>Overview</h1>
      <p className="subtitle mono">{o.workspace}</p>

      <div className="grid">
        <div className="card">
          <div className="k">Skill docs</div>
          <div className="v">{o.docCount}</div>
        </div>
        <div className="card">
          <div className="k">Toolkits</div>
          <div className="v">{o.toolkitCounts.length}</div>
        </div>
        <div className="card">
          <div className="k">Tree chunks</div>
          <div className="v">{o.chunkTotal}</div>
        </div>
        <div className="card">
          <div className="k">Entities</div>
          <div className="v">{o.entityCount}</div>
        </div>
      </div>

      <div className="panel">
        <h2>Stores on disk</h2>
        <div className="tablewrap">
          <table>
            <thead>
              <tr>
                <th>Store</th>
                <th>Path</th>
                <th>Present</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>Skill documents</td>
                <td className="mono muted">memory_tree/skill_docs.db</td>
                <td>
                  <Bool value={o.exists.skillDocs} />
                </td>
              </tr>
              <tr>
                <td>Memory tree</td>
                <td className="mono muted">memory_tree/chunks.db</td>
                <td>
                  <Bool value={o.exists.chunks} />
                </td>
              </tr>
              <tr>
                <td>Entities</td>
                <td className="mono muted">memory_tree/content/entities/</td>
                <td>
                  <Bool value={o.exists.entities} />
                </td>
              </tr>
              <tr>
                <td>Sync manifest</td>
                <td className="mono muted">sync_manifest.json</td>
                <td>
                  <Bool value={o.exists.manifest} />
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>

      {o.toolkitCounts.length > 0 && (
        <div className="panel">
          <h2>Documents by toolkit</h2>
          <div className="tablewrap">
            <table>
              <thead>
                <tr>
                  <th>Toolkit</th>
                  <th>Documents</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {o.toolkitCounts.map((t) => (
                  <tr key={t.toolkit}>
                    <td>
                      <span className="badge toolkit">{t.toolkit}</span>
                    </td>
                    <td className="mono">{t.count}</td>
                    <td>
                      <Link className="rowlink" href={`/docs?toolkit=${t.toolkit}`}>
                        browse →
                      </Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </>
  );
}

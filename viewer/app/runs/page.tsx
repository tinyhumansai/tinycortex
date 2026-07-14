import { readManifest, workspacePath } from "@/lib/memory";
import { Bool, Empty, NoWorkspace } from "@/lib/ui";

export default function RunsPage() {
  const ws = workspacePath();
  if (!ws) {
    return (
      <>
        <h1>Sync runs</h1>
        <NoWorkspace />
      </>
    );
  }

  const manifest = readManifest(ws);
  if (!manifest) {
    return (
      <>
        <h1>Sync runs</h1>
        <p className="subtitle">Results from the last harness sync run.</p>
        <Empty>
          No <code>sync_manifest.json</code> in this workspace yet. Run the harness with{" "}
          <code>TINYCORTEX_WORKSPACE</code> set.
        </Empty>
      </>
    );
  }

  return (
    <>
      <h1>Sync runs</h1>
      <p className="subtitle">
        Last run · {manifest.documentsPersisted ?? "?"} documents persisted.
      </p>

      <div className="panel">
        <h2>Toolkit results</h2>
        <div className="tablewrap">
          <table>
            <thead>
              <tr>
                <th>Toolkit</th>
                <th>Pass</th>
                <th>Ingested</th>
                <th>Actions</th>
                <th>Cost $</th>
                <th>Taint</th>
                <th>Cursor</th>
                <th>Idempotency</th>
                <th>Error</th>
              </tr>
            </thead>
            <tbody>
              {manifest.toolkits.map((t) => (
                <tr key={t.toolkit}>
                  <td>
                    <span className="badge toolkit">{t.toolkit}</span>
                  </td>
                  <td>
                    <Bool value={t.passed} />
                  </td>
                  <td className="mono">{t.ingested ?? "—"}</td>
                  <td className="mono">{t.actions ?? "—"}</td>
                  <td className="mono">{(t.costUsd ?? 0).toFixed(4)}</td>
                  <td>
                    <Bool value={t.taintOk} />
                  </td>
                  <td className="mono muted">{t.cursorAdvanced ? "advanced" : "none"}</td>
                  <td className="mono muted">{t.idempotency ?? "—"}</td>
                  <td className="mono" style={{ color: t.error ? "var(--fail)" : undefined }}>
                    {t.error ?? "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      <div className="panel">
        <h2>Events ({manifest.events.length})</h2>
        <div className="tablewrap">
          <table>
            <thead>
              <tr>
                <th>Toolkit</th>
                <th>Source</th>
                <th>Stage</th>
                <th>Message</th>
              </tr>
            </thead>
            <tbody>
              {manifest.events.map((e, i) => (
                <tr key={i}>
                  <td className="mono">{e.toolkit ?? "—"}</td>
                  <td className="mono muted">{e.sourceId ?? e.source_id ?? "—"}</td>
                  <td className="mono">{e.stage ?? "—"}</td>
                  <td className="muted">{e.message ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  );
}

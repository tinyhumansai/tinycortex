import Link from "next/link";
import { listSkillDocs, skillDocCountsByToolkit, workspacePath } from "@/lib/memory";
import { Empty, NoWorkspace, secondsTs } from "@/lib/ui";

export default async function DocsPage({
  searchParams,
}: {
  searchParams: Promise<{ toolkit?: string; q?: string }>;
}) {
  const ws = workspacePath();
  if (!ws) {
    return (
      <>
        <h1>Skill documents</h1>
        <NoWorkspace />
      </>
    );
  }

  const { toolkit, q } = await searchParams;
  const all = listSkillDocs(ws);
  const toolkits = skillDocCountsByToolkit(all);
  const needle = (q ?? "").trim().toLowerCase();

  const docs = all.filter((d) => {
    if (toolkit && d.toolkit !== toolkit) return false;
    if (needle && !`${d.title} ${d.content}`.toLowerCase().includes(needle)) return false;
    return true;
  });

  return (
    <>
      <h1>Skill documents</h1>
      <p className="subtitle">
        {docs.length} of {all.length} documents ingested by sync (secrets/PII scrubbed on write).
      </p>

      <form className="filters" method="get">
        {toolkit && <input type="hidden" name="toolkit" value={toolkit} />}
        <input
          type="search"
          name="q"
          placeholder="Search title or content…"
          defaultValue={q ?? ""}
        />
      </form>

      <div className="filters">
        <Link className={`chip${!toolkit ? " active" : ""}`} href="/docs">
          all
        </Link>
        {toolkits.map((t) => (
          <Link
            key={t.toolkit}
            className={`chip${toolkit === t.toolkit ? " active" : ""}`}
            href={`/docs?toolkit=${t.toolkit}`}
          >
            {t.toolkit} ({t.count})
          </Link>
        ))}
      </div>

      {docs.length === 0 ? (
        <Empty>No documents match.</Empty>
      ) : (
        <div className="panel">
          <div className="tablewrap">
            <table>
              <thead>
                <tr>
                  <th>Toolkit</th>
                  <th>Title</th>
                  <th>Document ID</th>
                  <th>Taint</th>
                  <th>Updated</th>
                </tr>
              </thead>
              <tbody>
                {docs.map((d) => (
                  <tr key={`${d.toolkit}:${d.documentId}`}>
                    <td>
                      <span className="badge toolkit">{d.toolkit}</span>
                    </td>
                    <td>
                      <Link
                        className="rowlink"
                        href={`/docs/${encodeURIComponent(d.toolkit)}/${encodeURIComponent(
                          d.documentId,
                        )}`}
                      >
                        {d.title}
                      </Link>
                    </td>
                    <td className="mono muted">{d.documentId}</td>
                    <td className="mono muted">
                      {String((d.metadata as Record<string, unknown>).taint ?? "—")}
                    </td>
                    <td className="mono muted">{secondsTs(d.updatedAt)}</td>
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

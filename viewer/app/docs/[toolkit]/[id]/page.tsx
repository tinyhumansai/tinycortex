import { Fragment } from "react";
import Link from "next/link";
import { getSkillDoc, workspacePath } from "@/lib/memory";
import { Empty, NoWorkspace, secondsTs } from "@/lib/ui";

export default async function DocDetailPage({
  params,
}: {
  params: Promise<{ toolkit: string; id: string }>;
}) {
  const ws = workspacePath();
  const { toolkit, id } = await params;
  const documentId = decodeURIComponent(id);
  const tk = decodeURIComponent(toolkit);

  if (!ws) {
    return (
      <>
        <h1>Document</h1>
        <NoWorkspace />
      </>
    );
  }

  const doc = getSkillDoc(ws, tk, documentId);
  if (!doc) {
    return (
      <>
        <Link className="back" href="/docs">
          ← Skill docs
        </Link>
        <h1>Document not found</h1>
        <Empty>
          <span className="mono">
            {tk}:{documentId}
          </span>{" "}
          is not in this workspace.
        </Empty>
      </>
    );
  }

  const meta = Object.entries(doc.metadata).filter(([k]) => k !== "raw");
  const raw = (doc.metadata as Record<string, unknown>).raw;

  return (
    <>
      <Link className="back" href={`/docs?toolkit=${doc.toolkit}`}>
        ← {doc.toolkit} docs
      </Link>
      <h1>{doc.title}</h1>
      <p className="subtitle">
        <span className="badge toolkit">{doc.toolkit}</span>{" "}
        <span className="mono muted">{doc.documentId}</span>
      </p>

      <div className="panel">
        <h2>Metadata</h2>
        <dl className="kv">
          <dt>connection</dt>
          <dd>{doc.connectionId || "—"}</dd>
          <dt>skill id</dt>
          <dd>{doc.namespaceSkillId}</dd>
          <dt>updated</dt>
          <dd>{secondsTs(doc.updatedAt)}</dd>
          {meta.map(([k, v]) => (
            <Fragment key={k}>
              <dt>{k}</dt>
              <dd>{typeof v === "string" ? v : JSON.stringify(v)}</dd>
            </Fragment>
          ))}
        </dl>
      </div>

      <div className="panel">
        <h2>Content</h2>
        <div style={{ padding: 16 }}>
          <pre className="content">{doc.content || "(empty)"}</pre>
        </div>
      </div>

      {raw !== undefined && (
        <div className="panel">
          <h2>Raw provider payload</h2>
          <div style={{ padding: 16 }}>
            <pre className="content">{JSON.stringify(raw, null, 2)}</pre>
          </div>
        </div>
      )}
    </>
  );
}

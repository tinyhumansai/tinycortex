import { Fragment } from "react";
import { listEntities, workspacePath } from "@/lib/memory";
import { Empty, NoWorkspace } from "@/lib/ui";

export default function EntitiesPage() {
  const ws = workspacePath();
  if (!ws) {
    return (
      <>
        <h1>Entities</h1>
        <NoWorkspace />
      </>
    );
  }

  const entities = listEntities(ws);
  if (entities.length === 0) {
    return (
      <>
        <h1>Entities</h1>
        <p className="subtitle">Canonical entities extracted into the memory tree.</p>
        <Empty>
          No entities under <code>memory_tree/content/entities/</code> yet.
        </Empty>
      </>
    );
  }

  return (
    <>
      <h1>Entities</h1>
      <p className="subtitle">{entities.length} entity files.</p>
      {entities.map((e) => (
        <div className="panel" key={`${e.kind}/${e.file}`}>
          <h2>
            {e.kind} · {e.file}
          </h2>
          {Object.keys(e.frontMatter).length > 0 && (
            <dl className="kv">
              {Object.entries(e.frontMatter).map(([k, v]) => (
                <Fragment key={k}>
                  <dt>{k}</dt>
                  <dd>{v}</dd>
                </Fragment>
              ))}
            </dl>
          )}
          {e.body.trim() && (
            <div style={{ padding: "0 16px 16px" }}>
              <pre className="content">{e.body.trim()}</pre>
            </div>
          )}
        </div>
      ))}
    </>
  );
}

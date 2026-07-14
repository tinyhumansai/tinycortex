// Small shared presentational helpers (server components).

export function NoWorkspace() {
  return (
    <div className="panel">
      <div className="empty">
        <p>
          No workspace configured. Set <code>TINYCORTEX_WORKSPACE</code> to a
          TinyCortex workspace directory and restart the viewer.
        </p>
        <p className="muted">
          Seed a demo workspace:
          <br />
          <code>
            TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo cargo run --example seed_memory
            --features sync
          </code>
        </p>
      </div>
    </div>
  );
}

export function Empty({ children }: { children: React.ReactNode }) {
  return (
    <div className="panel">
      <div className="empty">{children}</div>
    </div>
  );
}

export function Bool({ value }: { value: boolean | undefined }) {
  return (
    <span className={`badge ${value ? "ok" : "fail"}`}>{value ? "yes" : "no"}</span>
  );
}

export function ts(ms: number | null | undefined): string {
  if (!ms) return "—";
  const seconds = ms < 1e12 ? ms : ms; // stored as ms already
  try {
    return new Date(seconds).toISOString().replace("T", " ").slice(0, 19);
  } catch {
    return "—";
  }
}

export function secondsTs(sec: number | null | undefined): string {
  if (!sec) return "—";
  try {
    return new Date(sec * 1000).toISOString().replace("T", " ").slice(0, 19);
  } catch {
    return "—";
  }
}

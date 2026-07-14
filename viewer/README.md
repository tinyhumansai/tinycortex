# TinyCortex Memory Viewer

A local, read-only debug UI for inspecting a TinyCortex memory workspace. It is
a small Next.js app whose **server-side** code opens the workspace's SQLite
stores and reads the JSON/markdown artifacts a sync run leaves behind — nothing
touches the network, and the database is opened read-only.

## What it shows

- **Overview** — which stores exist on disk, document/toolkit/chunk/entity counts.
- **Skill docs** — every `SkillDocument` sync ingested (secrets/PII scrubbed on
  write by the KV store), filterable by toolkit and searchable, with a detail
  view (content + metadata + raw provider payload).
- **Memory tree** — chunks from `memory_tree/chunks.db` (present after a full
  ingest pass).
- **Entities** — canonical entities under `memory_tree/content/entities/`.
- **Sync runs** — the last run's per-toolkit results and event stream from
  `sync_manifest.json`.

## Run it

```sh
# 1. Produce a workspace to inspect. Either seed a demo (no API key needed):
TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo \
  cargo run --example seed_memory --features sync

#    …or run a real Composio sync into a workspace:
TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo \
  cargo run --example composio_harness --features sync

# 2. Point the viewer at the same workspace and start it.
cd viewer
npm install
TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo npm run dev
# → http://localhost:4319
```

The viewer reads whatever is on disk each request, so re-running a sync and
refreshing the page shows the new state.

## Environment

- `TINYCORTEX_WORKSPACE` — path to the workspace to inspect (required).

## Stack

Next.js (App Router, server components), `better-sqlite3` for read-only SQLite
access. No client-side data fetching; all reads happen on the server.

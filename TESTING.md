# Testing TinyCortex

This guide covers how to exercise TinyCortex end-to-end: the Rust test suite, the
**Composio connection + memory-sync harness**, an offline **seed** for demo data,
and the **memory viewer** web app used to observe/debug what sync produced.

- [Prerequisites](#prerequisites)
- [Rust: build & test](#rust-build--test)
- [Composio harness](#composio-harness) — connection test + live memory sync
  - [Setup](#setup)
  - [Environment variables](#environment-variables)
  - [What each phase does](#what-each-phase-does)
  - [Common runs](#common-runs)
  - [Testing Gmail sync (connect flow)](#testing-gmail-sync-connect-flow)
- [Seed demo data (no API key)](#seed-demo-data-no-api-key)
- [Memory viewer](#memory-viewer)
- [End-to-end walkthrough](#end-to-end-walkthrough)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

| Tool | Version | For |
| --- | --- | --- |
| Rust | 2021 edition (stable) | the crate, harness, seed |
| Node.js | 18+ (tested on 22) | the `viewer/` web app |
| A Composio API key | direct mode (`ak_…`) | live sync only (not needed for seed) |

The sync code is behind the `sync` cargo feature; every command below that needs
it passes `--features sync`.

---

## Rust: build & test

```sh
cargo fmt --all --check      # formatting
cargo clippy --features sync # lints
cargo check                  # fast type-check (default features)
cargo test                   # unit + integration tests (default features)
cargo test --features sync   # includes the Composio mock tests
```

Notable test targets:

- `tests/composio_sync_mock.rs` — Composio pipelines against a `wiremock` server
  (no network): pagination, cursor persistence, taint tagging, idempotency,
  retry accounting.
- `tests/composio_sync_live.rs` — opt-in live checks (ignored unless configured).
- `src/memory/sync/persist_tests.rs` — the durable `KvSkillDocSink`.
- `src/memory/sync/composio/connect_tests.rs` — entity-id store + connect flow
  parsing.

---

## Composio harness

`examples/composio_harness.rs` is a runnable, end-to-end check that a Composio API
key is wired correctly and that TinyCortex's sync pipelines actually ingest memory
for every connected toolkit. It doubles as a CI smoke test (non-zero exit on
failure).

Supported toolkits (those with a TinyCortex pipeline): **gmail, github, linear,
notion, clickup, slack**.

### Setup

```sh
cp .env.example .env      # then edit — .env is gitignored
# set at minimum:  COMPOSIO_API_KEY=ak_...
```

The harness auto-loads `.env` from the working directory (via `dotenvy`); real
process env still overrides it.

### Environment variables

| Variable | Required | Purpose |
| --- | --- | --- |
| `COMPOSIO_API_KEY` | yes | Direct-mode API key. Never printed or logged. |
| `COMPOSIO_TOOLKITS` | no | Comma list restricting which toolkits to sync (e.g. `gmail,github`). Also the set the connect flow may log in. Unset = all connected+supported, non-interactive. |
| `COMPOSIO_BASE_URL` | no | Override API base (default `https://backend.composio.dev/api/v3`). |
| `COMPOSIO_ENTITY_ID` | no | Global Composio `user_id` fallback (see the user_id note below). |
| `COMPOSIO_MAX_ITEMS` | no | Per-toolkit ingest cap (default 25). |
| `TINYCORTEX_WORKSPACE` | no | When set, persist ingested documents + a `sync_manifest.json` into this workspace so the viewer can inspect them. |
| `COMPOSIO_<TK>_CONNECTION_ID` | no | Pin/override a connection id, e.g. `COMPOSIO_GMAIL_CONNECTION_ID`. |
| `COMPOSIO_<TK>_AUTH_CONFIG_ID` | no | Auth-config id used when initiating a login for that toolkit. Unset = looked up via `GET /auth_configs`. |
| `COMPOSIO_CALLBACK_URL` | no | OAuth callback/redirect passed to a connect link. |
| `COMPOSIO_CONNECT_TIMEOUT_SECS` | no | How long to poll a pending login for `ACTIVE` (default 120). |

> **The `user_id` requirement (Composio v3).** Composio v3 rejects tool execution
> with HTTP 400 (`ActionExecute_ConnectedAccountEntityIdRequired`, code 1811)
> unless the account's `user_id` is sent alongside `connected_account_id`. The
> harness captures each account's `user_id` during Phase 1 discovery (and from the
> connect flow) and scopes that toolkit's transport to it, falling back to
> `COMPOSIO_ENTITY_ID`. You normally don't need to set anything — discovery
> supplies the `user_id`.

### What each phase does

1. **Phase 1 — connection test.** Validates the key by listing connected accounts
   (`GET /connected_accounts`), printing each toolkit, its `connected_account_id`,
   status, and capturing its `user_id`.
2. **Phase 1.5 — login/connect flow.** Only for toolkits named in
   `COMPOSIO_TOOLKITS` that have no `ACTIVE` account. Resolves the toolkit's
   auth-config, creates a Connect link scoped to a remembered per-toolkit entity id
   (`.composio-harness.json`, gitignored), prints the OAuth URL, and polls until
   the account is `ACTIVE` (or times out). Skipped entirely when `COMPOSIO_TOOLKITS`
   is unset, so the default run stays non-interactive.
3. **Phase 2 — memory sync.** For each selected toolkit, runs the pipeline's
   `tick()` twice against real Composio and grades: records ingested, provider
   actions, cost, cursor advance, `taint=external_sync` tagging, and idempotency
   (a second unchanged tick must ingest 0 — unless the first hit the item cap, which
   is reported as `incremental`).

### Common runs

```sh
# All connected + supported toolkits (non-interactive):
cargo run --example composio_harness --features sync

# Just GitHub, and persist into a workspace for the viewer:
COMPOSIO_TOOLKITS=github TINYCORTEX_WORKSPACE=/tmp/tinycortex-live \
  cargo run --example composio_harness --features sync
```

A healthy run ends with `HARNESS PASS` and a table like:

```
toolkit   result   recs  acts    cost$ taint  idempotency  notes
github    PASS       50     2   0.0000 ok     incremental  conn=ca_… cursor=none
```

### Testing Gmail sync (connect flow)

If no Gmail account is connected yet, request it explicitly and the harness will
run the **Phase 1.5 login flow**:

```sh
COMPOSIO_TOOLKITS=gmail TINYCORTEX_WORKSPACE=/tmp/tinycortex-live \
  cargo run --example composio_harness --features sync
```

It prints an OAuth URL — open it, authorize Gmail, and the harness polls until the
connection is `ACTIVE`, then syncs it. This needs a Gmail auth-config in your
Composio dashboard; pin one explicitly with `COMPOSIO_GMAIL_AUTH_CONFIG_ID=…` if
auto-lookup can't find it. Once connected, subsequent runs reuse the account (the
entity id is remembered in `.composio-harness.json`).

---

## Seed demo data (no API key)

`examples/seed_memory.rs` builds a realistic workspace with **no Composio key**: it
persists sample skill documents (via the same `KvSkillDocSink` sync uses) and builds
a real summary tree (chunks → L0 → L1/L2 via the offline `ConcatSummariser`). Use it
to exercise the viewer — including the memory-graph view — offline.

```sh
TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo \
  cargo run --example seed_memory --features sync
```

---

## Memory viewer

A local, read-only Next.js app (`viewer/`) whose **server-side** code opens the
workspace's SQLite stores read-only and reads the sync artifacts. No network; the
DB is opened read-only.

```sh
cd viewer
npm install
TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo npm run dev
# → http://localhost:4319
```

Checks:

```sh
npx tsc --noEmit    # type-check
npm run build       # production build
```

Views:

| Route | Shows |
| --- | --- |
| `/` | Overview — which stores exist, doc/toolkit/chunk/entity counts |
| `/docs` | Skill documents ingested by sync, filter by toolkit + search, detail with content/metadata/raw payload |
| `/graph` | Force-directed summary tree (root → source → summary levels → chunks); pan/zoom/drag/hover/fit |
| `/tree` | Memory-tree chunks (`memory_tree/chunks.db`) |
| `/entities` | Canonical entities under `memory_tree/content/entities/` |
| `/runs` | The last sync run's per-toolkit results + event stream (`sync_manifest.json`) |

The viewer reads the workspace on every request — re-run a sync and refresh to see
new state. It must point at the **same** `TINYCORTEX_WORKSPACE` the harness/seed
wrote to.

> Note: the harness persists **skill documents** into a workspace; it does not build
> the full chunk/summary tree (that is a separate ingest pass). So the `/graph` and
> `/tree` views are populated by the **seed** example (or a full ingest), while
> `/docs` and `/runs` reflect live harness sync output.

---

## End-to-end walkthrough

```sh
# 1. Sync real data into a workspace (GitHub shown; see Gmail section for OAuth):
COMPOSIO_TOOLKITS=github TINYCORTEX_WORKSPACE=/tmp/tinycortex-live \
  cargo run --example composio_harness --features sync
#   → HARNESS PASS, N documents persisted to /tmp/tinycortex-live

# 2. Observe it:
cd viewer && npm install
TINYCORTEX_WORKSPACE=/tmp/tinycortex-live npm run dev
#   → open http://localhost:4319/docs  (and /runs for the run manifest)

# 3. For an offline demo with a full memory graph, seed a second workspace:
TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo \
  cargo run --example seed_memory --features sync
#   restart the viewer against /tmp/tinycortex-demo and open /graph
```

---

## Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `HTTP 400 … User ID is required` | Old build. Update — the harness now sends each account's `user_id`. If a connection has none, set `COMPOSIO_ENTITY_ID`. |
| `connected_accounts returned HTTP 401` | Invalid/expired `COMPOSIO_API_KEY`. |
| `No supported+connected toolkits to sync` | None of your connected accounts map to a supported toolkit, or all are `EXPIRED`. Connect one (or use the Gmail connect flow), or set `COMPOSIO_<TK>_CONNECTION_ID`. |
| Toolkit account shows `EXPIRED` | Re-authorize it; the harness treats terminal-status accounts as absent so the connect flow can re-establish them. |
| `/graph` or `/tree` empty in the viewer | That workspace has skill docs but no memory tree. Use the seed example or point at a workspace produced by a full ingest. |
| Viewer shows "No workspace configured" | `TINYCORTEX_WORKSPACE` is unset for the `npm run dev` process. |
| Secrets in synced content | Expected to be redacted: KV writes sanitize values, so the viewer never surfaces raw credentials. |

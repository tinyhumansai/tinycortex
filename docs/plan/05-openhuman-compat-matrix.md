# 5. OpenHuman Feature Compatibility Matrix

Scope: a living inventory of every **OpenHuman-origin feature surface** and its
current TinyCortex compatibility status, so nothing silently falls out of scope
during the I-phase cutover (doc 04). This is the companion tracker to
[04-openhuman-integration.md](04-openhuman-integration.md): doc 04 is the
*phased plan*, this is the *checklist of surfaces* each phase must preserve.

It exists because the user asked us to "track all the OpenHuman-related features
that need to be made compatible — Obsidian wikis, Obsidian keys, local files,
these kinds of things." The three called-out items are resolved precisely in
§5.2–§5.4 below; the short version:

- **"local files"** → the `folder` source kind. **Ported** (§5.1).
- **"Obsidian wikis"** → a *storage/output* surface (`content::wiki_git` git
  wiki mirror + `content::obsidian*` vault registry), **not** a source kind.
  Partially present (wikilinks/tags/front matter) with the registry/mirror
  **deferred** (§5.2).
- **"Obsidian keys"** → not a literal concept in the OpenHuman reference. It
  maps to either **Composio connection keys** (`toolkit` + `connection_id`,
  ported as fields) or **YAML front-matter keys** (ported per file type). No
  `CredentialResolver` exists to port (§5.3).

## Legend

| Status | Meaning |
| --- | --- |
| ✅ ported | Implemented in TinyCortex with wire-compatible contracts. |
| 🟡 partial | Domain primitives exist; a wrapping surface or sub-feature is missing. |
| ⏸ host-owned | Intentionally **not** in TinyCortex; owned by the OpenHuman host (sync, OAuth, scheduling). Compatibility = the seam is present. |
| ❌ deferred | An OpenHuman feature with a real TinyCortex gap, not yet ported. |
| — n/a | Not present in the OpenHuman reference; do not build it. |

Anchors are `file:line` at the time of the July 2026 audit; re-verify before
acting (see the memory note on recalled anchors).

---

## 5.1 Source connectors ("what feeds memory?")

OpenHuman defines exactly **7** `SourceKind`s. TinyCortex has ported all 7 with
matching snake_case wire strings — there is **no missing source kind**, and in
particular **no Obsidian/wiki source kind** in either codebase.

| Kind (wire string) | Required fields | Reader | TinyCortex status |
| --- | --- | --- | --- |
| `folder` **(local files)** | `path` (opt `glob`) | local | ✅ ported — reader `sources/readers/folder.rs`; glob default `**/*.md`, 10 MB cap, path-traversal guard |
| `conversation` | — | local | ✅ ported — reader `sources/readers/conversation.rs` over `<workspace>/threads/*.json` |
| `composio` | `toolkit`, `connection_id` | host network | ✅ contract + validation; ⏸ live fetch host-owned |
| `github_repo` | `url` (opt `branch`, `paths`, `max_commits`, `max_issues`, `max_prs`) | host network | ✅ contract; ⏸ fetch host-owned (`gh` CLI / REST) |
| `rss_feed` | `url` (opt `max_items`) | host network | ✅ contract; ⏸ fetch host-owned |
| `web_page` | `url` (opt `selector`) | host network | ✅ contract; ⏸ fetch host-owned |
| `twitter_query` | `query` (opt `since_days`) | host network | ✅ contract; ⏸ fetch is a placeholder even in OpenHuman |

Grounding: enum + wire strings `src/memory/sources/types.rs:29-59`; flattened
kind fields + sync budgets `types.rs:68-146`; discriminator validation
`src/memory/sources/validation.rs:23-55`; reader trait + `reader_for`
(local for `folder`/`conversation`, `None` otherwise)
`src/memory/sources/readers/mod.rs:35-78`; reader output contracts
(`SourceItem`, `SourceContent`, `ContentType = markdown|html|plaintext`)
`types.rs:162-199`.

- [ ] **X1 — Source registry CRUD parity.** Confirm the TOML-backed registry
      (`src/memory/sources/registry.rs`, `[[memory_sources]]`) covers all
      OpenHuman ops: `add_source`, `get_source`, `list_sources`,
      `list_enabled_by_kind`, `update_source`, `remove_source`,
      `upsert_composio_source`. Add any missing (`list_enabled_by_kind`,
      `upsert_composio_source` are the likely gaps). Atomic load→modify→
      validate→save.
- [ ] **X2 — Raw-archive self-healing.** Preserve `mem_tree_ingested_sources`
      reconciliation with `source_kind = raw_file` so interrupted syncs don't
      strand raw files (spec: sources-registry-sync.md §Raw Archive Coverage).

Network fetchers (composio/github/rss/web/twitter) are **⏸ host-owned by
design** — the TinyCortex boundary is the on-demand ingest call. Their only
compatibility obligation is that the `reader_for` seam returns `None` so the
host routes them through its own sync runner. **Do not port the live sync
scheduler.**

---

## 5.2 Obsidian vault & wiki compatibility ("Obsidian wikis")

Obsidian is a **storage + output layout**, not a connector: "Obsidian-readable
files are a first-class product surface, not an export"
(`openhuman-memory-engine-spec.md:126`). The content-compose layer is ported;
the vault *registry* and git *wiki mirror* are deferred host-adapter surfaces.

| Surface | Status | Grounding / gap |
| --- | --- | --- |
| Summary-child **wikilinks** `[[<basename>]]` | ✅ ported | `store/content/compose/summary.rs:84-101`; tests `compose/compose_tests.rs:272-335` |
| **Hierarchical tags** `kind/Value` (atomic rewrite, body SHA preserved) | ✅ ported | `store/content/tags.rs:24-96` |
| **Graph-view source tag** `source/<slug>` | ✅ ported | `store/content/compose/yaml.rs:3-26`; seeded per chunk `compose/chunk.rs:47` |
| **YAML front matter** compose / parse / round-trip | ✅ ported | `compose/yaml.rs:32-85`; summary `compose/summary.rs:64-129` |
| **Vault paths / layout** (`wiki/summaries/...`, `{email\|chat\|document}/<slug>`, `entities/<kind>/<id>.md`) | ✅ ported | `store/content/paths.rs:1-56`; entities `src/memory/entities/store.rs:1-40` |
| Front-matter **aliases** | ✅ ported | `compose/summary.rs:115-160` |
| `content::obsidian*` **vault registry** (vault-root reads, "Obsidian defaults") | ❌ deferred | explicitly not ported: `store/content/mod.rs:19-20` |
| `content::wiki_git` **git wiki mirror** ← the "Obsidian wiki" item | ❌ deferred | `store/content/mod.rs:19-20`; migration.md:87 lists "obsidian/wiki-git content" as deferred host adapter |
| `update_summary_tags` (SQLite/Config-aware tag rewrite) | ❌ deferred | `store/content/tags.rs:8-10` |
| `obsidian_vault_status` / `vault_health_check` controllers | ❌ deferred | see §5.5 (whole controller layer deferred) |

- [ ] **X3 — Port `content::wiki_git`** (git-backed Obsidian wiki mirror) as a
      host adapter when the diff/git layer stabilizes. This is the surface
      behind "Obsidian wikis." Depends-on: `git-diff` feature (present).
- [ ] **X4 — Port `content::obsidian*`** vault registry (vault-root raw reads +
      Obsidian defaults) so a host can point Obsidian at the live vault.
- [ ] **X5 — `update_summary_tags`** entity-index-driven tag rewrite over the
      chunk store.

---

## 5.3 Keys & credentials ("Obsidian keys")

There is **no literal "Obsidian key"** in the OpenHuman reference (grep for
`credential|api_key|CredentialResolver` finds nothing). The grounded readings:

| Interpretation | Status | Grounding / gap |
| --- | --- | --- |
| **Composio connection credentials** (`toolkit` + `connection_id`) | ✅ fields + validation | `sources/types.rs:82-86`, `validation.rs:31-34`. Secret storage / OAuth exchange is ⏸ host-owned |
| **YAML front-matter keys** (Obsidian "properties") | ✅ ported per file type | summary keys `compose/summary.rs:76-127`; entity keys engine-spec:423-425 |
| **OAuth / webhook credentials** | ⏸ host-owned | sources-registry-sync.md:16, migration.md:82 — never in TinyCortex |
| **Twitter credentials** | ⏸ host-owned | placeholder pending API path (sources-registry-sync.md:86) |
| `CredentialResolver` host-hook | — n/a | not defined in any OpenHuman doc or source; do **not** invent one |

Net: TinyCortex holds **no secret material** and needs none — credentials are a
host surface. The only compatibility obligation is that `toolkit` /
`connection_id` strings round-trip through the registry unchanged. Note doc 04's
planned `CredentialResolver → credentials::AuthService` host hook (04 §4.2) is an
*adapter-side* trait to define during I1, not an existing OpenHuman contract.

- [ ] **X6 — Confirm** the I1 host-hook set (04 §4.2) is the complete credential
      seam; there is no lower-level key API to port from OpenHuman.

---

## 5.4 Content / markdown storage primitives

Body storage is essentially at parity — this underpins both Obsidian
compatibility and the on-disk contract doc 04 relies on for cheap rollback.

| Primitive | Status | Grounding |
| --- | --- | --- |
| Immutable chunk bodies (never overwrite) | ✅ | `store/content/mod.rs:57-101` |
| SHA-256 over **body bytes only** (tag rewrite ≠ hash change) | ✅ | `compose/mod.rs:16-19`, `mod.rs:88-89` |
| Atomic writes (tempfile + fsync + rename) | ✅ | `content/atomic.rs`; `tags.rs:38-57` |
| Front-matter round-trip (compose/split/parse) | ✅ | `compose/yaml.rs:32-85` |
| Path-escape guard | ✅ | `content/mod.rs:49-52`; readers `validation.rs:65-79` |
| `content_path` / `content_sha256` pointers (SQLite stores pointers, not bodies) | ✅ | `store/content/mod.rs:93-97`; `src/memory/types.rs:227` |
| Raw archive (immutable verbatim per item) | ✅ | `content/raw.rs`; `content/mod.rs:15,45-48` |
| Format markers `memory_artifact_format=2`, `openhuman_core_version` | ✅ | `compose/mod.rs:29-34` (kept as OpenHuman wire strings) |

No deferred items here beyond the SQLite-aware readers that ride with the chunk
store (T2 golden tests, doc 03, pin these).

---

## 5.5 Controller / tool registry & agent tools

The entire JSON-RPC controller layer and agent-tool wrappers are **deferred**
(no `src/memory/controllers/` or `src/memory/tools/`); the domain operations
they would wrap are ported. This is goal **C4/C5** territory (doc 02) and I3/I4
(doc 04). Wire names/schemas are pinned by T2 golden tests so clients don't
change on cutover.

| Namespace | Functions (OpenHuman) | Status |
| --- | --- | --- |
| `memory` controller | `ingest`, `search`, `recall`, `list_chunks`, `get_chunk`, `entity_index_for`, `chunks_for_entity`, `top_entities`, `chunk_score`, `delete_chunk`, `delete_source`, **`graph_export`**, **`obsidian_vault_status`**, **`vault_health_check`**, `flush_now`, `flush_source`, `wipe_all`, `reset_tree`, `pipeline_status`, `set_enabled`, `smart_walk`, `doctor`, `retry_failed`, `memory_backfill_status`, `list_sources` | ❌ deferred (domain ops exist) |
| `memory_sources` | `list`, `get`, `add`, `update`, `remove`, `list_items`, `read_item`, `sync`, `reconcile`, `status_list`, `supported_toolkits`, `sync_audit_log`, `estimate_sync_cost`, `monthly_cost_summary`, `apply_all_in` | 🟡 registry/reader domain ✅; RPC wrappers ❌; sync/cost ⏸ host-owned |
| `memory_diff` | `take_snapshot`, `list_snapshots`, `diff`, `diff_since_last`, `diff_since_read`, `mark_read`, `create_checkpoint`, `list_checkpoints`, `diff_since_checkpoint`, `cleanup` | 🟡 diff ledger ✅ (`src/memory/diff/`, feature `git-diff`); RPC wrappers ❌ |
| `memory_goals` | `list`, `add`, `edit`, `delete`, `reflect` | 🟡 goals store ✅ (`src/memory/goals/`); RPC wrappers ❌ |
| retrieval controllers | source/global/topic query, cover window, entity search, drill down, fetch leaves | 🟡 primitives ✅ (`src/memory/retrieval/`); RPC wrappers ❌ |
| agent tools | `MemoryTree*Tool` family, `MemoryDiffTool`, `Goals*Tool`, `MemoryTools*Tool` | ❌ deferred |

Specifically for the called-out items: **`graph_export`** sits on the ported
co-occurrence graph (`src/memory/graph/`) but has no exporter;
**`obsidian_vault_status` / `vault_health_check`** additionally need the
deferred vault registry (§5.2) beneath them.

- [ ] **X7 — Controller registry (C4/C5).** Register the namespaces above with
      schema-pinned names; golden-test the wire schemas (T2). Sync/cost handlers
      stay ⏸ host-owned; `sync`/`reconcile` call the host runner.
- [ ] **X8 — `graph_export`** exporter over `src/memory/graph/`.
- [ ] **X9 — Agent-tool wrappers** once the controller registry lands.

---

## 5.6 How to use this doc

- Every **I-phase PR** (doc 04) should check the rows it touches off here and add
  any newly discovered surface. Treat a surface that is neither ✅ nor an
  explicit ⏸/— as a blocker for declaring that layer "cut over."
- `X*` items are the actionable gaps in `/goal` format; promote them into a
  goal doc when scheduled. They are deliberately *tracking stubs* here, not full
  goals — the definition-of-done for each lives with its owning C-/I- goal.
- Keep the file:line anchors honest: re-grep before acting on any row.

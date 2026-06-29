# Sources, Registry, and Sync Spec

OpenHuman modules: `memory_sources`, `memory_sync`.

## Responsibility

The source registry answers "what feeds memory?" Sync pipelines answer "how do
we pull from upstream and land data into memory?" Canonicalizers normalize
upstream payloads into a common markdown shape for ingestion.

## Source Registry

Sources are persisted as `[[memory_sources]]` entries in `config.toml`.

Common fields:

- `id`, `kind`, `label`, `enabled`.
- optional sync budgets: `max_tokens_per_sync`, `max_cost_per_sync_usd`.
- optional `sync_depth_days`.

Kinds and required fields:

- `composio`: `toolkit`, `connection_id`.
- `conversation`: no kind-specific fields.
- `folder`: `path`, optional `glob`.
- `github_repo`: `url`, optional `branch`, `paths`, `max_commits`,
  `max_issues`, `max_prs`.
- `twitter_query`: `query`, optional `since_days`; sync is currently
  placeholder.
- `rss_feed`: `url`, optional `max_items`.
- `web_page`: `url`, optional `selector`.

Validation is runtime/discriminator-based because kind-specific fields are
flattened optional fields on one struct.

## Registry Operations

Required CRUD:

- `add_source`
- `get_source`
- `list_sources`
- `list_enabled_by_kind`
- `update_source`
- `remove_source`
- `upsert_composio_source`

Mutations must load current config, modify, validate, and save atomically.
OpenHuman lazily reconciles active Composio connections on list, so integrations
connected before process boot become visible without restart.

## Source Readers

The `SourceReader` trait must support listing items and reading item content.
Reader output:

- `SourceItem`: `id`, `title`, optional `updated_at_ms`.
- `SourceContent`: `id`, `title`, `body`, `content_type`, `metadata`.
- `ContentType`: `markdown`, `html`, `plaintext`.

Reader requirements:

- Folder reader: glob files, default markdown pattern, 10 MB cap, path traversal
  guard.
- GitHub reader: commits, issues, PRs via `gh` CLI or public REST fallback.
- RSS reader: RSS/Atom item extraction.
- Web page reader: fetch URL and optionally select content by CSS selector.
- Conversation reader: local conversation source integration.
- Composio reader: connection placeholder; provider-driven sync owns real fetch.
- Twitter reader: placeholder until credentials/API path exist.

## Manual Sync

Manual sync should return immediately after queuing/backgrounding work. Progress
is communicated through memory sync stage events with source id/connection id.
Stages include requested, fetching, stored, ingesting, completed, and failed.

Reader-backed sources ingest each item through `memory::ingest_pipeline`.
Composio-backed sources delegate to `memory_sync::composio::run_connection_sync`.

## Status and Cost Surfaces

OpenHuman exposes source status by querying chunks matching source-id prefixes.
Freshness labels:

- active: last chunk within 30 seconds.
- recent: last chunk within 5 minutes.
- idle: no chunk or older activity.

Additional source RPC surfaces include supported toolkits, sync audit log, sync
cost estimate, monthly cost summary, and all-in/apply-all setup.

## Sync Pipelines

`SyncPipelineKind` values:

- `composio`
- `workspace`
- `mcp`

Each pipeline implements:

- `id()`
- `kind()`
- `init(config)`
- `tick(config) -> SyncOutcome`

`SyncOutcome` contains `records_ingested`, `more_pending`, and optional `note`.
Pipelines own pagination, cursors, rate limits, and retries. The orchestrator
owns cadence.

OpenHuman background loops:

- Composio periodic: 20-minute tick and per-connection interval.
- Workspace periodic: 20-minute tick; default daily source sync interval.
- Queue scheduler: stale buffer flush and transient-failure recovery.

## Canonicalization

Canonicalization produces `CanonicalisedSource { markdown, metadata }`.

Adapters:

- Chat: sorted `## <ts> - <author>` blocks; empty input returns `None`.
- Document: trimmed markdown; time range is modified-at point.
- Email: message blocks with From/Subject/Date plus cleaned body.
- Email clean helpers: strip reply chains, legal/footer boilerplate, truncate,
  escape markdown, extract email, parse dates.

Canonicalizers must not interpret semantic meaning and must not emit a leading
title header. Titles/provenance belong in content-store front matter.

## Raw Archive Coverage

OpenHuman records raw files summarized by a batch in `mem_tree_ingested_sources`
with `source_kind = raw_file`. Reconciliation diffs disk against gate state and
summarizes uncovered remainder. TinyCortex should preserve this self-healing
behavior so interrupted syncs do not strand raw files.

## Controller Namespace

Namespace: `memory_sources`.

Functions:

- `list`, `get`, `add`, `update`, `remove`
- `list_items`, `read_item`, `sync`, `reconcile`
- `status_list`, `supported_toolkits`
- `sync_audit_log`, `estimate_sync_cost`, `monthly_cost_summary`
- `apply_all_in`

## TinyCortex Landing Area

```text
src/memory/sources/
src/memory/sync/
src/memory/sync/canonicalize/
```

Port order: source kind/types/validation, registry patch semantics, reader
trait and static reader contracts, canonicalizer pure functions, then live sync.


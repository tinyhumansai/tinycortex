---
description: How TinyCortex stores entities as markdown files and derives a co-occurrence graph on demand from the tree scorer's occurrence index.
---

# Entities & Graph

TinyCortex maintains a registry of the named things in a user's world — people,
organizations, topics, technologies, and the mechanical kinds (emails, URLs,
handles, hashtags) the tree scorer extracts. Each entity is a **markdown file in
the content store**, so the vault stays the source of truth and any tool (an
editor, `grep`, vector search) can read or edit it without going through a
database.

On top of that registry sits a **co-occurrence graph**. TinyCortex does *not*
keep a separate triple store: the graph is *derived on demand* from the entity
occurrence index the tree scorer already populates. Two entities that appear on
the same tree node form an edge; the edge weight is the number of distinct nodes
they share.

![Apple email graph derived from co-occurrence](.gitbook/assets/AppleEmailGraph.gif)

This page covers both halves: the entity file layout and the derived graph.

- Entity module: `src/memory/entities/` (`mod.rs`, `types.rs`, `canonical.rs`,
  `frontmatter.rs`, `store.rs`)
- Graph module: `src/memory/graph/` (`mod.rs`, `types.rs`, `query.rs`)

---

## Entity records

### On-disk layout

Every entity is persisted at:

```text
<content_root>/entities/<kind>/<canonical_id>.md
```

The `<kind>` directory is the entity kind's wire string (see the table below)
and the filename stem is a slugified canonical id. The content root is resolved
from `MemoryConfig::workspace`; the entity module borrows nothing else from
storage internals — no SQLite, no async, no upward dependencies on orchestration
or tools.

A file is YAML front matter followed by a free-form notes body:

```markdown
---
id: person:alice
kind: person
display_name: Alice Cooper
aliases:
  - Ali
emails:
  - alice@example.com
handles:
  - kind: slack
    value: U12345
created_at: 2026-05-23T22:00:00+00:00
updated_at: 2026-05-23T22:00:00+00:00
---

Free-form notes the user can edit. Preserved across upserts.
```

The front matter is read and written by a hand-rolled reader/writer in
`frontmatter.rs` — there is no `serde_yaml` dependency, and the on-disk format
is byte-for-byte identical to OpenHuman's.

### The `Entity` shape

A single `Entity` struct (`src/memory/entities/types.rs`) covers every kind; the
`kind` field discriminates and the optional fields populate as relevant.

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `String` | Canonical id `<kind>:<value>` (e.g. `person:alice`, `email:alice@example.com`). Stable across renames and aliases. |
| `kind` | `EntityKind` | Discriminator; selects the on-disk directory and the id prefix. |
| `display_name` | `Option<String>` | `None` when the entity hasn't been named yet. Skipped in serialization when absent. |
| `aliases` | `Vec<String>` | Alternate strings (nicknames, old names). Skipped when empty. |
| `emails` | `Vec<String>` | Email addresses; pulled out of the generic `handles` for `Person` convenience. Skipped when empty. |
| `handles` | `Vec<EntityHandle>` | Source-specific identifiers. Skipped when empty. |
| `created_at` | `DateTime<Utc>` | First write timestamp. |
| `updated_at` | `DateTime<Utc>` | Last upsert timestamp. |

`Entity::new(id, kind)` builds a fresh record with empty collections and both
timestamps set to `Utc::now()`. The `id` passed in is expected to already be
canonicalized — see [Canonical ids](#canonical-ids).

An `EntityHandle` is an opaque label by which a source knows the entity (a
generalization of a legacy person handle):

```rust
pub struct EntityHandle {
    pub kind: String,  // e.g. "imessage", "slack", "discord", "gmail"
    pub value: String, // the channel-specific identifier
}
```

### Entity kinds

`EntityKind` mirrors the entity-kind taxonomy the tree scorer emits, so the
canonical ids produced during scoring round-trip through this module unchanged.
It is kept as a *local* enum (not a re-export of the score module's type) so
`entities` stays usable independently — but the wire strings are byte-for-byte
identical across the two modules. The serde representation is `snake_case`, and
`as_str` / `parse` are exact inverses.

| Wire string | Variant | Category | Meaning |
| --- | --- | --- | --- |
| `person` | `Person` | semantic | A named individual. |
| `organization` | `Organization` | semantic | A company, team, or institution. |
| `topic` | `Topic` | semantic | A subject or theme. |
| `email` | `Email` | mechanical | An email address. |
| `url` | `Url` | mechanical | A URL. |
| `handle` | `Handle` | mechanical | A source-specific handle (social/user id). |
| `hashtag` | `Hashtag` | mechanical | A `#hashtag`. |
| `location` | `Location` | semantic | A place. |
| `event` | `Event` | semantic | An occurrence in time. |
| `product` | `Product` | semantic | A product or offering. |
| `datetime` | `Datetime` | mechanical | A date or timestamp reference. |
| `technology` | `Technology` | semantic | A tool, language, framework, or other technology. |
| `artifact` | `Artifact` | semantic | A produced file, document, or output. |
| `quantity` | `Quantity` | mechanical | A measured amount or numeric quantity. |
| `misc` | `Misc` | catch-all | Matches no other kind. |

The `as_str` wire string serves double duty: it is both the on-disk directory
name (`entities/<kind>/…`) and the `kind:` prefix of canonical ids.

### Canonical ids

`canonical_id_for(kind, surface)` (`src/memory/entities/canonical.rs`) derives a
stable `<kind>:<value>` id from a surface form. Canonicalization is **exact-match
only**: it normalizes casing and decoration so cross-source mentions of the same
thing collapse onto one registry file, but it does *not* attempt fuzzy matching
(e.g. `alice-slack` ≡ `Alice-Discord`), which would risk false merges.

Normalization rules per kind:

| Kind | Rule | Example |
| --- | --- | --- |
| `email` | lowercase | `email:alice@example.com` |
| `handle` | lowercase, strip leading `@` | `handle:alice` |
| `hashtag` | lowercase, strip leading `#` | `hashtag:rust` |
| `topic` | lowercase, strip leading `@`/`#` | `topic:memory` |
| `url` | **case preserved**, trimmed only | `url:https://Example.com/Path` |
| all others | lowercase surface | `person:alice` |

URLs keep their original case because path and query components are
case-significant; folding them would break exact matching. Every other kind is
lowercased so casing never fragments an identity. (The surface is also `trim()`ed
before normalization in every case.)

The filename stem comes from `slugify_id(id)`, which replaces `:` along with the
Windows-reserved characters (`/ \ : * ? " < > |`), the NUL byte, and any control
characters with `_`. The authoritative id always lives in the file's YAML `id:`
field — the parser never reconstructs the id from the filename, so the slug is
purely a content-addressed handle and the same layout works on every platform.

### Store API

The public surface (`src/memory/entities/store.rs`, re-exported from `mod.rs`):

| Function | Behaviour |
| --- | --- |
| `put_entity` | Upsert by canonical id. **Preserves the free-form notes body** across writes (see below). |
| `get_entity` | Read an entity by canonical id. |
| `list_entities` | Walk a kind's directory and return its entities. |
| `lookup_alias` | Find a canonical id by alias / email / handle value / display name, via a case-insensitive linear scan. |
| `canonical_id_for` | Derive a stable canonical id from a surface form (re-exported from `canonical`). |

#### Notes preservation on upsert

The critical contract of `put_entity` is that it is an *upsert that never
destroys human-authored notes*. When a record is rewritten — for example, when a
new sync run adds an alias or a handle — only the YAML front matter is
regenerated from the `Entity` struct. The notes body below the front-matter
delimiter, which the user may have edited by hand, is read off the existing file
and written back unchanged. This is what lets automated ingestion and manual
curation share the same file safely.

---

## The co-occurrence graph

### Premise: the graph is the tree, mapped out

A separate triple store would be redundant: every chunk the tree scorer
processes already lands an `(entity_id, node_id)` row in the occurrence index
(`mem_tree_entity_index` in OpenHuman). So TinyCortex derives relationship edges
directly from that index rather than maintaining a parallel storage table:

- An **edge** exists between two entities that co-occur on the same node.
- The **weight** is the count of *distinct* nodes the pair both appear on — a
  cheap proxy for relationship strength.

The graph module (`src/memory/graph/`) owns no state and performs no writes; it
is a pure, read-only derivation.

{% hint style="info" %}
**Out of scope here:** the LLM-extracted `(subject, predicate, object)` triple
surface. That is a separate, persisted shape —
`crate::memory::types::GraphRelationRecord` — and is intentionally not part of
this derived graph. The derived `GraphEdge` has *no explicit predicate*.
{% endhint %}

### `GraphEdge`

```rust
pub struct GraphEdge {
    pub subject: String, // the entity the query was anchored on
    pub object: String,  // a co-occurring entity sharing >= 1 node
    pub weight: u32,      // number of distinct nodes both appear on
}
```

### The `EntityOccurrenceIndex` contract

The graph deliberately does not hard-depend on the occurrence index's concrete
storage (which lives in `memory_store::entities`). Instead, the queries read
through a two-method trait and take any implementation by injection — production
wires in a SQLite-backed adapter; tests inject a small in-memory fixture.

```rust
pub trait EntityOccurrenceIndex {
    /// Distinct node ids on which `entity_id` has been indexed.
    /// No occurrences -> empty vector (not an error).
    fn nodes_for_entity(&self, entity_id: &str) -> Result<Vec<String>>;

    /// Distinct canonical entity ids indexed against `node_id`.
    /// No entities -> empty vector (not an error).
    fn entities_on_node(&self, node_id: &str) -> Result<Vec<String>>;
}
```

The two methods correspond to the two sides of OpenHuman's SQL self-join over
`mem_tree_entity_index`:

- `nodes_for_entity` is the `WHERE a.entity_id = ?` side.
- `entities_on_node` is the `JOIN … ON a.node_id = b.node_id` side.

Both **must return distinct ids** so the derived weight equals
`COUNT(DISTINCT node_id)`, matching the original query semantics.

### Queries

All three live in `src/memory/graph/query.rs` and are re-exported from
`graph::mod`.

#### `co_occurring_entities`

```rust
pub fn co_occurring_entities(
    index: &dyn EntityOccurrenceIndex,
    subject_entity: &str,
    limit: Option<usize>,
) -> Result<Vec<GraphEdge>>
```

The core derivation. It gathers the subject's nodes, fans out to the entities
sharing each node, and counts distinct shared nodes per neighbour:

```text
subject_nodes = index.nodes_for_entity(subject)
for node in subject_nodes:
    for object in index.entities_on_node(node):
        if object == subject: skip          # self-edges excluded
        shared[object].insert(node)          # a set, so duplicates collapse
emit GraphEdge { subject, object, weight = shared[object].len() }
```

A `HashSet` of nodes per neighbour (rather than a bare counter) keeps the
distinct-node count correct even if an index implementation returns the same node
more than once. Results are sorted **weight DESC, then object id ASC** for
deterministic output regardless of the index's iteration order — mirroring the
SQL `ORDER BY weight DESC, object ASC`. `limit` caps the result set; `None`
defaults to `DEFAULT_LIMIT = 100`. Weight is saturated at `u32::MAX`.

This reproduces, in pure Rust over the injected trait, the original OpenHuman
query:

```sql
SELECT b.entity_id AS object, COUNT(DISTINCT a.node_id) AS weight
  FROM mem_tree_entity_index a
  JOIN mem_tree_entity_index b ON a.node_id = b.node_id
 WHERE a.entity_id = ?1 AND b.entity_id <> ?1
 GROUP BY b.entity_id
 ORDER BY weight DESC, object ASC
 LIMIT ?2
```

#### `neighbors`

```rust
pub fn neighbors(
    index: &dyn EntityOccurrenceIndex,
    subject_entity: &str,
    limit: Option<usize>,
) -> Result<Vec<String>>
```

A convenience wrapper around `co_occurring_entities` that drops the weights and
returns just the neighbour entity ids, still in weight-descending order.

#### `group_by_weight`

```rust
pub fn group_by_weight(edges: Vec<GraphEdge>) -> HashMap<u32, Vec<String>>
```

A pure helper that buckets edges by weight, returning `weight -> [object ids]`.
Useful for UIs that render strong vs weak relationships separately. It is kept as
a free function rather than a method on `GraphEdge`.

---

## How the two halves connect

```text
ingest / tree scorer
  └─ emits (entity_id, node_id) rows ──► EntityOccurrenceIndex (mem_tree_entity_index)
  └─ emits canonical ids ─────────────► entities/<kind>/<id>.md  (registry files)

graph::co_occurring_entities(index, "person:alice")
  └─ reads the occurrence index ──────► [GraphEdge { object, weight }, …]
        objects are canonical ids that resolve back to registry files
```

The same canonical-id format ties them together: the ids the scorer writes into
the occurrence index are exactly the ids of the entity markdown files, so a
`GraphEdge.object` can be looked up directly with `get_entity`.

---

## See also

- [Scoring and Extraction](scoring-and-extraction.md) — where entity kinds and
  canonical ids are emitted during ingest.
- [Summary Trees](memory-tree.md) — the tree nodes the occurrence index is keyed
  on.
- [Storage Primitives](storage-primitives.md) — the content store and derived
  indexes the registry and graph build on.
- [Core Concepts](core-concepts.md) — provenance, taint, and the layer boundaries
  this module respects.

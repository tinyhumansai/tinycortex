# Entities and Graph Spec

OpenHuman modules: `memory_entities`, `memory_graph`, plus entity indexes in
`memory_tree::score` and `memory_store::entities`.

## Responsibility

Entities make extracted people, organizations, locations, topics, and
mechanical identifiers addressable. The graph derives relationships from
co-occurrence in tree nodes rather than owning a separate graph database.
TinyCortex has two separate entity surfaces:

- the occurrence index, which is SQLite-backed and used by retrieval/graph
  queries; and
- the user-editable markdown registry, which stores profile-style entity files
  under the content root.

## Entity Extraction Contract

The tree scorer emits extracted entities and topics. Mechanical extractors cover
emails, URLs, handles, and hashtags. LLM extraction covers semantic named
entities and importance. The resolver canonicalizes surface forms into stable
canonical ids.

Canonicalization examples:

- emails lowercased.
- leading `@` removed from handles.
- leading `#` removed from topics/hashtags.
- URLs trim surrounding whitespace but preserve path/query case.
- semantically extracted topics promoted into the canonical entity stream.

Supported entity kinds are `email`, `url`, `handle`, `hashtag`, `person`,
`organization`, `location`, `event`, `product`, `datetime`, `technology`,
`artifact`, `quantity`, `misc`, and `topic`. The markdown registry keeps a
local `EntityKind` enum with the same wire strings so registry files can
round-trip independently of the scoring module.

## Entity Occurrence Index

`memory_store::entities` re-exports the entity index backed by
`mem_tree_entity_index`.

Required operations:

- index one entity occurrence.
- index many entity occurrences.
- index summary-node entity ids when only canonical ids are available.
- lookup entity occurrence.
- list entity ids for a node.
- clear entity index for a node.
- count entity rows.
- transaction-scoped batch indexing for callers that need to commit entity
  rows atomically with a larger write.

Each index row must retain enough node/source information for retrieval,
co-occurrence graph derivation, and topic-tree routing: `entity_id`, `node_id`,
`node_kind`, `entity_kind`, `surface`, `score`, `timestamp_ms`, optional
`tree_id`, and `is_user`.

The `is_user` value is resolved at index time through an injectable
`SelfIdentity` implementation. Storage must not depend directly on a host
identity registry; registry-less hosts use the default no-op resolver.

## Markdown Entity Registry

`memory_entities` stores user-editable entity records under:

```text
<content_root>/entities/<kind>/<canonical_id>.md
```

Front matter fields:

- `id`
- `kind`
- `display_name`
- `aliases`
- `emails`
- `handles`
- `created_at`
- `updated_at`

The body is free-form notes and must be preserved across upserts.

Required API:

- `put_entity(config, Entity) -> Entity`
- `get_entity(config, kind, canonical_id) -> Option<Entity>`
- `list_entities(config, kind) -> Vec<Entity>`
- `lookup_alias(config, kind, needle) -> Option<Entity>`

`put_entity` writes via a same-directory temporary file and atomic rename. File
names are slugged from canonical ids for portability; the YAML `id` field
remains authoritative.

## Entity Handles

Handles should be normalized into records with a handle `kind` and `value`.
Legacy person-store mappings:

- iMessage handle -> `EntityHandle { kind: "imessage", value }`
- email handle -> `emails`
- display name handle -> `aliases`

## Graph Derivation

`memory_graph` premise: the graph is the tree mapped out. If two entities
co-occur on the same tree node, they form an edge. Edge weight is the count of
distinct shared nodes.

Graph query code owns no storage. It reads through an `EntityOccurrenceIndex`
trait with two operations: `nodes_for_entity(entity_id)` and
`entities_on_node(node_id)`. Concrete SQLite adapters and in-memory test
fixtures can both satisfy that contract.

Required API:

- `co_occurring_entities(config, subject, limit) -> Vec<GraphEdge>`
- `neighbors(config, subject, limit) -> Vec<String>`
- `group_by_weight(edges) -> HashMap<u32, Vec<String>>`

`GraphEdge` includes `subject`, `object`, and `weight`.
Results sort by `weight DESC, object ASC`; `group_by_weight` preserves the
incoming order within each weight bucket.

## Non-Goals

The co-occurrence graph does not cover LLM-extracted `(subject, predicate,
object)` triples written through legacy unified graph storage. TinyCortex does
not currently port a unified triple store; adding explicit triples would require
a separate spec and storage surface rather than changing the derived
co-occurrence graph.

## Required Invariants

- Entity markdown files are source of truth for editable entity profiles.
- Upsert must preserve user-edited notes.
- No SQLite dependency in markdown registry; it depends only on workspace/content
  root config.
- Graph is read-only derived state unless a separate triple-store spec is added.
- Entity ids and kinds must remain stable across scoring, registry, graph, and
  retrieval.
- Occurrence indexing is idempotent on `(entity_id, node_id)` and clears a
  node's previous rows before re-indexing when extraction changes.

## TinyCortex Landing Area

```text
src/memory/entities/
src/memory/graph/
src/memory/store/entity_index/
src/memory/score/extract/
```

Port order: entity kinds/types, canonical id resolver, markdown parser/renderer,
alias lookup, occurrence index trait, graph query trait.

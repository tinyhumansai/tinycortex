# Entities and Graph Spec

OpenHuman modules: `memory_entities`, `memory_graph`, plus entity indexes in
`memory_tree::score` and `memory_store::entities`.

## Responsibility

Entities make extracted people, organizations, locations, topics, and
mechanical identifiers addressable. The graph derives relationships from
co-occurrence in tree nodes rather than owning a separate graph database.

## Entity Extraction Contract

The tree scorer emits extracted entities and topics. Mechanical extractors cover
emails, URLs, handles, and hashtags. LLM extraction covers semantic named
entities and importance. The resolver canonicalizes surface forms into stable
canonical ids.

Canonicalization examples:

- emails lowercased.
- leading `@` removed from handles.
- leading `#` removed from topics/hashtags.
- semantically extracted topics promoted into the canonical entity stream.

## Entity Occurrence Index

`memory_store::entities` re-exports the entity index backed by
`mem_tree_entity_index`.

Required operations:

- index one entity occurrence.
- index many entity occurrences.
- lookup entity occurrence.
- list entity ids for a node.
- clear entity index for a node.
- count entity rows.

Each index row must retain enough node/source information for retrieval,
co-occurrence graph derivation, and topic-tree routing.

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

Required API:

- `co_occurring_entities(config, subject, limit) -> Vec<GraphEdge>`
- `neighbors(config, subject, limit) -> Vec<String>`
- `group_by_weight(edges) -> HashMap<u32, Vec<String>>`

`GraphEdge` includes `subject`, `object`, and `weight`.

## Non-Goals

The co-occurrence graph does not cover LLM-extracted `(subject, predicate,
object)` triples written through legacy unified graph storage. TinyCortex must
decide whether to drop that legacy surface or represent triples as separate
markdown/index records.

## Required Invariants

- Entity markdown files are source of truth for editable entity profiles.
- Upsert must preserve user-edited notes.
- No SQLite dependency in markdown registry except through content root config.
- Graph is read-only derived state unless a separate triple-store spec is added.
- Entity ids and kinds must remain stable across scoring, registry, graph, and
  retrieval.

## TinyCortex Landing Area

```text
src/memory/entities/
src/memory/graph/
src/memory/score/extract/
src/memory/score/resolver.rs
```

Port order: entity kinds/types, canonical id resolver, markdown parser/renderer,
alias lookup, occurrence index trait, graph query trait.


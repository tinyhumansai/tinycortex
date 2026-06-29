# Data & Organization

### How do namespaces work?

Namespaces are logical scopes for organizing memories. Each memory belongs to exactly one namespace, and `(namespace, key)` uniquely identifies a memory item.

Common patterns: per-user (`user-123-preferences`), per-domain (`support-tickets`), per-tenant (`company-abc`).

See [Namespaces](../developers/concepts/namespaces.md).

### How does deduplication work?

Ingest is an upsert operation. Same `(namespace, key)` = update, not duplicate. Beyond key-based dedup, TinyCortex also performs semantic deduplication during compression, merging near-duplicate content across sources.

### Can I delete specific memories?

Yes. Delete by key, by namespace, or everything. Permanent and immediate.

See [Delete](../developers/concepts/delete.md).

***

#### Getting Help

* **Discord**: [discord.com/invite/k23Kn8nK](https://discord.com/invite/k23Kn8nK)
* **Reddit**: [r/alphahuman](https://www.reddit.com/r/alphahuman/)
* **GitHub**: [github.com/tinyhumansai](https://github.com/tinyhumansai)
* **Email**: [founders@tinyhumans.ai](mailto:founders@tinyhumans.ai)

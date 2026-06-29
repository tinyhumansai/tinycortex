# Memory Items

A **memory item** is the basic unit of storage. Every piece of knowledge you store in TinyCortex is a memory item.

| Field        | Required | Description                                                              |
| ------------ | -------- | ------------------------------------------------------------------------ |
| `key`        | yes      | Unique identifier within a namespace. Used for upsert and deduplication. |
| `content`    | yes      | The memory text: what the AI should remember.                            |
| `namespace`  | yes      | A scope for organizing related memories.                                 |
| `metadata`   | no       | Arbitrary key-value pairs for tagging and filtering.                     |
| `created_at` | no       | Unix timestamp (seconds) for when the memory was created.                |
| `updated_at` | no       | Unix timestamp (seconds) for when the memory was last updated.           |

If you ingest a memory item with the same `(namespace, key)` as an existing item, the existing item is **updated** rather than duplicated.

```python
from tinyhumansai import MemoryItem

item = MemoryItem(
    key="user-preference-theme",
    content="User prefers dark mode",
    namespace="preferences",
    metadata={"source": "onboarding"},
)
```

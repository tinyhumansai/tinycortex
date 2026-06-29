# Timestamps

Every memory item can carry two timestamps:

* **`created_at`** When the memory was originally created. Useful for time-aware retrieval and decay.
* **`updated_at`** When the memory was last modified. Must be greater than or equal to `created_at`.

Both are **Unix timestamps in seconds**. If not provided, the system assigns them automatically.

## Why Timestamps Matter

Timestamps power TinyCortex's **time-decay model** older memories that aren't accessed naturally lose importance, keeping your memory lean and current.

They also enable **temporal reasoning**. When a user asks "what happened last week?", TinyCortex uses timestamps to surface the right memories not just the most semantically similar ones.

## Example

```python
import time

client.ingest_memory(item={
    "key": "meeting-standup",
    "content": "Team decided to ship v2 by end of March",
    "namespace": "meetings",
    "created_at": time.time(),
})
```

## Validation

The SDK validates timestamps for you:

* Must be non-negative numbers
* Cannot be more than \~100 years in the future
* `updated_at` must be greater than or equal to `created_at`

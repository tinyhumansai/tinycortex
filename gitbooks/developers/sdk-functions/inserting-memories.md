# Inserting Memories

Inserting memory is how your app writes durable knowledge into TinyCortex. This is an **upsert** operation: if the same identity already exists, it updates; otherwise it creates.

## When to Use This

- Persist user preferences and profile facts
- Save conversation outcomes and action items
- Backfill historical memory with timestamps
- Attach metadata for filtering/routing later

## Request Shape

### Required Fields

| Field       | Type   | Description                                                     |
| ----------- | ------ | --------------------------------------------------------------- |
| `title`     | string | Stable memory identifier (often equivalent to key/document id). |
| `content`   | string | The memory text payload.                                        |
| `namespace` | string | Logical scope (for example `preferences`, `crm`, `support`).    |

### Optional Fields

| Field        | Type                    | Description                                                      |
| ------------ | ----------------------- | ---------------------------------------------------------------- |
| `sourceType` | `doc \| chat \| email`  | Source channel for downstream interpretation.                    |
| `metadata`   | object                  | Arbitrary structured tags (user id, tenant, source, etc.).       |
| `priority`   | `high \| medium \| low` | Optional importance hint.                                        |
| `createdAt`  | number                  | Unix timestamp (seconds). Useful for backfilling older memories. |
| `updatedAt`  | number                  | Unix timestamp (seconds), usually >= `createdAt`.                |
| `documentId` | string                  | Explicit external id if you need strict cross-system mapping.    |

## Step-by-Step

1. Choose a namespace.
2. Pick a stable title/key for deduping.
3. Write content and optional metadata.
4. Insert and inspect status (`completed`, `updated`, etc.).

## Example Request Body

```json
{
  "title": "user-preference-theme",
  "content": "User prefers dark mode",
  "namespace": "preferences",
  "sourceType": "doc",
  "metadata": { "source": "onboarding" },
  "priority": "medium"
}
```

## Examples by Language

{% tabs %}
{% tab title="cURL" %}

```bash
# 1) Send one insert/upsert request.
curl -X POST "https://api.tinyhumans.ai/memory/insert" \
  -H "Authorization: Bearer $TINYHUMANS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "user-preference-theme",              # stable identity for this memory
    "content": "User prefers dark mode",          # memory payload
    "namespace": "preferences",                    # retrieval scope
    "sourceType": "doc",                           # optional source channel
    "metadata": {"source": "onboarding"},        # optional tags
    "priority": "medium"                           # optional importance hint
  }'
```

{% endtab %}

{% tab title="TypeScript" %}

```ts
// npm install @tinyhumansai/tinycortex
import { TinyHumanMemoryClient } from "@tinyhumansai/tinycortex";

async function main() {
  // 1) Read API token from environment.
  const token = process.env.TINYHUMANS_TOKEN;
  if (!token) throw new Error("Set TINYHUMANS_TOKEN");

  // 2) Create client.
  const client = new TinyHumanMemoryClient({ token });

  // 3) Insert memory with optional fields.
  const result = await client.insertMemory({
    title: "user-preference-theme", // required identity
    content: "User prefers dark mode", // required payload
    namespace: "preferences", // required scope
    sourceType: "doc", // optional
    metadata: { source: "onboarding", userId: "u_123" }, // optional
    priority: "medium", // optional
  });

  // 4) Inspect status/stats.
  console.log(result.data.status, result.data.stats);
}

main().catch(console.error);
```

{% endtab %}

{% tab title="Python" %}

```python
import os
import tinyhumansai as api

# 1) Read API token.
token = os.getenv("TINYHUMANS_TOKEN")
if not token:
    raise RuntimeError("Set TINYHUMANS_TOKEN")

# 2) Create client.
client = api.TinyHumanMemoryClient(token=token)

# 3) Insert one memory item (Python maps key -> title under the hood).
result = client.ingest_memory(
    item={
        "key": "user-preference-theme",                 # stable key
        "content": "User prefers dark mode",            # payload
        "namespace": "preferences",                     # scope
        "metadata": {"source": "onboarding", "userId": "u_123"},
        # Optional timestamp fields for backfill:
        # "created_at": 1700000000,
        # "updated_at": 1700000100,
    }
)

# 4) Ingested vs updated counters.
print(result.ingested, result.updated, result.errors)
```

{% endtab %}

{% tab title="Go" %}

```go
package main

import (
	"fmt"
	"log"
	"os"
	"time"

	"github.com/tinyhumansai/tinycortex-sdk-go/tinyhumans"
)

func main() {
	// 1) Read token.
	token := os.Getenv("TINYHUMANS_TOKEN")
	if token == "" {
		log.Fatal("set TINYHUMANS_TOKEN")
	}

	// 2) Create client.
	client, err := tinyhumans.NewClient(token)
	if err != nil {
		log.Fatal(err)
	}
	defer client.Close()

	// 3) Optional explicit timestamps.
	now := float64(time.Now().Unix())

	// 4) Insert one memory item.
	resp, err := client.IngestMemory(tinyhumans.MemoryItem{
		Key:       "user-preference-theme",                 // identity
		Content:   "User prefers dark mode",                // payload
		Namespace: "preferences",                           // scope
		Metadata:  map[string]interface{}{"source": "onboarding", "userId": "u_123"},
		CreatedAt: &now,                                     // optional
		UpdatedAt: &now,                                     // optional
	})
	if err != nil {
		log.Fatal(err)
	}

	// 5) Print result counters.
	fmt.Println(resp.Ingested, resp.Updated, resp.Errors)
}
```

{% endtab %}

{% tab title="Rust" %}

```rust
use std::env;
use tinyhumansai::{InsertMemoryParams, TinyHumanConfig, TinyHumanMemoryClient, SourceType, Priority};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1) Read token.
    let token = env::var("TINYHUMANS_TOKEN")?;

    // 2) Create client.
    let client = TinyHumanMemoryClient::new(TinyHumanConfig::new(token))?;

    // 3) Insert one memory item with optional fields.
    let response = client
        .insert_memory(InsertMemoryParams {
            title: "user-preference-theme".into(),       // required identity
            content: "User prefers dark mode".into(),    // required payload
            namespace: "preferences".into(),             // required scope
            source_type: Some(SourceType::Doc),           // optional
            priority: Some(Priority::Medium),             // optional
            metadata: Some(serde_json::json!({
                "source": "onboarding",
                "userId": "u_123"
            })),
            ..Default::default()
        })
        .await?;

    // 4) Inspect status.
    println!("{:?}", response.data.status);
    Ok(())
}
```

{% endtab %}

{% tab title="Java" %}

```java
import java.util.Map;
import xyz.tinyhuman.sdk.*;

public class InsertExample {
    public static void main(String[] args) {
        // 1) Read token.
        String token = System.getenv("TINYHUMANS_TOKEN");
        if (token == null || token.isEmpty()) throw new RuntimeException("Set TINYHUMANS_TOKEN");

        // 2) Create client and insert memory.
        try (TinyHumanMemoryClient client = new TinyHumanMemoryClient(token)) {
            InsertMemoryResponse response = client.insertMemory(
                new InsertMemoryParams("user-preference-theme", "User prefers dark mode", "preferences")
                    .setSourceType("doc")
                    .setMetadata(Map.of("source", "onboarding", "userId", "u_123"))
                    .setPriority("medium")
            );

            // 3) Inspect status/stats.
            System.out.println(response.getStatus());
            System.out.println(response.getStats());
        }
    }
}
```

{% endtab %}

{% tab title="C++" %}

```cpp
#include "tinyhuman/tinyhuman.hpp"

#include <cstdlib>
#include <iostream>
#include <stdexcept>

using namespace tinyhuman;

int main() {
    // 1) Read token.
    const char* token = std::getenv("TINYHUMANS_TOKEN");
    if (!token) throw std::runtime_error("Set TINYHUMANS_TOKEN");

    // 2) Create client.
    TinyHumanMemoryClient client(token);

    // 3) Build insert params (required + optional).
    InsertMemoryParams params;
    params
        .set_title("user-preference-theme")
        .set_content("User prefers dark mode")
        .set_namespace("preferences")
        .set_source_type("doc")
        .set_metadata({{"source", "onboarding"}, {"userId", "u_123"}})
        .set_priority("medium");

    // 4) Insert and inspect status.
    auto response = client.insert_memory(params);
    std::cout << response.status << std::endl;
    return 0;
}
```

{% endtab %}
{% endtabs %}

## Response Notes

- `status`: operation status (for example completed/updated)
- `stats`: backend insert statistics when available
- `usage`: token/cost metadata when enabled

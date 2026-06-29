# Recalling Memories

Recall retrieves the most relevant memory context for downstream reasoning. Think of this as building a **prompt-ready memory snapshot**.

## When to Use This

- Build system context before an LLM response
- Fetch tenant/user-specific background quickly
- Pull compact context instead of scanning all stored memory

## Request Fields

### Optional Fields

| Field | Type | Description |
| --- | --- | --- |
| `namespace` | string | Scope retrieval to one namespace. Highly recommended in production. |
| `maxChunks` | number | Maximum number of chunks to return (default usually 10). |

## Step-by-Step

1. Choose namespace scope.
2. Set chunk budget (`maxChunks` / `num_chunks`).
3. Execute recall.
4. Inject returned context into your LLM flow.

## Example Request Body

```json
{
  "namespace": "preferences",
  "maxChunks": 10
}
```

## Examples by Language

{% tabs %}
{% tab title="cURL" %}
```bash
# 1) Recall top chunks for a namespace.
curl -X POST "https://api.tinyhumans.ai/memory/recall" \
  -H "Authorization: Bearer $TINYHUMANS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "preferences",  # scope
    "maxChunks": 10                # chunk budget
  }'
```
{% endtab %}

{% tab title="TypeScript" %}
```ts
// npm install @tinyhumansai/tinycortex
import { TinyHumanMemoryClient } from "@tinyhumansai/tinycortex";

async function main() {
  // 1) Token + client.
  const token = process.env.TINYHUMANS_TOKEN;
  if (!token) throw new Error("Set TINYHUMANS_TOKEN");
  const client = new TinyHumanMemoryClient({ token });

  // 2) Recall context.
  const result = await client.recallMemory({
    namespace: "preferences",    // optional but recommended
    maxChunks: 10,                // optional
  });

  // 3) Use the LLM context message.
  console.log(result.data.llmContextMessage);
  console.log(result.data.counts);
}

main().catch(console.error);
```
{% endtab %}

{% tab title="Python" %}
```python
import os
import tinyhumansai as api

# 1) Token + client.
token = os.getenv("TINYHUMANS_TOKEN")
if not token:
    raise RuntimeError("Set TINYHUMANS_TOKEN")
client = api.TinyHumanMemoryClient(token=token)

# 2) Prompt-driven recall (Python-specific high-level flow).
ctx = client.recall_memory(
    namespace="preferences",
    prompt="What are this user's strongest preferences?",  # required in this method
    num_chunks=10,                                          # optional
    # key="user-preference-theme",                         # optional exact key
    # keys=["k1", "k2"],                                 # optional exact keys
)

# 3) Context string is ready for LLM system prompts.
print(ctx.context)
print(ctx.count)
```
{% endtab %}

{% tab title="Go" %}
```go
package main

import (
	"fmt"
	"log"
	"os"

	"github.com/tinyhumansai/tinycortex-sdk-go/tinyhumans"
)

func main() {
	// 1) Token + client.
	token := os.Getenv("TINYHUMANS_TOKEN")
	if token == "" {
		log.Fatal("set TINYHUMANS_TOKEN")
	}
	client, err := tinyhumans.NewClient(token)
	if err != nil {
		log.Fatal(err)
	}
	defer client.Close()

	// 2) Recall context.
	ctx, err := client.RecallMemory(
		"preferences",
		"What does the user prefer?",
		&tinyhumans.RecallMemoryOptions{NumChunks: 10}, // optional tuning
	)
	if err != nil {
		log.Fatal(err)
	}

	// 3) Use LLM-ready context.
	fmt.Println(ctx.Context)
	fmt.Println(ctx.Count)
}
```
{% endtab %}

{% tab title="Rust" %}
```rust
use std::env;
use tinyhumansai::{RecallMemoryParams, TinyHumanConfig, TinyHumanMemoryClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1) Token + client.
    let token = env::var("TINYHUMANS_TOKEN")?;
    let client = TinyHumanMemoryClient::new(TinyHumanConfig::new(token))?;

    // 2) Recall context with optional fields.
    let response = client
        .recall_memory(RecallMemoryParams {
            namespace: Some("preferences".into()),
            max_chunks: Some(10.0),
        })
        .await?;

    // 3) Access LLM context and counts.
    println!("{:?}", response.data.llm_context_message);
    println!("{:?}", response.data.counts);
    Ok(())
}
```
{% endtab %}

{% tab title="Java" %}
```java
import xyz.tinyhuman.sdk.*;

public class RecallExample {
    public static void main(String[] args) {
        // 1) Token + client.
        String token = System.getenv("TINYHUMANS_TOKEN");
        if (token == null || token.isEmpty()) throw new RuntimeException("Set TINYHUMANS_TOKEN");

        try (TinyHumanMemoryClient client = new TinyHumanMemoryClient(token)) {
            // 2) Recall with optional params.
            RecallMemoryResponse response = client.recallMemory(
                new RecallMemoryParams()
                    .setNamespace("preferences")
                    .setMaxChunks(10)
            );

            // 3) Use returned context.
            System.out.println(response.getLlmContextMessage());
            System.out.println(response.getCounts());
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
    // 1) Token + client.
    const char* token = std::getenv("TINYHUMANS_TOKEN");
    if (!token) throw std::runtime_error("Set TINYHUMANS_TOKEN");
    TinyHumanMemoryClient client(token);

    // 2) Recall with optional params.
    RecallMemoryParams params;
    params
        .set_namespace("preferences")
        .set_max_chunks(10);

    auto response = client.recall_memory(params);

    // 3) Use returned context.
    if (response.llm_context_message) {
        std::cout << *response.llm_context_message << std::endl;
    }
    return 0;
}
```
{% endtab %}
{% endtabs %}

## Response Notes

- `llmContextMessage`: compact prompt-ready memory context
- `counts`: retrieval diagnostics (entities/relations/chunks)
- `cached`: cache hit indicator when available

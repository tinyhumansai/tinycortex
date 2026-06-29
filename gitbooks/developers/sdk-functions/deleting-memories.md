# Deleting Memories

Delete removes memory immediately from a namespace. Use this when you need deterministic cleanup (privacy requests, data reset, bad data rollback).

## When to Use This

- User requests data removal
- Memory is incorrect or unsafe
- You are resetting a namespace before re-ingest

## Request Fields

### Required Fields

| Field | Type | Description |
| --- | --- | --- |
| `namespace` | string | Namespace to clear. |

## Step-by-Step

1. Choose namespace to remove.
2. Call delete endpoint.
3. Validate deleted count from response.

## Example Request Body

```json
{
  "namespace": "preferences"
}
```

## Examples by Language

{% tabs %}
{% tab title="cURL" %}
```bash
# 1) Delete all memory in one namespace.
curl -X POST "https://api.tinyhumans.ai/memory/admin/delete" \
  -H "Authorization: Bearer $TINYHUMANS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"namespace": "preferences"}'
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

  // 2) Delete namespace.
  const result = await client.deleteMemory({
    namespace: "preferences", // required
  });

  // 3) Verify deletion count.
  console.log(result.data.nodesDeleted);
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

# 2) Delete namespace memory (SDK requires delete_all confirmation).
response = client.delete_memory(
    namespace="preferences",
    delete_all=True,
    # key="k1",             # currently not used by backend delete route
    # keys=["k1", "k2"],   # currently not used by backend delete route
)

# 3) Verify count.
print(response.deleted)
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

	// 2) Delete namespace.
	resp, err := client.DeleteMemory("preferences", nil)
	if err != nil {
		log.Fatal(err)
	}

	// 3) Verify count.
	fmt.Println(resp.Deleted)
}
```
{% endtab %}

{% tab title="Rust" %}
```rust
use std::env;
use tinyhumansai::{DeleteMemoryParams, TinyHumanConfig, TinyHumanMemoryClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1) Token + client.
    let token = env::var("TINYHUMANS_TOKEN")?;
    let client = TinyHumanMemoryClient::new(TinyHumanConfig::new(token))?;

    // 2) Delete namespace.
    let response = client
        .delete_memory(DeleteMemoryParams {
            namespace: Some("preferences".into()),
        })
        .await?;

    // 3) Verify count.
    println!("{}", response.data.nodes_deleted);
    Ok(())
}
```
{% endtab %}

{% tab title="Java" %}
```java
import xyz.tinyhuman.sdk.*;

public class DeleteExample {
    public static void main(String[] args) {
        // 1) Token + client.
        String token = System.getenv("TINYHUMANS_TOKEN");
        if (token == null || token.isEmpty()) throw new RuntimeException("Set TINYHUMANS_TOKEN");

        try (TinyHumanMemoryClient client = new TinyHumanMemoryClient(token)) {
            // 2) Delete namespace.
            DeleteMemoryResponse response = client.deleteMemory(
                new DeleteMemoryParams().setNamespace("preferences")
            );

            // 3) Verify count.
            System.out.println(response.getNodesDeleted());
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

    // 2) Delete namespace.
    DeleteMemoryParams params;
    params.set_namespace("preferences");
    auto response = client.delete_memory(params);

    // 3) Verify count.
    std::cout << response.nodes_deleted << std::endl;
    return 0;
}
```
{% endtab %}
{% endtabs %}

## Response Notes

- `nodesDeleted` / `deleted`: number of records removed
- deletion is immediate and irreversible

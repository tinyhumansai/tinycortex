# SDK Functions

This section documents SDK setup and core TinyCortex memory operations across TypeScript, Python, Go, Rust, Java, C++, and cURL.

## Get API Access

TinyCortex is currently in closed alpha.

1. Request access: email [founders@tinyhumans.ai](mailto:founders@tinyhumans.ai) with your use case.
2. Wait for approval.
3. Receive your server API key.

## Environment Setup

```bash
export TINYHUMANS_TOKEN="YOUR_API_KEY"
export TINYHUMANS_BASE_URL="https://api.tinyhumans.ai" # optional
```

{% hint style="warning" %}
Never expose server API keys in frontend/browser code or commit them to git.
{% endhint %}

## Install by Language

{% tabs %}
{% tab title="TypeScript" %}
```bash
npm install @tinyhumansai/tinycortex
```

```ts
import { TinyHumanMemoryClient } from "@tinyhumansai/tinycortex";
const client = new TinyHumanMemoryClient({ token: process.env.TINYHUMANS_TOKEN ?? "" });
```
{% endtab %}

{% tab title="Python" %}
```bash
pip install tinyhumansai
```

```python
import os
import tinyhumansai as api
client = api.TinyHumanMemoryClient(token=os.getenv("TINYHUMANS_TOKEN"))
```
{% endtab %}

{% tab title="Go" %}
```bash
go get github.com/tinyhumansai/tinycortex-sdk-go
```

```go
client, err := tinyhumans.NewClient(os.Getenv("TINYHUMANS_TOKEN"))
if err != nil { log.Fatal(err) }
defer client.Close()
```
{% endtab %}

{% tab title="Rust" %}
Add to `Cargo.toml`:

```toml
[dependencies]
tinyhumansai = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use tinyhumansai::{TinyHumanConfig, TinyHumanMemoryClient};
let client = TinyHumanMemoryClient::new(TinyHumanConfig::new(std::env::var("TINYHUMANS_TOKEN")?))?;
```
{% endtab %}

{% tab title="Java" %}
```bash
cd packages/sdk-java
gradle build
```

```java
import xyz.tinyhuman.sdk.*;
TinyHumanMemoryClient client = new TinyHumanMemoryClient(System.getenv("TINYHUMANS_TOKEN"));
```
{% endtab %}

{% tab title="C++" %}
```bash
cd packages/sdk-cpp
make build
```

```cpp
#include "tinyhuman/tinyhuman.hpp"
tinyhuman::TinyHumanMemoryClient client(std::getenv("TINYHUMANS_TOKEN"));
```
{% endtab %}

{% tab title="cURL" %}
```bash
curl -X POST "https://api.tinyhumans.ai/memory/recall" \
  -H "Authorization: Bearer $TINYHUMANS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"namespace":"preferences","maxChunks":5}'
```
{% endtab %}
{% endtabs %}

## Core Operations

| Operation | TypeScript | Python | Go | Rust | Java | C++ |
| --- | --- | --- | --- | --- | --- | --- |
| Ingest / Insert | `insertMemory` | `ingest_memory` | `IngestMemory` | `insert_memory` | `insertMemory` | `insert_memory` |
| Recall Context | `recallMemory` | `recall_memory` | `RecallMemory` | `recall_memory` | `recallMemory` | `recall_memory` |
| Delete | `deleteMemory` | `delete_memory` | `DeleteMemory` | `delete_memory` | `deleteMemory` | `delete_memory` |

Next:

- [Inserting Memories](inserting-memories.md)
- [Recalling Memories](recalling-memories.md)
- [Deleting Memories](deleting-memories.md)

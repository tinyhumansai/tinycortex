# Privacy Architecture

TinyCortex processes sensitive conversational data. The privacy architecture is designed around a simple principle: users own their data, and the system should retain as little as possible for as short as necessary.

### What Stays Local vs. What Is Processed

TinyCortex operates as an API service. Conversation data flows through the compression pipeline and is stored as a compressed knowledge graph.

**Sent to TinyCortex API:**

* Conversation text (messages, tool outputs, system context)
* User/session identifiers for memory association
* Query requests for memory retrieval

**Stored in TinyCortex:**

* Compressed knowledge graph (entities, relationships, relevance scores)
* Session metadata (timestamps, interaction counts for decay calculations)
* User-scoped memory partitions

**Not stored:**

* Raw conversation text (discarded after compression pipeline completes)
* LLM outputs or model responses
* Authentication credentials or API keys from connected services

The compression pipeline is destructive by design. Once text passes through the five stages, the original content no longer exists in the system. Only the structured graph representation remains.

### Encryption

**In transit:** All API communication uses TLS 1.2+ encryption. No data is sent in plaintext.

**At rest:** Stored knowledge graphs are encrypted at rest using AES-256. Each user's memory partition is encrypted independently.

Key management: Encryption keys are managed through the infrastructure provider's key management service. Keys are rotated on a regular schedule.

#### Zero Data Retention Policy <a href="#zero-data-retention-policy" id="zero-data-retention-policy"></a>

TinyCortex does not retain raw conversation data beyond the processing window. Once the compression pipeline completes:

1. The raw text input is discarded
2. Intermediate representations (from stages 1-4) are discarded
3. Only the final compressed graph (stage 5 output) is persisted

There is no logging of conversation content for debugging, analytics, or quality improvement. API request logs contain metadata (timestamps, response codes, latency) but not message content.

#### No Model Training <a href="#no-model-training" id="no-model-training"></a>

User data is never used to train, fine-tune, or evaluate models. TinyCortex does not operate its own language models. It is a memory layer, not a model provider.The LLM calls made during the compression pipeline (for entity extraction and relationship mapping) are made to third-party model providers under their respective data processing agreements. These calls use ephemeral sessions with no data retention enabled.

#### User-Sovereign Deletion <a href="#user-sovereign-deletion" id="user-sovereign-deletion"></a>

Users can delete their memory at any time through the API:# Delete all memory for a usercurl -X DELETE \<https://api.tinyhumans.ai/v1/memory/{user\_id}> \\\\-H "Authorization: Bearer YOUR\_API\_KEY"​# Delete a specific memory nodecurl -X DELETE \<https://api.tinyhumans.ai/v1/memory/{user\_id}/nodes/{node\_id}> \\\\-H "Authorization: Bearer YOUR\_API\_KEY"Deletion is immediate and irreversible:

* The compressed graph nodes and edges are permanently removed
* Associated metadata (timestamps, interaction counts) is permanently removed
* There is no soft-delete or recovery period
* Deletion propagates to all replicas and backups within 24 hours

#### Data Isolation <a href="#data-isolation" id="data-isolation"></a>

Each API key maps to an isolated data partition. Memory stored under one API key is never accessible from another, even within the same organization account.Within a partition, memory is further scoped by user ID. There is no cross-user memory access unless explicitly configured by the developer through the API.

#### Compliance Considerations <a href="#compliance-considerations" id="compliance-considerations"></a>

TinyCortex's architecture supports compliance with data protection regulations:

* **Right to erasure (GDPR Art. 17):** Covered by the deletion API. All user data can be permanently removed on request.
* **Data minimization (GDPR Art. 5):** The compression pipeline inherently minimizes stored data. Raw text is not retained.
* **Purpose limitation:** Stored data is used exclusively for memory retrieval. No secondary processing, profiling, or analytics.

For specific compliance requirements, contact the Tiny Humans team at [hello@tinyhumans.ai](mailto:hello@tinyhumans.ai).

#### Summary <a href="#summary" id="summary"></a>

| Concern               | How TinyCortex Handles It                   |
| --------------------- | ------------------------------------------ |
| Raw data retention    | Discarded after compression. Not stored.   |
| Encryption in transit | TLS 1.2+                                   |
| Encryption at rest    | AES-256, per-user partitions               |
| Model training        | Never. User data is not used for training. |
| Deletion              | Immediate, irreversible, API-accessible    |
| Cross-user isolation  | Enforced at API key and user ID levels     |
| Logging               | Metadata only. No conversation content.    |

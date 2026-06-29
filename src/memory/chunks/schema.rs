//! The chunk DB schema (pure SQL DDL).
//!
//! Applied once per DB path by [`super::connection`]. The schema declares the
//! full `mem_tree_*` table family used by the wider OpenHuman memory-tree
//! subsystem; the chunk store here only reads/writes the chunk-centric subset
//! (`mem_tree_chunks`, its embedding sidecars + tombstones, the score and
//! entity-index tables it cascade-deletes, and the source ingest gate). The
//! remaining tables are created so future modules (tree, queue, score,
//! retrieval) can share the same database file without a second migration step.

/// `CREATE TABLE IF NOT EXISTS …` statements for the whole chunk DB.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS mem_tree_chunks (
    id                     TEXT PRIMARY KEY,
    source_kind            TEXT NOT NULL,
    source_id              TEXT NOT NULL,
    path_scope             TEXT,
    source_ref             TEXT,
    owner                  TEXT NOT NULL,
    timestamp_ms           INTEGER NOT NULL,
    time_range_start_ms    INTEGER NOT NULL,
    time_range_end_ms      INTEGER NOT NULL,
    tags_json              TEXT NOT NULL DEFAULT '[]',
    content                TEXT NOT NULL,
    token_count            INTEGER NOT NULL,
    seq_in_source          INTEGER NOT NULL,
    created_at_ms          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_source
    ON mem_tree_chunks(source_kind, source_id);
CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_timestamp
    ON mem_tree_chunks(timestamp_ms);
CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_owner
    ON mem_tree_chunks(owner);
CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_source_seq
    ON mem_tree_chunks(source_kind, source_id, seq_in_source);

CREATE TABLE IF NOT EXISTS mem_tree_chunk_embeddings (
    chunk_id               TEXT NOT NULL REFERENCES mem_tree_chunks(id) ON DELETE CASCADE,
    model_signature        TEXT NOT NULL,
    vector                 BLOB NOT NULL,
    dim                    INTEGER NOT NULL,
    created_at             REAL NOT NULL,
    PRIMARY KEY (chunk_id, model_signature)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_chunk_embeddings_model
    ON mem_tree_chunk_embeddings(model_signature);

CREATE TABLE IF NOT EXISTS mem_tree_chunk_reembed_skipped (
    chunk_id               TEXT NOT NULL REFERENCES mem_tree_chunks(id) ON DELETE CASCADE,
    model_signature        TEXT NOT NULL,
    reason                 TEXT NOT NULL,
    skipped_at_ms          INTEGER NOT NULL,
    PRIMARY KEY (chunk_id, model_signature)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_chunk_reembed_skipped_model
    ON mem_tree_chunk_reembed_skipped(model_signature);

CREATE TABLE IF NOT EXISTS mem_tree_score (
    chunk_id               TEXT PRIMARY KEY,
    total                  REAL NOT NULL,
    token_count_signal     REAL NOT NULL,
    unique_words_signal    REAL NOT NULL,
    metadata_weight        REAL NOT NULL,
    source_weight          REAL NOT NULL,
    interaction_weight     REAL NOT NULL,
    entity_density         REAL NOT NULL,
    dropped                INTEGER NOT NULL DEFAULT 0,
    reason                 TEXT,
    computed_at_ms         INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_score_total
    ON mem_tree_score(total);
CREATE INDEX IF NOT EXISTS idx_mem_tree_score_dropped
    ON mem_tree_score(dropped);

CREATE TABLE IF NOT EXISTS mem_tree_entity_index (
    entity_id              TEXT NOT NULL,
    node_id                TEXT NOT NULL,
    node_kind              TEXT NOT NULL,
    entity_kind            TEXT NOT NULL,
    surface                TEXT NOT NULL,
    score                  REAL NOT NULL,
    timestamp_ms           INTEGER NOT NULL,
    tree_id                TEXT,
    is_user                INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (entity_id, node_id)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_entity
    ON mem_tree_entity_index(entity_id);
CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_node
    ON mem_tree_entity_index(node_id);
CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_timestamp
    ON mem_tree_entity_index(timestamp_ms);

CREATE TABLE IF NOT EXISTS mem_tree_entity_edges (
    entity_a               TEXT NOT NULL,
    entity_b               TEXT NOT NULL,
    weight                 INTEGER NOT NULL DEFAULT 1,
    updated_ms             INTEGER NOT NULL,
    PRIMARY KEY (entity_a, entity_b)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_edges_a
    ON mem_tree_entity_edges(entity_a);
CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_edges_b
    ON mem_tree_entity_edges(entity_b);

CREATE TABLE IF NOT EXISTS mem_tree_trees (
    id                     TEXT PRIMARY KEY,
    kind                   TEXT NOT NULL,
    scope                  TEXT NOT NULL,
    root_id                TEXT,
    max_level              INTEGER NOT NULL DEFAULT 0,
    status                 TEXT NOT NULL DEFAULT 'active',
    created_at_ms          INTEGER NOT NULL,
    last_sealed_at_ms      INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_mem_tree_trees_kind_scope
    ON mem_tree_trees(kind, scope);
CREATE INDEX IF NOT EXISTS idx_mem_tree_trees_status
    ON mem_tree_trees(status);

CREATE TABLE IF NOT EXISTS mem_tree_summaries (
    id                     TEXT PRIMARY KEY,
    tree_id                TEXT NOT NULL,
    tree_kind              TEXT NOT NULL,
    level                  INTEGER NOT NULL,
    parent_id              TEXT,
    child_ids_json         TEXT NOT NULL DEFAULT '[]',
    content                TEXT NOT NULL,
    token_count            INTEGER NOT NULL,
    entities_json          TEXT NOT NULL DEFAULT '[]',
    topics_json            TEXT NOT NULL DEFAULT '[]',
    time_range_start_ms    INTEGER NOT NULL,
    time_range_end_ms      INTEGER NOT NULL,
    score                  REAL NOT NULL DEFAULT 0.0,
    sealed_at_ms           INTEGER NOT NULL,
    deleted                INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (tree_id) REFERENCES mem_tree_trees(id)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_tree_level
    ON mem_tree_summaries(tree_id, level);
CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_parent
    ON mem_tree_summaries(parent_id);
CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_sealed_at
    ON mem_tree_summaries(sealed_at_ms);
CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_deleted
    ON mem_tree_summaries(deleted);

CREATE TABLE IF NOT EXISTS mem_tree_summary_embeddings (
    summary_id             TEXT NOT NULL REFERENCES mem_tree_summaries(id) ON DELETE CASCADE,
    model_signature        TEXT NOT NULL,
    vector                 BLOB NOT NULL,
    dim                    INTEGER NOT NULL,
    created_at             REAL NOT NULL,
    PRIMARY KEY (summary_id, model_signature)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_summary_embeddings_model
    ON mem_tree_summary_embeddings(model_signature);

CREATE TABLE IF NOT EXISTS mem_tree_summary_reembed_skipped (
    summary_id             TEXT NOT NULL REFERENCES mem_tree_summaries(id) ON DELETE CASCADE,
    model_signature        TEXT NOT NULL,
    reason                 TEXT NOT NULL,
    skipped_at_ms          INTEGER NOT NULL,
    PRIMARY KEY (summary_id, model_signature)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_summary_reembed_skipped_model
    ON mem_tree_summary_reembed_skipped(model_signature);

CREATE TABLE IF NOT EXISTS mem_tree_buffers (
    tree_id                TEXT NOT NULL,
    level                  INTEGER NOT NULL,
    item_ids_json          TEXT NOT NULL DEFAULT '[]',
    token_sum              INTEGER NOT NULL DEFAULT 0,
    oldest_at_ms           INTEGER,
    updated_at_ms          INTEGER NOT NULL,
    PRIMARY KEY (tree_id, level),
    FOREIGN KEY (tree_id) REFERENCES mem_tree_trees(id)
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_buffers_oldest
    ON mem_tree_buffers(oldest_at_ms);

CREATE TABLE IF NOT EXISTS mem_tree_entity_hotness (
    entity_id              TEXT PRIMARY KEY,
    mention_count_30d      INTEGER NOT NULL DEFAULT 0,
    distinct_sources       INTEGER NOT NULL DEFAULT 0,
    last_seen_ms           INTEGER,
    query_hits_30d         INTEGER NOT NULL DEFAULT 0,
    graph_centrality       REAL,
    ingests_since_check    INTEGER NOT NULL DEFAULT 0,
    last_hotness           REAL,
    last_updated_ms        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_hotness_score
    ON mem_tree_entity_hotness(last_hotness);

CREATE TABLE IF NOT EXISTS mem_tree_jobs (
    id                     TEXT PRIMARY KEY,
    kind                   TEXT NOT NULL,
    payload_json           TEXT NOT NULL,
    dedupe_key             TEXT,
    status                 TEXT NOT NULL DEFAULT 'ready',
    attempts               INTEGER NOT NULL DEFAULT 0,
    max_attempts           INTEGER NOT NULL DEFAULT 5,
    available_at_ms        INTEGER NOT NULL,
    locked_until_ms        INTEGER,
    last_error             TEXT,
    created_at_ms          INTEGER NOT NULL,
    started_at_ms          INTEGER,
    completed_at_ms        INTEGER,
    failure_reason         TEXT,
    failure_class          TEXT
);

CREATE INDEX IF NOT EXISTS idx_mem_tree_jobs_ready
    ON mem_tree_jobs(status, available_at_ms);
CREATE INDEX IF NOT EXISTS idx_mem_tree_jobs_kind
    ON mem_tree_jobs(kind);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mem_tree_jobs_dedupe_active
    ON mem_tree_jobs(dedupe_key)
    WHERE dedupe_key IS NOT NULL AND status IN ('ready', 'running');

CREATE TABLE IF NOT EXISTS mem_tree_ingested_sources (
    source_kind            TEXT NOT NULL,
    source_id              TEXT NOT NULL,
    ingested_at_ms         INTEGER NOT NULL,
    PRIMARY KEY (source_kind, source_id)
);

CREATE TABLE IF NOT EXISTS mcp_writes (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp_ms           INTEGER NOT NULL,
    client_info            TEXT NOT NULL,
    tool_name              TEXT NOT NULL,
    args_summary           TEXT,
    resulting_chunk_id     TEXT,
    success                INTEGER NOT NULL,
    error_message          TEXT
);

CREATE INDEX IF NOT EXISTS idx_mcp_writes_timestamp
    ON mcp_writes(timestamp_ms DESC);
CREATE INDEX IF NOT EXISTS idx_mcp_writes_client
    ON mcp_writes(client_info);
CREATE INDEX IF NOT EXISTS idx_mcp_writes_tool
    ON mcp_writes(tool_name);
";

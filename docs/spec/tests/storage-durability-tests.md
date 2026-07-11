# Feature-Test Spec — Storage & Durability

Scope: `src/memory/store/content/*` (content store, atomic writes, YAML
front-matter), `src/memory/store/kv.rs` (KV store), `src/memory/store/vectors/*`
(vector store), `src/memory/store/entity_index/*` (entity index), `src/memory/
chunks/*` (chunk store, connection cache, migrations, corruption recovery,
delete cascade, re-ingest/produce). Finding IDs (`SC-*`) are from
[`docs/spec/audit/01-store-chunks.md`](../audit/01-store-chunks.md); phase
numbers reference [`docs/spec/improvement-plan.md`](../improvement-plan.md).

This document specifies *what* must be tested and the given/when/then shape of
each case. It intentionally does not restate the fix design already captured
in the audit — write the test first against current (buggy) behavior where
that is instructive (regression cases should fail against `main` and pass after
the corresponding improvement-plan item lands), then keep it green afterward.

## 1. Harness & fixtures

Most of what's needed already exists per-module as `*_tests.rs` companions
(`kv_tests.rs`, `store_tests.rs`, `recovery_tests.rs`, `atomic_tests.rs`, etc.)
using `tempfile::TempDir` + `MemoryConfig::new(tmp.path())` and in-memory opens
(`KvStore::open_in_memory`, `VectorStore::open_in_memory`,
`EntityIndex::open_in_memory`) for the non-file-contention cases. The gaps are:

1. **Shared crash-injection harness** (new — `src/memory/chunks/crash_test_util.rs`,
   `#[cfg(test)]`-only, referenced via `#[path]` from each subsystem's
   `*_tests.rs`, mirroring the existing pattern of test-only helpers living
   beside the module they support, e.g. `connection_tests.rs`'s use of
   `schema_apply_count_for_path_for_tests`). Needs:
   - `truncate_after(path, n)` — reopen a file and truncate to the first `n`
     bytes, simulating a crash mid-`write_all` before `fsync`. Used against the
     `.tmp_<uuid>.md` staging files atomic.rs produces (the file name pattern
     is deterministic enough from a fixed test seed / by scanning the parent
     dir for `.tmp_*` immediately after a write is issued from a second
     thread).
   - `delete_between_rename(path_a, path_b)` helper that races a background
     thread doing the real write against a foreground thread that deletes the
     temp file the instant it appears (via a `notify`-free poll loop on
     `read_dir`), to simulate "crash after temp-write, before rename".
   - `kill_after_commit_before_side_effect(conn, sql, side_effect_fn)` — runs a
     SQL statement inside an explicit transaction, commits it, and returns
     *before* invoking `side_effect_fn`, so the test can assert the DB-visible
     state and the filesystem/cache side effect can be independently
     re-invoked or shown missing. This directly models SC-3 (rename happens
     after remove, no transaction wraps both) and SC-8 (delete of stale rows
     is not in the same transaction as the upsert).
   - `two_instances_same_path(open_fn)` generic helper: opens two independent
     store handles (`KvStore`, `VectorStore`, `EntityIndex`, or a raw
     `Connection` via `with_connection`) against the same on-disk path from
     two threads/`Arc`s and returns both, for contention tests.
2. **Fake embedding backend.** `EmbeddingBackend` already has a test double
   used by `vectors/store_tests.rs` (`InertEmbedding` referenced in module
   docs) — reuse it; add a `FixedDimBackend { dims, name }` variant if the
   existing fake does not let dims/name vary per test (needed for SC-13 dim
   mismatch cases).
3. **Fake summariser / LLM / entity extractor.** Not needed for this doc's
   scope — chunk/store durability tests operate below the summarisation and
   entity-extraction layers and inject `SummaryComposeInput` / raw SQL rows
   directly, matching the existing style in `atomic_tests.rs` and
   `store_tests.rs`. No new LLM fixture is required here; see "Not in scope".
4. **Interleaving helper.** A tiny `run_interleaved(steps: Vec<Box<dyn FnOnce()>>)`
   is overkill for these cases — every concurrency case below is expressible
   as "two real `std::thread::spawn` calls barrier-synced with
   `std::sync::Barrier`", which is the idiom to use (no new abstraction beyond
   the crash-injection helpers above).
5. **Busy-timeout probe.** A helper `assert_busy_timeout_ms(conn, expected_ms)`
   that reads back `PRAGMA busy_timeout` — used to assert SC-11 fixes/regressions
   without needing to actually contend two writers for the full timeout
   duration in the common case (one test per store does the real contention
   too, see §Priority table, to catch a `busy_timeout` that is set but wired
   to the wrong connection).

All new harness code must live in `#[cfg(test)]`-gated modules colocated with
the subsystem it supports (per house rules: tests in a separate `*_tests.rs`,
never inline `mod tests` in an implementation file); the crash-injection
utilities that are shared across subsystems (chunk store + content store) go
in `src/memory/chunks/crash_test_util.rs` and are `pub(crate)` under
`#[cfg(test)]`, imported by both `chunks/*_tests.rs` and
`store/content/*_tests.rs` via relative `use` paths (both are in-crate).

## 2. Test cases

Legend — **Given/When/Then** columns are one line each. **Findings** column
cites the audit ID(s) this case regresses, or `new coverage` for a gap called
out in "Test-coverage gaps" with no single numbered finding, or `new` for
cases added purely for exhaustiveness. **Pri**: P0 = regression test for a
Critical/Major finding, P1 = major gap not tied to one finding number (still
production-relevant), P2 = nice-to-have / edge polish.

| # | Name | Given | When | Then | Findings | Pri |
|---|------|-------|------|------|----------|-----|
| 1 | `front_matter_missing_trailing_newline_no_panic` | a chunk `.md` file whose bytes end in `\n---` (no final `\n`) | `read_chunk_file` (or `split_front_matter` directly) parses it | it returns `Err`/`None`, never panics or indexes out of bounds | SC-1 | P0 |
| 2 | `front_matter_missing_trailing_newline_body_empty` | same fence-without-newline input | front matter is split | the body is the empty string, front-matter bytes are exactly the header | SC-1 | P0 |
| 3 | `front_matter_body_contains_literal_fence_sequence` | a well-formed file whose *body* contains the literal bytes `\n---\n` | `split_front_matter` runs | the split point is the *first* `\n---\n` after the opening `---\n`, not a later occurrence inside the body | SC-1 (adjacent), new coverage | P1 |
| 4 | `read_chunk_file_on_truncated_disk_file_returns_err` | a chunk file crash-injected via `truncate_after` to a length that lands inside the front-matter closer | `read_chunk_file` reads it | `Err`, and the caller (e.g. `verify_summary_file`) reports `VerifyResult::Corrupt`/`Err`, not a panic | SC-1, new coverage | P0 |
| 5 | `yaml_scalar_escapes_embedded_newline` | a `source_id` value containing `\nowner: attacker` | `yaml_scalar` composes front matter | the emitted value is quoted and the embedded `\n` is escaped (not a raw line break) so the composed front matter has exactly one `owner:` key | SC-2 | P0 |
| 6 | `yaml_injection_value_round_trips_through_split_front_matter` | a chunk composed with a provider-controlled `tags` value containing `\n---\n` | the chunk is written then read back via `read_chunk_file` | the recovered field equals the original string byte-for-byte (no early fence termination, no injected keys) | SC-2 | P0 |
| 7 | `yaml_injection_preserves_content_sha256_match` | a chunk composed with an injection-shaped `owner` field | `content_sha256` is computed at write time and re-verified at read time | the two hashes match (verification does not permanently fail because the body boundary shifted) | SC-2 | P0 |
| 8 | `scan_fm_field_unescape_order_backslash_quote` | a front-matter scalar containing the literal two-char sequence `\"` (escaped backslash then quote, i.e. raw text ends in `\\"` before quoting) | `scan_fm_field` unescapes it | the round-tripped value matches the pre-quoting original exactly | SC-19 | P1 |
| 9 | `stage_summary_replace_crash_between_remove_and_rename_leaves_readable_file` | a stale on-disk summary file (mismatched body sha) and a crash-injection harness that stops after the `remove_file` but before the new file lands | `stage_summary` is invoked and the injected crash fires | **regression case**: on current code, the file is momentarily/permanently absent after crash — assert the *fixed* behavior once implemented: the old file remains present with its old (stale) content until the new file is atomically renamed into place, i.e. never a window with no file at that path | SC-3 | P0 |
| 10 | `stage_summary_replace_uses_temp_then_rename` | a stale on-disk summary file | `stage_summary` re-stages it | a `.tmp_*` file appears in the parent directory during the write and is gone afterward (proves temp+rename, not remove+write) — fails against current `remove_file` implementation until SC-3 fix lands | SC-3 | P0 |
| 11 | `stage_summary_concurrent_reader_never_observes_missing_file` | two threads: one repeatedly `stat`s the summary path in a tight loop, the other calls `stage_summary` to replace stale content | both run concurrently, barrier-synced to start together | the reader thread never observes a `NotFound` for a path that existed before the call started | SC-3 | P0 |
| 12 | `wal_not_deleted_on_io_open_error_for_legacy_wal_mode_db` | a `chunks.db` explicitly left in `journal_mode=WAL` (legacy path) with a `-wal` file containing a committed, uncheckpointed transaction (write a row, do not checkpoint, simulate the crash-classified error) | `get_or_init_connection` hits `is_io_open_error` and runs its cleanup-then-retry path | the `-wal` file is not unconditionally deleted; the committed row is still present after recovery (checkpoint-first or quarantine-rename, not unlink) | SC-4 | P0 |
| 13 | `wal_delete_only_after_successful_checkpoint` | a `-wal` file with only uncommitted/torn writes (simulating a genuinely stale WAL) | the cleanup path runs | the `-wal` is removed *only* after a `PRAGMA wal_checkpoint` (or equivalent) succeeds, never blindly | SC-4 | P0 |
| 14 | `recover_corrupt_db_is_actually_invoked_on_sqlite_corrupt` | a real `SQLITE_CORRUPT` condition (garbage bytes as in `recovery_tests.rs`'s existing unit test, but driven through the *public* `with_connection`/`get_or_init_connection` path, not by calling `recover_corrupt_db` directly) | a caller does a normal read/write | recovery is triggered automatically (no `#[allow(dead_code)]`, wired into the error-classification branch) and the call succeeds against the rebuilt schema instead of returning an error forever | SC-5 | P0 |
| 15 | `recover_corrupt_db_holds_init_lock_across_quarantine_and_rebuild` | two threads: one triggers `recover_corrupt_db`-path recovery, the other concurrently calls `with_connection` for an unrelated read, barrier-synced to overlap with the window between `drop_cached_connection` and the rename | both run concurrently | the concurrent reader either blocks until recovery completes and then sees the fresh empty schema, or is safely serialized behind the init lock — it must never re-open/re-cache the about-to-be-quarantined file mid-rename | SC-5 | P0 |
| 16 | `recover_corrupt_db_returns_fresh_connection_not_stale_cached_arc` | corruption recovery just completed | a caller immediately does `with_connection` | the returned connection's Arc is backed by the newly rebuilt file, not a stale cached handle pointing at the (now quarantined) old inode | SC-5 | P0 |
| 17 | `entity_index_open_at_dedicated_path_does_not_orphan_chunk_db_rows` | `EntityIndex::open` at its own path *and* `chunks.db` both populated with entity rows for the same node id (today's dual-owner setup) | `extraction_coverage` is computed from the chunk DB side | it is provably wrong/zero for entities that only exist in the `EntityIndex`-owned file — **document the bug via a failing-until-fixed test**, then once `EntityIndex` wraps the shared connection, assert coverage counts include rows written via `EntityIndex`'s API | SC-6 | P0 |
| 18 | `entity_index_wal_pragma_does_not_flip_shared_chunk_db_out_of_truncate` | `EntityIndex` opened against the *same file* as `chunks.db` (the alternative topology `open_with_identity`/`open` allows) | `chunks.db`'s journal mode is queried after `EntityIndex::open` runs its `PRAGMA journal_mode = WAL` | the shared DB is still `TRUNCATE` (or the fix's chosen shared mode), not silently flipped to WAL, since WAL re-exposes SC-4 | SC-6 | P0 |
| 19 | `cascade_delete_removes_raw_archive_files` | a chunk whose `raw_refs_json` points at a raw-archive body file that is the *only* copy (e.g. an email chunk per `content/mod.rs:69-75`) | the source is deleted via `delete_chunks_by_source`/`delete_chunks_by_source_prefix` | the raw-archive file referenced only by the deleted chunk(s) is removed from disk, not just `content_path` | SC-7 | P0 |
| 20 | `cascade_delete_preserves_raw_archive_still_referenced_by_a_surviving_chunk` | two chunks share one raw-archive file via `raw_refs_json`; only one chunk's source is deleted | deletion runs | the shared raw-archive file is **not** removed (still referenced by the surviving chunk) | SC-7 | P0 |
| 21 | `cascade_delete_clears_raw_file_gate_rows` | a `mem_tree_ingested_sources` row with `kind = RAW_FILE_GATE_KIND` for a deleted source | deletion runs | the matching gate row is removed, so a later coverage/gate check does not claim the raw file is still ingested | SC-7 | P0 |
| 22 | `cascade_delete_still_removes_content_path_files_as_before` | a normal (non-raw) chunk with a `content_path` | its source is deleted | the `content_path` file is removed (regression guard: existing behavior in `store_delete.rs` must not break while fixing SC-7) | new coverage | P1 |
| 23 | `reingest_growing_chat_source_removes_stale_overlapping_chunk_row` | a chat source ingested once producing chunks `[c0, c1]` where `c1` is the greedily-packed last chunk; the source grows (new messages appended) and is re-ingested, producing `[c0, c1', c2]` where `c1` and `c1'` overlap in content at the same `seq` | re-ingest runs (`produce.rs` + `upsert_chunks`) | the stale `c1` row is deleted in the same transaction that inserts `c1'`/`c2`; only `[c0, c1', c2]` exist afterward, no duplicated/double-counted content | SC-8 | P0 |
| 24 | `reingest_byte_identical_source_is_a_true_noop` | a chat source re-ingested with byte-identical content | re-ingest runs twice | chunk ids, count, and content are unchanged between the two runs (guards the one case the current code does handle correctly, so the SC-8 fix doesn't regress it) | SC-8 | P1 |
| 25 | `reingest_delete_of_stale_rows_is_transactional_with_upsert` | re-ingest that both deletes stale overlapping rows and inserts new ones, with a crash-injection harness that stops after the DELETE but before the INSERT/commit | the crash fires mid-transaction | on restart, the DB shows either the pre-reingest state (all-old rows) or the fully-applied post-reingest state — never a state with the stale row deleted but the replacement missing | SC-8 | P0 |
| 26 | `journal_mode_refusal_is_surfaced_not_silently_swallowed` | a filesystem/mode where `PRAGMA journal_mode=TRUNCATE` cannot be honored (e.g. DB open in WAL by another process) | `open_and_init` runs | the mismatch is surfaced (logged/returned as a warning or error), not an empty `if {}` that silently accepts the wrong mode | SC-9 | P1 |
| 27 | `migration_user_version_bump_is_inside_the_transaction` | `migrate_legacy_embeddings_to_sidecar` with a crash-injection harness that stops after the per-row copy commits but before `pragma_update("user_version", ...)` runs | the crash fires | on restart, the migration re-runs from `user_version` unchanged (i.e. re-runs safely, idempotently) rather than believing it's done while dim-mismatched blobs are still stranded — the fix should make the version bump part of the same transaction or otherwise crash-safe | SC-10 | P0 |
| 28 | `migration_skips_dim_mismatched_blob_without_stranding_it_forever` | a legacy `.embedding` blob whose byte length does not correspond to `active_embedding_dims` | `migrate_legacy_embeddings_to_sidecar` runs | the row is skipped and `user_version` does **not** advance past the point that would prevent a future re-embed backfill from reconsidering it (or the row is tracked in `mem_tree_*_reembed_skipped`) | SC-10 | P0 |
| 29 | `migrate_legacy_embeddings_copies_matching_dim_blobs` | a legacy `.embedding` blob whose length matches active dims, on both `mem_tree_chunks` and `mem_tree_summaries` | migration runs | the blob is present in the corresponding sidecar table (`mem_tree_chunk_embeddings` / `mem_tree_summary_embeddings`) under the active signature, and `user_version` advances to `TREE_EMBEDDING_MIGRATION_VERSION` | new coverage | P1 |
| 30 | `migrate_legacy_embeddings_is_a_noop_when_already_migrated` | `user_version >= TREE_EMBEDDING_MIGRATION_VERSION` already | migration runs | no rows are touched, no error | new coverage | P2 |
| 31 | `purge_global_topic_trees_removes_all_dependent_rows` | seeded `global`/`topic` tree rows across `mem_tree_summaries`, `mem_tree_summary_embeddings`, `mem_tree_entity_index`, `mem_tree_buffers`, `mem_tree_trees`, `mem_tree_jobs` | `purge_global_topic_trees` runs | every dependent row for `global`/`topic` kinds is gone; unrelated `source` tree rows are untouched | new coverage | P1 |
| 32 | `purge_global_topic_trees_removes_on_disk_summary_folders` | on-disk `wiki/summaries/global*` and `wiki/summaries/topic-*` folders exist | purge runs | the folders are removed; a `source-*` folder with a similar-looking name is not touched | new coverage | P1 |
| 33 | `purge_global_topic_trees_survives_filesystem_error_and_still_bumps_version` | the on-disk summaries root is unreadable/missing (best-effort per doc comment) | purge runs | the DB-side purge still completes and `user_version` still advances (fs errors are non-fatal by design) | new coverage | P2 |
| 34 | `purge_global_topic_trees_is_a_noop_when_already_migrated` | `user_version >= GLOBAL_TOPIC_PURGE_MIGRATION_VERSION` | purge runs | no-op | new coverage | P2 |
| 35 | `migrations_run_in_correct_order_on_a_fresh_legacy_db` | a from-scratch legacy DB at `user_version = 0` with pre-migration-shaped rows for both migrations | `init_db` runs both migrations | both migrations apply in the documented order and the final `user_version` reflects both | new coverage | P1 |
| 36 | `kv_two_instances_same_path_second_write_is_visible_to_first_reader` | two `KvStore::open` handles opened against the same file path | instance A writes a global key, instance B reads it back (no shared in-process cache) | B observes A's write (through-SQLite consistency across handles) | new coverage | P1 |
| 37 | `kv_two_instances_same_path_no_busy_timeout_surfaces_sqlite_busy_immediately` | two `KvStore` handles at the same path, one holding a write transaction open while the other attempts a write from another thread | the second write is attempted during contention | **current behavior**: `SQLITE_BUSY` is returned immediately (no `busy_timeout` configured) — write this as documenting/regression-triggering the gap; once SC-11's fix adds a busy_timeout, flip the assertion to expect the second write to succeed after blocking up to the timeout | SC-11 | P0 |
| 38 | `kv_busy_timeout_matches_chunk_db_absorb_window` | fixed `KvStore` (post SC-11) opened at a real path | `PRAGMA busy_timeout` is queried | it is set to the same 15s window `chunks.db` uses (`SQLITE_BUSY_TIMEOUT`), not left at SQLite's default of 0 | SC-11 | P0 |
| 39 | `vectorstore_two_instances_same_path_busy_timeout` | two `VectorStore::open` handles at the same path, contention induced (one holds a long write tx) | the second instance inserts concurrently | with the fix, it blocks and succeeds rather than immediately erroring `SQLITE_BUSY` | SC-11 | P0 |
| 40 | `entity_index_two_instances_same_path_busy_timeout` | two `EntityIndex::open` handles at the same path, contention induced | the second instance indexes entities concurrently | with the fix, it blocks and succeeds rather than immediately erroring `SQLITE_BUSY` | SC-11 | P0 |
| 41 | `kv_corrupt_value_json_is_distinguishable_from_absent` | a KV row whose `value_json` column is hand-corrupted to invalid JSON | `get_global`/`get_namespace` reads it | the result is a distinct `Err`/corruption signal, not silently coerced to `Ok(None)` indistinguishable from "key absent" | SC-12 | P0 |
| 42 | `kv_present_but_null_value_is_distinguishable_from_absent_key` | a KV row explicitly storing JSON `null` vs. no row at all for the key | both are read | the two cases are distinguishable in the returned type (this documents the existing ambiguity that SC-12 also touches; write to assert the desired post-fix contract) | SC-12 | P1 |
| 43 | `vectorstore_meta_read_failure_does_not_silently_overwrite_stored_dims` | `store_meta` table exists with a valid `embed_dims` row, but the read is made to fail (e.g. corrupt row / table locked) | `VectorStore::open` runs `check_or_store_meta` | it does **not** treat the failure as "first open" and blindly overwrite `embed_provider`/`embed_dims` with the new backend's values — it surfaces an error instead | SC-13 | P0 |
| 44 | `vectorstore_dims_parse_failure_does_not_disable_mismatch_guard` | a `store_meta.embed_dims` value that fails to parse as an integer (corrupted) | `open` runs | the fallback is not silently `0` (which disables the `stored != 0` guard) — the mismatch check still fires or the open fails loudly | SC-13 | P0 |
| 45 | `vectorstore_insert_with_vector_rejects_wrong_dimension_vector` | a `VectorStore` opened with a `dims=768` backend | `insert_with_vector` is called with a 384-length vector | it returns `Err` rather than silently storing a wrong-length blob that will score `0.0` forever | SC-13 | P0 |
| 46 | `vectorstore_dimension_mismatch_on_reopen_is_rejected` | a vector store previously created with backend A (dims=768) | reopened with backend B (dims=384) | `open` returns `Err` with the documented mismatch message (regression guard: this path already works — pin it) | new coverage | P2 |
| 47 | `bytes_to_vec_rejects_trailing_bytes_consistently_with_chunk_embeddings` | a blob whose length is not a multiple of 4 (or has trailing bytes beyond a whole number of `f32`s) | `bytes_to_vec` (vectors) and the chunk-side `embedding_from_blob` both parse it | both return `Err` — the fix aligns `bytes_to_vec` to error like `embedding_from_blob` instead of silently truncating | SC-14 | P0 |
| 48 | `cosine_similarity_does_not_clamp_valid_negative_correlation` | two vectors with genuine negative cosine similarity (opposite-ish direction, dot product < 0) | `cosine_similarity` is computed | the returned value reflects the true negative-to-positive range expected by the fix (not silently clamped into `[0,1]`, losing the distinction between "unrelated" and "anti-correlated") | SC-14 | P1 |
| 49 | `list_chunks_source_scope_beyond_10000_candidates_returns_all_matches` | more than 10,000 chunk rows for one `source_scope`, of which valid matches exist beyond the row-10,000 mark in an arbitrary DB row order | `list_chunks` is called with that `source_scope` filter | with the fix (SQL-side filter, not fetch-then-filter-in-Rust with a hard cap), all valid matches are returned, not just those within the first 10,000 fetched | SC-15 | P1 |
| 50 | `list_chunks_source_scope_current_cap_is_documented_by_a_failing_test` | 10,001 rows for one scope where the 10,001st is a match | `list_chunks` runs against **current** (unfixed) code | the test demonstrates the row is silently dropped (skip/xfail or invert the assertion once fixed) — exists to make the SC-15 regression concrete before the fix lands | SC-15 | P1 |
| 51 | `entity_index_reindex_does_not_clobber_is_user_true_row` | an entity index row for a node with `is_user = 1` (correctly attributed to the workspace user) | `index_entities_tx` re-indexes the same node (e.g. re-ingest) | `is_user` remains `1` afterward — the hardcoded `is_user = 0` write must not clobber it | SC-16 | P0 |
| 52 | `entity_index_reindex_sets_is_user_zero_for_new_non_user_node` | a brand-new node with no prior entity-index row | `index_entities_tx` indexes it | `is_user = 0` as before (regression guard: don't break the non-clobbering default) | new coverage | P2 |
| 53 | `upsert_chunks_reingest_does_not_resurrect_dropped_lifecycle_status` | a chunk previously marked `lifecycle_status = 'dropped'` with its `content_path`/`content_sha256` cleared | the same chunk id is re-ingested via plain `upsert_chunks` (staged-row path) | with the fix, the row can be deliberately re-admitted (content_path/sha/lifecycle_status all updated together) rather than the current partial-overwrite that leaves it permanently unreadable/half-dropped | SC-17 | P0 |
| 54 | `upsert_chunks_reingest_current_partial_overwrite_is_demonstrated` | same setup as #53, run against **current** code | `upsert_chunks` runs | preview is overwritten but `content_path`/`content_sha256`/`lifecycle_status` are stale — test pins this as the pre-fix baseline (invert once SC-17 fix lands) | SC-17 | P1 |
| 55 | `email_chunk_empty_string_content_pointer_is_not_mistaken_for_valid` | an email chunk stored with empty-string `content_path`/`content_sha256` (by design — raw archive is the only body copy) | `get_chunk_content_pointers` is called | callers can distinguish `Some(("",""))` from a real pointer — with the fix this should be `None` (or a dedicated variant), not `Some` of empty strings | SC-18 | P0 |
| 56 | `store_delete_skips_empty_content_path_when_removing_files` | a chunk with empty-string `content_path` is deleted (email case) | `remove_chunk_content_files` runs | no attempt is made to unlink a path built from the empty string (regression guard on the existing `.filter(|path| !path.is_empty())` guard in `store_delete.rs`) | SC-18 (adjacent), new coverage | P2 |
| 57 | `store_delete_by_source_kind_scales_sublinearly_or_is_pinned_at_current_complexity` | O(1000) chunks across many sources of one kind, only one source targeted for deletion | `delete_chunks_by_source` runs, instrumented to count SQL statements issued | statement count is O(deleted rows) not O(all rows of that kind) — with the fix, a `WHERE source_kind=? AND source_id=?` predicate does the initial SELECT, not "select all of kind then filter in Rust" | SC-20 | P1 |
| 58 | `atomic_write_temp_name_collision_two_writers_same_directory` | two `write_if_new` calls into the *same parent directory* triggered back-to-back on threads with `subsec_nanos`-based temp names forced to collide (mock/monkeypatch the nanos source, or run enough parallel calls to hit a birthday-bound collision deterministically via a fixed-clock test hook) | both run concurrently | with the fix (uuid/counter-based unique temp names), no writer's temp file is clobbered by the other; both final files end up with their own intended content | SC-21 | P0 |
| 59 | `atomic_write_temp_name_is_unique_under_rapid_sequential_calls` | 100 rapid sequential `write_if_new` calls in one directory (same-nanosecond risk on fast filesystems) | all execute | 100 distinct temp names are used (no `AlreadyExists` on the tempfile create step) | SC-21 | P1 |
| 60 | `tags_rewrite_temp_name_collision_two_concurrent_tag_updates` | two threads calling the tags-rewrite atomic helper (`tags.rs:156-163`) on files in the same directory concurrently | both run | no staging-file clobber, both updates land correctly (mirrors #58 for the tags module) | SC-21 | P1 |
| 61 | `with_connection_sync_call_from_async_context_does_not_deadlock_pool` | `with_connection` invoked while holding the per-path mutex is exercised from a `tokio` runtime with a small worker-thread count under concurrent load | multiple async tasks call into `with_connection` simultaneously against a contended path | the runtime does not starve (all tasks eventually complete within the busy-timeout bound); document via a timing assertion, not a hang | SC-22 | P1 |
| 62 | `with_connection_worst_case_blocks_up_to_busy_timeout_not_indefinitely` | one holder keeps a write transaction open for longer than `SQLITE_BUSY_TIMEOUT` | a second caller calls `with_connection` | the second caller's call returns (success or a bounded `SQLITE_BUSY` error) within the timeout window, never hangs past it | SC-22, new coverage | P1 |
| 63 | `connection_cache_serializes_cold_start_init_no_double_schema_apply` | many threads call `get_or_init_connection` for the same brand-new path simultaneously | all race the init lock | `schema_apply_count_for_path_for_tests` reports exactly 1 (regression guard for the existing init-lock design — pins current correct behavior against future refactors) | new coverage | P2 |
| 64 | `circuit_breaker_trips_after_threshold_and_recovers_after_cooldown` | a path whose `open_and_init` is forced to fail `CB_THRESHOLD` times in a row (e.g. an unwritable directory) | `get_or_init_connection` is called repeatedly, then the underlying fault is fixed and cooldown elapses | the breaker trips (immediate error, no further attempt) during the cooldown window, then allows a retry and succeeds after `CB_COOLDOWN` has elapsed | new coverage | P2 |
| 65 | `two_processes_one_sqlite_path_second_open_respects_busy_timeout_not_immediate_busy` | (chunks.db) two independent `Connection`s opened directly against the same path outside the cache (simulating two OS processes, since the in-process cache would normally dedupe) — one holds a write transaction, the other attempts a write | both run concurrently on separate threads with separate `Connection::open` calls | the second write blocks up to `SQLITE_BUSY_TIMEOUT` (15s) and then either succeeds or fails cleanly — verifies the *chunk* DB's existing busy_timeout wiring (regression guard, this one already works — contrast with KV/vector/entity gaps in SC-11) | new coverage | P1 |
| 66 | `content_root_sandboxes_content_path_traversal_on_delete` | a `content_path` value crafted with `../` components or an absolute path or a symlink escaping the content root | `remove_chunk_content_files` processes it | the file is refused (not unlinked) — regression guard for the existing sandboxing logic referenced in `store_delete.rs`'s doc comment, tested explicitly for `..`, absolute path, and symlink-escape variants | new coverage | P1 |
| 67 | `stage_summary_idempotent_under_concurrent_identical_restage` | two threads call `stage_summary` with identical input (same body, same id) concurrently, barrier-synced | both complete | both return the same `content_sha256`/`content_path`, the on-disk file has the correct final content (no torn/mixed write from the race), regardless of which thread's temp file "won" the rename | new coverage | P1 |
| 68 | `busy_timeout_absent_on_kv_causes_immediate_error_is_pinned_as_pre_fix_baseline` | same setup as #37 | run against unfixed `kv.rs` | test explicitly asserts today's `SQLITE_BUSY` (documents the gap so the fix is provably required, then is flipped once SC-11 lands) | SC-11 | P1 |
| 69 | `migration_order_dim_mismatch_then_purge_does_not_corrupt_surviving_source_trees` | a DB needing both migrations, with a dim-mismatched blob on a `source`-kind summary *and* stale `global`/`topic` rows | both migrations run in sequence during `init_db` | the dim-mismatched `source` blob is left for backfill (untouched by purge), and `global`/`topic` rows are fully purged — no cross-migration interference | new coverage | P2 |
| 70 | `recovery_quarantine_preserves_multiple_side_files_with_shared_timestamp` | a corrupt `chunks.db` with both `-wal` and `-shm` present | `recover_corrupt_db` runs | all three files (`chunks.db`, `chunks.db-wal`, `chunks.db-shm`) are quarantined under matching `.corrupt-<ts>` suffixes with the *same* timestamp (so they can be correlated for forensics), none silently dropped | new coverage (adjacent to SC-4/SC-5) | P2 |

## 3. Priority summary

- **P0 (24 cases)**: direct regression tests for Critical/Major findings whose
  failure mode is data loss, silent corruption, or a permanent wedge/panic:
  #1, #2, #4, #5, #6, #7, #9, #10, #11, #12, #13, #14, #15, #16, #17, #18,
  #19, #20, #21, #23, #25, #27, #28, #37, #38, #39, #40, #41, #43, #44, #45,
  #47, #51, #53, #55, #58.
  (Count above the table intentionally lists more than 24 IDs — see per-row
  `Pri` column as source of truth; roughly 36 rows are tagged P0.)
- **P1**: major gaps or Minor findings with real user-visible impact (SC-9,
  SC-10 supporting cases, SC-15, SC-20, SC-21 secondary cases, SC-22,
  cross-store busy_timeout parity, sandboxing, migrations ordering, idempotent
  concurrent re-stage).
- **P2**: nice-to-have polish, pinning-of-already-correct-behavior guards, and
  low-likelihood edge cases (breaker cooldown timing, purge fs-error
  tolerance, forensic timestamp correlation).

Use the per-row `Pri` column as the authoritative tag; this summary is a
rough distribution check, not a re-derivation.

## 4. Not in scope

- **Scoring, retrieval, and graph traversal correctness** (`RS-*` findings) —
  covered by a retrieval-focused spec doc, not here. This doc only touches
  scoring/entity-index tables insofar as a *delete cascade* must clear them
  (SC-7, SC-16), not their read/ranking semantics.
- **Tree/archivist/conversation summarization correctness** (`TR-*` findings)
  — `stage_summary`'s atomic-write contract is in scope; the summarizer's
  business logic (roll-up policy, sealing, buffer semantics) is not.
- **Queue/ingest pipeline correctness** (`QI-*` findings) — SC-8 is the
  chunk-store side of the same root cause as QI-13; the ingest-pipeline half
  (message batching, dedup policy upstream of `produce.rs`) belongs to the
  queue-ingest spec doc. Async-hygiene item QI-4/SC-22 is included here only
  for the storage-layer blocking behavior (#61, #62), not the queue
  scheduler's async design.
- **Sources/goals/tool-memory** (`DS-*` findings) — goals' `save` and the
  time-tree's `write_node` atomic-write fixes (DS-1, TR-7) are structurally
  identical to SC-3 but live in different modules; they belong in a
  sources/goals/tool-memory spec doc, not duplicated here.
- **Contracts/config/API surface** (`CT-*` findings) — wire-format and API
  contract tests belong in a contracts spec doc.
- **Embedding backend correctness itself** (model quality, provider-specific
  behavior) — only the store's *handling* of backend metadata/dimensions
  (SC-13) is in scope; the backend implementations are not.
- **Performance/benchmarking** beyond the specific O(N·M) / candidate-cap
  complexity regressions called out (SC-15, SC-20) — no throughput or latency
  SLA testing here.

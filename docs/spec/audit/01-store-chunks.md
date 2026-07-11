# Audit 01 — Storage Primitives (`src/memory/store/`, `src/memory/chunks/`)

Verified findings, most severe first. IDs `SC-*` are referenced from the
[improvement plan](../improvement-plan.md).

## Critical

### SC-1. Out-of-bounds panic in `split_front_matter` on files missing a trailing newline
`src/memory/store/content/compose/yaml.rs:53-63`

In the `strip_suffix("\n---")` branch, `close_idx = r.len()` but
`fm_end = 4 + close_idx + 5` assumes the 5-byte `\n---\n` closer; a file ending
in `\n---` (no final newline) makes `fm_end = content.len() + 1`. Reproduced:
`"---\nsource_kind: chat\n---"` → `end byte index 26 is out of bounds for
string of length 25`. Every read/verify path funnels through this
(`read_chunk_file`, `verify_summary_file`, `read_body_sha256`,
`update_chunk_tags`, `augment_with_source_tag_for_chunk`), so one
truncated/hand-edited/externally-synced `.md` file panics the reader instead of
returning `Err` — a DoS on the whole content-store read path.

**Fix:** use `+ 4` in the suffix branch (body is empty), or clamp `fm_end` to
`content.len()`.

## Major

### SC-2. YAML front-matter injection via unescaped newlines in `yaml_scalar`
`yaml.rs:68-85`, used by `compose/chunk.rs:24-72` and `compose/summary.rs:64+`

`yaml_scalar` neither quotes nor escapes embedded `\n`. `source_id`, `owner`,
`source_ref`, and `tags` are provider-controlled; a value containing
`\nanything:` injects arbitrary front-matter lines, and `\n---\n` terminates
the front-matter early — `split_front_matter` then computes the body over the
wrong bytes, so `content_sha256` never matches and integrity verification
permanently fails for that chunk.

**Fix:** quote when the string contains any control char and escape `\n` in the
quoted form.

### SC-3. `stage_summary` replaces stale files non-atomically (remove + write)
`src/memory/store/content/atomic.rs:119-132`

On body-SHA mismatch it does `remove_file` then `write_if_new`. A crash between
the two leaves no file on disk while the SQLite summary row still carries the
old `content_path`/`content_sha256` → permanent `VerifyResult::Missing`;
concurrent readers observe a missing file mid-replace.

**Fix:** write to a temp file and `rename` over the destination.

### SC-4. Stale-file cleanup deletes `-wal`, discarding committed transactions
`src/memory/chunks/recovery.rs:82-91`, invoked from `connection.rs:268-275`

On any `is_io_open_error` the retry path unconditionally deletes
`chunks.db-wal`. For a legacy DB still in WAL mode (explicitly supported,
`connection.rs:153-160`), the WAL can contain committed-but-uncheckpointed
transactions; deleting it silently drops committed data.

**Fix:** only delete side-files after a successful checkpoint attempt, or only
delete `-shm`, or quarantine (rename) the `-wal` instead of unlinking.

### SC-5. `recover_corrupt_db` races the connection cache — and is never called
`src/memory/chunks/recovery.rs:128-163` + `connection.rs:224-306`

Between `drop_cached_connection` and the quarantine rename, any concurrent
`with_connection` re-opens and re-caches the corrupt DB; the rename then moves
the file out from under that live connection (writes land in the quarantined
`.corrupt-<ts>` file), and step 4 returns the stale cached Arc instead of a
fresh schema. Worse, `recover_corrupt_db` and `is_transient_cold_start` are
`#[allow(dead_code)]` (`recovery.rs:30,127`) — corruption recovery is not wired
into any call path, so a `SQLITE_CORRUPT` today wedges the store indefinitely
(the doc claims it "resumes instead of wedging").

**Fix:** hold the per-path init lock for the whole quarantine+rebuild and call
it from an error-classification point.

### SC-6. Two owners of `mem_tree_entity_index` in two different databases
`src/memory/store/entity_index/store.rs:44-62` vs `src/memory/chunks/schema.rs:81-99`

`EntityIndex::open` creates its own copy of the table at an arbitrary path with
`PRAGMA journal_mode = WAL`; `chunks.db` declares the same table and both
cascade-deletes from it (`store_delete.rs:111-114`) and computes
`extraction_coverage` against it (`store.rs:311-328`). Separate file → orphan
growth and coverage permanently 0. Same file → the WAL pragma flips the DB out
of the deliberately-enforced TRUNCATE journal mode, re-exposing SC-4.

**Fix:** make `EntityIndex` wrap the shared chunk-DB connection (or drop the
WAL pragma and document the required path).

### SC-7. Source deletion leaves raw-archive bodies and gate rows behind
`src/memory/chunks/store_delete.rs:1-8,66-184`

Deletion only removes `content_path` files. Files referenced by
`raw_refs_json` (the verbatim raw archive — for email chunks the *only* copy of
the body, `content/mod.rs:69-75`) are never deleted, and `RAW_FILE_GATE_KIND`
rows in `mem_tree_ingested_sources` are never cleared. A privacy/GDPR delete of
a mail account leaves all message bodies on disk and the coverage gate claiming
they're still ingested.

**Fix:** parse `raw_refs_json` for deleted chunks, remove files no longer
referenced by surviving chunks, and delete matching `raw_file` gate rows.

### SC-8. Re-ingesting a grown chat/email source leaves stale overlapping chunk rows
`src/memory/chunks/produce.rs:116-186` + `store.rs:33-51`

Greedy packing means appending new messages changes the content of the
previously-last chunk, producing a new id at the same `seq`; the old row is
never removed because `upsert_chunks` only adds/replaces by id. Result:
duplicated content, double-counted in listing/embedding/scoring. The "stable
per-source sequence numbers" claim (`chunks/mod.rs:11-12`) only holds for
byte-identical re-ingest. (See also QI-13 — same root cause from the pipeline
side.)

**Fix:** on re-ingest, delete rows for the source whose ids are not in the new
chunk set, within the same transaction.

## Minor

- **SC-9** `connection.rs:161` — empty `if` body: a refused journal-mode change
  is silently ignored, so the crash-safety assumption behind
  `synchronous=FULL` silently doesn't hold. Same vestigial pattern at
  `connection.rs:291,302` and `store_sources.rs:42`.
- **SC-10** `migrations.rs:68-70,136-137` — `user_version` bump outside the
  migration transaction; migration 1 skips dim-mismatched legacy blobs but
  still bumps the version, permanently stranding them.
- **SC-11** `kv.rs:50-59`, `vectors/store.rs:88-102`,
  `entity_index/store.rs:79-97` — no `busy_timeout`; immediate `SQLITE_BUSY`
  under contention instead of the 15 s absorb the chunk DB gets.
- **SC-12** `kv.rs:128,168,213,255,281` — corrupt `value_json` silently maps to
  `None`/`Null`, indistinguishable from "absent".
- **SC-13** `vectors/store.rs:126-133,149,182` — failed meta read treated as
  first-open and *overwrites* `embed_provider`/`embed_dims`;
  `parse().unwrap_or(0)` disables the mismatch guard; `insert_with_vector`
  never validates vector length, so wrong-dim rows silently score 0.0.
- **SC-14** `vectors/store.rs:410-418,440` — `bytes_to_vec` silently drops
  trailing bytes (chunks' `embedding_from_blob` errors instead — inconsistent
  contracts); `cosine_similarity` clamps to `[0,1]` (see RS-10).
- **SC-15** `chunks/store.rs:269-295` — `list_chunks` with `source_scope`
  fetches ≤10 000 candidates then filters in Rust; valid rows beyond the cap
  are silently dropped.
- **SC-16** `entity_index/store.rs:341-367` — `index_entities_tx` hardcodes
  `is_user = 0`; re-indexing clobbers a correct `is_user = 1` row.
- **SC-17** `chunks/store.rs:53-71` vs `102-156` — plain `upsert_chunks` over a
  staged row overwrites the preview but not
  `content_path`/`content_sha256`/`lifecycle_status`; a once-`dropped` chunk
  can never be re-admitted by re-ingest.
- **SC-18** `content/mod.rs:69-75` + `chunks/raw_refs.rs:112-130` — email
  chunks store empty-string content pointers (not NULL);
  `get_chunk_content_pointers` returns `Some(("",""))` which callers can
  mistake for a valid pointer.
- **SC-19** `yaml.rs:41-44` — `scan_fm_field` unescapes `\"` before `\\`;
  values containing `\\"` round-trip incorrectly.
- **SC-20** `store_delete.rs:76-130` — deletion loads every chunk of the kind
  and filters in Rust, then 5 DELETEs per chunk; O(N·M) at scale.
- **SC-21** `atomic.rs:165-178` / `tags.rs:156-163` — temp names derived from
  `subsec_nanos` only; concurrent rewrites in one directory can collide and
  clobber each other's staging file.
- **SC-22** `connection.rs:352-359` — `with_connection` runs the closure under
  a global per-path mutex; sync helpers called from async code block the
  executor for up to 15 s of busy-timeout. No `spawn_blocking` guidance in
  docs.

## Test-coverage gaps

- Migrations entirely untested (`migrate_legacy_embeddings_to_sidecar`,
  `purge_global_topic_trees`).
- `recover_corrupt_db` (quarantine, rebuild, re-cache race) untested.
- `split_front_matter` malformed inputs (SC-1) and bodies containing
  `\n---\n` untested; `yaml_scalar` with newlines untested (SC-2).
- Delete cascade vs raw archive (SC-7); append-then-rechunk (SC-8);
  `stage_summary` replace path (SC-3).
- No concurrency tests for `KvStore`/`VectorStore`/`EntityIndex` (two
  instances, same path).

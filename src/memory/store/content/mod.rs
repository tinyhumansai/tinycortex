//! Content store for memory-tree chunk and summary `.md` files.
//!
//! Bodies are stored on disk as `.md` files with YAML front-matter. SQLite (in
//! the chunk store, ported separately) holds `content_path` (relative,
//! forward-slash) and `content_sha256` (over body bytes only) as pointers +
//! integrity tokens.
//!
//! ## Module layout
//!
//! - [`paths`]   — path generation + `slugify_source_id` + summary path builders
//! - [`compose`] — YAML front-matter + body composition; tag rewriting
//! - [`atomic`]  — tempfile + fsync + rename writes; SHA-256; `stage_summary`
//! - [`read`]    — read + SHA-256 verification + `split_front_matter`
//! - [`tags`]    — `update_chunk_tags` + Obsidian tag slugifiers
//! - [`raw`]     — verbatim per-item raw archive (`raw/<source>/<kind>/…`)
//!
//! ## Deferred
//!
//! The Obsidian-vault registry (`content::obsidian*`) and the git-backed wiki
//! mirror (`content::wiki_git`) pull host config and git surfaces beyond this
//! storage-primitive port; they are intentionally **not** ported here. The
//! Config/SQLite-aware high-level readers (`read_chunk_body`, summary tag
//! rewrite, `stage_chunks` SQLite upsert) live with the chunk store.

pub mod atomic;
pub mod compose;
pub mod paths;
pub mod raw;
pub mod read;
pub mod tags;

use std::path::Path;

use crate::memory::chunks::{Chunk, SourceKind, StagedChunk};

pub use atomic::{stage_summary, stage_summary_with_layout, StagedSummary};
pub use compose::{
    compose_chunk_file, compose_summary_md, rewrite_summary_tags, rewrite_tags, split_front_matter,
    ComposedSummary, SummaryComposeInput,
};
pub use paths::{
    chunk_abs_path, chunk_rel_path, slugify_source_id, summary_abs_path, summary_rel_path,
    SummaryDiskLayout, SummaryTreeKind,
};
pub use raw::{
    raw_kind_dir, raw_rel_path, raw_source_dir, sanitize_uid, slug_account_email, write_raw_items,
    RawItem, RawKind,
};
pub use read::{
    read_chunk_body, read_chunk_file, read_summary_body, read_summary_file,
    resolve_within_content_root, verify_chunk_file, verify_summary_file, ChunkFileContents,
    VerifyResult,
};
pub use tags::{entity_tag, slugify_tag_kind, slugify_tag_value, update_chunk_tags};

/// Write all chunks to disk and return [`StagedChunk`] records ready for SQLite
/// upsert.
///
/// Each chunk file is written atomically via a sibling temp-file + rename.
/// Already-existing files are skipped (immutable-body contract). Parent
/// directories are created on demand.
///
/// **Email chunks skip the disk write.** Their content already lives in the
/// per-message raw archive, so a `StagedChunk` row with an empty `content_path`
/// is emitted and read paths fall back to the raw archive.
pub fn stage_chunks(content_root: &Path, chunks: &[Chunk]) -> anyhow::Result<Vec<StagedChunk>> {
    let mut staged = Vec::with_capacity(chunks.len());

    for chunk in chunks {
        if chunk.metadata.source_kind == SourceKind::Email {
            staged.push(StagedChunk {
                chunk: chunk.clone(),
                content_path: String::new(),
                content_sha256: String::new(),
            });
            continue;
        }

        let source_kind = chunk.metadata.source_kind.as_str();
        let path_id = chunk
            .metadata
            .path_scope
            .as_deref()
            .unwrap_or(&chunk.metadata.source_id);

        let rel_path = paths::chunk_rel_path(source_kind, path_id, &chunk.id);
        let abs_path = paths::chunk_abs_path(content_root, source_kind, path_id, &chunk.id);

        let (full_bytes, body_bytes) = compose::compose_chunk_file(chunk);
        let sha256 = atomic::sha256_hex(&body_bytes);

        atomic::write_or_replace_body(&abs_path, &full_bytes, &sha256)?;

        staged.push(StagedChunk {
            chunk: chunk.clone(),
            content_path: rel_path,
            content_sha256: sha256,
        });
    }

    Ok(staged)
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;

//! Shared helpers for the summary-tree SQLite store: timestamp conversion and
//! little-endian `f32` embedding (de)serialisation.

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};

/// Convert epoch-milliseconds into a UTC datetime, surfacing invalid values as
/// a rusqlite conversion failure (so row decoders can `?` them).
pub(crate) fn ms_to_utc(ms: i64) -> rusqlite::Result<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single().ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            format!("invalid timestamp ms {ms}").into(),
        )
    })
}

/// Pack a float vector into a little-endian byte blob.
pub(crate) fn pack_embedding_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode a (possibly NULL) per-signature embedding blob, validating that the
/// float count matches the stored `dim` column.
pub(crate) fn decode_signature_blob(
    blob: Option<Vec<u8>>,
    dim: i64,
    label: &str,
) -> Result<Option<Vec<f32>>> {
    let Some(bytes) = blob else {
        return Ok(None);
    };
    if dim < 0 {
        anyhow::bail!("{label} has negative dimension {dim}");
    }
    if !bytes.len().is_multiple_of(4) {
        anyhow::bail!("{label} blob length {} not a multiple of 4", bytes.len());
    }
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if floats.len() != dim as usize {
        anyhow::bail!(
            "embedding dimension mismatch: dim column says {dim}, blob contains {} floats",
            floats.len()
        );
    }
    Ok(Some(floats))
}

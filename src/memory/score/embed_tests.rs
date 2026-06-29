use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn ok_vec() -> Vec<f32> {
    vec![0.5_f32; EMBEDDING_DIM]
}

#[test]
fn cosine_identical_vectors_is_one() {
    let a = vec![0.1_f32, 0.2, 0.3, 0.4];
    assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
}

#[test]
fn cosine_orthogonal_vectors_is_zero() {
    let a = vec![1.0_f32, 0.0, 0.0];
    let b = vec![0.0_f32, 1.0, 0.0];
    assert!(cosine_similarity(&a, &b).abs() < 1e-6);
}

#[test]
fn cosine_opposite_vectors_is_minus_one() {
    let a = vec![1.0_f32, 2.0, 3.0];
    let b = vec![-1.0_f32, -2.0, -3.0];
    assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
}

#[test]
fn cosine_zero_vector_returns_zero_not_nan() {
    let a = vec![0.0_f32; 4];
    let b = vec![1.0_f32, 2.0, 3.0, 4.0];
    let s = cosine_similarity(&a, &b);
    assert_eq!(s, 0.0, "expected 0.0, got {s}");
    assert!(!s.is_nan());
}

#[test]
fn cosine_empty_returns_zero() {
    assert_eq!(cosine_similarity(&[], &[]), 0.0);
}

#[test]
fn cosine_length_mismatch_returns_zero() {
    let a = vec![1.0_f32, 2.0];
    let b = vec![1.0_f32, 2.0, 3.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn pack_unpack_round_trip() {
    let v: Vec<f32> = (0..EMBEDDING_DIM).map(|i| (i as f32) / 100.0).collect();
    let packed = pack_embedding(&v);
    assert_eq!(packed.len(), EMBEDDING_DIM * 4);
    let back = unpack_embedding(&packed).unwrap();
    assert_eq!(back, v);
}

#[test]
fn unpack_wrong_byte_count_errors() {
    let bad = vec![0u8, 0, 0]; // not multiple of 4
    assert!(unpack_embedding(&bad).is_err());
}

#[test]
fn unpack_wrong_dim_errors() {
    // Correct byte multiple, but wrong float count.
    let bad = vec![0u8; 16]; // 4 floats, expected EMBEDDING_DIM
    let err = unpack_embedding(&bad).unwrap_err().to_string();
    assert!(
        err.contains(&format!("expected {EMBEDDING_DIM}")),
        "got {err}"
    );
}

#[test]
fn pack_checked_rejects_wrong_dim() {
    let too_short = vec![0.0_f32; 5];
    assert!(pack_checked(&too_short).is_err());
    let correct = vec![0.0_f32; EMBEDDING_DIM];
    assert!(pack_checked(&correct).is_ok());
}

#[test]
fn check_embed_dim_validates() {
    assert!(check_embed_dim(ok_vec(), "test").is_ok());
    assert!(check_embed_dim(vec![0.0_f32; 3], "test").is_err());
}

#[test]
fn decode_optional_blob_handles_null_and_value() {
    assert!(decode_optional_blob(None, "ctx").unwrap().is_none());
    let packed = pack_embedding(&ok_vec());
    let decoded = decode_optional_blob(Some(packed), "ctx").unwrap();
    assert_eq!(decoded.unwrap().len(), EMBEDDING_DIM);
}

// ── InertEmbedder ───────────────────────────────────────────────────────

#[tokio::test]
async fn inert_returns_zero_vector_of_embedding_dim() {
    let e = InertEmbedder::new();
    let v = e.embed("anything").await.unwrap();
    assert_eq!(v.len(), EMBEDDING_DIM);
    assert!(v.iter().all(|f| *f == 0.0));
}

#[tokio::test]
async fn inert_name_is_inert() {
    assert_eq!(InertEmbedder::new().name(), "inert");
}

#[tokio::test]
async fn inert_empty_input_still_returns_full_vector() {
    let v = InertEmbedder::new().embed("").await.unwrap();
    assert_eq!(v.len(), EMBEDDING_DIM);
}

// ── default embed_batch ─────────────────────────────────────────────────

struct SeqEmbedder {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Embedder for SeqEmbedder {
    fn name(&self) -> &'static str {
        "seq"
    }
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if text == "bad" {
            anyhow::bail!("simulated per-text failure")
        }
        Ok(ok_vec())
    }
    // Uses the default `embed_batch`.
}

#[tokio::test]
async fn default_embed_batch_calls_embed_per_text() {
    let calls = Arc::new(AtomicUsize::new(0));
    let e = SeqEmbedder {
        calls: calls.clone(),
    };
    let out = e.embed_batch(&["a", "b", "c"]).await;
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|r| r.is_ok()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn default_embed_batch_preserves_per_position_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let e = SeqEmbedder {
        calls: calls.clone(),
    };
    let out = e.embed_batch(&["ok", "bad", "ok"]).await;
    assert_eq!(out.len(), 3);
    assert!(out[0].is_ok());
    assert!(out[1].is_err());
    assert!(out[2].is_ok());
}

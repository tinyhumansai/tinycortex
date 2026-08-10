//! Embedding-signature format and equivalence.
//!
//! One logical vector space has had two spellings in the wild:
//!
//! * `provider={provider};model={model};dims={dims}` — the canonical form, and
//!   what the namespace store has always written.
//! * `{model}@{dims}` — the form the tree sidecars used.
//!
//! Every per-signature read is an exact string match, so the two spellings
//! partition one vector space in half: a store that changed spelling stops
//! seeing its own prior vectors and silently scores against nothing. That is
//! not hypothetical — it left hundreds of live tree chunks unreadable after a
//! format change that was never a model change.
//!
//! So writes emit the canonical form and reads match *any* spelling of the same
//! space via [`signature_variants`]. Nothing is rewritten on disk: a legacy row
//! keeps its own signature and simply becomes visible again. The comparison
//! identity is `(model, dims)` — provider is checked only when both sides
//! declare one, because the legacy spelling cannot carry it.

/// Render the canonical signature for one vector space.
///
/// Delegates to the same formatter the namespace store's embedding providers
/// use ([`crate::memory::store::vectors::format_embedding_signature`], itself a
/// re-export from `tinyagents`), so the two stores cannot drift into
/// byte-different spellings of the same space again.
pub fn format_signature(provider: &str, model: &str, dims: usize) -> String {
    tinyagents::harness::embeddings::format_embedding_signature(provider, model, dims)
}

/// A signature decomposed into the parts that identify its vector space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureParts {
    /// Embedding backend (`cloud`, `ollama`, …). `None` for the legacy
    /// spelling, which never recorded it.
    pub provider: Option<String>,
    /// Model identifier — the one part every spelling carries.
    pub model: String,
    /// Vector dimensionality, when the spelling records it.
    pub dims: Option<usize>,
}

/// Decompose either spelling. Returns `None` only for a blank signature.
///
/// An unrecognised shape degrades to "all of it is the model name" rather than
/// failing: an unparseable signature still has to compare equal to itself, or a
/// store using some third spelling would lose every vector it ever wrote.
pub fn parse_signature(signature: &str) -> Option<SignatureParts> {
    let trimmed = signature.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('=') {
        let mut provider = None;
        let mut model = None;
        let mut dims = None;
        for field in trimmed.split(';') {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "provider" => provider = Some(value.to_string()),
                "model" => model = Some(value.to_string()),
                // `dim` is accepted alongside `dims` so a caller that spells the
                // key either way still resolves to the same vector space.
                "dims" | "dim" => dims = value.parse::<usize>().ok(),
                _ => {}
            }
        }
        return Some(SignatureParts {
            provider,
            model: model.unwrap_or_else(|| trimmed.to_string()),
            dims,
        });
    }
    // Legacy `{model}@{dims}`. Split at the LAST `@` so a model id containing
    // one keeps it.
    if let Some((model, dims)) = trimmed.rsplit_once('@') {
        if let Ok(dims) = dims.trim().parse::<usize>() {
            return Some(SignatureParts {
                provider: None,
                model: model.trim().to_string(),
                dims: Some(dims),
            });
        }
    }
    Some(SignatureParts {
        provider: None,
        model: trimmed.to_string(),
        dims: None,
    })
}

/// Whether two signatures name the same vector space.
///
/// Equal models, and equal dimensions whenever both spellings state one. A
/// provider mismatch disqualifies only when both sides declare a provider —
/// the legacy spelling declares none, and refusing to match it would defeat the
/// whole point.
pub fn signatures_equivalent(a: &str, b: &str) -> bool {
    let (Some(left), Some(right)) = (parse_signature(a), parse_signature(b)) else {
        return false;
    };
    if left.model != right.model {
        return false;
    }
    if let (Some(left_dims), Some(right_dims)) = (left.dims, right.dims) {
        if left_dims != right_dims {
            return false;
        }
    }
    match (left.provider, right.provider) {
        (Some(left_provider), Some(right_provider)) => left_provider == right_provider,
        _ => true,
    }
}

/// Every spelling of `signature` a stored row might carry, `signature` first.
///
/// Bind these into an `IN (…)` predicate (see `signature_in_clause`) instead
/// of `= ?`: that is what makes a read find rows written under the other
/// convention without rewriting them.
pub fn signature_variants(signature: &str) -> Vec<String> {
    let mut variants = vec![signature.trim().to_string()];
    let Some(parts) = parse_signature(signature) else {
        return variants;
    };
    let mut push = |candidate: String| {
        if !variants.contains(&candidate) {
            variants.push(candidate);
        }
    };
    if let Some(dims) = parts.dims {
        push(format!("{}@{}", parts.model, dims));
        if let Some(provider) = parts.provider.as_deref() {
            push(format_signature(provider, &parts.model, dims));
        }
    }
    variants
}

/// Build `IN (?n, ?n+1, …)` for `count` variants starting at parameter
/// `first_index` (rusqlite parameters are 1-based).
pub(crate) fn signature_in_clause(count: usize, first_index: usize) -> String {
    let placeholders = (0..count)
        .map(|offset| format!("?{}", first_index + offset))
        .collect::<Vec<_>>()
        .join(", ");
    format!("IN ({placeholders})")
}

#[cfg(test)]
#[path = "signature_tests.rs"]
mod tests;

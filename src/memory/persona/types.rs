//! Canonical evidence model for persona distillation (doc 06 §6.3).
//!
//! Everything the readers emit and the pipeline consumes is expressed in terms
//! of the contracts here: the [`PersonaSourceKind`] a unit of evidence came
//! from, its [`EvidenceTier`] (confidence ladder), the [`PersonaFacet`]s it
//! informs, and the redacted [`PersonaEvidence`] excerpt itself. The LLM map
//! step (§6.5) turns a batch of evidence into a [`SessionDigest`].
//!
//! ## Redaction boundary (fail-closed)
//!
//! [`PersonaEvidence`] can only be constructed through [`PersonaEvidence::new`],
//! which runs the raw excerpt through
//! [`sanitize_text`](crate::memory::store::safety::sanitize_text) *before* the
//! value is stored on the struct. `sanitize_text` is the composite redactor: it
//! scrubs secrets/tokens/keys (OpenAI `sk-…`, GitHub `gh*_…`, OAuth/bearer
//! credentials) *and* runs the formatted-PII pass
//! ([`redact_pii`](crate::memory::store::safety::pii::redact_pii)) the plan
//! names in §6.3 — the stronger of the two, chosen because transcripts are full
//! of tokens and keys, not just national-id-shaped PII. There is no way to build
//! evidence from unredacted text, so nothing unredacted can leave the reader
//! layer or reach an LLM. The struct field is private to enforce this.
//!
//! ## Deterministic ids
//!
//! Evidence ids are content-addressed — `sha256(source_id ‖ excerpt)[..32]` —
//! mirroring the archivist leaf-id convention so re-runs dedupe naturally.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::memory::store::safety::sanitize_text;

/// The on-disk source a unit of persona evidence came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaSourceKind {
    /// Claude Code transcripts (`~/.claude/projects/**/*.jsonl`).
    ClaudeCode,
    /// Codex rollout transcripts (`~/.codex/sessions/**/rollout-*.jsonl`).
    Codex,
    /// opencode SQLite store (`opencode.db`).
    Opencode,
    /// Cursor workspace/global KV stores (`state.vscdb`).
    Cursor,
    /// User-supplied ChatGPT export (`conversations.json`).
    ChatgptExport,
    /// User-supplied Claude.ai / Cowork export JSON.
    ClaudeExport,
    /// Explicitly authored agent instruction files (CLAUDE.md / AGENTS.md / …).
    InstructionFile,
    /// Git commit history.
    GitHistory,
}

impl PersonaSourceKind {
    /// Stable wire/string form (matches the serde `snake_case` encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            PersonaSourceKind::ClaudeCode => "claude_code",
            PersonaSourceKind::Codex => "codex",
            PersonaSourceKind::Opencode => "opencode",
            PersonaSourceKind::Cursor => "cursor",
            PersonaSourceKind::ChatgptExport => "chatgpt_export",
            PersonaSourceKind::ClaudeExport => "claude_export",
            PersonaSourceKind::InstructionFile => "instruction_file",
            PersonaSourceKind::GitHistory => "git_history",
        }
    }
}

/// Confidence ladder used for weighting and conflict resolution (§6.3).
///
/// Higher tiers win conflicts; within a tier, newer evidence wins. `T3` may
/// only corroborate — it can never establish or override a preference on its
/// own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    /// Inferred from accepted outcomes (merged diffs, un-corrected output).
    /// Lowest confidence; corroborates only.
    T3,
    /// User prompt phrasing, slash-command habits, commit-message style.
    T2,
    /// In-transcript corrections / interrupts — the single highest inference
    /// signal short of an explicit rule.
    T1,
    /// Explicit instruction files — the person wrote the rule down.
    T0,
}

impl EvidenceTier {
    /// Stable string form (matches the serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceTier::T0 => "t0",
            EvidenceTier::T1 => "t1",
            EvidenceTier::T2 => "t2",
            EvidenceTier::T3 => "t3",
        }
    }

    /// Numeric rank (0 = weakest `T3`, 3 = strongest `T0`) for ordering and
    /// weighting. Deliberately aligned with the [`Ord`] derive above.
    pub fn rank(self) -> u8 {
        match self {
            EvidenceTier::T3 => 0,
            EvidenceTier::T2 => 1,
            EvidenceTier::T1 => 2,
            EvidenceTier::T0 => 3,
        }
    }

    /// Parse the loose forms an LLM might emit (`"T1"`, `"t1"`, `"tier1"`, `1`).
    pub fn parse_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().trim_start_matches("tier").trim() {
            "t0" | "0" => Some(EvidenceTier::T0),
            "t1" | "1" => Some(EvidenceTier::T1),
            "t2" | "2" => Some(EvidenceTier::T2),
            "t3" | "3" => Some(EvidenceTier::T3),
            _ => None,
        }
    }
}

/// The seven distillation lenses (§6.3). Each maps to one flavoured tree with a
/// purpose-written `ask` and one section of the compiled pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaFacet {
    /// Tone, verbosity, directness, phrasing quirks, how they give feedback.
    Communication,
    /// Naming, structure, comments, error handling, testing habits, size
    /// discipline.
    CodingStyle,
    /// Languages, frameworks, libraries, recurring architectural choices.
    Stack,
    /// Branching/commit granularity, plan-first vs. dive-in, PR habits, review
    /// strictness, parallelism.
    Workflow,
    /// Editors/harnesses, CLIs, package managers, OS.
    Environment,
    /// Explicit standing rules (mostly T0, near-verbatim).
    Directives,
    /// Pet peeves: things they correct agents for, revert, or forbid.
    AntiPreferences,
}

impl PersonaFacet {
    /// Every facet, in the fixed compile order used by the pack.
    pub const ALL: [PersonaFacet; 7] = [
        PersonaFacet::Directives,
        PersonaFacet::Communication,
        PersonaFacet::CodingStyle,
        PersonaFacet::Stack,
        PersonaFacet::Workflow,
        PersonaFacet::Environment,
        PersonaFacet::AntiPreferences,
    ];

    /// Stable string form (matches the serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            PersonaFacet::Communication => "communication",
            PersonaFacet::CodingStyle => "coding_style",
            PersonaFacet::Stack => "stack",
            PersonaFacet::Workflow => "workflow",
            PersonaFacet::Environment => "environment",
            PersonaFacet::Directives => "directives",
            PersonaFacet::AntiPreferences => "anti_preferences",
        }
    }

    /// Human-facing section heading used in the compiled pack (§6.9).
    pub fn heading(self) -> &'static str {
        match self {
            PersonaFacet::Communication => "Communication style",
            PersonaFacet::CodingStyle => "Coding style",
            PersonaFacet::Stack => "Stack",
            PersonaFacet::Workflow => "Workflow",
            PersonaFacet::Environment => "Environment",
            PersonaFacet::Directives => "Directives",
            PersonaFacet::AntiPreferences => "Anti-preferences",
        }
    }

    /// Flavoured-tree scope for this facet (`persona/<facet>`).
    pub fn tree_scope(self) -> String {
        format!("persona/{}", self.as_str())
    }

    /// Parse the loose forms an LLM might emit.
    pub fn parse_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "communication" | "comms" | "tone" => Some(PersonaFacet::Communication),
            "coding_style" | "code_style" | "coding" | "style" => Some(PersonaFacet::CodingStyle),
            "stack" | "tech_stack" | "technology" => Some(PersonaFacet::Stack),
            "workflow" | "process" => Some(PersonaFacet::Workflow),
            "environment" | "env" | "tooling" => Some(PersonaFacet::Environment),
            "directives" | "rules" | "directive" => Some(PersonaFacet::Directives),
            "anti_preferences" | "anti_preference" | "antipreferences" | "dislikes"
            | "pet_peeves" => Some(PersonaFacet::AntiPreferences),
            _ => None,
        }
    }

    /// Default natural-language `ask` steering this facet's flavoured tree
    /// (§6.5). Overridable via `PersonaConfig`.
    pub fn default_ask(self) -> &'static str {
        match self {
            PersonaFacet::Communication => {
                "Distill how this person communicates with their coding agents: tone, \
                 verbosity, directness, recurring phrasing and quirks, and how they give \
                 feedback. Describe the patterns as prescriptive guidance an agent should \
                 mirror, not a list of individual messages."
            }
            PersonaFacet::CodingStyle => {
                "Distill this person's coding style: naming, code structure, comment and \
                 documentation habits, error handling, testing discipline, and module/size \
                 discipline. Phrase as concrete rules an agent writing code for them should \
                 follow."
            }
            PersonaFacet::Stack => {
                "Distill this person's technology stack and architectural preferences: the \
                 languages, frameworks, libraries, and recurring architectural choices they \
                 favour. Phrase as defaults an agent should reach for."
            }
            PersonaFacet::Workflow => {
                "Distill this person's development workflow: branching and commit \
                 granularity, plan-first vs. dive-in, PR and review habits, and how they use \
                 parallelism (worktrees, subagents). Phrase as process rules an agent should \
                 follow."
            }
            PersonaFacet::Environment => {
                "Distill this person's working environment: the editors and agent harnesses, \
                 CLIs, package managers, and operating system they use. Phrase as facts an \
                 agent should assume."
            }
            PersonaFacet::Directives => {
                "Collect this person's explicit standing rules for their agents, kept as \
                 close to verbatim as possible. These are commands they have written down; \
                 preserve their exact intent and wording."
            }
            PersonaFacet::AntiPreferences => {
                "Distill the things this person dislikes, corrects agents for, reverts, or \
                 explicitly forbids — phrased as rules an agent must not break."
            }
        }
    }
}

/// Provenance of a unit of evidence: where in the source corpus it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSource {
    /// Which on-disk source family produced it.
    pub kind: PersonaSourceKind,
    /// Project / repo / channel the evidence is scoped to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Session id or commit sha the evidence belongs to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Absolute or repo-relative path the evidence was read from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl EvidenceSource {
    /// Build a source with only its kind set; use the `with_*` setters to add
    /// provenance.
    pub fn new(kind: PersonaSourceKind) -> Self {
        Self {
            kind,
            scope: None,
            session_id: None,
            path: None,
        }
    }

    /// Attach the project/repo/channel scope.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Attach the session id or commit sha.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Attach the source file path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Stable string used as the first half of a content-addressed evidence id.
    /// Deterministic in the fields that identify the *origin* (not the excerpt).
    pub fn source_id(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.kind.as_str(),
            self.scope.as_deref().unwrap_or(""),
            self.session_id.as_deref().unwrap_or(""),
            self.path.as_deref().unwrap_or(""),
        )
    }
}

/// Content-address an evidence id: `sha256(source_id ‖ excerpt)[..32]` (hex).
pub fn evidence_id(source_id: &str, excerpt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    hasher.update(b"\x1f"); // unit separator, keeps the two halves unambiguous
    hasher.update(excerpt.as_bytes());
    let digest = hasher.finalize();
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    hex[..32].to_string()
}

/// One unit of redacted persona evidence.
///
/// Construct only via [`PersonaEvidence::new`]; the `excerpt` field is private
/// so evidence cannot be built from unredacted text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaEvidence {
    /// Content-addressed id (`sha256(source_id ‖ excerpt)[..32]`).
    pub id: String,
    /// Where the evidence came from.
    pub source: EvidenceSource,
    /// When the underlying event happened.
    pub timestamp: DateTime<Utc>,
    /// Confidence tier.
    pub tier: EvidenceTier,
    /// Facets this evidence is a candidate for. The digest step may refine this;
    /// readers set a coarse default.
    #[serde(default)]
    pub facets: Vec<PersonaFacet>,
    /// Redacted excerpt (private: only [`PersonaEvidence::new`] may set it,
    /// after running the raw text through `redact_pii`).
    excerpt: String,
}

impl PersonaEvidence {
    /// Build a unit of evidence, redacting `raw_excerpt` before it is stored.
    ///
    /// This is the *only* constructor. The id is derived from the redacted
    /// excerpt so it is stable across re-runs and independent of any secret or
    /// PII that was stripped.
    pub fn new(
        source: EvidenceSource,
        timestamp: DateTime<Utc>,
        tier: EvidenceTier,
        raw_excerpt: &str,
        facets: Vec<PersonaFacet>,
    ) -> Self {
        let excerpt = sanitize_text(raw_excerpt).value;
        let id = evidence_id(&source.source_id(), &excerpt);
        Self {
            id,
            source,
            timestamp,
            tier,
            facets,
            excerpt,
        }
    }

    /// The redacted excerpt. There is no way to read unredacted text back.
    pub fn excerpt(&self) -> &str {
        &self.excerpt
    }
}

/// One observation emitted by the LLM map step for a single facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestObservation {
    /// Which facet this observation informs.
    pub facet: PersonaFacet,
    /// Prescriptive statement of the pattern (an agent-facing rule).
    pub observation: String,
    /// Short supporting quote drawn from the (already redacted) evidence.
    #[serde(default)]
    pub quote: String,
    /// Confidence tier the model assigned this observation.
    pub tier: EvidenceTier,
}

/// Output of the LLM map step (§6.5): structured, per-facet observations for one
/// digested session or commit batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDigest {
    /// Provenance of the digested unit.
    pub source: EvidenceSource,
    /// Per-facet observations.
    #[serde(default)]
    pub observations: Vec<DigestObservation>,
}

impl SessionDigest {
    /// A digest with no observations (the soft-fallback result for a session
    /// the model failed to process — see §6.5).
    pub fn empty(source: EvidenceSource) -> Self {
        Self {
            source,
            observations: Vec::new(),
        }
    }

    /// True when the model produced no usable observations.
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;

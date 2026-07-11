//! Turn-based reflection over the goals list.
//!
//! In OpenHuman, reflection is performed by a real multi-turn agent
//! (`goals_agent`, restricted to the `goals_*` tools + `memory_recall`): it
//! reads the current list, considers a supplied context, and applies
//! add/edit/delete over several turns. On an empty list (first run) it
//! bootstraps an initial set; otherwise it makes the *minimal* set of justified
//! changes.
//!
//! TinyCortex does not call a real LLM. Instead the LLM step is abstracted
//! behind the [`GoalsGenerator`] trait, which — given the current document, the
//! context, and a `first_run` flag — proposes a list of [`GoalMutation`]s. The
//! *deterministic* part of reflection (applying mutations, de-duplicating
//! additions, and re-enforcing the persistence caps) is ported faithfully here
//! and is fully testable without any model.
//!
//! The "minimal changes unless empty" decision from OpenHuman is preserved:
//! [`reflect`] computes `first_run` from the loaded document and threads it (and
//! the corresponding [`build_prompt`] instruction) into the generator. The
//! default [`NoopGenerator`] proposes nothing, so reflecting a non-empty list is
//! a no-op — exactly the "make no churn unless justified" behaviour.

use crate::memory::config::MemoryConfig;
use crate::memory::error::MemoryEngineResult;

use super::store;
use super::types::GoalsDoc;

/// A single proposed change to the goals list, emitted by a [`GoalsGenerator`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalMutation {
    /// Append a new goal with this text.
    Add {
        /// Goal body to append.
        text: String,
    },
    /// Replace the text of the goal addressed by `id`.
    Edit {
        /// Id of the goal to edit.
        id: String,
        /// Replacement goal body.
        text: String,
    },
    /// Remove the goal addressed by `id`.
    Delete {
        /// Id of the goal to remove.
        id: String,
    },
}

/// Abstraction over the LLM-driven reflection step.
///
/// Implementors decide *what* should change; the deterministic [`reflect`]
/// driver decides *how* it is applied (dedupe, validation, caps, persistence).
/// A production host would back this with the `goals_agent`; tests inject a
/// canned generator.
pub trait GoalsGenerator {
    /// Propose the set of mutations to apply, given the current `doc`, the
    /// `context` nudge, and whether this is the first (empty-list) run.
    fn propose(&self, doc: &GoalsDoc, context: &str, first_run: bool) -> Vec<GoalMutation>;
}

/// A generator that proposes nothing. Reflecting with this leaves a non-empty
/// list untouched (minimal-change behaviour) and an empty list empty.
pub struct NoopGenerator;

impl GoalsGenerator for NoopGenerator {
    fn propose(&self, _doc: &GoalsDoc, _context: &str, _first_run: bool) -> Vec<GoalMutation> {
        Vec::new()
    }
}

/// Outcome of a reflection pass.
#[derive(Debug, Clone)]
pub struct ReflectOutcome {
    /// Whether this was a first run (the list was empty on entry).
    pub first_run: bool,
    /// Number of proposed mutations actually applied.
    pub applied: usize,
    /// Number of proposed mutations skipped (duplicate add, unknown id,
    /// invalid text).
    pub skipped: usize,
    /// Short human-readable summary of what happened.
    pub summary: String,
    /// The goals list after reflection (post-cap-enforcement).
    pub goals: GoalsDoc,
}

/// Build the instruction handed to the goals generator. `first_run` switches
/// the instruction between initial population and incremental maintenance.
/// Ported from OpenHuman's `goals_agent` prompt builder.
pub fn build_prompt(context_input: &str, first_run: bool) -> String {
    let mode = if first_run {
        "The goals list is currently EMPTY. This is the first run — populate \
         an initial set of the user's durable long-term goals (max ~8) from \
         the context below. Start by calling goals_list to confirm, then use \
         goals_add for each goal."
    } else {
        "Maintain the existing goals list. Call goals_list first, then make \
         the MINIMAL set of changes (goals_add / goals_edit / goals_delete) \
         justified by the context below. Do not churn goals that are still \
         valid."
    };

    format!(
        "{mode}\n\n\
         Keep goals concise (one sentence each), durable (long-term, not \
         per-task), and free of secrets or PII.\n\n\
         ## Context\n\n{context_input}\n"
    )
}

/// Normalise goal text for dedupe comparison: trim, lowercase, and collapse
/// internal whitespace runs to single spaces.
fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Apply `mutations` to `doc` deterministically, de-duplicating additions by
/// normalised text. Returns `(applied, skipped)` counts. Does not persist.
///
/// NOTE: the normalised-duplicate check only guards `Add`. An `Edit` that
/// rewrites one goal's text to match another existing goal's (normalised)
/// text is still applied as-is, so a generator can converge the list to
/// several items with identical text. Callers that need strict uniqueness
/// must check for this themselves.
fn apply_mutations(doc: &mut GoalsDoc, mutations: &[GoalMutation]) -> (usize, usize) {
    let mut applied = 0usize;
    let mut skipped = 0usize;
    for mutation in mutations {
        match mutation {
            GoalMutation::Add { text } => {
                let norm = normalise(text);
                let duplicate =
                    !norm.is_empty() && doc.items.iter().any(|i| normalise(&i.text) == norm);
                if duplicate {
                    skipped += 1;
                    continue;
                }
                match doc.add(text) {
                    Ok(_) => applied += 1,
                    Err(_) => skipped += 1,
                }
            }
            GoalMutation::Edit { id, text } => match doc.edit(id, text) {
                Ok(()) => applied += 1,
                Err(_) => skipped += 1,
            },
            GoalMutation::Delete { id } => match doc.delete(id) {
                Ok(()) => applied += 1,
                Err(_) => skipped += 1,
            },
        }
    }
    (applied, skipped)
}

/// Run a reflection pass over the goals list rooted at `config`.
///
/// Loads the current list, decides `first_run` from emptiness, asks the
/// injected `generator` for proposed mutations (informed by `context`), applies
/// them deterministically (dedupe + validation), persists with cap enforcement,
/// and returns a [`ReflectOutcome`]. The mutation lock in [`super::store`]
/// serialises the persisted save against concurrent edits.
pub fn reflect(
    config: &MemoryConfig,
    context: &str,
    generator: &dyn GoalsGenerator,
) -> MemoryEngineResult<ReflectOutcome> {
    let _guard = store::goals_mutation_lock().lock();
    let mut doc = store::load(&config.workspace)?;
    let first_run = doc.is_empty();

    let mutations = generator.propose(&doc, context, first_run);
    let (applied, skipped) = apply_mutations(&mut doc, &mutations);

    if applied > 0 {
        store::save(&config.workspace, &mut doc)?;
    } else {
        // No applied changes: re-load to reflect on-disk truth (the generator
        // may have proposed only no-ops). Avoids rewriting the file needlessly.
        doc = store::load(&config.workspace)?;
    }

    let summary = if first_run {
        format!("first run: populated {applied} goal(s) ({skipped} skipped)")
    } else {
        format!("maintenance: applied {applied} change(s) ({skipped} skipped)")
    };

    Ok(ReflectOutcome {
        first_run,
        applied,
        skipped,
        summary,
        goals: doc,
    })
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;

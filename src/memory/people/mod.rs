//! People: contact resolution + scoring.
//!
//! A5 module. Deterministic resolver maps (imessage handle | email | display
//! name) to a stable `PersonId`. Scoring blends recency × frequency ×
//! reciprocity × depth from interaction rows into a ranked `people.list`.
//!
//! Intentionally self-contained: no dependency on `life_capture`,
//! `chronicle`, `nudges`, or UI. Integration happens in later slices.

pub mod address_book;
pub mod migrations;
pub mod resolver;
pub mod scorer;
pub mod store;
pub mod types;

#[cfg(test)]
mod tests;

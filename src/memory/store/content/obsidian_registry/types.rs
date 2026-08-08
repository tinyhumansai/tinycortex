//! Types for Obsidian vault-*registration* detection: the probe result and
//! the minimal `obsidian.json` shape.

use std::collections::HashMap;

use serde::Deserialize;

/// Outcome of a registration probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRegistration {
    /// `true` when some registered Obsidian vault's path equals or is an
    /// ancestor of the content root.
    pub registered: bool,
    /// `true` when at least one candidate `obsidian.json` was found/read (even
    /// if parsing it later fails — see the parse-error branch, which still
    /// counts the file as found). Lets the UI distinguish "Obsidian is set up,
    /// vault just not added yet" from "couldn't find Obsidian at all" (offer
    /// install vs. offer add-as-vault).
    pub config_found: bool,
}

/// Minimal shape of Obsidian's `obsidian.json`. We only need each vault's
/// `path`; `ts`/`open` and any future keys are ignored by `serde`.
#[derive(Debug, Deserialize)]
pub struct ObsidianConfig {
    #[serde(default)]
    pub vaults: HashMap<String, VaultEntry>,
}

#[derive(Debug, Deserialize)]
pub struct VaultEntry {
    pub path: String,
}

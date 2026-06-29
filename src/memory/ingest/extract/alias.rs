//! Alias resolution: build a short→long alias map from extracted entities and
//! resolve names through it transitively.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::types::RawEntity;

/// Invert an alias map (`alias → canonical`) into `canonical → [aliases]`.
pub(super) fn reverse_aliases(aliases: &HashMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut reverse = BTreeMap::new();
    for (alias, canonical) in aliases {
        if alias == canonical {
            continue;
        }
        reverse
            .entry(canonical.clone())
            .or_insert_with(Vec::new)
            .push(alias.clone());
    }
    for values in reverse.values_mut() {
        values.sort();
        values.dedup();
    }
    reverse
}

/// Build a short→long alias map within each entity type. A shorter name that is
/// a leading/trailing word of a longer same-type name aliases to the longer one.
pub(super) fn build_alias_map(entities: &HashMap<String, RawEntity>) -> HashMap<String, String> {
    let mut by_type = HashMap::<String, Vec<String>>::new();
    for entity in entities.values() {
        by_type
            .entry(entity.entity_type.clone())
            .or_default()
            .push(entity.name.clone());
    }

    let mut aliases = HashMap::new();
    for names in by_type.values_mut() {
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        for short in names.iter() {
            for long in names.iter() {
                if short == long || long.len() <= short.len() {
                    continue;
                }
                if long.starts_with(&format!("{short} ")) || long.ends_with(&format!(" {short}")) {
                    aliases.entry(short.clone()).or_insert_with(|| long.clone());
                    break;
                }
            }
        }
    }
    aliases
}

/// Resolve `name` through the alias map, following chains and guarding cycles.
pub(super) fn resolve_alias(name: &str, aliases: &HashMap<String, String>) -> String {
    let mut current = name.to_string();
    let mut seen = BTreeSet::new();
    while let Some(next) = aliases.get(&current) {
        if !seen.insert(current.clone()) {
            break;
        }
        current = next.clone();
    }
    current
}

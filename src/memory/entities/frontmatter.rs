//! Hand-rolled YAML front-matter reader/writer.
//!
//! `serde_yaml` is intentionally not a dependency, so this module ports
//! OpenHuman's exact front-matter handling by hand. The on-disk format is
//! byte-for-byte identical to OpenHuman:
//!
//! ```markdown
//! ---
//! id: person:alice
//! kind: person
//! display_name: Alice Cooper
//! aliases:
//!   - Ali
//! emails:
//!   - alice@example.com
//! handles:
//!   - kind: slack
//!     value: U12345
//! created_at: 2026-05-23T22:00:00+00:00
//! updated_at: 2026-05-23T22:00:00+00:00
//! ---
//!
//! Free-form notes the user can edit. Preserved across upserts.
//! ```
//!
//! Only the well-known scalar/list shapes the [`Entity`] uses are emitted and
//! parsed; this is deliberately not a general YAML implementation. The free
//! text after the closing `---` is the notes body and is never interpreted —
//! [`extract_notes`] hands it back verbatim so upserts can round-trip it.

use chrono::{DateTime, Utc};

use super::types::{Entity, EntityHandle, EntityKind};

/// Render an [`Entity`]'s front matter followed by the preserved `notes` body.
///
/// The trailing newline of the document is normalised so files always end in
/// exactly one `\n`.
pub(crate) fn compose(entity: &Entity, notes: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("id: {}\n", entity.id));
    out.push_str(&format!("kind: {}\n", entity.kind.as_str()));
    if let Some(name) = entity.display_name.as_deref() {
        out.push_str(&format!("display_name: {}\n", yaml_string(name)));
    }
    if !entity.aliases.is_empty() {
        out.push_str("aliases:\n");
        for a in &entity.aliases {
            out.push_str(&format!("  - {}\n", yaml_string(a)));
        }
    }
    if !entity.emails.is_empty() {
        out.push_str("emails:\n");
        for e in &entity.emails {
            out.push_str(&format!("  - {}\n", yaml_string(e)));
        }
    }
    if !entity.handles.is_empty() {
        out.push_str("handles:\n");
        for h in &entity.handles {
            out.push_str(&format!(
                "  - kind: {}\n    value: {}\n",
                yaml_string(&h.kind),
                yaml_string(&h.value)
            ));
        }
    }
    out.push_str(&format!("created_at: {}\n", entity.created_at.to_rfc3339()));
    out.push_str(&format!("updated_at: {}\n", entity.updated_at.to_rfc3339()));
    out.push_str("---\n\n");
    out.push_str(notes);
    if !notes.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Quote a scalar only when it contains a character that would change YAML
/// parsing, escaping backslashes and double quotes inside the quoted form.
fn yaml_string(s: &str) -> String {
    let needs_quote = s
        .chars()
        .any(|c| matches!(c, ':' | '#' | '\n' | '"' | '\'' | '[' | ']' | '{' | '}'));
    if needs_quote {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Inverse of [`yaml_string`] — drop surrounding double quotes and unescape.
fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .map(|x| x.replace("\\\"", "\"").replace("\\\\", "\\"))
        .unwrap_or_else(|| s.to_string())
}

/// Split a document into `(front_matter_yaml, notes_body)`. Returns `None`
/// when the leading `---` fence or its closing `---` is absent.
fn split_front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let (yaml, after) = rest.split_at(end);
    let body = after.strip_prefix("\n---\n").unwrap_or(after);
    Some((yaml, body))
}

/// Return just the notes body of a document, or an empty string when the file
/// has no recognisable front matter.
pub(crate) fn extract_notes(text: &str) -> String {
    split_front_matter(text)
        .map(|(_, body)| body.to_string())
        .unwrap_or_default()
}

/// Parse a full entity document. Returns `None` when the front matter is
/// missing or omits the required `kind`. Missing timestamps default to "now"
/// so hand-authored files without them still load.
pub(crate) fn parse(text: &str) -> Option<Entity> {
    let (yaml, body) = split_front_matter(text)?;
    let mut id = String::new();
    let mut kind: Option<EntityKind> = None;
    let mut display_name: Option<String> = None;
    let mut aliases = Vec::new();
    let mut emails = Vec::new();
    let mut handles = Vec::new();
    let mut created_at: Option<DateTime<Utc>> = None;
    let mut updated_at: Option<DateTime<Utc>> = None;

    let mut current_list: Option<&'static str> = None;
    let mut handle_buf: Option<EntityHandle> = None;

    for raw in yaml.lines() {
        if raw.starts_with("  - kind:") {
            // Flush previous handle, start a new one.
            if let Some(h) = handle_buf.take() {
                handles.push(h);
            }
            let v = raw.trim_start_matches("  - kind:").trim();
            handle_buf = Some(EntityHandle {
                kind: unquote(v),
                value: String::new(),
            });
            current_list = Some("handles");
            continue;
        }
        if raw.starts_with("    value:") {
            let v = raw.trim_start_matches("    value:").trim();
            if let Some(h) = handle_buf.as_mut() {
                h.value = unquote(v);
            }
            continue;
        }
        if let Some(v) = raw.strip_prefix("  - ") {
            let v = unquote(v.trim());
            match current_list {
                Some("aliases") => aliases.push(v),
                Some("emails") => emails.push(v),
                _ => {}
            }
            continue;
        }
        // Flush any in-progress handle when we leave the handle list.
        if !raw.starts_with(' ') && !raw.starts_with("  - kind") {
            if let Some(h) = handle_buf.take() {
                handles.push(h);
            }
            current_list = None;
        }
        let Some((k, v)) = raw.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "id" => id = unquote(v),
            "kind" => kind = EntityKind::parse(&unquote(v)).ok(),
            "display_name" => display_name = Some(unquote(v)),
            "aliases" => current_list = Some("aliases"),
            "emails" => current_list = Some("emails"),
            "handles" => current_list = Some("handles"),
            "created_at" => {
                created_at = DateTime::parse_from_rfc3339(&unquote(v))
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }
            "updated_at" => {
                updated_at = DateTime::parse_from_rfc3339(&unquote(v))
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }
            _ => {}
        }
    }
    if let Some(h) = handle_buf {
        handles.push(h);
    }

    let now = Utc::now();
    let _ = body; // notes are preserved on write but not surfaced in Entity
    Some(Entity {
        id,
        kind: kind?,
        display_name,
        aliases,
        emails,
        handles,
        created_at: created_at.unwrap_or(now),
        updated_at: updated_at.unwrap_or(now),
    })
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;

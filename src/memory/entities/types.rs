//! Entity shape.
//!
//! One serde struct covers every kind. The `kind` field discriminates; the
//! optional fields (`emails`, `handles`, `aliases`, plus the on-disk notes
//! body) populate as relevant.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Kinds an entity can take.
///
/// Mirrors the entity-kind taxonomy the tree scorer emits so the canonical
/// ids produced during scoring round-trip through this module unchanged. The
/// taxonomy spans mechanical kinds (emails, URLs, handles, hashtags) and
/// semantic kinds (person, organization, location, topic, …).
///
/// Kept as a local enum (not a re-export of the score module's type) so
/// `entities` stays usable independently of the score module's internals,
/// while the wire strings stay byte-for-byte identical across the two.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Person,
    Organization,
    Topic,
    Email,
    Url,
    Handle,
    Hashtag,
    Location,
    Event,
    Product,
    Datetime,
    Technology,
    Artifact,
    Quantity,
    Misc,
}

impl EntityKind {
    /// Stable wire string for this kind. Used both as the on-disk directory
    /// name (`entities/<kind>/…`) and as the `kind:` prefix of canonical ids.
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Person => "person",
            EntityKind::Organization => "organization",
            EntityKind::Topic => "topic",
            EntityKind::Email => "email",
            EntityKind::Url => "url",
            EntityKind::Handle => "handle",
            EntityKind::Hashtag => "hashtag",
            EntityKind::Location => "location",
            EntityKind::Event => "event",
            EntityKind::Product => "product",
            EntityKind::Datetime => "datetime",
            EntityKind::Technology => "technology",
            EntityKind::Artifact => "artifact",
            EntityKind::Quantity => "quantity",
            EntityKind::Misc => "misc",
        }
    }

    /// Parse a wire string back into a kind. The inverse of [`as_str`].
    ///
    /// [`as_str`]: EntityKind::as_str
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "person" => Ok(Self::Person),
            "organization" => Ok(Self::Organization),
            "topic" => Ok(Self::Topic),
            "email" => Ok(Self::Email),
            "url" => Ok(Self::Url),
            "handle" => Ok(Self::Handle),
            "hashtag" => Ok(Self::Hashtag),
            "location" => Ok(Self::Location),
            "event" => Ok(Self::Event),
            "product" => Ok(Self::Product),
            "datetime" => Ok(Self::Datetime),
            "technology" => Ok(Self::Technology),
            "artifact" => Ok(Self::Artifact),
            "quantity" => Ok(Self::Quantity),
            "misc" => Ok(Self::Misc),
            other => Err(format!("unknown entity kind: {other}")),
        }
    }
}

/// A handle is an opaque label by which this entity is known to a source.
///
/// Generalisation of a legacy person `Handle` — works for emails, phone
/// numbers, social handles, anything that identifies the entity in one
/// channel without being its canonical id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityHandle {
    /// e.g. `"imessage"`, `"slack"`, `"discord"`, `"gmail"`.
    pub kind: String,
    /// The channel-specific identifier (a user id, address, phone number, …).
    pub value: String,
}

/// One entity. Persisted as `<content_root>/entities/<kind>/<canonical_id>.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    /// Canonical id — `<kind>:<value>` (e.g. `person:alice`,
    /// `email:alice@example.com`). Stable across renames and aliases.
    pub id: String,
    /// Discriminator selecting the on-disk directory and the id prefix.
    pub kind: EntityKind,
    /// Free-form display name. `None` when the user hasn't named the entity
    /// yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Alternate strings the entity is known by (nicknames, old names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Email addresses associated with the entity. Pulled out of the generic
    /// `handles` for Person convenience.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emails: Vec<String>,
    /// Source-specific handles (slack, discord, imessage, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handles: Vec<EntityHandle>,
    /// First write timestamp.
    pub created_at: DateTime<Utc>,
    /// Last upsert timestamp.
    pub updated_at: DateTime<Utc>,
}

impl Entity {
    /// Construct a fresh entity. `id` should already be canonicalised
    /// (`<kind>:<value>`); callers are responsible for that — see
    /// [`crate::memory::entities::canonical::canonical_id_for`].
    pub fn new(id: impl Into<String>, kind: EntityKind) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            kind,
            display_name: None,
            aliases: Vec::new(),
            emails: Vec::new(),
            handles: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;

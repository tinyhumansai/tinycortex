//! System-prompt construction for the LLM entity extractor.
//!
//! Split out of `llm.rs` to keep that file focused on the extractor lifecycle
//! (provider call, retry, JSON parse, span recovery). The prompt is a large
//! static template plus the optional `topics` / output-language toggles.

/// Human-readable language name for a BCP-47-ish code. Falls back to the raw
/// code when unknown so the directive is still meaningful.
fn language_display_name(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "zh-cn" | "zh-hans" | "zh" => "Simplified Chinese".to_string(),
        "zh-tw" | "zh-hant" => "Traditional Chinese".to_string(),
        "en" | "en-us" | "en-gb" => "English".to_string(),
        "es" => "Spanish".to_string(),
        "fr" => "French".to_string(),
        "de" => "German".to_string(),
        "ja" => "Japanese".to_string(),
        other => other.to_string(),
    }
}

/// Build the natural-language output directive, or `None` for no override.
///
/// Keeps JSON keys and enum values in English so parsing stays stable while
/// free-text fields (`importance_reason`, topic labels) follow the configured
/// language.
fn output_language_directive(lang: Option<&str>) -> Option<String> {
    let lang = lang?;
    if lang.trim().is_empty() {
        return None;
    }
    let name = language_display_name(lang);
    Some(format!(
        "Write all natural-language field values (such as \"importance_reason\" and topic \
         labels) in {name}. Keep JSON keys and enum values (the \"kind\" names) in English."
    ))
}

/// Build the system prompt for the extractor. When `emit_topics` is true the
/// schema, required-fields list, and example outputs include a `topics` array.
pub(super) fn build_system_prompt(emit_topics: bool, output_language: Option<&str>) -> String {
    let topics_schema_line = if emit_topics {
        "  \"topics\": [\"<short theme label>\"],\n"
    } else {
        ""
    };
    let topics_required = if emit_topics { "topics, " } else { "" };
    let fields_count = if emit_topics { "four" } else { "three" };
    let topics_guide = if emit_topics {
        "Topics are short free-form theme labels for what the text is ABOUT \
         (e.g. \"rate limiting\", \"memory tree\", \"auth flow\"). They are \
         distinct from entities — entities are specific named things mentioned \
         in the text; topics are the abstract themes those things relate to.\n"
    } else {
        ""
    };
    let example1_topics = if emit_topics {
        ",\"topics\":[\"shipping\",\"auth\"]"
    } else {
        ""
    };
    let example2_topics = if emit_topics {
        ",\"topics\":[\"product launch\",\"revenue\"]"
    } else {
        ""
    };
    let language_directive = output_language_directive(output_language)
        .map(|directive| format!("{directive}\n\n"))
        .unwrap_or_default();

    format!(
        "{language_directive}You are a named-entity extractor and importance rater. Return JSON only — \
no prose, no markdown, no commentary. Do not summarize. Extract every named \
entity mention you find, including duplicates, and rate the chunk's overall \
importance as a float in [0.0, 1.0].

Schema:
{{
  \"entities\": [
    {{ \"kind\": \"person|organization|location|event|product|datetime|technology|artifact|quantity\",
      \"text\": \"<exact surface form as it appears in the text>\" }}
  ],
{topics_schema_line}  \"importance\": 0.0,
  \"importance_reason\": \"<one short sentence explaining the rating>\"
}}

Kinds guide:
  person       named human                            (\"Alice\", \"Steven Enamakel\")
  organization company / team / project               (\"Anthropic\", \"TinyHumans\")
  location     place                                  (\"SF office\", \"London\")
  event        scheduled occurrence                   (\"Q2 launch\", \"design review\")
  product      commercial offering                    (\"Claude Code\", \"OpenHuman\")
  datetime     temporal expression                    (\"Friday\", \"Q2 2026\", \"EOD tomorrow\")
  technology   tool / framework / language / service  (\"Rust\", \"OAuth\", \"Slack API\")
  artifact     code / ticket / doc reference          (\"PR #934\", \"src/foo.rs\", \"OH-42\")
  quantity     amount / metric / money                (\"$5K\", \"20/min\", \"10k tokens\")

{topics_guide}
If a mention doesn't clearly fit a kind above, omit it rather than guessing.
Always emit ALL {fields_count} top-level fields (entities, {topics_required}importance, importance_reason),
even when entities is empty.

Examples:

Input: alice and bob shipped the auth migration friday. PR #42 ships OAuth refactor in src/auth/.
Output: {{\"entities\":[{{\"kind\":\"person\",\"text\":\"alice\"}},{{\"kind\":\"person\",\"text\":\"bob\"}},{{\"kind\":\"event\",\"text\":\"auth migration\"}},{{\"kind\":\"datetime\",\"text\":\"friday\"}},{{\"kind\":\"artifact\",\"text\":\"PR #42\"}},{{\"kind\":\"technology\",\"text\":\"OAuth\"}},{{\"kind\":\"artifact\",\"text\":\"src/auth/\"}}]{example1_topics},\"importance\":0.9,\"importance_reason\":\"explicit shipping commitment\"}}

Input: Anthropic shipped Claude Code in SF — $20M ARR target by Q2.
Output: {{\"entities\":[{{\"kind\":\"organization\",\"text\":\"Anthropic\"}},{{\"kind\":\"product\",\"text\":\"Claude Code\"}},{{\"kind\":\"location\",\"text\":\"SF\"}},{{\"kind\":\"quantity\",\"text\":\"$20M ARR\"}},{{\"kind\":\"datetime\",\"text\":\"Q2\"}}]{example2_topics},\"importance\":0.85,\"importance_reason\":\"factual content with key business metric\"}}

Importance guide:
  0.9+  actionable decisions, key information, explicit commitments
  0.6+  substantive discussion, factual content, named entities
  0.3+  ambient context, low-density prose
  <0.3  reactions, acknowledgments, bots, trivial exchanges
"
    )
}

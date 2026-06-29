use super::*;

#[test]
fn layout_doc_subtree_nests_under_docs_and_version() {
    let p = summary_rel_path_with_layout(
        SummaryTreeKind::Source,
        "notion-conn1",
        2,
        "summary:1700000000000:L2-deadbeef",
        SummaryDiskLayout::DocSubtree {
            doc_slug: "notion-conn1-pageA",
            version_ms: Some(1717500000000),
        },
    );
    assert_eq!(
        p,
        "wiki/summaries/source-notion-conn1/docs/notion-conn1-pagea/v-1717500000000/L2/summary-1700000000000-L2-deadbeef.md"
    );
}

#[test]
fn layout_doc_subtree_unversioned_folder() {
    let p = summary_rel_path_with_layout(
        SummaryTreeKind::Source,
        "notion-conn1",
        1,
        "summary:1700000000000:L1-abcd0000",
        SummaryDiskLayout::DocSubtree {
            doc_slug: "notion-conn1-pageB",
            version_ms: None,
        },
    );
    assert!(
        p.contains("/docs/notion-conn1-pageb/v-unversioned/L1/"),
        "got {p}"
    );
}

#[test]
fn layout_merge_tier_nests_under_merge() {
    let p = summary_rel_path_with_layout(
        SummaryTreeKind::Source,
        "notion-conn1",
        1000,
        "summary:1700000000000:L1000-aaaa1111",
        SummaryDiskLayout::Merge,
    );
    assert_eq!(
        p,
        "wiki/summaries/source-notion-conn1/merge/L1000/summary-1700000000000-L1000-aaaa1111.md"
    );
}

#[test]
fn layout_standard_matches_flat_path() {
    let id = "summary:1700000000000:L1-cccc2222";
    let flat = summary_rel_path(SummaryTreeKind::Source, "slack-eng", 1, id);
    let std_layout = summary_rel_path_with_layout(
        SummaryTreeKind::Source,
        "slack-eng",
        1,
        id,
        SummaryDiskLayout::Standard,
    );
    assert_eq!(flat, std_layout);
}

// ─── slugify tests ────────────────────────────────────────────────────────

#[test]
fn slugify_slack_channel() {
    assert_eq!(slugify_source_id("slack:#general"), "slack-general");
}

#[test]
fn slugify_gmail_thread() {
    assert_eq!(
        slugify_source_id("gmail:thread/abc-123"),
        "gmail-thread-abc-123"
    );
}

#[test]
fn slugify_collapses_consecutive_separators() {
    assert_eq!(slugify_source_id("foo::bar"), "foo-bar");
}

#[test]
fn slugify_uppercase_lowercased() {
    assert_eq!(slugify_source_id("Slack:ABC"), "slack-abc");
}

#[test]
fn slugify_empty_falls_back_to_unknown() {
    assert_eq!(slugify_source_id(""), "unknown");
    assert_eq!(slugify_source_id(":::"), "unknown");
}

#[test]
fn slugify_truncates_at_120_chars() {
    let long = "a".repeat(200);
    let slug = slugify_source_id(&long);
    assert_eq!(slug.len(), 120);
}

#[test]
fn slugify_preserves_interior_underscore() {
    let s = slugify_source_id("_solo_");
    assert_eq!(s, "solo");
}

#[test]
fn slugify_preserves_interior_underscore_between_chars() {
    assert_eq!(slugify_source_id("foo_bar"), "foo_bar");
}

// ─── chunk_rel_path tests ─────────────────────────────────────────────────

#[test]
fn email_one_to_one_conversation_path() {
    let p = chunk_rel_path("email", "gmail:alice@x.com|bob@y.com", "abc");
    assert_eq!(p, "email/alice-x-com-bob-y-com/abc.md");
}

#[test]
fn email_group_conversation_path() {
    let p = chunk_rel_path("email", "gmail:notifications@github.com|sanil@x.com", "def");
    assert_eq!(p, "email/notifications-github-com-sanil-x-com/def.md");
}

#[test]
fn email_solo_no_to_path() {
    let p = chunk_rel_path("email", "gmail:alice@x.com", "solo123");
    assert_eq!(p, "email/alice-x-com/solo123.md");
}

#[test]
fn email_malformed_source_id_falls_back_to_flat_layout() {
    let p = chunk_rel_path("email", "legacyid", "xyz");
    assert!(p.starts_with("email/"));
    assert!(p.ends_with("/xyz.md"));
}

#[test]
fn email_three_participant_path() {
    let p = chunk_rel_path("email", "gmail:alice@x.com|bob@y.com|carol@z.com", "g42");
    assert_eq!(p, "email/alice-x-com-bob-y-com-carol-z-com/g42.md");
}

#[test]
fn chat_path() {
    let p = chunk_rel_path("chat", "slack:#eng", "xyz789");
    assert_eq!(p, "chat/slack-eng/xyz789.md");
}

#[test]
fn document_path() {
    let p = chunk_rel_path("document", "doc:notes.md", "uvw");
    assert_eq!(p, "document/doc-notes-md/uvw.md");
}

#[test]
fn chunk_abs_path_uses_os_separator() {
    use std::path::Path;
    let root = Path::new("/workspace/content");
    let abs = chunk_abs_path(root, "email", "gmail:alice@x.com|bob@y.com", "abc");
    assert!(abs.starts_with(root));
    assert!(abs.ends_with("abc.md"));
}

// ─── summary_rel_path tests ───────────────────────────────────────────────

#[test]
fn summary_rel_path_source() {
    let p = summary_rel_path(
        SummaryTreeKind::Source,
        "gmail-alice-x-com-bob-y-com",
        1,
        "summary:L1:abc",
    );
    assert_eq!(
        p,
        "wiki/summaries/source-gmail-alice-x-com-bob-y-com/L1/summary-L1-abc.md"
    );
}

#[test]
fn summary_rel_path_current_ids_keep_time_first_basename() {
    let p = summary_rel_path(
        SummaryTreeKind::Source,
        "slack-eng",
        2,
        "summary:1700000000000:L2-deadbeef",
    );
    assert_eq!(
        p,
        "wiki/summaries/source-slack-eng/L2/summary-1700000000000-L2-deadbeef.md"
    );
}

#[test]
fn summary_rel_path_global() {
    let p = summary_rel_path(SummaryTreeKind::Global, "global", 0, "summary:L0:daily");
    assert_eq!(p, "wiki/summaries/global/L0/summary-L0-daily.md");
}

#[test]
fn summary_rel_path_global_levels_share_one_folder() {
    let l0 = summary_rel_path(SummaryTreeKind::Global, "global", 0, "summary:L0:a");
    let l1 = summary_rel_path(SummaryTreeKind::Global, "global", 1, "summary:L1:b");
    let l3 = summary_rel_path(SummaryTreeKind::Global, "global", 3, "summary:L3:c");
    assert_eq!(l0, "wiki/summaries/global/L0/summary-L0-a.md");
    assert_eq!(l1, "wiki/summaries/global/L1/summary-L1-b.md");
    assert_eq!(l3, "wiki/summaries/global/L3/summary-L3-c.md");
}

#[test]
fn summary_rel_path_topic() {
    let p = summary_rel_path(
        SummaryTreeKind::Topic,
        "person-alex-johnson",
        1,
        "summary:L1:xyz",
    );
    assert_eq!(
        p,
        "wiki/summaries/topic-person-alex-johnson/L1/summary-L1-xyz.md"
    );
}

#[test]
fn summary_rel_path_strips_trailing_md_extension() {
    let p = summary_rel_path(
        SummaryTreeKind::Topic,
        "entity-slug",
        2,
        "summary:L2:foo.md",
    );
    assert_eq!(p, "wiki/summaries/topic-entity-slug/L2/summary-L2-foo.md");
}

#[test]
fn summary_filename_preserves_legacy_level_first_shape() {
    assert_eq!(
        summary_filename("summary:L3:legacy-uuid"),
        "summary-L3-legacy-uuid"
    );
}

#[test]
fn summary_filename_rejects_canonical_shape_with_path_separators() {
    let basename = summary_filename("summary:1700000000000:L2-a/b");
    assert!(!basename.contains('/'), "got {basename}");
    assert_eq!(basename, "summary-1700000000000-L2-a-b");
}

#[test]
fn summary_filename_rejects_canonical_shape_with_non_numeric_level() {
    let basename = summary_filename("summary:1700000000000:Lxyz-tail");
    assert_eq!(basename, "summary-1700000000000-Lxyz-tail");
}

#[test]
fn summary_filename_legacy_branch_rejects_path_separator_in_level() {
    let basename = summary_filename("summary:L1/2:abc");
    assert!(!basename.contains('/'), "got {basename}");
    assert_eq!(basename, "summary-L1-2-abc");
}

#[test]
fn summary_filename_legacy_branch_rejects_traversal_in_level() {
    let basename = summary_filename("summary:L../../x:tail");
    assert!(!basename.contains('/'), "got {basename}");
}

#[test]
fn summary_filename_falls_back_for_unknown_shapes() {
    assert_eq!(
        summary_filename("summary:experimental:value:tail"),
        "summary-experimental-value-tail"
    );
}

#[test]
fn summary_abs_path_rooted_under_content_root() {
    use std::path::Path;
    let root = Path::new("/workspace/content");
    let abs = summary_abs_path(root, SummaryTreeKind::Global, "global", 0, "daily-123");
    assert!(abs.starts_with(root));
    assert!(abs.ends_with("daily-123.md"));
    assert!(abs.to_string_lossy().contains("summaries/global/L0/"));
}

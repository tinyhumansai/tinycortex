//! Unit tests for the chunker (`super`).

use super::super::types::conservative_token_estimate;
use super::*;
use chrono::Utc;

fn meta() -> Metadata {
    Metadata::point_in_time(SourceKind::Chat, "slack:#eng", "alice", Utc::now())
}

fn meta_email() -> Metadata {
    Metadata::point_in_time(SourceKind::Email, "gmail:t1", "alice", Utc::now())
}

fn meta_doc() -> Metadata {
    Metadata::point_in_time(SourceKind::Document, "doc1", "alice", Utc::now())
}

#[test]
fn tiny_input_produces_single_chunk() {
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "slack:#eng".into(),
        markdown: "## 2026-01-01T00:00:00Z — alice\nhello world".into(),
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].content.contains("hello world"));
    assert_eq!(chunks[0].seq_in_source, 0);
    assert!(!chunks[0].partial_message);
}

#[test]
fn empty_chat_input_produces_one_empty_chunk() {
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "x".into(),
        markdown: "".into(),
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "");
    assert!(!chunks[0].partial_message);
}

#[test]
fn chat_messages_keep_one_chunk_per_message_when_small() {
    let md = "## 2026-01-01T00:00:00Z — alice\nHello world\n\n## 2026-01-01T00:01:00Z — bob\nParagraph one.\n\nParagraph two.".to_string();
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "slack:#eng".into(),
        markdown: md.clone(),
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(
        chunks.len(),
        2,
        "each message must keep a stable replay boundary; got {chunks:?}"
    );
    assert!(chunks[0].content.contains("alice"));
    assert!(chunks[1].content.contains("bob"));
    assert!(chunks[1].content.contains("Paragraph one."));
    assert!(chunks[1].content.contains("Paragraph two."));
    assert!(chunks.iter().all(|chunk| !chunk.partial_message));
}

#[test]
fn chat_messages_split_at_boundary_when_large() {
    let msg_body = "x".repeat(12_000);
    let md = format!(
        "## 2026-01-01T00:00:00Z — alice\n{msg_body}\n\n## 2026-01-01T00:01:00Z — bob\n{msg_body}"
    );
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "slack:#eng".into(),
        markdown: md,
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 5_000 });
    assert_eq!(
        chunks.len(),
        2,
        "two large messages should land in separate chunks; got {chunks:?}"
    );
    assert!(chunks[0].content.contains("alice"));
    assert!(chunks[1].content.contains("bob"));
    for c in &chunks {
        assert!(!c.partial_message, "whole messages must not be partial");
    }
}

#[test]
fn email_threads_keep_one_chunk_per_message_when_small() {
    let md = "---\nFrom: alice@example.com\nSubject: Hello\nDate: 2026-01-01T00:00:00Z\n\nFirst body.\n---\nFrom: bob@example.com\nSubject: Re: Hello\nDate: 2026-01-01T00:01:00Z\n\nSecond body.\n---\nFrom: carol@example.com\nSubject: Re: Hello\nDate: 2026-01-01T00:02:00Z\n\nThird body.".to_string();
    let input = ChunkerInput {
        source_kind: SourceKind::Email,
        source_id: "gmail:t1".into(),
        markdown: md,
        metadata: meta_email(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(
        chunks.len(),
        3,
        "each email must keep a stable replay boundary; got {chunks:?}"
    );
    assert!(chunks[0].content.contains("First body."));
    assert!(chunks[1].content.contains("Second body."));
    assert!(chunks[2].content.contains("Third body."));
    assert!(chunks.iter().all(|chunk| !chunk.partial_message));
}

#[test]
fn email_thread_large_splits_at_email_boundaries() {
    let email_body = "y".repeat(16_000); // ~4k tokens
    let md = format!(
        "---\nFrom: a@x.com\nDate: 2026-01-01T00:00:00Z\n\n{email_body}\n\
         ---\nFrom: b@x.com\nDate: 2026-01-01T00:01:00Z\n\n{email_body}\n\
         ---\nFrom: c@x.com\nDate: 2026-01-01T00:02:00Z\n\n{email_body}"
    );
    let input = ChunkerInput {
        source_kind: SourceKind::Email,
        source_id: "gmail:t1".into(),
        markdown: md,
        metadata: meta_email(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 5_000 });
    assert!(
        chunks.len() >= 2,
        "large thread must split into multiple chunks; got {}",
        chunks.len()
    );
    for c in &chunks {
        assert!(!c.partial_message, "whole-email chunks must not be partial");
    }
}

#[test]
fn oversize_single_email_splits_with_partial_flag() {
    let big_body = "z".repeat(50_000); // ~12.5k tokens at 4 chars/token
    let md = format!("---\nFrom: a@x.com\nDate: 2026-01-01T00:00:00Z\n\n{big_body}");
    let input = ChunkerInput {
        source_kind: SourceKind::Email,
        source_id: "gmail:t1".into(),
        markdown: md,
        metadata: meta_email(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 1_000 });
    assert!(chunks.len() > 1, "oversize email must split");
    for c in &chunks {
        assert!(
            c.partial_message,
            "all sub-pieces of an oversize email must have partial_message=true"
        );
    }
}

#[test]
fn chat_units_keep_independent_chunks() {
    let md =
        "## 2026-01-01T00:00:00Z — alice\nfoo\n\n## 2026-01-01T00:01:00Z — bob\nbar".to_string();
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "x".into(),
        markdown: md,
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].content.contains("alice"));
    assert!(chunks[1].content.contains("bob"));
}

#[test]
fn overlapping_chat_batches_reuse_message_chunk_ids() {
    let first = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "slack:eng".into(),
        markdown: "## 2026-01-01T00:00:00Z — alice\none\n\n## 2026-01-01T00:01:00Z — bob\ntwo"
            .into(),
        metadata: meta(),
    };
    let second = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "slack:eng".into(),
        markdown: "## 2026-01-01T00:01:00Z — bob\ntwo\n\n## 2026-01-01T00:02:00Z — carol\nthree"
            .into(),
        metadata: meta(),
    };
    let first_chunks = chunk_markdown(&first, &ChunkerOptions::default());
    let second_chunks = chunk_markdown(&second, &ChunkerOptions::default());

    assert_eq!(first_chunks[1].content, second_chunks[0].content);
    assert_eq!(first_chunks[1].id, second_chunks[0].id);
}

#[test]
fn oversize_message_falls_back_with_partial_flag() {
    let long_body = "x".repeat(8000); // ~2000 tokens at 4 chars/token
    let md = format!("## 2026-01-01T00:00:00Z — alice\n{long_body}");
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "x".into(),
        markdown: md,
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 100 });
    assert!(chunks.len() > 1, "oversize message must split");
    for c in &chunks {
        assert!(
            c.partial_message,
            "all sub-pieces of an oversize message must have partial_message=true"
        );
    }
    let rejoined: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(rejoined.contains(&long_body[..100]));
}

#[test]
fn document_falls_through_to_paragraph_split() {
    let para1 = "a".repeat(400); // ~100 tokens
    let para2 = "b".repeat(400);
    let para3 = "c".repeat(400);
    let text = format!("{para1}\n\n{para2}\n\n{para3}");
    let input = ChunkerInput {
        source_kind: SourceKind::Document,
        source_id: "doc1".into(),
        markdown: text,
        metadata: meta_doc(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 150 });
    assert!(chunks.len() >= 2);
    for c in &chunks {
        let first = c.content.chars().next().unwrap();
        assert!(
            matches!(first, 'a' | 'b' | 'c'),
            "document chunk starts with unexpected char: {:?}",
            c.content.chars().take(10).collect::<String>()
        );
        assert!(
            !c.partial_message,
            "document chunks must not have partial_message=true"
        );
    }
}

#[test]
fn header_line_dropped_in_chat() {
    let md =
        "# Chat transcript — slack / #eng\n\n## 2026-01-01T00:00:00Z — alice\nhello".to_string();
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "x".into(),
        markdown: md,
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(chunks.len(), 1);
    assert!(
        !chunks[0].content.contains("# Chat transcript"),
        "leading `# ` header must be dropped from chunk content"
    );
    assert!(chunks[0].content.contains("hello"));
}

#[test]
fn chunk_ids_are_stable_across_runs() {
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "slack:#eng".into(),
        markdown: "## 2026-01-01T00:00:00Z — alice\nhello".into(),
        metadata: meta(),
    };
    let a = chunk_markdown(&input, &ChunkerOptions::default());
    let b = chunk_markdown(&input, &ChunkerOptions::default());
    assert_eq!(
        a.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        b.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn sequence_numbers_start_at_zero() {
    let msgs: String = (0..5)
        .map(|i| format!("## 2026-01-01T00:0{}:00Z — user{i}\nContent {i}\n\n", i))
        .collect();
    let input = ChunkerInput {
        source_kind: SourceKind::Chat,
        source_id: "x".into(),
        markdown: msgs,
        metadata: meta(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    for (idx, c) in chunks.iter().enumerate() {
        assert_eq!(c.seq_in_source, idx as u32);
    }
}

#[test]
fn paragraph_boundaries_preferred_for_documents() {
    let para1 = "a".repeat(400); // ~100 tokens
    let para2 = "b".repeat(400);
    let para3 = "c".repeat(400);
    let text = format!("{para1}\n\n{para2}\n\n{para3}");
    let input = ChunkerInput {
        source_kind: SourceKind::Document,
        source_id: "doc1".into(),
        markdown: text,
        metadata: meta_doc(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 150 });
    assert!(chunks.len() >= 2);
    for c in &chunks {
        let first = c.content.chars().next().unwrap();
        assert!(
            matches!(first, 'a' | 'b' | 'c'),
            "chunk starts with unexpected char: {:?}",
            c.content.chars().take(10).collect::<String>()
        );
    }
}

#[test]
fn falls_back_to_line_split_when_no_paragraphs_document() {
    let text = (0..30)
        .map(|i| format!("line-{i}-{}", "x".repeat(40)))
        .collect::<Vec<_>>()
        .join("\n");
    let input = ChunkerInput {
        source_kind: SourceKind::Document,
        source_id: "x".into(),
        markdown: text,
        metadata: meta_doc(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 80 });
    assert!(chunks.len() >= 2);
    for c in &chunks {
        assert!(!c.content.contains("\n\n")); // no paragraph joins in output
    }
}

#[test]
fn utf8_boundaries_preserved_on_hard_split_document() {
    let text = "中".repeat(400);
    let input = ChunkerInput {
        source_kind: SourceKind::Document,
        source_id: "d".into(),
        markdown: text.clone(),
        metadata: meta_doc(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 50 });
    let rejoined: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(rejoined, text);
}

#[test]
fn zero_token_budget_is_clamped_without_empty_leading_chunk_document() {
    let input = ChunkerInput {
        source_kind: SourceKind::Document,
        source_id: "d".into(),
        markdown: "abcdef".into(),
        metadata: meta_doc(),
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions { max_tokens: 0 });
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
    let rejoined: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(rejoined, "abcdef");
}

// ── Embed-safety: conservative-token splitting + overlap ──

fn assert_all_within_budget(pieces: &[String], budget: u32) {
    for (i, p) in pieces.iter().enumerate() {
        assert!(
            conservative_token_estimate(p) <= budget,
            "piece {i} is {} est-tokens, over budget {budget}",
            conservative_token_estimate(p),
        );
    }
}

#[test]
fn dense_hash_content_splits_under_budget() {
    let dense =
        "claude-memory:openhuman:MEMORY.md:67d6fe2727d431b16d41630babfdcf1cdf61bda7b9ba99656\n"
            .repeat(200);
    let pieces = split_by_token_budget(&dense, 3000);
    assert!(
        pieces.len() > 1,
        "dense content must split, got {}",
        pieces.len()
    );
    assert_all_within_budget(&pieces, 3000);
}

#[test]
fn hebrew_content_splits_under_budget() {
    let he = "איריס דיברה עם העורך דין ועם עידו על הדירה החדשה ".repeat(300);
    let pieces = split_by_token_budget(&he, 1000);
    assert!(pieces.len() > 1, "hebrew content must split");
    assert_all_within_budget(&pieces, 1000);
}

#[test]
fn mixed_markdown_code_splits_under_budget() {
    let md = "## Section\nSome prose here. And more prose follows.\n\n\
              ```rust\nfn f(x: u32) -> u32 { x * 4 + 3 }\n```\n\n\
              - bullet a1b2c3d4\n- bullet e5f6a7b8\n"
        .repeat(120);
    let pieces = split_by_token_budget(&md, 2000);
    assert!(pieces.len() > 1, "mixed content must split");
    assert_all_within_budget(&pieces, 2000);
}

#[test]
fn oversize_content_splits_into_multiple_ordered_pieces() {
    let text = (0..50)
        .map(|i| format!("Paragraph number {i} with several words of filler content here."))
        .collect::<Vec<_>>()
        .join("\n\n");
    let pieces = split_by_token_budget(&text, 80);
    assert!(
        pieces.len() > 2,
        "expected several pieces, got {}",
        pieces.len()
    );
    assert_all_within_budget(&pieces, 80);
    let joined = pieces.join("\n");
    let p0 = joined.find("number 0").expect("first paragraph present");
    let p49 = joined.find("number 49").expect("last paragraph present");
    assert!(p0 < p49, "ordering not preserved");
}

#[test]
fn adjacent_chunks_overlap_without_duplicating() {
    let text = (0..40)
        .map(|i| format!("Sentence {i} carries a unique token marker{i} inside it."))
        .collect::<Vec<_>>()
        .join(" ");
    let pieces = split_by_token_budget(&text, 200);
    assert!(
        pieces.len() >= 3,
        "expected several pieces, got {}",
        pieces.len()
    );
    assert_all_within_budget(&pieces, 200);
    let overlap = pieces.windows(2).any(|w| {
        (0..40).any(|i| {
            let m = format!("marker{i} ");
            w[0].contains(&m) && w[1].contains(&m)
        })
    });
    assert!(overlap, "expected ~12% overlap between adjacent chunks");
    for w in pieces.windows(2) {
        assert_ne!(w[0], w[1], "adjacent chunks must not be identical");
    }
}

#[test]
fn normal_small_content_is_single_chunk_unchanged() {
    let text = "# Title\nA short paragraph that easily fits.\n\nAnother short one.";
    let pieces = split_by_token_budget(text, 3000);
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0], text);
}

#[test]
fn large_consecutive_segments_stay_within_budget() {
    let budget = 100;
    let text = format!(
        "{}\n\n{}\n\n{}",
        "a".repeat(40),
        "b".repeat(110),
        "c".repeat(110)
    );
    let pieces = split_by_token_budget(&text, budget);
    assert!(
        pieces.len() >= 2,
        "expected multiple chunks, got {}",
        pieces.len()
    );
    assert_all_within_budget(&pieces, budget);
    for w in pieces.windows(2) {
        assert_ne!(w[0], w[1], "adjacent chunks must not be identical");
    }
}

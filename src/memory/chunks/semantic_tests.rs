//! Unit tests for the semantic markdown chunker (`super`).

use super::*;

#[test]
fn empty_text() {
    assert!(chunk_markdown("", 512).is_empty());
    assert!(chunk_markdown("   ", 512).is_empty());
}

#[test]
fn single_short_paragraph() {
    let chunks = chunk_markdown("Hello world", 512);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "Hello world");
    assert!(chunks[0].heading.is_none());
}

#[test]
fn heading_sections() {
    let text = "# Title\nSome intro.\n\n## Section A\nContent A.\n\n## Section B\nContent B.";
    let chunks = chunk_markdown(text, 512);
    assert!(chunks.len() >= 3);
    assert!(chunks[0].heading.is_none() || chunks[0].heading.as_deref() == Some("# Title"));
}

#[test]
fn respects_max_tokens() {
    let long_text: String = (0..200).fold(String::new(), |mut s, i| {
        use std::fmt::Write;
        let _ = writeln!(
            s,
            "This is sentence number {i} with some extra words to fill it up."
        );
        s
    });
    let chunks = chunk_markdown(&long_text, 50); // 50 tokens ≈ 200 chars
    assert!(
        chunks.len() > 1,
        "Expected multiple chunks, got {}",
        chunks.len()
    );
    for chunk in &chunks {
        assert!(
            chunk.content.len() <= 300,
            "Chunk too long: {} chars",
            chunk.content.len()
        );
    }
}

#[test]
fn preserves_heading_in_split_sections() {
    let mut text = String::from("## Big Section\n");
    for i in 0..100 {
        use std::fmt::Write;
        let _ = write!(text, "Line {i} with some content here.\n\n");
    }
    let chunks = chunk_markdown(&text, 50);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        if chunk.heading.is_some() {
            assert_eq!(chunk.heading.as_deref(), Some("## Big Section"));
        }
    }
}

#[test]
fn indexes_are_sequential() {
    let text = "# A\nContent A\n\n# B\nContent B\n\n# C\nContent C";
    let chunks = chunk_markdown(text, 512);
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, i);
    }
}

#[test]
fn chunk_count_reasonable() {
    let text = "Hello world. This is a test document.";
    let chunks = chunk_markdown(text, 512);
    assert_eq!(chunks.len(), 1);
}

#[test]
fn headings_only_no_body() {
    let text = "# Title\n## Section A\n## Section B\n### Subsection";
    let chunks = chunk_markdown(text, 512);
    assert!(!chunks.is_empty());
}

#[test]
fn deep_atx_headings_split_through_h6() {
    let text = "# Top\nIntro\n#### Deep heading\nDeep content";
    let chunks = chunk_markdown(text, 512);
    assert!(
        chunks.len() >= 2,
        "expected the #### heading to start a new section, got {} chunk(s)",
        chunks.len(),
    );
    let deep = chunks
        .iter()
        .find(|c| c.heading.as_deref() == Some("#### Deep heading"));
    assert!(
        deep.is_some(),
        "expected a chunk with heading '#### Deep heading'; chunks: {chunks:?}"
    );
}

#[test]
fn all_atx_heading_levels_h1_through_h6_split() {
    let text = "# H1\na\n\n## H2\nb\n\n### H3\nc\n\n#### H4\nd\n\n##### H5\ne\n\n###### H6\nf";
    let chunks = chunk_markdown(text, 512);
    let headings: Vec<_> = chunks.iter().filter_map(|c| c.heading.as_deref()).collect();
    assert_eq!(
        headings,
        vec![
            "# H1",
            "## H2",
            "### H3",
            "#### H4",
            "##### H5",
            "###### H6"
        ],
        "each ATX heading depth h1-h6 must split into its own section",
    );
}

#[test]
fn seven_or_more_hashes_are_not_a_heading() {
    let text = "# Top\nIntro\n####### Not a heading\nMore content";
    let chunks = chunk_markdown(text, 512);
    assert_eq!(
        chunks.len(),
        1,
        "7-hash line should not split; got {}",
        chunks.len()
    );
    assert_eq!(chunks[0].heading.as_deref(), Some("# Top"));
    assert!(chunks[0].content.contains("####### Not a heading"));
}

#[test]
fn atx_heading_requires_trailing_space() {
    let text = "# Real heading\nIntro\n###NoSpace\nbody";
    let chunks = chunk_markdown(text, 512);
    assert_eq!(
        chunks.len(),
        1,
        "missing trailing space disqualifies the heading"
    );
    assert_eq!(chunks[0].heading.as_deref(), Some("# Real heading"));
}

#[test]
fn very_long_single_line_no_newlines() {
    let text = "word ".repeat(5000); // 25000 chars
    let max_tokens = 50; // 200 chars
    let max_chars = max_tokens * 4;
    let chunks = chunk_markdown(&text, max_tokens);
    assert!(
        chunks.len() > 1,
        "Expected multiple chunks for a 25KB single-line input, got {}",
        chunks.len()
    );
    for chunk in &chunks {
        assert!(
            chunk.content.len() <= max_chars + 50,
            "Chunk exceeds max_chars: {} chars (limit {})",
            chunk.content.len(),
            max_chars
        );
    }
}

#[test]
fn oversize_line_splits_on_word_boundary() {
    let line = "abcde ".repeat(100);
    let text = format!("# Heading\n{line}");
    let chunks = chunk_markdown(&text, 25); // 25 tokens = 100 chars max
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        for word in chunk.content.split_whitespace() {
            if word.starts_with('#') {
                continue; // heading
            }
            assert!(
                word == "abcde" || word == "Heading",
                "Unexpected split word: '{word}'"
            );
        }
    }
}

#[test]
fn oversize_line_no_spaces_hard_splits() {
    let text = "x".repeat(1000);
    let chunks = chunk_markdown(&text, 25); // 100 chars max
    assert!(
        chunks.len() > 1,
        "Should hard-split when no spaces exist, got {} chunk(s)",
        chunks.len()
    );
    let reassembled: String = chunks.iter().map(|c| c.content.trim()).collect();
    assert_eq!(reassembled.len(), 1000);
}

#[test]
fn only_newlines_and_whitespace() {
    assert!(chunk_markdown("\n\n\n   \n\n", 512).is_empty());
}

#[test]
fn max_tokens_zero() {
    let chunks = chunk_markdown("Hello world", 0);
    assert!(!chunks.is_empty());
}

#[test]
fn max_tokens_one() {
    let text = "Line one\nLine two\nLine three";
    let chunks = chunk_markdown(text, 1);
    assert!(!chunks.is_empty());
}

#[test]
fn unicode_content() {
    let text = "# 日本語\nこんにちは世界\n\n## Émojis\n🦀 Rust is great 🚀";
    let chunks = chunk_markdown(text, 512);
    assert!(!chunks.is_empty());
    let all: String = chunks.iter().map(|c| c.content.clone()).collect();
    assert!(all.contains("こんにちは"));
    assert!(all.contains("🦀"));
}

#[test]
fn fts5_special_chars_in_content() {
    let text = "Content with \"quotes\" and (parentheses) and * asterisks *";
    let chunks = chunk_markdown(text, 512);
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].content.contains("\"quotes\""));
}

#[test]
fn multiple_blank_lines_between_paragraphs() {
    let text = "Paragraph one.\n\n\n\n\nParagraph two.\n\n\n\nParagraph three.";
    let chunks = chunk_markdown(text, 512);
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].content.contains("Paragraph one"));
    assert!(chunks[0].content.contains("Paragraph three"));
}

#[test]
fn heading_at_end_of_text() {
    let text = "Some content\n# Trailing Heading";
    let chunks = chunk_markdown(text, 512);
    assert!(!chunks.is_empty());
}

#[test]
fn single_heading_no_content() {
    let text = "# Just a heading";
    let chunks = chunk_markdown(text, 512);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].heading.as_deref(), Some("# Just a heading"));
}

#[test]
fn no_content_loss() {
    let text = "# A\nContent A line 1\nContent A line 2\n\n## B\nContent B\n\n## C\nContent C";
    let chunks = chunk_markdown(text, 512);
    let reassembled: String = chunks.iter().fold(String::new(), |mut s, c| {
        use std::fmt::Write;
        let _ = writeln!(s, "{}", c.content);
        s
    });
    for word in ["Content", "line", "1", "2"] {
        assert!(
            reassembled.contains(word),
            "Missing word '{word}' in reassembled chunks"
        );
    }
}

//! Shared line-streaming JSONL helper for the transcript readers.
//!
//! Reads a `.jsonl` file one line at a time (never materialising the whole
//! file), yielding `(byte_len, Value)` for each line that parses as JSON.
//! Lines that fail to parse — truncated writes, non-JSON scaffolding — are
//! skipped, and their bytes are still reported so byte-reduction accounting
//! stays honest. Unknown event shapes are the caller's concern.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};

/// Stream `path` line-by-line, invoking `on_line(byte_len, value)` for every
/// line that parses as JSON. `byte_len` counts the raw line bytes (including the
/// newline) so the caller can total the file's read size incrementally.
///
/// Returns the total number of raw bytes walked (including unparseable lines).
pub fn for_each_json_line<F>(path: &Path, mut on_line: F) -> Result<u64>
where
    F: FnMut(u64, &serde_json::Value),
{
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut total: u64 = 0;
    for line in reader.lines() {
        // A single unreadable line (invalid UTF-8) shouldn't abort the file.
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let byte_len = line.len() as u64 + 1; // + newline
        total += byte_len;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            on_line(byte_len, &value);
        }
    }
    Ok(total)
}

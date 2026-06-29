//! Token-count signal — penalises very short or very long chunks.
//!
//! Rationale: "+1", "lol", "👍" are usually noise; multi-page walls of text
//! are often pasted logs or attachments that overwhelm summarisation.
//! The signal is strongest in a middle band that corresponds to substantive
//! prose/discussion.
//!
//! Output is a score in `[0.0, 1.0]` shaped as a plateau between
//! `TOKEN_MIN` and `TOKEN_MAX` with linear ramps on both sides.

/// Below this token count the chunk scores 0 (treated as noise).
pub const TOKEN_MIN: u32 = 10;
/// Top of the linear ramp from 0 → 1 starting at [`TOKEN_MIN`].
pub const TOKEN_RAMP_LOW: u32 = 30;
/// Start of the linear ramp from 1 → 0.5 ending at [`TOKEN_MAX`].
pub const TOKEN_RAMP_HIGH: u32 = 3_000;
/// Above this token count the score is clamped to 0.5 (oversized content
/// still carries information but loses the plateau bonus).
pub const TOKEN_MAX: u32 = 8_000;

/// Score for a chunk's token count. See module docs for shape.
pub fn score(token_count: u32) -> f32 {
    if token_count < TOKEN_MIN {
        return 0.0;
    }
    if token_count <= TOKEN_RAMP_LOW {
        // linear 0..1 over [MIN, RAMP_LOW]
        let span = (TOKEN_RAMP_LOW - TOKEN_MIN) as f32;
        return (token_count - TOKEN_MIN) as f32 / span;
    }
    if token_count <= TOKEN_RAMP_HIGH {
        return 1.0;
    }
    if token_count <= TOKEN_MAX {
        // linear 1.0..0.5 over [RAMP_HIGH, MAX]
        let span = (TOKEN_MAX - TOKEN_RAMP_HIGH) as f32;
        let t = (token_count - TOKEN_RAMP_HIGH) as f32 / span;
        return 1.0 - 0.5 * t;
    }
    0.5
}

#[cfg(test)]
#[path = "token_count_tests.rs"]
mod tests;

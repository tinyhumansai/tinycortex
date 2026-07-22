//! Byte-oriented candidate pre-filter for the PII redactor.
//!
//! Replaces the always-resident combined `RegexSet` (one shared NFA plus a
//! per-thread lazy-DFA cache in *every* process/thread) with a single cheap pass
//! over the raw bytes. The scan derives per-class candidate flags from a handful
//! of structural signals — digit-run lengths, punctuation presence, uppercase /
//! alpha presence, `+`, and case-insensitive keyword probes (including the
//! non-Latin Aadhaar `आधार` and My-Number `マイナンバー` / `個人番号` keywords).
//! Each flag then decides whether that class's precise validation regex is worth
//! compiling and running; the precise `Regex`es stay `LazyLock`, so a class that
//! never sees a candidate is never compiled at all. At 100–1000 concurrent
//! agents that turns "combined NFA + N thread-local DFA caches resident forever"
//! into "only the regexes a workload actually needs, compiled on first hit".
//!
//! Correctness: every flag is a NECESSARY CONDITION of the class's *precise*
//! regex, so a flag can only over-fire (harmless — the precise regex then simply
//! fails to match), never under-fire on real PII. Consequently, whenever a
//! precise pattern would have matched under the old code path, its flag is set
//! and it still runs — output is unchanged. The union of the flags is a superset
//! of the old `SCREEN` set (pinned by `prefilter_is_superset_of_legacy_screen`).
//! The NANP phone class gates on the *screen*-entry necessary condition — an
//! internal `digit sep digit` separator OR a `\d{11,}` run (the old SCREEN reached
//! `PHONE_NANP_RE` through both) — faithfully preserving the documented "a bare
//! 10-digit NANP run is never reached" behavior while still redacting a bare
//! `1`+10-digit country-code number — see
//! `redact_pii_does_not_reach_bare_10_digit_nanp_today`.

/// Per-class candidate flags produced by [`scan_candidates`]. A set flag means
/// "run this class's precise regex"; an unset flag means the class cannot
/// possibly match, so its regex is skipped (and never compiled).
#[derive(Default, Clone, Copy)]
pub(super) struct Candidates {
    pub(super) cpf_fmt: bool,
    pub(super) cnpj_fmt: bool,
    pub(super) cuit: bool,
    pub(super) iban: bool,
    pub(super) cc: bool,
    pub(super) cnpj_bare: bool,
    pub(super) cpf_bare: bool,
    pub(super) aadhaar_fmt: bool,
    pub(super) aadhaar_kw: bool,
    pub(super) dni: bool,
    pub(super) nie: bool,
    pub(super) nino: bool,
    pub(super) ssn: bool,
    pub(super) rrn: bool,
    pub(super) rfc: bool,
    pub(super) pan_in: bool,
    pub(super) phone_e164: bool,
    pub(super) phone_nanp: bool,
    pub(super) mynumber: bool,
}

impl Candidates {
    /// True if any class is a candidate — i.e. the text is worth a precise pass.
    pub(super) fn any(&self) -> bool {
        self.cpf_fmt
            || self.cnpj_fmt
            || self.cuit
            || self.iban
            || self.cc
            || self.cnpj_bare
            || self.cpf_bare
            || self.aadhaar_fmt
            || self.aadhaar_kw
            || self.dni
            || self.nie
            || self.nino
            || self.ssn
            || self.rrn
            || self.rfc
            || self.pan_in
            || self.phone_e164
            || self.phone_nanp
            || self.mynumber
    }
}

/// Case-insensitive (ASCII-only case folding) substring test over raw bytes.
/// Non-ASCII bytes compare exactly, so this also serves as an exact matcher for
/// the multibyte Devanagari / Japanese keyword needles.
fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len())
        .any(|w| w.iter().zip(needle).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

/// Aadhaar keyword needles — ASCII forms plus Devanagari `आधार`.
const AADHAAR_KEYWORDS: &[&[u8]] = &[b"aadhaar", b"aadhar", b"uidai", b"uid", "आधार".as_bytes()];
/// My-Number Japanese keyword needles. The English `My\s?Number` variant is
/// handled separately (see `scan_candidates`) so any `\s` separator between the
/// two words is recognised, not just a literal space.
const MYNUMBER_JP_KEYWORDS: &[&[u8]] = &["マイナンバー".as_bytes(), "個人番号".as_bytes()];

/// Single linear pass over the bytes deriving every per-class candidate flag.
///
/// Only ASCII structural bytes carry signal here; multibyte UTF-8 lead /
/// continuation bytes are all `>= 0x80`, so scanning `as_bytes()` for ASCII
/// digits/punctuation/letters is boundary-safe. Keyword probes run over the
/// same byte slice so the non-Latin needles match verbatim.
pub(super) fn scan_candidates(text: &str) -> Candidates {
    let bytes = text.as_bytes();

    let mut total_digits: usize = 0;
    let mut max_digit_run: usize = 0;
    let mut cur_run: usize = 0;
    let mut has_dot = false;
    let mut has_dash = false;
    let mut has_slash = false;
    // Any ASCII whitespace separator (space, tab, newline, CR, form feed,
    // vertical tab). The precise Aadhaar pattern separates its groups with
    // `[\s-]`, which matches the whole `\s` class — so gating on space/tab
    // alone would under-fire on newline-separated Aadhaar (a real PII drop).
    let mut has_ws = false;
    let mut has_upper = false;
    let mut has_alpha = false;
    let mut has_xyz = false;
    let mut has_plus = false;
    // NANP-style "separated group" signal: some `[digit or ')'] [sep] [digit]`
    // window exists (sep ∈ space/tab/./-). This is the necessary condition of
    // the old SCREEN NANP entry, which required internal separators — keeping
    // bare separator-less 10-digit runs out of the phone path.
    let mut nanp_sep = false;

    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_digit() {
            total_digits += 1;
            cur_run += 1;
            if cur_run > max_digit_run {
                max_digit_run = cur_run;
            }
        } else {
            cur_run = 0;
            match b {
                b'.' => has_dot = true,
                b'-' => has_dash = true,
                b'/' => has_slash = true,
                b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => has_ws = true,
                b'+' => has_plus = true,
                b'A'..=b'Z' => {
                    has_upper = true;
                    has_alpha = true;
                    if matches!(b, b'X' | b'Y' | b'Z') {
                        has_xyz = true;
                    }
                }
                b'a'..=b'z' => {
                    has_alpha = true;
                    if matches!(b, b'x' | b'y' | b'z') {
                        has_xyz = true;
                    }
                }
                _ => {}
            }
        }

        if matches!(b, b' ' | b'\t' | b'.' | b'-') && i > 0 && i + 1 < bytes.len() {
            let prev = bytes[i - 1];
            let next = bytes[i + 1];
            if (prev.is_ascii_digit() || prev == b')') && next.is_ascii_digit() {
                nanp_sep = true;
            }
        }
    }

    let has_digit = total_digits > 0;
    let aadhaar_kw = AADHAAR_KEYWORDS.iter().any(|kw| contains_ci(bytes, kw));
    // English `My\s?Number` accepts any single `\s` between the words, so a tab-
    // or newline-separated keyword (`My\tNumber`) must still flag. Requiring both
    // `my` and `number` substrings is a necessary condition of the precise regex
    // and covers every whitespace variant; it may over-fire (harmless — the
    // precise `MYNUM_RE` re-checks the separator and the trailing 12 digits).
    let mynumber = MYNUMBER_JP_KEYWORDS.iter().any(|kw| contains_ci(bytes, kw))
        || (contains_ci(bytes, b"my") && contains_ci(bytes, b"number"));

    let cand = Candidates {
        // Formatted CPF `\d{3}\.\d{3}\.\d{3}-\d{2}` — needs digits, `.`, `-`.
        cpf_fmt: has_digit && has_dot && has_dash,
        // Formatted CNPJ `\d{2}\.\d{3}\.\d{3}/\d{4}-\d{2}` — adds `/`.
        cnpj_fmt: has_digit && has_dot && has_slash && has_dash,
        // CUIT `\d{2}-\d{8}-\d` — needs digits and `-`.
        cuit: has_digit && has_dash,
        // IBAN `[A-Z]{2}\d{2}…` — case-sensitive uppercase letters and digits.
        iban: has_upper && has_digit,
        // Credit card `(?:\d[\s\-]?){13,19}` — at least 13 digits total.
        cc: total_digits >= 13,
        // Bare CNPJ `\d{14}` — a 14-long digit run.
        cnpj_bare: max_digit_run >= 14,
        // Bare CPF `\d{11}` — an 11-long digit run.
        cpf_bare: max_digit_run >= 11,
        // Formatted Aadhaar `\d{4}[\s-]\d{4}[\s-]\d{4}` — 12 digits + a `\s`/dash
        // separator (any ASCII whitespace, matching the precise `[\s-]` class).
        aadhaar_fmt: total_digits >= 12 && (has_ws || has_dash),
        // Keyword-gated Aadhaar — keyword suffices (precise regex checks digits).
        aadhaar_kw,
        // Spain DNI `\d{8}[A-Z]` — 8-run plus a letter.
        dni: max_digit_run >= 8 && has_alpha,
        // Spain NIE `[XYZ]\d{7}[A-Z]` — X/Y/Z, 7-run, letter.
        nie: has_xyz && max_digit_run >= 7 && has_alpha,
        // UK NINO `[A-Z]{2}\d{6}[A-D]` — letters and a 6-run.
        nino: max_digit_run >= 6 && has_alpha,
        // US SSN `\d{3}-\d{2}-\d{4}` — digits and `-`.
        ssn: has_digit && has_dash,
        // Korea RRN `\d{6}-[1-4]\d{6}` — a 6-run and `-`.
        rrn: max_digit_run >= 6 && has_dash,
        // Mexico RFC `[A-ZÑ&]{3,4}\d{6}[A-Z0-9]{3}` — a 6-run (leading class may
        // be all non-ASCII `Ñ`, so gate on the digit run alone, not on letters).
        rfc: max_digit_run >= 6,
        // India PAN `[A-Z]{5}\d{4}[A-Z]` — letters and a 4-run.
        pan_in: max_digit_run >= 4 && has_alpha,
        // E.164 `\+\d{7,15}` — a `+` and a 7+ digit run.
        phone_e164: has_plus && max_digit_run >= 7,
        // NANP — screen-entry necessary condition. The old SCREEN reached
        // `PHONE_NANP_RE` via either the separated-group pattern OR the long
        // `\d{11,}` run (which covers a bare `1`+10-digit country-code number
        // like `12025551234`). A bare 10-digit run still stays out of the phone
        // path (no internal separator, run length 10 < 11).
        phone_nanp: nanp_sep || max_digit_run >= 11,
        // My Number — keyword suffices (precise regex checks the 12 digits).
        mynumber,
    };

    log::trace!(
        "[pii] scan_candidates bytes={} digits={} max_run={} nanp_sep={} any={}",
        bytes.len(),
        total_digits,
        max_digit_run,
        nanp_sep,
        cand.any()
    );

    cand
}

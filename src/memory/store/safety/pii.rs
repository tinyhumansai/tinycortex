//! Multilingual personal-PII redaction (national IDs, financial identifiers,
//! international phone) — on-device, regex + checksum only, zero network.
//!
//! ## Design — security first
//!
//! 1. **Checksum gating where possible.** CPF, CNPJ, CUIT, credit-card (Luhn),
//!    IBAN (mod-97), Aadhaar (Verhoeff), Spanish DNI/NIE (check letter), and
//!    US SSN reserved-range filters all reject look-alikes that aren't real
//!    identifiers. The false-positive rate from format alone is too high; the
//!    checksums bring it back to acceptable.
//!
//! 2. **Bypass-resistant.** Inputs are normalized before
//!    matching, which:
//!      - strips zero-width characters (U+200B/200C/200D/FEFF/2060/180E),
//!      - folds fullwidth digits (`０-９` → `0-9`) and fullwidth `．－／：`
//!        to their ASCII counterparts,
//!      - folds Arabic-Indic and Eastern Arabic-Indic digits to ASCII.
//!    Match offsets are mapped back to the original text so we only redact
//!    the bytes that actually carry PII; surrounding text is untouched.
//!
//! 3. **Overlap-safe.** Patterns are run in priority order; later matches
//!    that overlap an earlier redaction are dropped, so a credit-card span
//!    can't also be partially matched as a phone number.
//!
//! 4. **Out of scope.** Contextual PII (`"call me at the usual number"`),
//!    compound PII (`name + employer + city`), arbitrary names, and freeform
//!    dates-of-birth all require NER/LLM and are NOT addressed here. This
//!    module is honest about its scope.

use regex::Regex;
use std::sync::LazyLock;

use super::{SanitizationReport, Sanitized};

mod checks;
use checks::*;

// ---------- Replacement tokens ----------

const PII_RFC: &str = "[REDACTED_PII_RFC]";
const PII_CPF: &str = "[REDACTED_PII_CPF]";
const PII_CNPJ: &str = "[REDACTED_PII_CNPJ]";
const PII_CUIT: &str = "[REDACTED_PII_CUIT]";
const PII_MYNUM: &str = "[REDACTED_PII_MYNUMBER]";
const PII_PHONE: &str = "[REDACTED_PII_PHONE]";
const PII_SSN: &str = "[REDACTED_PII_SSN]";
const PII_CC: &str = "[REDACTED_PII_CREDIT_CARD]";
const PII_IBAN: &str = "[REDACTED_PII_IBAN]";
const PII_AADHAAR: &str = "[REDACTED_PII_AADHAAR]";
const PII_PAN_IN: &str = "[REDACTED_PII_PAN_IN]";
const PII_NINO: &str = "[REDACTED_PII_NINO]";
const PII_DNI: &str = "[REDACTED_PII_DNI]";
const PII_RRN: &str = "[REDACTED_PII_RRN]";

// ---------- Patterns ----------

// Brazilian CPF, formatted: NNN.NNN.NNN-NN
static CPF_FMT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{3}\.\d{3}\.\d{3}-\d{2}\b").expect("cpf fmt"));
// Brazilian CPF, bare: 11 consecutive digits. Checksum-gated; ~1% raw FP.
static CPF_BARE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{11}\b").expect("cpf bare"));

// Brazilian CNPJ, formatted: NN.NNN.NNN/NNNN-NN
static CNPJ_FMT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{2}\.\d{3}\.\d{3}/\d{4}-\d{2}\b").expect("cnpj fmt"));
// Brazilian CNPJ, bare: 14 consecutive digits.
static CNPJ_BARE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{14}\b").expect("cnpj bare"));

// Argentine CUIT/CUIL: NN-NNNNNNNN-N (formatted only — bare 11-digit with
// single check digit has ~9% FP on random IDs, too noisy without context).
static CUIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{2}-\d{8}-\d\b").expect("cuit"));

// Mexican RFC: 3-4 letters (incl. Ñ &) + 6 digits + 3 alphanumeric homoclave.
static RFC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[A-ZÑ&]{3,4}\d{6}[A-Z0-9]{3}\b").expect("rfc"));

// Japan My Number (12 digits) gated by a Japanese or English keyword within
// ~30 chars. Bare 12-digit runs without keyword are too noisy.
static MYNUM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:マイナンバー|個人番号|My\s?Number)[\s:はがを、.\-]{0,12}(\d{12})\b")
        .expect("my number")
});

// E.164 phone: + followed by 7-15 digits, no separators.
static PHONE_E164_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+\d{7,15}\b").expect("e164"));

// NANP (US/Canada) formatted phone. Area code must start 2-9; first digit of
// central-office code also 2-9 (real NANP rule).
static PHONE_NANP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\+?1[\s.\-]?)?\(?([2-9]\d{2})\)?[\s.\-]?([2-9]\d{2})[\s.\-]?(\d{4})\b")
        .expect("nanp phone")
});

// US SSN: NNN-NN-NNNN. Range filter applied below.
static SSN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").expect("ssn"));

// Credit card: 13-19 digits with optional spaces/dashes every 4. Luhn-gated.
static CC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d[\s\-]?){13,19}\b").expect("credit card"));

// IBAN: 2 letter country code + 2 check digits + 11-30 alphanumeric.
// Allow optional spaces every 4 chars (common human format).
static IBAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{2}\d{2}(?:[\s]?[A-Z0-9]){11,30}\b").expect("iban"));

// India Aadhaar: 4-4-4 digit groups (space or hyphen) OR contiguous 12 digits
// gated by keyword. Verhoeff-checksum-gated when grouped, keyword-gated when
// bare (Verhoeff alone has ~10% raw FP rate on random 12-digit runs).
static AADHAAR_FMT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{4}[\s\-]\d{4}[\s\-]\d{4}\b").expect("aadhaar formatted"));
static AADHAAR_KW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:aadhaar|aadhar|आधार|uidai|uid)[\s:#\-no.]{0,10}(\d{12})\b")
        .expect("aadhaar keyword")
});

// India PAN: 5 letters, 4 digits, 1 letter. Very high signal — no checksum.
static PAN_IN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[A-Z]{5}\d{4}[A-Z]\b").expect("pan-in"));

// UK NINO: 2 letters + 6 digits + suffix A/B/C/D.
static NINO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[A-Z]{2}\d{6}[A-D]\b").expect("nino"));

// Spain DNI: 8 digits + check letter. NIE: starts X/Y/Z, then 7 digits + letter.
static DNI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b\d{8}[A-Z]\b").expect("dni"));
static NIE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[XYZ]\d{7}[A-Z]\b").expect("nie"));

// South Korea RRN: NNNNNN-CXXXXXX where C is gender/century digit (1-4).
static RRN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{6}-[1-4]\d{6}\b").expect("rrn"));
static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").expect("email"));

// ---------- Byte-oriented candidate pre-filter ----------
//
// Replaces the always-resident combined `RegexSet` (one shared NFA plus a
// per-thread lazy-DFA cache in *every* process/thread) with a single cheap pass
// over the raw bytes. The scan derives per-class candidate flags from a handful
// of structural signals — digit-run lengths, punctuation presence, uppercase /
// alpha presence, `+`, and case-insensitive keyword probes (including the
// non-Latin Aadhaar `आधार` and My-Number `マイナンバー` / `個人番号` keywords).
// Each flag then decides whether that class's precise validation regex is worth
// compiling and running; the precise `Regex`es stay `LazyLock`, so a class that
// never sees a candidate is never compiled at all. At 100–1000 concurrent
// agents that turns "combined NFA + N thread-local DFA caches resident forever"
// into "only the regexes a workload actually needs, compiled on first hit".
//
// Correctness: every flag is a NECESSARY CONDITION of the class's *precise*
// regex, so a flag can only over-fire (harmless — the precise regex then simply
// fails to match), never under-fire on real PII. Consequently, whenever a
// precise pattern would have matched under the old code path, its flag is set
// and it still runs — output is unchanged. The union of the flags is a superset
// of the old `SCREEN` set (pinned by `prefilter_is_superset_of_legacy_screen`).
// The two phone classes intentionally gate on the *screen*-entry necessary
// condition (an internal `digit sep digit` separator) rather than the looser
// precise regex, faithfully preserving the documented "a bare 10-digit NANP run
// is never reached" behavior — see `redact_pii_does_not_reach_bare_10_digit_nanp_today`.

/// Per-class candidate flags produced by [`scan_candidates`]. A set flag means
/// "run this class's precise regex"; an unset flag means the class cannot
/// possibly match, so its regex is skipped (and never compiled).
#[derive(Default, Clone, Copy)]
struct Candidates {
    cpf_fmt: bool,
    cnpj_fmt: bool,
    cuit: bool,
    iban: bool,
    cc: bool,
    cnpj_bare: bool,
    cpf_bare: bool,
    aadhaar_fmt: bool,
    aadhaar_kw: bool,
    dni: bool,
    nie: bool,
    nino: bool,
    ssn: bool,
    rrn: bool,
    rfc: bool,
    pan_in: bool,
    phone_e164: bool,
    phone_nanp: bool,
    mynumber: bool,
}

impl Candidates {
    /// True if any class is a candidate — i.e. the text is worth a precise pass.
    fn any(&self) -> bool {
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
/// My-Number keyword needles — ASCII forms plus Japanese literals.
const MYNUMBER_KEYWORDS: &[&[u8]] = &[
    b"mynumber",
    b"my number",
    "マイナンバー".as_bytes(),
    "個人番号".as_bytes(),
];

/// Single linear pass over the bytes deriving every per-class candidate flag.
///
/// Only ASCII structural bytes carry signal here; multibyte UTF-8 lead /
/// continuation bytes are all `>= 0x80`, so scanning `as_bytes()` for ASCII
/// digits/punctuation/letters is boundary-safe. Keyword probes run over the
/// same byte slice so the non-Latin needles match verbatim.
fn scan_candidates(text: &str) -> Candidates {
    let bytes = text.as_bytes();

    let mut total_digits: usize = 0;
    let mut max_digit_run: usize = 0;
    let mut cur_run: usize = 0;
    let mut has_dot = false;
    let mut has_dash = false;
    let mut has_slash = false;
    let mut has_space = false;
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
                b' ' | b'\t' => has_space = true,
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
    let mynumber = MYNUMBER_KEYWORDS.iter().any(|kw| contains_ci(bytes, kw));

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
        // Formatted Aadhaar `\d{4}[\s-]\d{4}[\s-]\d{4}` — 12 digits + space/dash.
        aadhaar_fmt: total_digits >= 12 && (has_space || has_dash),
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
        // NANP — screen-entry necessary condition (internal separator).
        phone_nanp: nanp_sep,
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

// ---------- Public API ----------

/// Redact format-based multilingual PII from `text`.
///
/// Runs a Unicode normalization pre-pass to defeat fullwidth-digit and
/// zero-width-char bypasses. Match indices from the normalized form are
/// translated back to original byte offsets so only the PII bytes are
/// replaced — surrounding text (including any preserved fullwidth glyphs)
/// is untouched.
pub fn redact_pii(text: &str) -> Sanitized<String> {
    let mut report = SanitizationReport::default();

    // Fast path: cheap byte pre-filter on the raw text. Fullwidth / Arabic-Indic
    // digits and folded punctuation only surface after normalization, so a clean
    // raw scan still re-checks the normalized view before declaring the text PII-
    // free (mirrors the old two-phase SCREEN check).
    let raw_cand = scan_candidates(text);
    if !raw_cand.any() {
        let nview = NormalizedView::build(text);
        let ncand = scan_candidates(&nview.normalized);
        if !ncand.any() {
            log::trace!(
                "[pii] redact_pii: no candidate before or after normalization (len={})",
                text.len()
            );
            return Sanitized {
                value: text.to_string(),
                report,
            };
        }
        log::debug!("[pii] redact_pii: candidate surfaced only after normalization");
        return splice_redactions(
            text,
            &nview,
            collect_redactions(&nview.normalized, &ncand),
            &mut report,
        );
    }

    let nview = NormalizedView::build(text);
    // Gate on candidates from the NORMALIZED text — the precise regexes run
    // against it, so normalization-induced classes (folded digits) are included.
    let ncand = scan_candidates(&nview.normalized);
    let redactions = collect_redactions(&nview.normalized, &ncand);
    splice_redactions(text, &nview, redactions, &mut report)
}

/// True if `value` looks like it carries any PII. Used to *reject*
/// namespace/key inputs at boundary checks (analogous to
/// [`super::has_likely_secret`]).
///
/// Uses the **strict** match set — only formatted / keyword-gated patterns.
/// Bare-numeric patterns whose only signal is a digit run (credit card via
/// Luhn, bare CPF, bare CNPJ) or a phone-shaped digit run (NANP without
/// separators, E.164 leading `+`) are excluded here because their false-
/// positive rate against scanner-built namespace/key identifiers (WhatsApp
/// JIDs like `12025551234-1543890267@g.us`, telegram numeric peer IDs,
/// millisecond timestamps, padded counters) is too high to use as a hard
/// rejection signal. Content scrubbing via [`redact_pii`] still applies
/// those patterns — false positives are tolerable there because they only
/// replace bytes inside a string, not reject the whole write.
pub fn has_likely_pii(value: &str) -> bool {
    let nview = NormalizedView::build(value);
    let cand = scan_candidates(&nview.normalized);
    if !cand.any() {
        return false;
    }
    !collect_strict_redactions(&nview.normalized, &cand).is_empty()
}

/// True when `value` contains an ordinary email address. Kept separate from
/// [`has_likely_pii`] because scanner-built identifiers may legitimately
/// contain email-like `@` segments.
pub fn has_likely_email(value: &str) -> bool {
    // Cheap gate: every email requires an `@`. Skip compiling the regex when
    // the byte is absent (the common namespace/key case).
    if !value.as_bytes().contains(&b'@') {
        return false;
    }
    EMAIL_RE.is_match(value)
}

// ---------- Match collection ----------

#[derive(Debug)]
struct Hit {
    start: usize, // byte offset in NORMALIZED text
    end: usize,
    token: &'static str,
}

fn collect_redactions(norm: &str, cand: &Candidates) -> Vec<Hit> {
    collect_redactions_inner(norm, cand, true)
}

/// Variant of [`collect_redactions`] that omits bare-numeric patterns
/// whose only signal is a digit-run shape: credit card via Luhn, bare
/// CPF, bare CNPJ, NANP phones (separators optional, so any 10-11 digit
/// run starting `[2-9]`/`1[2-9]` matches), and E.164 phones (literal `+`
/// the only signal). Used for boundary checks like [`has_likely_pii`]
/// where rejection on such a hit alone would have too many false
/// positives on scanner-built identifiers (WhatsApp group JIDs
/// `<phone>-<unix>@g.us`, timestamps, padded counters).
fn collect_strict_redactions(norm: &str, cand: &Candidates) -> Vec<Hit> {
    collect_redactions_inner(norm, cand, false)
}

/// Run only the precise regexes whose class was flagged by [`scan_candidates`].
/// Priority order (and therefore overlap-resolution) is byte-identical to the
/// unconditional version; the `if cand.*` guards only decide whether each class
/// runs, so a flagged class produces exactly the hits it always did.
fn collect_redactions_inner(norm: &str, cand: &Candidates, include_bare_numeric: bool) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();

    // Priority order: most specific / highest-confidence first.
    if cand.cpf_fmt {
        push_checksum(&mut hits, norm, &CPF_FMT_RE, PII_CPF, |s| {
            valid_cpf(digits(s).as_slice())
        });
    }
    if cand.cnpj_fmt {
        push_checksum(&mut hits, norm, &CNPJ_FMT_RE, PII_CNPJ, |s| {
            valid_cnpj(digits(s).as_slice())
        });
    }
    if cand.cuit {
        push_checksum(&mut hits, norm, &CUIT_RE, PII_CUIT, |s| {
            valid_cuit(digits(s).as_slice())
        });
    }

    // IBAN before credit card: CC can match an IBAN tail of all digits.
    if cand.iban {
        push_checksum(&mut hits, norm, &IBAN_RE, PII_IBAN, valid_iban);
    }

    if include_bare_numeric {
        // Credit card before bare CPF/CNPJ to avoid catching a 13-19 digit run as CPF/CNPJ.
        if cand.cc {
            push_checksum(&mut hits, norm, &CC_RE, PII_CC, valid_luhn);
        }
        if cand.cnpj_bare {
            push_checksum(&mut hits, norm, &CNPJ_BARE_RE, PII_CNPJ, |s| {
                valid_cnpj(digits(s).as_slice())
            });
        }
        if cand.cpf_bare {
            push_checksum(&mut hits, norm, &CPF_BARE_RE, PII_CPF, |s| {
                valid_cpf(digits(s).as_slice())
            });
        }
    }

    if cand.aadhaar_fmt {
        push_checksum(&mut hits, norm, &AADHAAR_FMT_RE, PII_AADHAAR, |s| {
            valid_verhoeff(digits(s).as_slice())
        });
    }
    // Keyword-gated Aadhaar redacts only the captured 12-digit group.
    if cand.aadhaar_kw {
        push_captured(&mut hits, norm, &AADHAAR_KW_RE, PII_AADHAAR, |digits_str| {
            valid_verhoeff(digits(digits_str).as_slice())
        });
    }

    if cand.dni {
        push_checksum(&mut hits, norm, &DNI_RE, PII_DNI, valid_dni_es);
    }
    if cand.nie {
        push_checksum(&mut hits, norm, &NIE_RE, PII_DNI, valid_nie_es);
    }
    if cand.nino {
        push_checksum(&mut hits, norm, &NINO_RE, PII_NINO, valid_nino);
    }
    if cand.ssn {
        push_checksum(&mut hits, norm, &SSN_RE, PII_SSN, valid_ssn);
    }
    if cand.rrn {
        push_simple(&mut hits, norm, &RRN_RE, PII_RRN);
    }
    if cand.rfc {
        push_simple(&mut hits, norm, &RFC_RE, PII_RFC);
    }
    if cand.pan_in {
        push_simple(&mut hits, norm, &PAN_IN_RE, PII_PAN_IN);
    }

    if include_bare_numeric {
        // Phones: E.164 first (more specific), then NANP. Both are bare-numeric
        // shapes — NANP allows optional separators (`\b\d{10,11}\b` matches as
        // `XXX-XXX-XXXX`), and E.164 keys on a literal `+` with no further gate.
        // Strict callers (boundary checks like `has_likely_pii`) exclude these
        // so scanner-built namespace/key values (WhatsApp JIDs
        // `<phone>-<unix>@g.us`, telegram numeric peer IDs) don't get rejected.
        if cand.phone_e164 {
            push_simple(&mut hits, norm, &PHONE_E164_RE, PII_PHONE);
        }
        if cand.phone_nanp {
            push_simple(&mut hits, norm, &PHONE_NANP_RE, PII_PHONE);
        }
    }

    // My Number — captured digit group only, keyword remains visible.
    if cand.mynumber {
        push_captured(&mut hits, norm, &MYNUM_RE, PII_MYNUM, |_| true);
    }

    dedupe_overlaps(&mut hits);
    log::debug!(
        "[pii] collect_redactions strict={} hits={}",
        !include_bare_numeric,
        hits.len()
    );
    hits
}

fn push_simple(hits: &mut Vec<Hit>, norm: &str, re: &Regex, token: &'static str) {
    for m in re.find_iter(norm) {
        hits.push(Hit {
            start: m.start(),
            end: m.end(),
            token,
        });
    }
}

fn push_checksum(
    hits: &mut Vec<Hit>,
    norm: &str,
    re: &Regex,
    token: &'static str,
    ok: impl Fn(&str) -> bool,
) {
    for m in re.find_iter(norm) {
        if ok(m.as_str()) {
            hits.push(Hit {
                start: m.start(),
                end: m.end(),
                token,
            });
        }
    }
}

fn push_captured(
    hits: &mut Vec<Hit>,
    norm: &str,
    re: &Regex,
    token: &'static str,
    ok: impl Fn(&str) -> bool,
) {
    for caps in re.captures_iter(norm) {
        let Some(group) = caps.get(1) else { continue };
        if ok(group.as_str()) {
            hits.push(Hit {
                start: group.start(),
                end: group.end(),
                token,
            });
        }
    }
}

// Sort by start asc, length desc. Then walk in order, dropping any hit whose
// range overlaps a kept hit. Result: earlier + longer wins; no double-redact.
fn dedupe_overlaps(hits: &mut Vec<Hit>) {
    hits.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then((b.end - b.start).cmp(&(a.end - a.start)))
    });
    let mut kept: Vec<Hit> = Vec::with_capacity(hits.len());
    for h in hits.drain(..) {
        let overlaps = kept.last().is_some_and(|k| h.start < k.end);
        if !overlaps {
            kept.push(h);
        }
    }
    *hits = kept;
}

// Splice redactions (whose indices reference NORMALIZED text) back into the
// ORIGINAL text via NormalizedView's byte-offset mapping. This preserves
// non-PII original bytes verbatim (including fullwidth glyphs the user
// intentionally typed) while still scrubbing detected PII.
fn splice_redactions(
    original: &str,
    nview: &NormalizedView,
    hits: Vec<Hit>,
    report: &mut SanitizationReport,
) -> Sanitized<String> {
    if hits.is_empty() {
        return Sanitized {
            value: original.to_string(),
            report: *report,
        };
    }
    let mut out = String::with_capacity(original.len());
    let mut cursor = 0;
    for h in &hits {
        let start_orig = nview.norm_to_orig(h.start);
        let end_orig = nview.norm_to_orig(h.end);
        if start_orig < cursor || start_orig > original.len() || end_orig > original.len() {
            continue;
        }
        out.push_str(&original[cursor..start_orig]);
        out.push_str(h.token);
        cursor = end_orig;
    }
    out.push_str(&original[cursor..]);
    report.pii_redactions += hits.len();
    Sanitized {
        value: out,
        report: *report,
    }
}

// ---------- Unicode normalization for matching ----------

struct NormalizedView {
    normalized: String,
    // For each byte offset i in `normalized`, `byte_map[i]` is the byte offset
    // in the original string where the corresponding char *starts*.
    // The last entry maps the normalized length to the original length, so
    // `norm_to_orig(normalized.len())` is well-defined.
    byte_map: Vec<usize>,
}

impl NormalizedView {
    fn build(original: &str) -> Self {
        let mut normalized = String::with_capacity(original.len());
        let mut byte_map: Vec<usize> = Vec::with_capacity(original.len() + 1);
        for (idx, ch) in original.char_indices() {
            if is_zero_width(ch) {
                continue;
            }
            let mapped = fold_char(ch);
            let start = normalized.len();
            normalized.push(mapped);
            // One byte_map entry per byte of the normalized char.
            let added = normalized.len() - start;
            for _ in 0..added {
                byte_map.push(idx);
            }
        }
        byte_map.push(original.len());
        Self {
            normalized,
            byte_map,
        }
    }

    fn norm_to_orig(&self, norm_byte: usize) -> usize {
        if norm_byte >= self.byte_map.len() {
            return *self.byte_map.last().unwrap_or(&0);
        }
        self.byte_map[norm_byte]
    }
}

fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{2060}'
            | '\u{180E}'
            | '\u{FEFF}'
    )
}

fn fold_char(c: char) -> char {
    match c {
        // Fullwidth digits 0-9
        '\u{FF10}'..='\u{FF19}' => char::from_u32(c as u32 - 0xFF10 + 0x30).unwrap_or(c),
        // Arabic-Indic digits ٠-٩
        '\u{0660}'..='\u{0669}' => char::from_u32(c as u32 - 0x0660 + 0x30).unwrap_or(c),
        // Eastern Arabic-Indic digits ۰-۹
        '\u{06F0}'..='\u{06F9}' => char::from_u32(c as u32 - 0x06F0 + 0x30).unwrap_or(c),
        // Common fullwidth punctuation we care about for PII formats
        '\u{FF0D}' => '-',
        '\u{FF0E}' => '.',
        '\u{FF0F}' => '/',
        '\u{FF1A}' => ':',
        '\u{2010}'..='\u{2015}' => '-', // various unicode hyphens/dashes
        '\u{2212}' => '-',              // minus sign
        other => other,
    }
}

// ---------- Checksum helpers ----------

#[cfg(test)]
#[path = "pii_tests.rs"]
mod tests;

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

// Credit card: 13-19 digits with optional spaces/dashes every 4. Every match
// is Luhn-gated; a match with no separators at all additionally needs
// corroboration — a real network IIN at an issued length, or a card keyword
// nearby — because Luhn alone passes ~10% of arbitrary digit runs, and bare
// 13-digit epoch-millisecond timestamps were being redacted out of stored
// JSON envelopes at exactly that rate (opencompany#1201). Same split as
// Aadhaar below: formatted keeps the checksum-only gate, bare needs more.
static CC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d[\s\-]?){13,19}\b").expect("credit card"));

// Card keyword corroborating a bare digit run. Three tiers, matched
// case-insensitively:
//
// * Standalone words, bounded by `[\W_]` rather than `\b` — the regex crate
//   counts `_` as a word character, so `\bcard\b` never fires inside
//   `card_number`, which is among the most common serialized key shapes a
//   stored payload carries. The explicit class keeps `pan` from firing
//   inside `japan` while still matching `card_number=` and `cc=`.
// * Compound identifiers matched as substrings, because camelCase provides
//   no boundary of any kind: `cardNumber`, `creditCard`, `ccNum`, `cardNo`,
//   `panNumber` all lowercase into these.
// * Native-script terms, per this module's multilingual mandate (the Aadhaar
//   and My Number patterns already carry theirs). CJK terms match as
//   substrings because CJK sentences provide no `[\W_]` boundaries around a
//   word — the case this buys is a keyword embedded mid-sentence
//   (`…信用卡账单 <PAN>`). It does not buy `卡号<PAN>` with the digits
//   directly attached: there `CC_RE`'s own leading `\b` already fails
//   (CJK is `\w`), so the run is never a candidate in the first place.
static CC_KEYWORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[\W_])(?:card|credit|debit|visa|mastercard|amex|american\s?express|discover|jcb|diners|unionpay|hipercard|rupay|cvv|cvc|cc|pan|tarjeta|cart[aã]o|carte|karte|карта|карты|карту|картой|карте|кредитка)(?:[\W_]|$)|(?i:cardnumber|creditcard|ccnum|cardno|pannumber|カード|信用卡|卡号|银行卡|카드)",
    )
    .expect("cc keyword")
});

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
// The single cheap byte pass that replaces the always-resident combined
// `RegexSet`. Lives in its own module — see `prefilter.rs` for the full rationale.
mod prefilter;
use prefilter::{scan_candidates, Candidates};

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
/// those patterns — a content false positive replaces bytes inside a string
/// rather than rejecting the whole write, which is cheaper but *not* free:
/// a redaction landing inside structured content corrupts it for whatever
/// wrote it (opencompany#1201 — timestamps in stored JSON envelopes), which
/// is why the credit-card pattern's bare form now demands corroboration
/// beyond its checksum.
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
            push_credit_cards(&mut hits, norm);
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

/// Credit-card collection. Every match must pass Luhn; a bare match (no
/// separators) additionally needs structural or contextual corroboration.
///
/// Luhn alone passes ~10% of arbitrary digit runs, and 13-19 contiguous
/// digits is a common machine-identifier shape — 13-digit epoch-millisecond
/// timestamps above all, which were being redacted out of stored JSON
/// envelopes at that rate and corrupting them (opencompany#1201). The strict
/// boundary set ([`collect_strict_redactions`]) already excludes credit card
/// for exactly this reason; this brings the content path to the same
/// judgement without giving up real cards: a separated run keeps the
/// Luhn-only gate it always had, and a bare run still redacts when its
/// prefix is a real network IIN at an issued length
/// ([`plausible_card_number`]) or a card keyword sits within
/// [`CC_KEYWORD_WINDOW`] bytes.
fn push_credit_cards(hits: &mut Vec<Hit>, norm: &str) {
    for m in CC_RE.find_iter(norm) {
        let s = m.as_str();
        if !valid_luhn(s) {
            continue;
        }
        // Judge bare-vs-separated on the interior of the digit run: the
        // regex's per-digit `[\s\-]?` can capture one trailing separator
        // (`"…773 "`), which is not the human 4-4-4-4 grouping this
        // distinction is after.
        let interior = s.trim_matches(|c: char| !c.is_ascii_digit());
        let bare = interior.bytes().all(|b| b.is_ascii_digit());
        let corroborated =
            !bare || plausible_card_number(&digits(s)) || cc_keyword_near(norm, m.start(), m.end());
        if corroborated {
            hits.push(Hit {
                start: m.start(),
                end: m.end(),
                token: PII_CC,
            });
        }
    }
}

/// Bytes of context searched either side of a bare digit run for a card
/// keyword. 64 bytes rather than 32 because the window is counted in bytes
/// while text is not: 32 bytes is only ~10 CJK characters or 8 emoji, so a
/// run of either could evict an English keyword that a reader would call
/// adjacent. 64 comfortably spans `{"payment_method":{"card":{"number":…`
/// and a `カード`-prefixed line alike.
const CC_KEYWORD_WINDOW: usize = 64;

/// True when [`CC_KEYWORD_RE`] matches within the window around
/// `start..end`, widened outward to char boundaries so the slice cannot
/// split a multi-byte character.
fn cc_keyword_near(norm: &str, start: usize, end: usize) -> bool {
    let mut lo = start.saturating_sub(CC_KEYWORD_WINDOW);
    while lo > 0 && !norm.is_char_boundary(lo) {
        lo -= 1;
    }
    let mut hi = (end + CC_KEYWORD_WINDOW).min(norm.len());
    while hi < norm.len() && !norm.is_char_boundary(hi) {
        hi += 1;
    }
    CC_KEYWORD_RE.is_match(&norm[lo..hi])
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

// Fullwidth / zero-width normalization used before matching. Lives in its own
// module — see `normalize.rs`.
mod normalize;
use normalize::NormalizedView;

// ---------- Checksum helpers ----------

#[cfg(test)]
#[path = "pii_tests.rs"]
mod tests;

//! Checksum and structural validators for PII candidates.

pub(super) fn digits(s: &str) -> Vec<u32> {
    s.chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c.to_digit(10).expect("ascii digit"))
        .collect()
}

pub(super) fn valid_cpf(d: &[u32]) -> bool {
    if d.len() != 11 || d.iter().all(|x| *x == d[0]) {
        return false;
    }
    let s1: u32 = (0..9).map(|i| d[i] * (10 - i as u32)).sum();
    let dv1 = (s1 * 10) % 11 % 10;
    if dv1 != d[9] {
        return false;
    }
    let s2: u32 = (0..10).map(|i| d[i] * (11 - i as u32)).sum();
    let dv2 = (s2 * 10) % 11 % 10;
    dv2 == d[10]
}

pub(super) fn valid_cnpj(d: &[u32]) -> bool {
    if d.len() != 14 || d.iter().all(|x| *x == d[0]) {
        return false;
    }
    let w1: [u32; 12] = [5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
    let s1: u32 = (0..12).map(|i| d[i] * w1[i]).sum();
    let r1 = s1 % 11;
    let dv1 = if r1 < 2 { 0 } else { 11 - r1 };
    if dv1 != d[12] {
        return false;
    }
    let w2: [u32; 13] = [6, 5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
    let s2: u32 = (0..13).map(|i| d[i] * w2[i]).sum();
    let r2 = s2 % 11;
    let dv2 = if r2 < 2 { 0 } else { 11 - r2 };
    dv2 == d[13]
}

pub(super) fn valid_cuit(d: &[u32]) -> bool {
    if d.len() != 11 {
        return false;
    }
    let w: [u32; 10] = [5, 4, 3, 2, 7, 6, 5, 4, 3, 2];
    let s: u32 = (0..10).map(|i| d[i] * w[i]).sum();
    let r = s % 11;
    let dv = match r {
        0 => 0,
        1 => return false,
        _ => 11 - r,
    };
    dv == d[10]
}

// Luhn — used for credit-card validation.
pub(super) fn valid_luhn(s: &str) -> bool {
    let d = digits(s);
    if d.len() < 13 || d.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    let mut alt = false;
    for x in d.iter().rev() {
        let v = if alt {
            let doubled = x * 2;
            if doubled > 9 {
                doubled - 9
            } else {
                doubled
            }
        } else {
            *x
        };
        sum += v;
        alt = !alt;
    }
    sum.is_multiple_of(10)
}

// IBAN mod-97. Steps: strip spaces, move first 4 chars to end, expand letters
// (A=10..Z=35), divide as a big-integer mod 97, require remainder == 1.
pub(super) fn valid_iban(s: &str) -> bool {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.len() < 15 || cleaned.len() > 34 {
        return false;
    }
    if !cleaned.chars().take(2).all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !cleaned[2..4].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let rotated: String = cleaned[4..].chars().chain(cleaned[..4].chars()).collect();
    let mut remainder: u64 = 0;
    for c in rotated.chars() {
        let chunk = if let Some(d) = c.to_digit(10) {
            d as u64
        } else if c.is_ascii_alphabetic() {
            (c.to_ascii_uppercase() as u64) - ('A' as u64) + 10
        } else {
            return false;
        };
        // Expand into the running remainder digit-by-digit so we never need
        // u128. Each letter contributes 2 decimal digits.
        if chunk >= 10 {
            remainder = (remainder * 100 + chunk) % 97;
        } else {
            remainder = (remainder * 10 + chunk) % 97;
        }
    }
    remainder == 1
}

// Verhoeff — used for Aadhaar.
const VERHOEFF_D: [[u8; 10]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
    [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
    [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
    [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
    [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
    [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
    [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
    [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
    [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
];
const VERHOEFF_P: [[u8; 10]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
    [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
    [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
    [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
    [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
    [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
    [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
];

pub(super) fn valid_verhoeff(d: &[u32]) -> bool {
    if d.len() != 12 {
        return false;
    }
    // Aadhaar can't start with 0 or 1.
    if d[0] < 2 {
        return false;
    }
    let mut c: u8 = 0;
    for (i, digit) in d.iter().rev().enumerate() {
        c = VERHOEFF_D[c as usize][VERHOEFF_P[i % 8][*digit as usize] as usize];
    }
    c == 0
}

// US SSN reserved/invalid ranges per SSA.
pub(super) fn valid_ssn(s: &str) -> bool {
    let d = digits(s);
    if d.len() != 9 {
        return false;
    }
    let area = d[0] * 100 + d[1] * 10 + d[2];
    let group = d[3] * 10 + d[4];
    let serial = d[5] * 1000 + d[6] * 100 + d[7] * 10 + d[8];
    if area == 0 || area == 666 || area >= 900 {
        return false;
    }
    if group == 0 || serial == 0 {
        return false;
    }
    true
}

// Spain DNI check letter — 8 digits mod 23 indexes into a fixed letter table.
const DNI_LETTERS: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";

pub(super) fn valid_dni_es(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() != 9 {
        return false;
    }
    let num_str = &upper[..8];
    let letter = bytes[8];
    let Ok(num) = num_str.parse::<u32>() else {
        return false;
    };
    DNI_LETTERS[(num % 23) as usize] == letter
}

pub(super) fn valid_nie_es(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() != 9 {
        return false;
    }
    let prefix = match bytes[0] {
        b'X' => 0u32,
        b'Y' => 1,
        b'Z' => 2,
        _ => return false,
    };
    let Ok(rest) = std::str::from_utf8(&bytes[1..8]) else {
        return false;
    };
    let Ok(num) = rest.parse::<u32>() else {
        return false;
    };
    let composed = prefix * 10_000_000 + num;
    DNI_LETTERS[(composed % 23) as usize] == bytes[8]
}

// UK NINO reserved-prefix blacklist.
pub(super) fn valid_nino(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() != 9 {
        return false;
    }
    // First char cannot be D F I Q U V; second cannot be D F I O Q U V.
    let bad_first = b"DFIQUV";
    let bad_second = b"DFIOQUV";
    if bad_first.contains(&bytes[0]) || bad_second.contains(&bytes[1]) {
        return false;
    }
    // Reserved two-letter prefixes.
    let reserved = ["BG", "GB", "KN", "NK", "NT", "TN", "ZZ"];
    let prefix = &upper[..2];
    if reserved.contains(&prefix) {
        return false;
    }
    true
}

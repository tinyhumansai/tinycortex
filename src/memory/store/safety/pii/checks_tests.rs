use super::*;

#[test]
fn tax_ids_enforce_lengths_checksums_and_repetition_rules() {
    assert!(valid_cpf(&digits("529.982.247-25")));
    assert!(!valid_cpf(&digits("111.111.111-11")));
    assert!(!valid_cpf(&digits("5299822472")));
    assert!(valid_cnpj(&digits("11.222.333/0001-81")));
    assert!(!valid_cnpj(&digits("11.222.333/0001-82")));
    assert!(!valid_cnpj(&digits("00000000000000")));
    assert!(valid_cuit(&digits("20-12345678-6")));
    assert!(!valid_cuit(&digits("20-12345678-7")));
    assert!(!valid_cuit(&digits("2012345678")));
}

#[test]
fn payment_checksums_reject_bad_bounds_and_checksums() {
    assert!(valid_luhn("4111 1111 1111 1111"));
    assert!(!valid_luhn("4111 1111 1111 1112"));
    assert!(!valid_luhn("7992739871"));
    assert!(valid_iban("GB82 WEST 1234 5698 7654 32"));
    assert!(!valid_iban("GB82 WEST 1234 5698 7654 33"));
    assert!(!valid_iban("GB00"));
}

#[test]
fn identity_validators_cover_checksums_reserved_values_and_prefixes() {
    assert!(valid_verhoeff(&digits("234567890124")));
    assert!(!valid_verhoeff(&digits("134567890124")));
    assert!(!valid_verhoeff(&digits("234567890125")));
    assert!(valid_ssn("123-45-6789"));
    assert!(!valid_ssn("666-45-6789"));
    assert!(!valid_ssn("123-00-6789"));
    assert!(!valid_ssn("123-45-0000"));
    assert!(valid_dni_es("12345678Z"));
    assert!(!valid_dni_es("12345678A"));
    assert!(valid_nie_es("X1234567L"));
    assert!(!valid_nie_es("A1234567L"));
    assert!(valid_nino("AA123456A"));
    assert!(!valid_nino("BG123456A"));
    assert!(!valid_nino("DA123456A"));
    assert!(!valid_nino("AA12345A"));
}

#[test]
fn plausible_card_number_requires_a_real_iin_at_an_issued_length() {
    // No network's prefix: epoch-millisecond timestamps and other machine ids.
    assert!(!plausible_card_number(&digits("1787178633773"))); // 13-digit epoch ms
    assert!(!plausible_card_number(&digits("1700000000000")));
    assert!(!plausible_card_number(&digits("9111111111111119")));
    assert!(!plausible_card_number(&digits("2000000000000000"))); // year-2033 epoch-µs shape
    assert!(!plausible_card_number(&digits("1900000000000"))); // 13-digit, no IIN starts 1

    // Out of the card length window entirely.
    assert!(!plausible_card_number(&digits("411111111111"))); // 12
    assert!(!plausible_card_number(&digits("41111111111111111111"))); // 20
}

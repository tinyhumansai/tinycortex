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
    // Issued shapes on major networks.
    assert!(plausible_card_number(&digits("4111111111111111"))); // Visa 16
    assert!(plausible_card_number(&digits("4222222222222"))); // Visa 13 (legacy)
    assert!(plausible_card_number(&digits("5500005555555559"))); // Mastercard 55
    assert!(plausible_card_number(&digits("2221000000000009"))); // Mastercard 2-series
    assert!(plausible_card_number(&digits("378282246310005"))); // Amex 37, 15
    assert!(plausible_card_number(&digits("6011111111111117"))); // Discover
    assert!(plausible_card_number(&digits("3530111333300000"))); // JCB
    assert!(plausible_card_number(&digits("36700102000000"))); // Diners 36, 14
    assert!(plausible_card_number(&digits("6200000000000005"))); // UnionPay

    // Right prefix at a length the network does not issue.
    assert!(!plausible_card_number(&digits("41111111111111"))); // Visa at 14
    assert!(!plausible_card_number(&digits("37828224631000"))); // Amex at 14
    assert!(!plausible_card_number(&digits("55000055555555590"))); // MC at 17

    // No network's prefix: epoch-millisecond timestamps and other machine ids.
    assert!(!plausible_card_number(&digits("1787178633773"))); // 13-digit epoch ms
    assert!(!plausible_card_number(&digits("1700000000000")));
    assert!(!plausible_card_number(&digits("9111111111111119")));

    // Out of the card length window entirely.
    assert!(!plausible_card_number(&digits("411111111111"))); // 12
    assert!(!plausible_card_number(&digits("41111111111111111111"))); // 20
}

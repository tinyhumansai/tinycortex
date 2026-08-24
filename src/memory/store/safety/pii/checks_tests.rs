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

// The accept direction, per network, at its boundary lengths — with the
// reject cases one step past each length and each range edge. Luhn is
// irrelevant here: the function judges shape only, and the redaction path
// tests Luhn separately.
#[test]
fn plausible_card_number_accepts_each_network_at_its_boundary_lengths() {
    // Visa 13/16/19; nothing between or past.
    assert!(plausible_card_number(&digits("4222222222222"))); // 13
    assert!(plausible_card_number(&digits("4111111111111111"))); // 16
    assert!(plausible_card_number(&digits("4111111111111111111"))); // 19
    assert!(!plausible_card_number(&digits("41111111111111"))); // 14
    assert!(!plausible_card_number(&digits("411111111111111"))); // 15
    assert!(!plausible_card_number(&digits("41111111111111111"))); // 17

    // Mastercard 51-55 and 2221-2720, 16 only; edges out both sides.
    assert!(plausible_card_number(&digits("5100000000000000")));
    assert!(plausible_card_number(&digits("5500005555555559")));
    assert!(plausible_card_number(&digits("2221000000000009")));
    assert!(plausible_card_number(&digits("2720000000000000")));
    assert!(!plausible_card_number(&digits("550000555555555"))); // 15
    assert!(!plausible_card_number(&digits("55000055555555590"))); // 17
    assert!(!plausible_card_number(&digits("5000000000000000"))); // 50: not MC
    assert!(!plausible_card_number(&digits("2220000000000000"))); // below 2221
    assert!(!plausible_card_number(&digits("2721000000000000"))); // above 2720

    // Mir 2200-2204, 16 only.
    assert!(plausible_card_number(&digits("2200000000000004")));
    assert!(plausible_card_number(&digits("2204000000000000")));
    assert!(!plausible_card_number(&digits("2205000000000009"))); // past range
    assert!(!plausible_card_number(&digits("220000000000000"))); // 15
    assert!(!plausible_card_number(&digits("22000000000000004"))); // 17

    // Amex 34/37, 15 only.
    assert!(plausible_card_number(&digits("378282246310005")));
    assert!(plausible_card_number(&digits("340000000000009")));
    assert!(!plausible_card_number(&digits("37828224631000"))); // 14
    assert!(!plausible_card_number(&digits("3782822463100051"))); // 16
    assert!(!plausible_card_number(&digits("350000000000000"))); // 35 alone

    // JCB 3528-3589, 16-19.
    assert!(plausible_card_number(&digits("3530111333300000")));
    assert!(plausible_card_number(&digits("3589000000000000000"))); // 19
    assert!(!plausible_card_number(&digits("3527000000000000")));
    assert!(!plausible_card_number(&digits("3590000000000000")));
    assert!(!plausible_card_number(&digits("353011133330000"))); // 15

    // Diners Club — one scheme, one rule: 36, 300-305, 3095, 38, 39 all at
    // 14-19. 30569309025904 is the canonical Diners test PAN; regression
    // for the review finding that 14-digit Diners had been split away.
    assert!(plausible_card_number(&digits("30569309025904"))); // 300-305 @ 14
    assert!(plausible_card_number(&digits("38520000023237"))); // 38 @ 14
    assert!(plausible_card_number(&digits("36700102000000"))); // 36 @ 14
    assert!(plausible_card_number(&digits("30950000000000"))); // 3095 @ 14
    assert!(plausible_card_number(&digits("39000000000005"))); // 39 @ 14
    assert!(plausible_card_number(&digits("3050000000000000002"))); // 305 @ 19
    assert!(!plausible_card_number(&digits("3060000000000000"))); // 306
    assert!(!plausible_card_number(&digits("3700010200000"))); // 37 @ 13: not Diners

    // Discover 6011 / 644-649 / 65 at 16 or 19.
    assert!(plausible_card_number(&digits("6011111111111117")));
    assert!(plausible_card_number(&digits("6440000000000000")));
    assert!(plausible_card_number(&digits("6500000000000000000"))); // 19
    assert!(!plausible_card_number(&digits("60111111111111170"))); // 17
    assert!(!plausible_card_number(&digits("6430000000000000"))); // 643

    // UnionPay 62 at 16-19.
    assert!(plausible_card_number(&digits("6200000000000005")));
    assert!(plausible_card_number(&digits("6200000000000000005"))); // 19
    assert!(!plausible_card_number(&digits("620000000000000"))); // 15

    // Maestro (5018/5020/5038/5893, 56-58, 6304/6759/6761-6763) at 13-19.
    assert!(plausible_card_number(&digits("5018000000000"))); // 13
    assert!(plausible_card_number(&digits("5600000000002"))); // 56 @ 13
    assert!(plausible_card_number(&digits("5800000000000000008"))); // 58 @ 19
    assert!(plausible_card_number(&digits("6759000000000000000"))); // 19
    assert!(plausible_card_number(&digits("6763000000000000")));
    assert!(!plausible_card_number(&digits("5019000000000000"))); // 5019
    assert!(!plausible_card_number(&digits("5900000000000000"))); // 59
    assert!(!plausible_card_number(&digits("6760000000000000"))); // 6760

    // RuPay 60 / 508 / 81 / 82 at 16.
    assert!(plausible_card_number(&digits("6069850000000000")));
    assert!(plausible_card_number(&digits("5080000000000002")));
    assert!(plausible_card_number(&digits("8100000000000000")));
    assert!(plausible_card_number(&digits("8200000000000000")));
    assert!(!plausible_card_number(&digits("8300000000000000"))); // 83
    assert!(!plausible_card_number(&digits("810000000000000"))); // 15
    assert!(!plausible_card_number(&digits("5090000000000000"))); // 509

    // Elo 5041/5066/5067/6277/6362/6363 and Hipercard 6062, 16.
    assert!(plausible_card_number(&digits("5067310000000010")));
    assert!(plausible_card_number(&digits("5041000000000000")));
    assert!(plausible_card_number(&digits("6362000000000009")));
    assert!(plausible_card_number(&digits("6277000000000000")));
    // Hipercard 6062 sits inside RuPay's blanket 60 range as well — listed
    // in the table in its own right, but note 60xx@16 is corroborable for
    // any xx, which is what the published RuPay range says.
    assert!(plausible_card_number(&digits("6062821234567890"))); // Hipercard
    assert!(!plausible_card_number(&digits("5065000000000001"))); // 5065
    assert!(!plausible_card_number(&digits("6364000000000007"))); // 6364
    assert!(!plausible_card_number(&digits("506731000000001"))); // 15

    // The documented expiry (see the function doc): 16-digit
    // epoch-microsecond stamps enter Mir/Mastercard-2-series territory
    // around 2039/2040. Asserted as truth, not as an endorsement — when
    // this line starts mattering, the gate needs a rethink.
    assert!(plausible_card_number(&digits("2221787178633773"))); // µs in ~2040
    assert!(!plausible_card_number(&digits("1787178633773000"))); // µs today
}

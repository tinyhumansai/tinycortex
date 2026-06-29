use super::*;

#[test]
fn short_text_returns_neutral() {
    assert_eq!(score(""), 0.5);
    assert_eq!(score("hi bob"), 0.5);
}

#[test]
fn high_repetition_scored_low() {
    let noisy = "yay yay yay yay yay yay yay yay yay yay lol lol lol lol";
    assert!(score(noisy) < 0.2);
}

#[test]
fn substantive_text_scored_high() {
    let good = "We decided to ship Phoenix on Friday after reviewing the migration plan carefully.";
    assert!(score(good) >= 0.9);
}

#[test]
fn medium_repetition_ramps() {
    // ~50% unique ratio should score around 0.5
    let med = "alpha beta alpha beta gamma alpha delta beta gamma alpha";
    let s = score(med);
    assert!(s > 0.2 && s < 0.8);
}

#[test]
fn punctuation_stripped() {
    let s1 = score("ship phoenix friday ship phoenix friday ship phoenix");
    let s2 = score("ship! phoenix, friday. ship! phoenix, friday. ship! phoenix.");
    assert!((s1 - s2).abs() < 0.05);
}

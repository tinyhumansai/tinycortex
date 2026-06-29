use super::*;

#[test]
fn sanitize_entity_name_trims_punctuation_and_uppercases() {
    assert_eq!(sanitize_entity_name("  Alice Smith. "), "ALICE SMITH");
    assert_eq!(sanitize_entity_name("\"openhuman\""), "OPENHUMAN");
    assert_eq!(sanitize_entity_name(""), "");
}

#[test]
fn sanitize_fact_text_collapses_whitespace_and_strips_edges() {
    assert_eq!(sanitize_fact_text("  -  Hello   world. "), "Hello world");
    assert_eq!(sanitize_fact_text(":: spaced\ttext ;;"), "spaced text");
}

#[test]
fn classify_entity_detects_dates_people_and_products() {
    let mut known_people = HashMap::new();
    known_people.insert("ALICE SMITH".to_string(), "ALICE SMITH".to_string());

    assert_eq!(classify_entity("Jan 5, 2026", &known_people), "DATE");
    assert_eq!(classify_entity("Alice Smith", &known_people), "PERSON");
    assert_eq!(classify_entity("OpenHuman", &known_people), "PRODUCT");
    assert_eq!(classify_entity("Kitchen", &known_people), "ROOM");
    assert_eq!(classify_entity("phoenix-project", &known_people), "PROJECT");
}

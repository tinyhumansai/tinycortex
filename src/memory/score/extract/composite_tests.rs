use super::*;
use crate::memory::score::extract::EntityKind;

#[tokio::test]
async fn regex_only_extractor_works() {
    let c = CompositeExtractor::regex_only();
    let out = c.extract("hi @alice a@b.com #launch").await.unwrap();
    assert!(out.entities.iter().any(|e| e.kind == EntityKind::Handle));
    assert!(out.entities.iter().any(|e| e.kind == EntityKind::Email));
    assert!(out.entities.iter().any(|e| e.kind == EntityKind::Hashtag));
}

struct FailingExtractor;
#[async_trait]
impl EntityExtractor for FailingExtractor {
    fn name(&self) -> &'static str {
        "failing"
    }
    async fn extract(&self, _: &str) -> anyhow::Result<ExtractedEntities> {
        Err(anyhow::anyhow!("boom"))
    }
}

#[tokio::test]
async fn composite_survives_one_failing_extractor() {
    let c = CompositeExtractor::new(vec![
        Box::new(FailingExtractor),
        Box::new(RegexEntityExtractor),
    ]);
    let out = c.extract("@alice").await.unwrap();
    assert!(out.entities.iter().any(|e| e.kind == EntityKind::Handle));
}

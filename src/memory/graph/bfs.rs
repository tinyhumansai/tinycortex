//! Bounded shortest paths over persisted co-occurrence edges.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::graph::edge_store::edge_neighbors;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairDistance {
    pub a: String,
    pub b: String,
    pub dist: u32,
}

pub fn pair_distances(
    config: &MemoryConfig,
    entity_ids: &[String],
    max_h: u32,
) -> Result<Vec<PairDistance>> {
    if max_h == 0 || entity_ids.len() < 2 {
        return Ok(Vec::new());
    }
    let mut unique = entity_ids.to_vec();
    unique.sort();
    unique.dedup();
    let targets: HashSet<_> = unique.iter().cloned().collect();
    let mut cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut emitted = HashSet::new();
    let mut output = Vec::new();

    for source in &unique {
        let mut remaining = targets.clone();
        remaining.remove(source);
        let mut visited = HashSet::from([source.clone()]);
        let mut queue = VecDeque::from([(source.clone(), 0)]);
        while let Some((node, distance)) = queue.pop_front() {
            if distance >= max_h || remaining.is_empty() {
                continue;
            }
            if !cache.contains_key(&node) {
                cache.insert(
                    node.clone(),
                    edge_neighbors(config, &node)?
                        .into_iter()
                        .map(|(neighbor, _)| neighbor)
                        .collect(),
                );
            }
            for neighbor in cache.get(&node).cloned().unwrap_or_default() {
                if !visited.insert(neighbor.clone()) {
                    continue;
                }
                let next_distance = distance + 1;
                if remaining.remove(&neighbor) {
                    let pair = if source < &neighbor {
                        (source.clone(), neighbor.clone())
                    } else {
                        (neighbor.clone(), source.clone())
                    };
                    if emitted.insert(pair.clone()) {
                        output.push(PairDistance {
                            a: pair.0,
                            b: pair.1,
                            dist: next_distance,
                        });
                    }
                }
                queue.push_back((neighbor, next_distance));
            }
        }
    }
    output.sort_by(|a, b| {
        a.dist
            .cmp(&b.dist)
            .then_with(|| a.a.cmp(&b.a))
            .then_with(|| a.b.cmp(&b.b))
    });
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::graph::{pairs_from_entities, upsert_edges};

    #[test]
    fn bounded_bfs_finds_two_hop_pair() {
        let temp = tempfile::tempdir().unwrap();
        let config = MemoryConfig::new(temp.path());
        upsert_edges(
            &config,
            &pairs_from_entities(&["alice".into(), "bob".into()]),
            1,
        )
        .unwrap();
        upsert_edges(
            &config,
            &pairs_from_entities(&["bob".into(), "carol".into()]),
            1,
        )
        .unwrap();
        assert!(
            pair_distances(&config, &["alice".into(), "carol".into()], 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            pair_distances(&config, &["alice".into(), "carol".into()], 2).unwrap(),
            vec![PairDistance {
                a: "alice".into(),
                b: "carol".into(),
                dist: 2,
            }]
        );
    }
}

//! Cover the feasible pairs of the supplied matrix, preserving its constraints.
use super::{Configuration, Engine, OptLevel};
use std::collections::BTreeSet;

type Pair = (usize, String, usize, String);

fn values(c: &Configuration) -> Vec<String> {
    let o = &c.options;
    let mut values = vec![
        o.target.triple.clone(),
        o.regions.to_string(),
        o.elide.to_string(),
        o.reuse.to_string(),
        o.flat_walk.to_string(),
    ];
    values.extend(match c.engine {
        Engine::Heap { campaign } => vec![
            format!("heap/{campaign}"),
            "none".into(),
            "none".into(),
            "none".into(),
        ],
        Engine::Native {
            opt,
            partition,
            runtime,
        } => vec![
            "llvm".into(),
            match opt {
                OptLevel::None => "O0",
                OptLevel::Default => "O2",
                OptLevel::Speed => "O3",
            }
            .into(),
            partition.name(),
            runtime.name().into(),
        ],
        Engine::Cranelift { partition, runtime } => vec![
            "cranelift".into(),
            "O0".into(),
            partition.name(),
            runtime.name().into(),
        ],
    });
    values
}

fn pairs(c: &Configuration) -> BTreeSet<Pair> {
    let values = values(c);
    (0..values.len())
        .flat_map(|i| {
            let values = &values;
            (i + 1..values.len()).map(move |j| (i, values[i].clone(), j, values[j].clone()))
        })
        .collect()
}

/// A deterministic covering set over *legal* rows, never a fabricated product.
/// Each pair occurring in `matrix` occurs in the result. A rotating mandatory
/// row ensures that each program visits every legal configuration within
/// `matrix.len()` consecutive seeds, even if greedy tie breaking omits it.
/// Launch inputs are retained; they are observations, not compiler dimensions.
pub fn pairwise_matrix(matrix: &[Configuration], program: &str, seed: u64) -> Vec<Configuration> {
    if matrix.is_empty() {
        return vec![];
    }
    let rows: Vec<_> = matrix.iter().map(pairs).collect();
    let mut uncovered: BTreeSet<_> = rows.iter().flatten().cloned().collect();
    let digest = hirsel_syntax::ContentHash::of_bytes(program.as_bytes());
    let offset = u64::from_le_bytes(digest.0[..8].try_into().unwrap());
    let start = ((seed % matrix.len() as u64 + offset % matrix.len() as u64) % matrix.len() as u64)
        as usize;
    let mut selected = BTreeSet::from([start]);
    uncovered.retain(|pair| !rows[start].contains(pair));
    while !uncovered.is_empty() {
        let next = (0..matrix.len())
            .map(|i| (start + i) % matrix.len())
            .filter(|i| !selected.contains(i))
            .max_by_key(|i| {
                (
                    rows[*i].intersection(&uncovered).count(),
                    std::cmp::Reverse((*i + matrix.len() - start) % matrix.len()),
                )
            })
            .expect("the input matrix covers its own pairs");
        uncovered.retain(|pair| !rows[next].contains(pair));
        selected.insert(next);
    }
    selected.into_iter().map(|i| matrix[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::super::Partition;
    use super::*;

    #[test]
    fn every_feasible_pair_is_covered_for_every_rotating_anchor() {
        let matrix = super::super::full_matrix();
        let required: BTreeSet<_> = matrix.iter().flat_map(pairs).collect();
        let mut visited = BTreeSet::new();
        for seed in 0..matrix.len() as u64 {
            let selected = pairwise_matrix(&matrix, "conformance/example.hir", seed);
            assert!(selected.len() < matrix.len());
            assert_eq!(
                selected.iter().flat_map(pairs).collect::<BTreeSet<_>>(),
                required
            );
            assert_eq!(
                selected.iter().map(Configuration::name).collect::<Vec<_>>(),
                pairwise_matrix(&matrix, "conformance/example.hir", seed)
                    .iter()
                    .map(Configuration::name)
                    .collect::<Vec<_>>()
            );
            visited.extend(selected.iter().map(Configuration::name));
        }
        assert_eq!(visited, matrix.iter().map(Configuration::name).collect());
    }

    #[test]
    fn filtered_and_topology_matrices_cover_only_their_feasible_pairs() {
        let mut matrices = vec![
            super::super::smoke_matrix(),
            super::super::topology_matrix(20),
            vec![],
        ];
        let mut matrix = super::super::full_matrix();
        matrix.retain(|c| {
            matches!(
                c.engine,
                Engine::Native {
                    partition: Partition::Whole,
                    ..
                }
            )
        });
        matrices.push(matrix);
        for matrix in matrices {
            let selected = pairwise_matrix(&matrix, "filtered", 17);
            assert_eq!(
                selected.iter().flat_map(pairs).collect::<BTreeSet<_>>(),
                matrix.iter().flat_map(pairs).collect()
            );
            assert!(
                selected
                    .iter()
                    .all(|c| matrix.iter().any(|m| m.name() == c.name()))
            );
        }
    }
}

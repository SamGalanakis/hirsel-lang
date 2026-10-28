//! strsim dacc84c0dc61eff0ee0ff66962bcf2e17018ad26, src/lib.rs:985-1026.
//! The oracle deliberately uses a full matrix, independently of std's one row.
use crate::source_std::tir;
use hirsel_engine::*;
use hirsel_interp::{Integer, Limits, Outcome, Value, library::Library};
use hirsel_types::Primitive;
use std::{fmt::Write, path::PathBuf};

const SCALAR_LIMIT: u64 = u64::MAX;
const CELL_LIMIT: u64 = u64::MAX - 1;

fn oracle(a: &str, b: &str) -> u64 {
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    let mut matrix = vec![vec![0u64; b.len() + 1]; a.len() + 1];
    for (i, row) in matrix.iter_mut().enumerate() {
        row[0] = u64::try_from(i).unwrap();
    }
    for (j, cell) in matrix[0].iter_mut().enumerate() {
        *cell = u64::try_from(j).unwrap();
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + u64::from(a[i - 1] != b[j - 1]));
        }
    }
    matrix[a.len()][b.len()]
}

type Row<'a> = (&'a str, &'a str, u64, u64, u64);

fn execute(rows: &[Row<'_>]) -> Vec<u64> {
    with_compiler_stack(|| {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let source = r#"
pub fn bounded(a: String, b: String, cells: u64, scalars: u64) -> u64 uses {} {
  match std@1/edit_distance.levenshtein(a, b, cells, scalars) {
    Ok(n) => n,
    Error(ScalarLimit()) => 18446744073709551615,
    Error(CellLimit()) => 18446744073709551614,
  }
}
pub fn main() -> () { let _ = bounded("", "", 0, 0) }
"#;
        let prepared = tir(&fixture, Some(source), Some("bounded")).unwrap();
        let runtime = &prepared.runtime;
        let sources = &prepared.sources;
        let config = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let integer =
            |value| Value::Integer(Integer::new(Primitive::U64, u128::from(value)).unwrap());
        let mut steps = Vec::new();
        for &(a, b, cells, scalars, expected) in rows {
            let mut library = Library::new(&config, Default::default()).unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut prepared.provider(),
                sources,
                &config,
                vec![
                    Value::String(a.into()),
                    Value::String(b.into()),
                    integer(cells),
                    integer(scalars),
                ],
                Limits::default(),
                &mut library,
                runtime,
            )
            .unwrap();
            match execution.outcome {
                Outcome::Returned { value, .. } => assert_eq!(
                    value,
                    integer(expected),
                    "a scalars={}, b scalars={}, cells={cells}, scalars={scalars}",
                    a.chars().count(),
                    b.chars().count()
                ),
                other => panic!("unexpected outcome: {other:?}"),
            }
            steps.push(execution.steps);
        }
        eprintln!(
            "edit-distance-7: {} comparisons, maximum {} interpreter steps",
            rows.len(),
            steps.iter().max().unwrap()
        );
        steps
    })
}

#[test]
fn fixed_vectors() {
    let vectors = [
        ("", "", 0),
        ("levenshtein", "levenshtein", 0),
        ("kitten", "sitting", 3),
        ("hello, world", "bye, world", 5),
        ("öঙ香", "abc", 3),
        ("abc", "öঙ香", 3),
        (
            "The quick brown fox jumped over the angry dog.",
            "Lorem ipsum dolor sit amet, dicta latine an eam.",
            37,
        ),
        ("", "sitting", 7),
        ("kitten", "", 6),
        ("é", "é", 2),
        ("👩‍💻", "", 3),
        ("🇩🇪", "", 2),
        ("👍🏽", "👍", 1),
        ("ab", "ba", 2),
        ("a b", "a\tb", 1),
        ("\0", "", 1),
    ];
    let mut rows = Vec::new();
    for (a, b, expected) in vectors {
        assert_eq!(oracle(a, b), expected);
        let m = u64::try_from(a.chars().count()).unwrap();
        let n = u64::try_from(b.chars().count()).unwrap();
        rows.push((a, b, m * n, m + n, expected));
        rows.push((b, a, m * n, m + n, expected));
    }
    assert_eq!(execute(&rows).len(), 32);
}

#[test]
fn budget_boundaries_and_precedence() {
    let rows = [
        ("", "", 0, 0, 0),
        ("", "💚", 0, 1, 1),
        ("💚", "", 0, 1, 1),
        ("", "💚", 0, 0, SCALAR_LIMIT),
        ("💚", "", 0, 0, SCALAR_LIMIT),
        ("kitten", "sitting", 42, 13, 3),
        ("kitten", "sitting", 41, 13, CELL_LIMIT),
        ("kitten", "sitting", 42, 12, SCALAR_LIMIT),
        ("kitten", "sitting", 0, 12, SCALAR_LIMIT),
        ("a", "b", 0, 2, CELL_LIMIT),
        ("a", "a", 0, 2, CELL_LIMIT),
        ("💚", "💚", 1, 2, 0),
        ("💚", "💚", 1, 1, SCALAR_LIMIT),
        ("💚", "é", u64::MAX, u64::MAX, 1),
    ];
    assert_eq!(execute(&rows).len(), 14);
}

#[test]
fn exhaustive_full_matrix_oracle() {
    let alphabet = ["a", "é", "💚"];
    let mut words = vec![String::new()];
    words.extend(alphabet.iter().map(|s| (*s).to_owned()));
    for a in alphabet {
        for b in alphabet {
            words.push(format!("{a}{b}"));
        }
    }
    let fixture =
        include_str!("../../../../conformance/SEM/SEM-3-edit-distance-7-exhaustive-run.hir");
    let compact: String = fixture.chars().filter(|c| !c.is_whitespace()).collect();
    let mut rows = Vec::new();
    for a in &words {
        for b in &words {
            let expected = oracle(a, b);
            let m = u64::try_from(a.chars().count()).unwrap();
            let n = u64::try_from(b.chars().count()).unwrap();
            let mut assertion = String::new();
            // Every native/corpus assertion is tied to this independent oracle.
            writeln!(
                assertion,
                "  failure.check(edit_distance.levenshtein(\"{a}\", \"{b}\", {}, {}) == Ok({expected}), TextInvariant())",
                m * n,
                m + n,
            ).unwrap();
            let assertion: String = assertion.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                compact.contains(&assertion),
                "missing oracle row: {assertion}"
            );
            rows.push((a.as_str(), b.as_str(), m * n, m + n, expected));
        }
    }
    assert_eq!(rows.len(), 169);
    assert_eq!(fixture.matches("failure.check(").count(), rows.len());
    assert_eq!(execute(&rows).len(), 169);
}

#[test]
fn oversized_refusal_and_asymmetric_row() {
    let huge = "💚".repeat(100_000);
    let long = "é".repeat(512);
    let cell_input = "💚".repeat(1_000);
    let rows = [
        (huge.as_str(), "a", u64::MAX, 4, SCALAR_LIMIT),
        ("a", huge.as_str(), u64::MAX, 4, SCALAR_LIMIT),
        (
            cell_input.as_str(),
            cell_input.as_str(),
            0,
            2_000,
            CELL_LIMIT,
        ),
        ("é", long.as_str(), 512, 513, 511),
        (long.as_str(), "é", 512, 513, 511),
    ];
    let steps = execute(&rows);
    // Refusal after max_scalars+1 scalars cannot scan/store the 100k tail.
    assert!(steps[0] < 2_000 && steps[1] < 2_000, "{steps:?}");
}

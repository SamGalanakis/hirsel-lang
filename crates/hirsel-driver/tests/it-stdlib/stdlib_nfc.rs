use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Limits, Outcome, Value, library::Library};
use hirsel_tir::{DefinitionRef, MaterializedRef};
use hirsel_types::BuiltinType;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

// Freeze already validated query results for repeated oracle calls. This does
// not implement normalization: every case executes the ordinary std source.
struct CorpusProvider {
    engine: QueryEngine,
    definitions: BTreeMap<DefPath, DefinitionRef>,
    constants: BTreeMap<DefPath, MaterializedRef>,
}
impl TirProvider for CorpusProvider {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        if let Some(value) = self.definitions.get(path) {
            return Ok(value.clone());
        }
        let value = self.engine.definition(path)?;
        self.definitions.insert(path.clone(), value.clone());
        Ok(value)
    }
    fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        if let Some(value) = self.constants.get(path) {
            return Ok(value.clone());
        }
        let value = self.engine.constant(path)?;
        self.constants.insert(path.clone(), value.clone());
        Ok(value)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        self.engine.constant_definitions()
    }
}
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nfc/main.hir")
}
fn text(column: &str) -> String {
    column
        .split_whitespace()
        .map(|cp| char::from_u32(u32::from_str_radix(cp, 16).unwrap()).unwrap())
        .collect()
}

const NORMALIZATION_SHARDS: usize = 64;

fn normalization_invariants(shard: usize) {
    with_compiler_stack(|| {
        assert!(shard < NORMALIZATION_SHARDS);
        let mut vectors = vec![];
        let mut catalogue = 0;
        let mut rows = 0;
        let mut part1 = false;
        let mut listed = BTreeSet::new();
        for (line_no, line) in include_str!("../../../../stdlib/unicode16/NormalizationTest.txt")
            .lines()
            .enumerate()
        {
            let line = line.split('#').next().unwrap().trim();
            if line.starts_with('@') {
                part1 = line.starts_with("@Part1");
                continue;
            }
            if line.is_empty() {
                continue;
            }
            let columns: Vec<_> = line.split(';').take(5).map(text).collect();
            assert_eq!(columns.len(), 5);
            if part1 {
                listed.extend(columns[0].chars().map(u32::from));
            }
            // UAX #15 conformance table, in NFC/NFD/NFKC/NFKD order.
            for i in 0..5 {
                if catalogue % NORMALIZATION_SHARDS == shard {
                    vectors.push((
                        line_no + 1,
                        i + 1,
                        columns[i].clone(),
                        vec![
                            columns[if i < 3 { 1 } else { 3 }].clone(),
                            columns[if i < 3 { 2 } else { 4 }].clone(),
                            columns[3].clone(),
                            columns[4].clone(),
                        ],
                    ));
                }
                catalogue += 1;
            }
            rows += 1;
        }
        assert_eq!(rows, 19965);
        assert_eq!(catalogue, 99825);
        // NormalizationTest conformance clause 2: every assigned scalar not
        // listed in Part 1 remains itself under all four forms. Expand UCD range rows.
        let mut range_start = None;
        let mut identities = 0;
        for line in include_str!("../../../../stdlib/unicode16/UnicodeData.txt").lines() {
            let fields: Vec<_> = line.split(';').collect();
            let cp = u32::from_str_radix(fields[0], 16).unwrap();
            if fields[1].ends_with(", First>") {
                range_start = Some(cp);
                continue;
            }
            let start = if fields[1].ends_with(", Last>") {
                range_start.take().unwrap()
            } else {
                cp
            };
            for value in start..=cp {
                if let Some(ch) = char::from_u32(value)
                    && !listed.contains(&value)
                {
                    if catalogue % NORMALIZATION_SHARDS == shard {
                        let text = ch.to_string();
                        vectors.push((
                            value as usize,
                            0,
                            text.clone(),
                            vec![text.clone(), text.clone(), text.clone(), text],
                        ));
                    }
                    catalogue += 1;
                    identities += 1;
                }
            }
        }
        assert_eq!(identities, 275446);
        eprintln!(
            "Unicode16: {rows} rows / 399,300 four-form comparisons + {identities} unlisted assigned scalar identities"
        );
        // The deterministic catalogue includes every official column and
        // every unlisted assigned scalar, once across all bounded actions.
        assert_eq!(catalogue, 375271);
        let expected =
            375271 / NORMALIZATION_SHARDS + usize::from(shard < 375271 % NORMALIZATION_SHARDS);
        assert_eq!(vectors.len(), expected);
        let jobs = hirsel_guard::jobs().min(4);
        let checked: usize = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..jobs)
                .map(|worker| {
                    let vectors = &vectors;
                    scope.spawn(move || {
                        with_compiler_stack(|| run_vectors(vectors, worker, jobs, "batch"))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .sum()
        });
        assert_eq!(checked, expected);
        eprintln!(
            "Unicode16 shard {shard}: {} comparisons across all four forms",
            checked * 4
        );
    });
}

macro_rules! normalization_shards {
    ($($name:ident: $index:literal),* $(,)?) => {$(
        #[test]
        #[cfg_attr(debug_assertions, ignore = "full Unicode oracle runs in release")]
        fn $name() {
            normalization_invariants($index);
        }
    )*};
}

normalization_shards! {
    every_official_unicode16_normalization_shard_0: 0,
    every_official_unicode16_normalization_shard_1: 1,
    every_official_unicode16_normalization_shard_2: 2,
    every_official_unicode16_normalization_shard_3: 3,
    every_official_unicode16_normalization_shard_4: 4,
    every_official_unicode16_normalization_shard_5: 5,
    every_official_unicode16_normalization_shard_6: 6,
    every_official_unicode16_normalization_shard_7: 7,
    every_official_unicode16_normalization_shard_8: 8,
    every_official_unicode16_normalization_shard_9: 9,
    every_official_unicode16_normalization_shard_10: 10,
    every_official_unicode16_normalization_shard_11: 11,
    every_official_unicode16_normalization_shard_12: 12,
    every_official_unicode16_normalization_shard_13: 13,
    every_official_unicode16_normalization_shard_14: 14,
    every_official_unicode16_normalization_shard_15: 15,
    every_official_unicode16_normalization_shard_16: 16,
    every_official_unicode16_normalization_shard_17: 17,
    every_official_unicode16_normalization_shard_18: 18,
    every_official_unicode16_normalization_shard_19: 19,
    every_official_unicode16_normalization_shard_20: 20,
    every_official_unicode16_normalization_shard_21: 21,
    every_official_unicode16_normalization_shard_22: 22,
    every_official_unicode16_normalization_shard_23: 23,
    every_official_unicode16_normalization_shard_24: 24,
    every_official_unicode16_normalization_shard_25: 25,
    every_official_unicode16_normalization_shard_26: 26,
    every_official_unicode16_normalization_shard_27: 27,
    every_official_unicode16_normalization_shard_28: 28,
    every_official_unicode16_normalization_shard_29: 29,
    every_official_unicode16_normalization_shard_30: 30,
    every_official_unicode16_normalization_shard_31: 31,
    every_official_unicode16_normalization_shard_32: 32,
    every_official_unicode16_normalization_shard_33: 33,
    every_official_unicode16_normalization_shard_34: 34,
    every_official_unicode16_normalization_shard_35: 35,
    every_official_unicode16_normalization_shard_36: 36,
    every_official_unicode16_normalization_shard_37: 37,
    every_official_unicode16_normalization_shard_38: 38,
    every_official_unicode16_normalization_shard_39: 39,
    every_official_unicode16_normalization_shard_40: 40,
    every_official_unicode16_normalization_shard_41: 41,
    every_official_unicode16_normalization_shard_42: 42,
    every_official_unicode16_normalization_shard_43: 43,
    every_official_unicode16_normalization_shard_44: 44,
    every_official_unicode16_normalization_shard_45: 45,
    every_official_unicode16_normalization_shard_46: 46,
    every_official_unicode16_normalization_shard_47: 47,
    every_official_unicode16_normalization_shard_48: 48,
    every_official_unicode16_normalization_shard_49: 49,
    every_official_unicode16_normalization_shard_50: 50,
    every_official_unicode16_normalization_shard_51: 51,
    every_official_unicode16_normalization_shard_52: 52,
    every_official_unicode16_normalization_shard_53: 53,
    every_official_unicode16_normalization_shard_54: 54,
    every_official_unicode16_normalization_shard_55: 55,
    every_official_unicode16_normalization_shard_56: 56,
    every_official_unicode16_normalization_shard_57: 57,
    every_official_unicode16_normalization_shard_58: 58,
    every_official_unicode16_normalization_shard_59: 59,
    every_official_unicode16_normalization_shard_60: 60,
    every_official_unicode16_normalization_shard_61: 61,
    every_official_unicode16_normalization_shard_62: 62,
    every_official_unicode16_normalization_shard_63: 63,
}

type OracleVector = (usize, usize, String, Vec<String>);
fn run_vectors(vectors: &[OracleVector], worker: usize, jobs: usize, entry_name: &str) -> usize {
    let mut loaded = load_program(&fixture()).unwrap();
    loaded.check_for_execution().unwrap();
    let runtime = loaded.engine.runtime_types().unwrap();
    let sources = loaded.engine.sources.clone();
    let mut entry = loaded.entry;
    entry.0.name = entry_name.into();
    let configuration = RunConfiguration {
        entry,
        hash_seed: 0,
        inputs: vec![],
    };
    let mut provider = CorpusProvider {
        engine: loaded.engine,
        definitions: BTreeMap::new(),
        constants: BTreeMap::new(),
    };
    let mut checked = 0;
    for (index, batch) in vectors.chunks(16).enumerate() {
        if index % jobs != worker {
            continue;
        }
        let arguments = vec![Value::Sequence {
            kind: BuiltinType::List,
            elements: batch
                .iter()
                .map(|(_, _, input, _)| Some(Value::String(input.clone())))
                .collect(),
            arena: None,
        }];
        let mut library = Library::new(&configuration, Default::default()).unwrap();
        let execution = hirsel_interp::run_with_runtime(
            &mut provider,
            &sources,
            &configuration,
            arguments,
            Limits {
                steps: Some(10_000_000),
                call_depth: 256,
            },
            &mut library,
            &runtime,
        )
        .unwrap();
        let Outcome::Returned {
            value: Value::Sequence { elements, .. },
            ..
        } = execution.outcome
        else {
            panic!(
                "oracle batch at line {}: {:?}",
                batch[0].0, execution.outcome
            );
        };
        let width = batch[0].3.len();
        assert_eq!(elements.len(), batch.len() * width);
        checked += batch.len();
        if checked % 16384 == 0 {
            eprintln!("Unicode16 worker {worker}: {checked} comparisons");
        }
        for (actual, (line, column, _, expected)) in elements.chunks_exact(width).zip(batch) {
            for (form, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert_eq!(
                    actual,
                    &Some(Value::String(expected.clone())),
                    "NormalizationTest.txt line {line}, c{column}, form {form}"
                );
            }
        }
    }
    checked
}

// The official pinned mapping files are the oracle; Rust's host Unicode version
// is deliberately not involved. Context cases belong in conformance instead.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "full Unicode mapping oracle runs in release"
)]
fn every_official_unicode16_full_case_mapping_executes_std_source() {
    with_compiler_stack(|| {
        let mut lower = BTreeMap::new();
        let mut upper = BTreeMap::new();
        let mut fold = BTreeMap::new();
        for line in include_str!("../../../../stdlib/unicode16/UnicodeData.txt").lines() {
            let f: Vec<_> = line.split(';').collect();
            let cp = u32::from_str_radix(f[0], 16).unwrap();
            if !f[13].is_empty() {
                lower.insert(cp, text(f[13]));
            }
            if !f[12].is_empty() {
                upper.insert(cp, text(f[12]));
            }
        }
        let mut special = 0;
        for line in include_str!("../../../../stdlib/unicode16/SpecialCasing.txt").lines() {
            let f: Vec<_> = line
                .split('#')
                .next()
                .unwrap()
                .split(';')
                .map(str::trim)
                .collect();
            if f.len() >= 5 && f[4].is_empty() {
                let cp = u32::from_str_radix(f[0], 16).unwrap();
                lower.insert(cp, text(f[1]));
                upper.insert(cp, text(f[3]));
                special += 1;
            }
        }
        assert_eq!(special, 103);
        for line in include_str!("../../../../stdlib/unicode16/CaseFolding.txt").lines() {
            let f: Vec<_> = line
                .split('#')
                .next()
                .unwrap()
                .split(';')
                .map(str::trim)
                .collect();
            if f.len() >= 3 && matches!(f[1], "C" | "F") {
                let cp = u32::from_str_radix(f[0], 16).unwrap();
                assert!(fold.insert(cp, text(f[2])).is_none());
            }
        }
        assert_eq!(fold.len(), 1557);
        let scalars: BTreeSet<_> = lower
            .keys()
            .chain(upper.keys())
            .chain(fold.keys())
            .copied()
            .collect();
        let vectors: Vec<_> = scalars
            .into_iter()
            .map(|cp| {
                let input = char::from_u32(cp).unwrap().to_string();
                let expected = [&upper, &lower, &fold]
                    .map(|m| m.get(&cp).unwrap_or(&input).clone())
                    .to_vec();
                (cp as usize, 0, input, expected)
            })
            .collect();
        assert_eq!(vectors.len(), 2981);
        let checked = run_vectors(&vectors, 0, 1, "batch_case");
        assert_eq!(checked, vectors.len());
        // Rust's long growth/ASCII vectors each get a separate invocation,
        // retaining the same step limit as the short mapping batches.
        let mut long_checked = 0;
        for (input, expected) in [
            (
                "İ".repeat(512),
                [
                    "İ".repeat(512),
                    "i\u{0307}".repeat(512),
                    "i\u{0307}".repeat(512),
                ],
            ),
            (
                "A".repeat(511),
                ["A".repeat(511), "a".repeat(511), "a".repeat(511)],
            ),
        ] {
            for (entry, expected) in ["batch_upper", "batch_lower", "batch_fold"]
                .into_iter()
                .zip(expected)
            {
                long_checked += run_vectors(&[(0, 0, input.clone(), vec![expected])], 0, 1, entry);
            }
        }
        assert_eq!(long_checked, 6);
        eprintln!(
            "Unicode16: {} full upper/lower/fold comparisons",
            checked * 3 + long_checked
        );
    });
}

use crate::source_std::load_program_source;
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
                source: None,
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

// ASCII shortcuts are checked against the pinned full Unicode mappings too,
// including unmapped controls and punctuation, and mixed ASCII/Unicode strings.
#[test]
fn ascii_case_shortcuts_match_full_unicode_tables_and_mixed_contexts() {
    with_compiler_stack(|| {
        let mut mappings: [BTreeMap<u32, String>; 3] = Default::default();
        for line in include_str!("../../../../stdlib/unicode16/UnicodeData.txt").lines() {
            let fields: Vec<_> = line.split(';').collect();
            let cp = u32::from_str_radix(fields[0], 16).unwrap();
            for (i, column) in [(0, 12), (1, 13)] {
                if !fields[column].is_empty() {
                    mappings[i].insert(cp, text(fields[column]));
                }
            }
        }
        for line in include_str!("../../../../stdlib/unicode16/SpecialCasing.txt").lines() {
            let fields: Vec<_> = line
                .split('#')
                .next()
                .unwrap()
                .split(';')
                .map(str::trim)
                .collect();
            if fields.len() >= 5 && fields[4].is_empty() {
                let cp = u32::from_str_radix(fields[0], 16).unwrap();
                mappings[0].insert(cp, text(fields[3]));
                mappings[1].insert(cp, text(fields[1]));
            }
        }
        for line in include_str!("../../../../stdlib/unicode16/CaseFolding.txt").lines() {
            let fields: Vec<_> = line
                .split('#')
                .next()
                .unwrap()
                .split(';')
                .map(str::trim)
                .collect();
            if fields.len() >= 3 && matches!(fields[1], "C" | "F") {
                mappings[2].insert(u32::from_str_radix(fields[0], 16).unwrap(), text(fields[2]));
            }
        }
        let ascii: String = (0u8..128).map(char::from).collect();
        let inputs = (0u8..128).map(|b| char::from(b).to_string()).chain([
            String::new(),
            ascii.clone(),
            format!("é{ascii}ßİKﬃ"),
            "Task 9999: BENCHMARK description".into(),
        ]);
        let vectors: Vec<_> = inputs
            .enumerate()
            .map(|(i, input)| {
                let expected = mappings
                    .each_ref()
                    .map(|map| {
                        input
                            .chars()
                            .map(|c| {
                                map.get(&u32::from(c))
                                    .cloned()
                                    .unwrap_or_else(|| c.to_string())
                            })
                            .collect::<String>()
                    })
                    .to_vec();
                (i, 0, input, expected)
            })
            .collect();
        assert_eq!(run_vectors(&vectors, 0, 1, "batch_case"), 132);
    });
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

#[test]
fn case_insensitive_search_and_equality_match_full_casefold_composition() {
    with_compiler_stack(|| {
        let source = std::fs::read_to_string(fixture().with_file_name("search.hir")).unwrap();
        let mut loaded = load_program_source(&fixture(), Some(&source)).unwrap();
        loaded.check_for_execution().unwrap();
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: Limits {
                steps: Some(100_000_000),
                call_depth: 256,
                source: None,
            },
            ..Default::default()
        }));
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry,
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            run.outcome
        );
    });
}

// Independent dense oracle: expand UnicodeData ranges and read its simple-case
// columns directly. No generated Hirsel tables or host Unicode predicates.
// Go src/unicode/letter_test.go:346 (TestLetterOptimizations) compares the public
// predicates with properties; here the pinned UCD is the independent reference.
#[derive(Clone, Copy)]
struct ScalarProperties {
    flags: u8,
    lower: u32,
    upper: u32,
}
fn property_oracle() -> &'static [ScalarProperties] {
    static ORACLE: std::sync::OnceLock<Vec<ScalarProperties>> = std::sync::OnceLock::new();
    ORACLE.get_or_init(|| {
        let mut values: Vec<_> = (0..=0x10ffff)
            .map(|cp| ScalarProperties {
                flags: 0,
                lower: cp,
                upper: cp,
            })
            .collect();
        let mut pending = None;
        for line in include_str!("../../../../stdlib/unicode16/UnicodeData.txt").lines() {
            let f: Vec<_> = line.split(';').collect();
            let cp = u32::from_str_radix(f[0], 16).unwrap();
            if f[1].ends_with(", First>") {
                assert!(pending.replace((cp, f[2])).is_none());
                continue;
            }
            let start = if f[1].ends_with(", Last>") {
                let (start, category) = pending.take().unwrap();
                assert_eq!(category, f[2]);
                start
            } else {
                assert!(pending.is_none());
                cp
            };
            for scalar in start..=cp {
                let v = &mut values[scalar as usize];
                v.flags = if f[2].starts_with('L') {
                    2
                } else if f[2] == "Nd" {
                    4
                } else {
                    0
                };
                if !f[13].is_empty() {
                    v.lower = u32::from_str_radix(f[13], 16).unwrap();
                }
                if !f[12].is_empty() {
                    v.upper = u32::from_str_radix(f[12], 16).unwrap();
                }
            }
        }
        assert!(pending.is_none());
        for line in include_str!("../../../../stdlib/unicode16/PropList.txt").lines() {
            let data = line.split('#').next().unwrap().trim();
            let Some((range, property)) = data.split_once(';') else {
                continue;
            };
            if property.trim() != "White_Space" {
                continue;
            }
            let mut ends = range.trim().split("..");
            let first = u32::from_str_radix(ends.next().unwrap(), 16).unwrap();
            let last = ends
                .next()
                .map(|v| u32::from_str_radix(v, 16).unwrap())
                .unwrap_or(first);
            for cp in first..=last {
                values[cp as usize].flags |= 1;
            }
        }
        assert_eq!(values.iter().filter(|v| v.flags & 1 != 0).count(), 25);
        assert_eq!(values.iter().filter(|v| v.flags & 2 != 0).count(), 141028);
        assert_eq!(values.iter().filter(|v| v.flags & 4 != 0).count(), 760);
        values
    })
}

const PROPERTY_SHARDS: u32 = 64;
fn check_properties(first: u32, last: u32) -> usize {
    let oracle = property_oracle();
    let mut loaded = load_program(&fixture()).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|error| panic!("{error}"));
    let runtime = loaded.engine.runtime_types().unwrap();
    let sources = loaded.engine.sources.clone();
    let mut entry = loaded.entry;
    entry.0.name = "batch_properties".into();
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
    let scalars: Vec<_> = (first..last).filter_map(char::from_u32).collect();
    for batch in scalars.chunks(128) {
        let mut library = Library::new(&configuration, Default::default()).unwrap();
        let execution = hirsel_interp::run_with_runtime(
            &mut provider,
            &sources,
            &configuration,
            vec![Value::String(batch.iter().collect())],
            Limits {
                steps: Some(10_000_000),
                call_depth: 256,
                source: None,
            },
            &mut library,
            &runtime,
        )
        .unwrap();
        let Outcome::Returned {
            value: Value::Bytes(bytes),
            ..
        } = execution.outcome
        else {
            panic!(
                "properties U+{:04X}: {:?}",
                u32::from(batch[0]),
                execution.outcome
            );
        };
        assert_eq!(bytes.len(), batch.len() * 7, "silently dropped a scalar");
        for (actual, c) in bytes.chunks_exact(7).zip(batch) {
            let v = oracle[u32::from(*c) as usize];
            let mut expected = vec![v.flags];
            expected.extend_from_slice(&v.lower.to_be_bytes()[1..]);
            expected.extend_from_slice(&v.upper.to_be_bytes()[1..]);
            assert_eq!(actual, expected, "properties U+{:04X}", u32::from(*c));
        }
    }
    scalars.len()
}

#[test]
fn unicode16_properties_oracle_controls_execute_std_source() {
    with_compiler_stack(|| {
        for (first, last) in [
            (0, 256),
            (0x12f, 0x132),
            (0xd7ff, 0xe001),
            (0x10d3f, 0x10d4b),
            (0x1137f, 0x11384),
            (0x10fffe, 0x110000),
        ] {
            check_properties(first, last);
        }
    });
}

fn check_properties_shard(shard: u32) {
    with_compiler_stack(|| {
        let width = 0x110000 / PROPERTY_SHARDS;
        let first = shard * width;
        let last = first + width;
        let checked = check_properties(first, last);
        let invalid = (first..last)
            .filter(|v| (0xd800..=0xdfff).contains(v))
            .count();
        assert_eq!(checked, width as usize - invalid);
        eprintln!(
            "Unicode16 properties shard {shard}: {checked} scalars, {} comparisons",
            checked * 5
        );
    });
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_0() {
    check_properties_shard(0);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_1() {
    check_properties_shard(1);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_2() {
    check_properties_shard(2);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_3() {
    check_properties_shard(3);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_4() {
    check_properties_shard(4);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_5() {
    check_properties_shard(5);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_6() {
    check_properties_shard(6);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_7() {
    check_properties_shard(7);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_8() {
    check_properties_shard(8);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_9() {
    check_properties_shard(9);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_10() {
    check_properties_shard(10);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_11() {
    check_properties_shard(11);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_12() {
    check_properties_shard(12);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_13() {
    check_properties_shard(13);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_14() {
    check_properties_shard(14);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_15() {
    check_properties_shard(15);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_16() {
    check_properties_shard(16);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_17() {
    check_properties_shard(17);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_18() {
    check_properties_shard(18);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_19() {
    check_properties_shard(19);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_20() {
    check_properties_shard(20);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_21() {
    check_properties_shard(21);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_22() {
    check_properties_shard(22);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_23() {
    check_properties_shard(23);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_24() {
    check_properties_shard(24);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_25() {
    check_properties_shard(25);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_26() {
    check_properties_shard(26);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_27() {
    check_properties_shard(27);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_28() {
    check_properties_shard(28);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_29() {
    check_properties_shard(29);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_30() {
    check_properties_shard(30);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_31() {
    check_properties_shard(31);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_32() {
    check_properties_shard(32);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_33() {
    check_properties_shard(33);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_34() {
    check_properties_shard(34);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_35() {
    check_properties_shard(35);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_36() {
    check_properties_shard(36);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_37() {
    check_properties_shard(37);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_38() {
    check_properties_shard(38);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_39() {
    check_properties_shard(39);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_40() {
    check_properties_shard(40);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_41() {
    check_properties_shard(41);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_42() {
    check_properties_shard(42);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_43() {
    check_properties_shard(43);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_44() {
    check_properties_shard(44);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_45() {
    check_properties_shard(45);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_46() {
    check_properties_shard(46);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_47() {
    check_properties_shard(47);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_48() {
    check_properties_shard(48);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_49() {
    check_properties_shard(49);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_50() {
    check_properties_shard(50);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_51() {
    check_properties_shard(51);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_52() {
    check_properties_shard(52);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_53() {
    check_properties_shard(53);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_54() {
    check_properties_shard(54);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_55() {
    check_properties_shard(55);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_56() {
    check_properties_shard(56);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_57() {
    check_properties_shard(57);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_58() {
    check_properties_shard(58);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_59() {
    check_properties_shard(59);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_60() {
    check_properties_shard(60);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_61() {
    check_properties_shard(61);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_62() {
    check_properties_shard(62);
}

#[test]
#[ignore = "full gate: every scalar's UCD properties and simple case"]
fn full_gate_unicode16_properties_shard_63() {
    check_properties_shard(63);
}

// Port check_exact's varying significant cutoffs and zero extension, together
// with check_exact_one's hard rounding requests. Expected digits are Rust's
// vectors, never formatted by the host. Corpus cases also exercise fstrings.
#[test]
fn rust_flt2dec_all_97_exact_rows_and_their_precision_sweeps() {
    with_compiler_stack(|| {
        let rows: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/flt2dec-exact.json")).unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 97);
        let corpus =
            include_str!("../../../../conformance/SEM/SEM-3-fstring-flt2dec-exact-1-run.hir");
        let helper = corpus.split("pub fn main").next().unwrap();
        // Compile the helper once. Passing vector data as interpreter arguments
        // avoids making the compiler track thousands of large literal rows.
        let source = helper
            .replace("fn exact(t: Test, ", "pub fn probe(")
            .replace("-> () {", "-> bool {")
            .replace(
                "test.check(t, same_digits(d, expected, cutoff))",
                "same_digits(d, expected, cutoff)",
            )
            + "\npub fn main(t: Test) -> Result[(), ()] { Ok(()) }\n";
        let mut loaded = load_program_source(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/SEM/SEM-3-fstring-flt2dec-exact-1-run.hir"),
            Some(&source),
        )
        .unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let runtime = loaded.engine.runtime_types().unwrap();
        let sources = loaded.engine.sources.clone();
        let mut entry = loaded.entry;
        entry.0.name = "probe".into();
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
        let signed = |value: i64| {
            let bits = u32::from_ne_bytes(i32::try_from(value).unwrap().to_ne_bytes());
            Value::Integer(
                hirsel_interp::Integer::new(hirsel_types::Primitive::I32, u128::from(bits))
                    .unwrap(),
            )
        };
        let mut checks = 0;
        for row in rows {
            let digits = row["digits"].as_str().unwrap().as_bytes();
            let exponent = row["k"].as_i64().unwrap();
            let cut = digits.iter().position(|&c| c == b' ');
            let end = cut.unwrap_or(digits.len() - 1);
            let precisions: Vec<_> = if row["kind"] == "one" {
                vec![digits.len()]
            } else {
                (1..end)
                    .chain(if cut.is_some() {
                        end..digits.len() - 1
                    } else {
                        0..0
                    })
                    .collect()
            };
            for precision in precisions {
                let mut expected = if row["kind"] == "one" {
                    digits.to_vec()
                } else {
                    digits[..precision.min(end)].to_vec()
                };
                expected.resize(precision, b'0');
                if precision < end && digits[precision] >= b'5' {
                    let tie_even = precision + 1 < digits.len()
                        && digits[precision - 1] & 1 == 0
                        && digits[precision] == b'5'
                        && digits[precision + 1] == b' ';
                    if !tie_even {
                        let mut carry = true;
                        for digit in expected.iter_mut().rev() {
                            if *digit != b'9' {
                                *digit += 1;
                                carry = false;
                                break;
                            }
                            *digit = b'0';
                        }
                        if carry {
                            expected.insert(0, b'1');
                        }
                    }
                }
                let arguments = vec![
                    Value::Integer(
                        hirsel_interp::Integer::new(
                            hirsel_types::Primitive::U64,
                            u128::from(row["m"].as_u64().unwrap()),
                        )
                        .unwrap(),
                    ),
                    signed(row["e"].as_i64().unwrap()),
                    Value::String(String::from_utf8(expected).unwrap()),
                    signed(exponent - precision as i64),
                ];
                let mut library = Library::new(&configuration, Default::default()).unwrap();
                let execution = hirsel_interp::run_with_runtime(
                    &mut provider,
                    &sources,
                    &configuration,
                    arguments,
                    Limits {
                        steps: Some(10_000_000),
                        call_depth: 256,
                        source: None,
                    },
                    &mut library,
                    &runtime,
                )
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Outcome::Returned {
                            value: Value::Bool(true),
                            ..
                        }
                    ),
                    "flt2dec source line {}, precision {precision}: {:?}",
                    row["line"],
                    execution.outcome
                );
                checks += 1;
            }
        }
        eprintln!("Rust flt2dec: 97 rows, {checks} exact rounding comparisons");
        assert_eq!(checks, 1706);
    });
}

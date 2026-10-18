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

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "full Unicode oracle runs in the scheduled release suite"
)]
fn every_official_unicode16_nfc_normalization_invariant_executes_std_source() {
    with_compiler_stack(|| {
        let mut vectors = vec![];
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
            // UAX #15: NFC(c1)=NFC(c2)=NFC(c3)=c2; NFC(c4)=NFC(c5)=c4.
            for i in 0..5 {
                vectors.push((
                    line_no + 1,
                    i + 1,
                    columns[i].clone(),
                    columns[if i < 3 { 1 } else { 3 }].clone(),
                ));
            }
            rows += 1;
        }
        assert_eq!(rows, 19965);
        assert_eq!(vectors.len(), 99825);
        // NormalizationTest conformance clause 2: every assigned scalar not
        // listed in Part 1 remains itself under NFC. Expand UCD range rows.
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
                    let text = ch.to_string();
                    vectors.push((value as usize, 0, text.clone(), text));
                    identities += 1;
                }
            }
        }
        assert_eq!(identities, 275446);
        eprintln!(
            "Unicode16: {rows} rows / 99,825 NFC comparisons + {identities} unlisted assigned scalar identities"
        );
        let jobs = std::env::var("HIRSEL_JOBS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(1, 4);
        let checked: usize = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..jobs)
                .map(|worker| {
                    let vectors = &vectors;
                    scope.spawn(move || with_compiler_stack(|| run_vectors(vectors, worker, jobs)))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .sum()
        });
        assert_eq!(checked, 375271);
    });
}

type OracleVector = (usize, usize, String, String);
fn run_vectors(vectors: &[OracleVector], worker: usize, jobs: usize) -> usize {
    let mut loaded = load_program(&fixture()).unwrap();
    loaded.check_for_execution().unwrap();
    let runtime = loaded.engine.runtime_types().unwrap();
    let sources = loaded.engine.sources.clone();
    let mut entry = loaded.entry;
    entry.0.name = "batch".into();
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
        assert_eq!(elements.len(), batch.len());
        checked += batch.len();
        if checked % 16384 == 0 {
            eprintln!("Unicode16 worker {worker}: {checked} comparisons");
        }
        for (actual, (line, column, _, expected)) in elements.into_iter().zip(batch) {
            assert_eq!(
                actual,
                Some(Value::String(expected.clone())),
                "NormalizationTest.txt line {line}, c{column}"
            );
        }
    }
    checked
}

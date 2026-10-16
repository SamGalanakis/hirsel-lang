use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Limits, Outcome, Value, library::Library};
use hirsel_tir::{DefinitionRef, MaterializedRef};
use hirsel_types::BuiltinType;
use std::{collections::BTreeMap, path::PathBuf};

// Memoize validated definitions/constants; segmentation executes std source.
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graphemes/main.hir")
}

#[test]
#[ignore = "full gate: official source oracle"]
fn full_gate_every_unicode16_grapheme_break_case_over_full_and_nonzero_origin_slices() {
    with_compiler_stack(|| {
        let mut vectors = vec![];
        for (line_no, line) in include_str!("../../../../stdlib/unicode16/GraphemeBreakTest.txt")
            .lines()
            .enumerate()
        {
            let data = line.split('#').next().unwrap().trim();
            if data.is_empty() {
                continue;
            }
            let mut input = String::new();
            let mut boundaries = vec![];
            for token in data.split_whitespace() {
                match token {
                    "÷" => boundaries.push(input.len()),
                    "×" => {}
                    cp => input.push(char::from_u32(u32::from_str_radix(cp, 16).unwrap()).unwrap()),
                }
            }
            assert_eq!(boundaries.first(), Some(&0));
            assert_eq!(boundaries.last(), Some(&input.len()));
            let expected = boundaries
                .windows(2)
                .map(|b| Value::String(input[b[0]..b[1]].into()))
                .collect::<Vec<_>>();
            vectors.push((line_no + 1, input, expected));
        }
        assert_eq!(vectors.len(), 1093);
        // Empty input has one boundary and zero clusters, unlike the nonempty official cases.
        vectors.push((0, String::new(), vec![]));
        // Adapted from unicode-segmentation tests/test.rs::test_graphemes,
        // ff47b3187adddb14813e6fb2cfc1a079f58cfddc; MIT, see third-party/NOTICES.md.
        for (input, expected) in [
            (" \u{600}\u{600} ", vec![" ", "\u{600}\u{600} "]),
            ("\u{600}  ", vec!["\u{600} ", " "]),
            ("👨\u{200d}👧\u{200d}👦", vec!["👨\u{200d}👧\u{200d}👦"]),
            ("🤸🏾🏾", vec!["🤸🏾🏾"]),
            (
                "a\u{310}e\u{301}o\u{308}\u{332}\r\n",
                vec!["a\u{310}", "e\u{301}", "o\u{308}\u{332}", "\r\n"],
            ),
            ("\n\r\n\r", vec!["\n", "\r\n", "\r"]),
        ] {
            vectors.push((
                0,
                input.into(),
                expected
                    .into_iter()
                    .map(|s| Value::String(s.into()))
                    .collect(),
            ));
        }
        assert_eq!(vectors.len(), 1100);
        let mut loaded = load_program(&fixture()).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let runtime = loaded.engine.runtime_types().unwrap();
        let sources = loaded.engine.sources.clone();
        let mut provider = CorpusProvider {
            engine: loaded.engine,
            definitions: BTreeMap::new(),
            constants: BTreeMap::new(),
        };
        let mut checked = 0;
        for name in ["batch", "sliced"] {
            let mut entry = loaded.entry.clone();
            entry.0.name = name.into();
            let configuration = RunConfiguration {
                entry,
                hash_seed: 0,
                inputs: vec![],
            };
            for batch in vectors.chunks(8) {
                let arguments = vec![Value::Sequence {
                    kind: BuiltinType::List,
                    elements: batch
                        .iter()
                        .map(|(_, s, _)| Some(Value::String(s.clone())))
                        .collect(),
                }];
                let mut library = Library::new(&configuration, Default::default()).unwrap();
                let execution = hirsel_interp::run_with_runtime(
                    &mut provider,
                    &sources,
                    &configuration,
                    arguments,
                    Limits {
                        steps: 10_000_000,
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
                    panic!("{name}: {:?}", execution.outcome);
                };
                assert_eq!(
                    elements.len(),
                    batch.len(),
                    "{name}: silently dropped an input"
                );
                for (actual, (line, _, expected)) in elements.into_iter().zip(batch) {
                    let Some(Value::Sequence {
                        kind: BuiltinType::List,
                        elements,
                    }) = actual
                    else {
                        panic!("{name} line {line}: {actual:?}");
                    };
                    assert_eq!(
                        elements,
                        expected.iter().cloned().map(Some).collect::<Vec<_>>(),
                        "GraphemeBreakTest line {line}, {name}"
                    );
                    checked += 1;
                    if checked % 128 == 0 {
                        eprintln!("Unicode16: {checked}/2200");
                    }
                }
            }
        }
        assert_eq!(checked, 2200);
        eprintln!(
            "Unicode16: all 1093 official cases in both modes plus two empty and twelve reference controls ({checked} comparisons)"
        );
    });
}

#[test]
fn segmentation_tables_regenerate_identically_from_pinned_inputs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        std::process::Command::new("python3")
            .current_dir(root)
            .args(["stdlib/unicode16/generate_graphemes.py", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

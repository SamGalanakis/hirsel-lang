//! Bounded ownership/spill shapes: SmallVec 4999262d src/lib.rs:586-615,674-703,
//! tests/main.rs:85-111 (MIT OR Apache-2.0). Lexical twins: serde_json 8c535a26
//! src/read.rs:521-557, tests/test.rs:1098-1155 (MIT OR Apache-2.0).
//! Hirsel retains owned decoded keys and rejects aliases even in ignored values.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn source_error(error: ToolError) -> ! {
    if let ToolError::Query {
        error: QueryError::Diagnostics(diagnostics),
        sources,
        ..
    } = &error
    {
        for diagnostic in diagnostics {
            eprintln!("{}", diagnostic.render_human(sources));
        }
    }
    panic!("{error}")
}

const SOURCE: &str = r###"
fn validate(data: Bytes) -> Result[(), json.JsonError] uses {} {
  let mut cursor = json.reader(data)
  json.skip(mut cursor)?
  json.finish_reader(mut cursor)
}
fn public_members(data: Bytes) -> Result[(), json.JsonError] uses {} {
  let mut cursor = json.reader(data)
  json.begin_object(mut cursor)?
  let mut first = true
  let mut keys: Set[String] = set.empty()
  while let Some(_) = json.object_next(mut cursor, mut first, mut keys)? {
    json.skip(mut cursor)?
  }
  json.finish_reader(mut cursor)
}
type Label { label: String }
fn label_from_json_reader(mut cursor: json.Reader) -> Result[unique Label, String] uses {} = comptime derive.from_json_reader(shape(Label))
fn label_from_json_bytes(data: Bytes) -> Result[unique Label, String] uses {} = comptime derive.from_json_bytes(shape(Label))
fn owned() -> Result[unique Label, String] uses {} {
  let mut source_buffer: List[u8] = []
  text.append(mut source_buffer, "{\"label\":\"owned\\u0021\",\"opts\":{\"a\":0,\"b\":1,\"c\":2}}")
  label_from_json_bytes(bytes.from_list(take source_buffer))
}
fn exact(t: Test, source: String, expected: Result[(), json.JsonError]) -> () {
  test.equal(t, validate(string.as_bytes(source)), expected)
  // Ordinary public callers retain the same lexical and error contract.
  test.equal(t, public_members(string.as_bytes(source)), expected)
  // Fresh objects remain independent after a previous parse has failed.
  test.equal(t, validate(b"{\"fresh\":0}"), Ok(()))
}
pub fn main(t: Test) -> Result[(), ()] {
  let valid: List[String] = [
    "{}", "{\"a\":0}", "{\"a\":0,\"b\":1}",
    "{\"a\":0,\"b\":1,\"c\":2}", "{\"a\":0,\"b\":1,\"c\":2,\"d\":3}",
    "{\"\":0,\"b\":1,\"c\":2}",
    "{\"é\":0,\"\\uD834\\uDD1E\":1,\"c\":2}",
    "{\"a\":[{}, {\"a\":0,\"b\":1,\"c\":2}],\"b\":{\"a\":0}}"
  ]
  for source in valid {
    exact(t, source, Ok(()))
    match json.parse(copy source) { Ok(_) => (), Error(_) => return Error(()) }
  }
  // Independently frozen byte offsets, including whitespace after commas.
  exact(t, "{\"a\":0,\"a\":1}", Error(DuplicateKey(7)))
  exact(t, "{\"a\":0, \"\\u0061\" ?}", Error(DuplicateKey(8)))
  exact(t, "{\"\\u0061\":0,\"a\":1}", Error(DuplicateKey(12)))
  exact(t, "{\"\":0,\"\":1}", Error(DuplicateKey(6)))
  exact(t, "{\"a\":0,\"b\":1,\"a\":2}", Error(DuplicateKey(13)))
  exact(t, "{\"a\":0,\"b\":1,\"\\u0062\":2}", Error(DuplicateKey(13)))
  exact(t, "{\"a\":0,\"\\u0062\":1,\"b\":2}", Error(DuplicateKey(18)))
  exact(t, "{\"a\":0,\"b\":1,\"c\":2,\"d\":3,\"a\":4}", Error(DuplicateKey(25)))
  exact(t, "{\"a\":0,\"b\":1,\"c\":2,\"d\":3,\"b\":4}", Error(DuplicateKey(25)))
  exact(t, "{\"a\":0,\"b\":1,\"c\":2,\"d\":3,\"\\u0063\":4}", Error(DuplicateKey(25)))
  exact(t, "{\"a\":0,\"b\":1,\"c\":2,\"d\":3,\"d\":4}", Error(DuplicateKey(25)))
  exact(t, "{\"a\":0,\"b\":1,\"c\":2,\"d\":3,\"a\" ?}", Error(DuplicateKey(25)))
  exact(t, "{\"a\":0,\"b\":1,\"c\":2,\"d\":3,\"c\":?}", Error(DuplicateKey(25)))
  exact(t, "{\"a\":0,\"a\" ?}", Error(DuplicateKey(7)))
  exact(t, "{\"a\":0,\"b\" ?}", Error(Syntax(11)))
  exact(t, "{\"a\":0,\"b\":?}", Error(Syntax(11)))
  exact(t, "{\"a\":0,}", Error(Syntax(7)))
  exact(t, "{\"a\":0 \"b\":1}", Error(Syntax(7)))
  exact(t, "{\"a\":0,\"\\q\":1}", Error(Syntax(9)))
  exact(t, "{\"a\":0,\"\n\":1}", Error(Syntax(8)))
  exact(t, "{\"a\":0,\"\u{1f}\":1}", Error(Syntax(8)))
  exact(t, "{\"\\uD800\":0}", Error(Syntax(8)))
  exact(t, "{\"\\uDC00\":0}", Error(Syntax(4)))
  exact(t, "{\"\\uD83C\\uFFFF\":0}", Error(Syntax(10)))
  exact(t, "{\"a\":{\"b\":0,\"b\" ?}}", Error(DuplicateKey(12)))
  exact(t, "{\"a\":[{\"b\":0,\"\\u0062\":?}]}", Error(DuplicateKey(13)))
  // Root derived slots and nested skip have distinct responsibilities.
  test.equal(t, label_from_json_bytes(b"{\"label\":\"x\",\"a\":0,\"b\":1,\"c\":2,\"a\":3}"), Error("not JSON"))
  test.equal(t, label_from_json_bytes(b"{\"label\":\"x\",\"opts\":{\"a\":0,\"b\":1,\"c\":2,\"a\":3}}"), Error("not JSON"))
  let Ok(value) = owned() else { return Error(()) }
  test.equal(t, value.label, "owned!")
  // Seventeen keys keep expected hashed behavior; duplicate all storage classes.
  let mut wide: List[u8] = [123]
  for i in 0u64..17 {
    if i != 0 { text.append(mut wide, ",") }
    text.append(mut wide, f"\"k{i}\":0")
  }
  let size = list.len(wide)
  list.push(mut wide, 125)
  test.equal(t, validate(bytes.from_list(copy wide)), Ok(()))
  for key in ["k0", "k1", "k2", "k16"] {
    let mut duplicate = copy wide
    duplicate[size] = 44
    text.append(mut duplicate, f"\"{key}\":0}}")
    test.equal(t, validate(bytes.from_list(take duplicate)), Error(DuplicateKey(size + 1)))
  }
  // The 128th container is accepted; the 129th fails at its opening byte.
  let mut deep: List[u8] = []
  for _ in 0u64..128 { text.append(mut deep, "[") }
  for _ in 0u64..128 { text.append(mut deep, "]") }
  test.equal(t, validate(bytes.from_list(copy deep)), Ok(()))
  let mut deeper: List[u8] = [91]
  bytes.append(mut deeper, bytes.from_list(take deep))
  list.push(mut deeper, 93)
  test.equal(t, validate(bytes.from_list(take deeper)), Error(Nesting(128)))
  let mut objects: List[u8] = []
  for _ in 0u64..128 { text.append(mut objects, "{\"a\":") }
  text.append(mut objects, "0")
  for _ in 0u64..128 { text.append(mut objects, "}") }
  test.equal(t, validate(bytes.from_list(copy objects)), Ok(()))
  let mut more: List[u8] = []
  text.append(mut more, "{\"a\":")
  bytes.append(mut more, bytes.from_list(take objects))
  text.append(mut more, "}")
  test.equal(t, validate(bytes.from_list(take more)), Error(Nesting(640)))
  Ok(())
}
"###;

#[test]
fn json_skip_small_keys_32_four_engines() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-json-corpus-run.hir");
        let mut loaded =
            load_program_source(&path, Some(SOURCE)).unwrap_or_else(|error| source_error(error));
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| source_error(error));
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(5_000_000),
                call_depth: 1024,
                source: None,
            },
            ..Default::default()
        }));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        eprintln!(
            "json_skip_small_keys_32 reference outcome: {:?}; observation: {:#?}",
            reference.outcome,
            native::Observation::of_reference(reference.observation.as_ref().unwrap())
        );
        assert!(matches!(
            reference.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ));
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = native::Validation::EveryStage;
        let configs = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let scratch = tempfile::Builder::new()
            .prefix("json-skip-small-keys-32-")
            .tempdir()
            .unwrap();
        let tools =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configs).unwrap();
        let outcomes = native::differential(
            &validated,
            &loaded.entry,
            &expected,
            &configs,
            &tools,
            scratch.path(),
        );
        assert_eq!(outcomes.len(), 3);
        for (config, outcome) in outcomes {
            eprintln!("json_skip_small_keys_32 {config}: {outcome:#?}; expected {expected:#?}");
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
        eprintln!(
            "json_skip_small_keys_32: 1 selected/executed body; reference + checked H + release LLVM + Cranelift"
        );
    });
}

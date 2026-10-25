//! Fixed origins survive nested iteration and physical pattern bindings.
//! The transition/oracle regression shape follows rustc_mir_dataflow's
//! framework/tests.rs:245–300 at 19833af7224424e8b53e288097d56b6af43d5bcf
//! (MIT, The Rust Project Contributors; license in third-party/NOTICES.md).
//! Source loops and independent scalar/vector answers replace MIR cursor seeks.
use crate::rc_support::*;

const ORIGINAL: &str =
    include_str!("../../../../conformance/PROV/PROV-4-nested-loop-field-element-run.hir");

// Stable aliases also feed the map-loop deep copies. Check owned output and
// early-error cleanup, including H's injected failures, with nonempty inputs.
const MAP: &str = r#"
type V = Text(String) | Num(u64)
fn texts(fields: Map[String, V]) -> Result[Map[String, String], u64] {
  let mut entries: Map[String, String] = map.empty()
  for (key, value) in fields {
    match value {
      Text(s) => map.insert(mut entries, unique key, unique s),
      Num(k) => return Error(k),
    }
  }
  Ok(take entries)
}
pub fn main() -> Result[(), u64] {
  if map.len(texts(map.empty())?) != 0 { return Error(1) }
  let mut input: Map[String, V] = map.empty()
  map.insert(mut input, "a", Text("ab"))
  map.insert(mut input, "b", Text("c"))
  let saved = texts(input)?
  if map.len(saved) != 2 || map.get(saved, "a") != Some("ab") || map.get(saved, "b") != Some("c") { return Error(2) }
  map.insert(mut input, "bad", Num(7))
  match texts(input) { Error(k) => { if k != 7 { return Error(3) } }, Ok(_) => return Error(4) }
  Ok(())
}
"#;

const EXITS: &str = r#"
fn exits(issues: List[Issue], mode: u64) -> u64 {
  let mut n: u64 = 0
  'outer: for issue in issues {
    for segment in issue.path {
      match segment {
        Field(name) => { n += string.byte_len(name) },
        Index(index) => { if index == 0 { continue }; n += index },
      }
      if mode == 1 { break }
      if mode == 2 { break 'outer }
      if mode == 3 { return n }
      if let Field(name) = segment { n += string.byte_len(name) }
    }
    n += 100
  }
  n
}
fn verify(issues: List[Issue], expected: List[u8], bytes: u64) -> bool {
  let first = codes(issues)
  let second = codes(issues)
  first == expected && second == expected && fields(issues) == bytes
}
"#;

fn bounded() -> String {
    use std::fmt::Write;
    let (declarations, _) = ORIGINAL.split_once("pub fn main").unwrap();
    let mut source = format!("{declarations}{EXITS}\npub fn main() -> Result[(), u64] {{\n");
    let mut vectors = 0;
    for outer in [0, 1, 3] {
        for inner in [0, 1, 3] {
            for offset in [0, 1] {
                let mut expected = Vec::new();
                let mut fields = 0;
                let rows: Vec<String> = (0..outer)
                    .map(|i| {
                        let segments: Vec<String> = (0..inner)
                            .map(|j| {
                                if (i + j + offset) % 2 == 0 {
                                    expected.push("1u8");
                                    fields += 2;
                                    "Field(\"ab\")".into()
                                } else {
                                    expected.push("2u8");
                                    format!("Index({j})")
                                }
                            })
                            .collect();
                        format!("Issue(path: [{}])", segments.join(","))
                    })
                    .collect();
                vectors += 1;
                writeln!(
                    source,
                    "if !verify([{}], [{}], {fields}) {{ return Error({vectors}) }}",
                    rows.join(","),
                    expected.join(",")
                )
                .unwrap();
            }
        }
    }
    assert_eq!(vectors, 18);
    source.push_str(r#"
  let rows = [Issue(path: []), Issue(path: [Field("ab"), Index(0), Index(3)]), Issue(path: [Index(4), Field("c")])]
  if exits(rows, 0) != 313 || exits(rows, 1) != 306 || exits(rows, 2) != 102 || exits(rows, 3) != 102 { return Error(19) }
  let mut owned = [Issue(path: [Field("ab")])]
  let n = fields(owned)
  list.push(mut owned, Issue(path: [Index(0)]))
  if n != 2 || list.len(owned) != 2 { return Error(20) }
  Ok(())
}
"#);
    source
}

#[test]
fn reference_checked_heap_llvm_o0_o2_cranelift() {
    use hirsel_driver::native::*;
    use hirsel_engine::*;
    with_compiler_stack(|| {
        let options = Options::new(target());
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Default,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let root =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-nested-loan-18");
        std::fs::create_dir_all(&root).unwrap();
        let tools = Toolchain::for_matrix(&root.join("runtime"), &configurations).unwrap();
        for (name, source, output) in [
            ("original", ORIGINAL.to_string(), b"ok\n".as_slice()),
            ("bounded-18-vectors", bounded(), b"".as_slice()),
            ("map-loop-cleanup", MAP.to_string(), b"".as_slice()),
        ] {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("nested.hir", source).unwrap();
            let diagnostics = check_snapshot(&mut engine, file, &context).unwrap();
            assert!(diagnostics.is_empty(), "{name}: {diagnostics:?}");
            let entry = entry_path(&context);
            let launch = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference =
                run_observed_snapshot(&mut engine, &launch, Default::default()).unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert_eq!(reference.output, output, "{name}");
            let checked = run_observed_checked(&mut engine, &launch, Default::default()).unwrap();
            assert_eq!(
                Observation::of_reference(checked.observation.as_ref().unwrap()),
                reference,
                "{name}: checked interpretation"
            );
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = build(&validated, &options).unwrap();
            let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
            assert_eq!(heap.observation, reference, "{name}: checked heap");
            assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
            let directory = root.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let outcomes = differential(
                &validated,
                &entry,
                &reference,
                &configurations,
                &tools,
                &directory,
            );
            assert_eq!(outcomes.len(), 4);
            for (configuration, outcome) in outcomes {
                assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
                eprintln!("{name}: {configuration}: agrees");
            }
        }
    });
}

#[test]
fn outer_mutation_and_escaped_view_are_rejected() {
    let (declarations, _) = ORIGINAL.split_once("fn codes").unwrap();
    for body in [
        r#"fn bad(mut issues: List[Issue]) -> u64 {
 let mut n: u64 = 0
 for issue in issues { for segment in issue.path {
  if let Field(name) = segment {
   list.push(mut issues, Issue(path: []))
   n += string.byte_len(name)
  }
 } }
 n
}"#,
        r#"fn bad(issues: List[Issue]) -> String {
 for issue in issues { for segment in issue.path {
  if let Field(name) = segment { return name }
 } }
 ""
}"#,
    ] {
        let error = try_compile(&format!("{declarations}{body}\npub fn main() {{}}"))
            .err()
            .expect("live nested views must protect their outer owner");
        assert!(
            error.contains("diagnostic:") && (error.contains("MODE-1") || error.contains("OWN-3")),
            "{error}"
        );
    }
}
const TYPES: &str =
    "type Segment = Field(String) | Index(u64)\ntype Issue { path: List[Segment] }\n";

fn check(body: &str) {
    let (_, lowered) = lower_valid(&format!("{TYPES}{body}\npub fn main() {{}}"));
    assert!(!lowered.units.is_empty());
}

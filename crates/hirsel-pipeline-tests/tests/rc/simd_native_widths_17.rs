//! MEM-8 through checked legalization, H, LLVM O3 and Cranelift.
//!
//! Boundary vectors ported from the independent interpreter oracle
//! `simd_add_checks_every_integer_width_and_each_lane` (a6419e68), whose
//! Rust boundary pattern cites rust-lang/rust a7c37f9f0ee5,
//! library/core/src/num/{int_macros.rs:586-587,uint_macros.rs:944-947}.
//! Deviations: 2/4 lanes instead of 3, two source operations, and canonical
//! SER-2 input envelopes because source SIMD has no lane setter. Neither the
//! interpreter's arithmetic kernel nor legalization generates expected sums.
use crate::rc_support::target;
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;
use std::fmt::Write as _;

#[derive(Clone, Copy)]
struct Width {
    name: &'static str,
    bytes: usize,
    tag: u8,
    min: i128,
    max: u128,
}

const WIDTHS: [Width; 10] = [
    Width {
        name: "u8",
        bytes: 1,
        tag: 1,
        min: 0,
        max: u8::MAX as u128,
    },
    Width {
        name: "u16",
        bytes: 2,
        tag: 2,
        min: 0,
        max: u16::MAX as u128,
    },
    Width {
        name: "u32",
        bytes: 4,
        tag: 3,
        min: 0,
        max: u32::MAX as u128,
    },
    Width {
        name: "u64",
        bytes: 8,
        tag: 4,
        min: 0,
        max: u64::MAX as u128,
    },
    Width {
        name: "u128",
        bytes: 16,
        tag: 5,
        min: 0,
        max: u128::MAX,
    },
    Width {
        name: "i8",
        bytes: 1,
        tag: 6,
        min: i8::MIN as i128,
        max: i8::MAX as u128,
    },
    Width {
        name: "i16",
        bytes: 2,
        tag: 7,
        min: i16::MIN as i128,
        max: i16::MAX as u128,
    },
    Width {
        name: "i32",
        bytes: 4,
        tag: 8,
        min: i32::MIN as i128,
        max: i32::MAX as u128,
    },
    Width {
        name: "i64",
        bytes: 8,
        tag: 9,
        min: i64::MIN as i128,
        max: i64::MAX as u128,
    },
    Width {
        name: "i128",
        bytes: 16,
        tag: 10,
        min: i128::MIN,
        max: i128::MAX as u128,
    },
];

/// SER-2's written envelope/descriptor and two's-complement lane order
/// (docs/spec/serialization.md). This only supplies vectors and exact expected
/// bytes; no production codec or arithmetic is used to calculate the oracle.
fn vector(w: Width, values: &[u128]) -> String {
    let mut descriptor = Vec::new();
    for n in [1u32, 0, 2] {
        descriptor.extend(n.to_le_bytes());
    }
    descriptor.extend([5, 3]); // Builtin, Simd
    descriptor.extend(2u32.to_le_bytes());
    descriptor.push(0); // type argument: scalar at node 1
    descriptor.extend(1u32.to_le_bytes());
    descriptor.push(1); // size argument
    descriptor.extend((values.len() as u64).to_le_bytes());
    descriptor.extend([0, w.tag]); // Scalar
    let mut bytes = b"HSC1".to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((descriptor.len() as u64).to_le_bytes());
    bytes.extend(descriptor);
    bytes.extend(((values.len() * w.bytes) as u64).to_le_bytes());
    for value in values {
        bytes.extend(&value.to_le_bytes()[..w.bytes]);
    }
    let mut literal = String::from("b\"");
    for byte in bytes {
        write!(literal, "\\x{byte:02x}").unwrap();
    }
    literal.push('"');
    literal
}

fn matrix_source() -> String {
    let mut source = String::new();
    let mut calls = String::new();
    let mut count = 0;
    for w in WIDTHS {
        for lanes in [2, 4] {
            let name = format!("{}_{}", w.name, lanes);
            let t = format!("Simd[{}, {lanes}]", w.name);
            let first_line = source.lines().count() + 2;
            let mut fitting = vec![w.min as u128, w.max];
            if lanes == 4 {
                fitting.extend([2, 3]);
            }
            let encoded = vector(w, &fitting);
            writeln!(source, "fn add_{name}(a: {t}, b: {t}, c: {t}) -> bool {{\n  let first = simd.add(a, b)\n  let second = simd.add(first, c)\n  codec.encode(second) == {encoded}\n}}").unwrap();
            writeln!(source, r#"fn decode_{name}(data: Bytes) -> {t} {{
  let decoded: Result[{t}, DecodeError] = codec.decode(data)
  match take decoded {{
    Ok(value) => value,
    Error(_) => {{ failure.check(false, Arithmetic())
      simd.splat(0{kind}) }},
  }}
}}
fn probe_{name}(tasks: Tasks, a_bytes: Bytes, b_bytes: Bytes, c_bytes: Bytes, expected_fit: bool, line: u64) {{
  let a = decode_{name}(a_bytes)
  let b = decode_{name}(b_bytes)
  let c = decode_{name}(c_bytes)
  let caught_sum: Result[bool, Failure] = task.isolate(tasks, fn() -> bool {{ add_{name}(a, b, c) }})
  match take caught_sum {{
    Ok(actual) => failure.check(expected_fit && actual, Arithmetic()),
    Error(caught) => failure.check(
      !expected_fit && caught.kind == Check() && caught.rule == "SEM-1"
      && caught.site.definition == "standalone@0/add_{name}" && caught.site.line == line
      && caught.site.column == (if line == {first_line}u64 {{ 15u64 }} else {{ 16u64 }}) && (match caught.cause {{ None() => true, Some(_) => false }}) && list.len(caught.spawn) == 0,
      Arithmetic()
    ),
  }}
}}"#, kind = w.name).unwrap();
            let mut case = |a: Vec<u128>,
                            b: Vec<u128>,
                            c: Vec<u128>,
                            expected: Option<Vec<u128>>,
                            line: usize| {
                if let Some(ref expected) = expected {
                    assert_eq!(expected, &fitting);
                }
                let expected = expected.is_some();
                writeln!(
                    calls,
                    "  probe_{name}(tasks, {}, {}, {}, {expected}, {line}u64)",
                    vector(w, &a),
                    vector(w, &b),
                    vector(w, &c)
                )
                .unwrap();
                count += 1;
            };
            // Last fitting sums at both extremes, with distinct interior
            // lanes so an accidental lane permutation cannot pass equality.
            let mut a = vec![0; lanes];
            let mut b = vec![0; lanes];
            let mut c = vec![0; lanes];
            let mut expected = vec![0; lanes];
            a[0] = if w.min < 0 { (w.min + 1) as u128 } else { 0 };
            b[0] = if w.min < 0 { (-1i128) as u128 } else { 0 };
            expected[0] = w.min as u128;
            a[1] = w.max - 1;
            b[1] = 1;
            expected[1] = w.max;
            if lanes == 4 {
                a[2] = 0;
                b[2] = 1;
                c[2] = 1;
                expected[2] = 2;
                a[3] = 1;
                b[3] = 1;
                c[3] = 1;
                expected[3] = 3;
            }
            case(a, b, c, Some(expected), 0);
            for lane in 0..lanes {
                for low in [false, true] {
                    if low && w.min == 0 {
                        continue;
                    }
                    let boundary = if low { w.min as u128 } else { w.max };
                    let delta = if low { (-1i128) as u128 } else { 1 };
                    let mut a = vec![0; lanes];
                    let mut b = vec![0; lanes];
                    let mut c = vec![0; lanes];
                    a[lane] = boundary;
                    b[lane] = delta;
                    c[lane] = delta;
                    // Each lane alone fails. Both source adds would fail at
                    // the boundary; execution must report the first site.
                    case(a.clone(), b.clone(), c.clone(), None, first_line);
                    // First add fits, second is the first overflowing site.
                    a[lane] = if low { (w.min + 1) as u128 } else { w.max - 1 };
                    case(a, b, c, None, first_line + 1);
                }
            }
        }
    }
    assert_eq!(count, 200); // 20 controls + 180 isolated lane/site failures
    writeln!(source, "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let tasks = io.tasks(rt)?\n{calls}  let _ = out.print(io.stdout(rt)?, \"simd widths ok\\n\")\n  Ok(())\n}}").unwrap();
    source
}

/// One validated program, emitted once per backend. The generated program
/// reuses its add/decode/probe functions across all lane vectors. Catching
/// failures keeps the entire bounded matrix in each single native launch.
fn parity(source: &str, expected_failure: Option<u32>, output: &[u8], name: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine.sources.add("simd.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(reference.output, output, "{reference:?}");
        match expected_failure {
            Some(line) => {
                assert_eq!(reference.exit_code, None, "{reference:?}");
                assert_eq!(reference.failures.len(), 1, "{reference:?}");
                assert_eq!(reference.failures[0].rule, "SEM-1");
                assert_eq!(reference.failures[0].line, line);
            }
            None => {
                assert_eq!(reference.exit_code, Some(0), "{reference:?}");
                assert!(reference.failures.is_empty(), "{reference:?}");
            }
        }
        let snapshot = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(snapshot).unwrap();
        let options = Options::new(target());
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let directory = tempfile::Builder::new()
            .prefix(name)
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        std::fs::write(directory.path().join("simd.hir"), source).unwrap();
        let toolchain =
            Toolchain::for_matrix(&directory.path().join("runtime"), &configurations).unwrap();
        let program = build(&validated, &Options::new(target())).unwrap();
        let sites = Sites::new(validated.snapshot());
        let heap = run_heap(&program, &entry, &sites).unwrap();
        assert_eq!(heap.observation, reference, "{name}: H");
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = emit_backend_objects(
                &program,
                backend,
                OptLevel::Speed,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let runnable = link_runnable(
                &program,
                &entry,
                &objects,
                &toolchain,
                directory.path(),
                None,
            )
            .unwrap();
            let launched = launch_runnable(
                &runnable,
                &program.target,
                &toolchain,
                directory.path(),
                true,
                Tape::Live {
                    record: None,
                    revocations: None,
                },
            )
            .unwrap();
            let actual = observe(&launched, &program, &sites).unwrap();
            assert_eq!(actual, reference, "{name}: {backend:?}");
            eprintln!("{name}: {backend:?} optimized: agrees");
        }
    });
}

#[test]
fn ten_widths_each_lane_and_both_source_sites() {
    parity(
        &matrix_source(),
        None,
        b"simd widths ok\n",
        "simd-native-widths-17",
    );
}

#[test]
fn public_fitting_control() {
    parity(
        include_str!("../../../../conformance/MEM/MEM-8-simd-native-widths-17-run.hir"),
        None,
        b"",
        "simd-native-widths-17-fit",
    );
}

#[test]
fn public_overflow_twin() {
    parity(
        include_str!("../../../../conformance/MEM/MEM-8-simd-native-widths-17-overflow-fail.hir"),
        Some(9),
        b"",
        "simd-native-widths-17-fail",
    );
}

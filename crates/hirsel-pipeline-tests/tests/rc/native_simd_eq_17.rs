//! Structural SIMD equality must agree across all execution engines.
use crate::rc_support::target;

const MINIMAL: &str =
    include_str!("../../../../conformance/MEM/MEM-8-structural-equality-native-17-run.hir");

#[test]
#[ignore = "native: SIMD structural equality on H, checked heap, LLVM and Cranelift"]
fn minimal_i128_equality() {
    native_parity(&[("minimal", MINIMAL)]);
}

// The codec is the existing source-level way to construct nonuniform lanes
// (conformance/SER/SER-2-vectors-run.hir). Change only an active payload byte.
const ALTER: &str = r#"
fn alter[T: plain + serializable](x: T, from_end: u64, value: u8) -> T {
    let encoded = codec.encode(x)
    let mut data: List[u8] = []
    for byte in encoded { list.push(mut data, byte) }
    let at = list.len(data) - from_end
    data[at] = value
    let decoded: Result[T, DecodeError] = codec.decode(bytes.from_list(take data))
    match take decoded {
        Ok(y) => y,
        Error(_) => { failure.check(false, TextInvariant()); x },
    }
}
"#;

#[test]
#[ignore = "native: every SIMD lane kind and fixed/odd widths"]
fn scalar_kinds_widths_and_last_lane() {
    use std::fmt::Write;
    let mut source = ALTER.to_owned();
    let mut calls = String::new();
    let kinds = [
        ("bool", "false", 1),
        ("u8", "0u8", 1),
        ("i8", "0i8", 1),
        ("u16", "0u16", 2),
        ("i16", "0i16", 2),
        ("u32", "0u32", 4),
        ("i32", "0i32", 4),
        ("u64", "0u64", 8),
        ("i64", "0i64", 8),
        ("u128", "0u128", 16),
        ("i128", "0i128", 16),
        ("f32", "0.0f32", 4),
        ("f64", "0.0f64", 8),
    ];
    for (kind, zero, bytes) in kinds {
        for lanes in [1, 2, 3, 17] {
            let name = format!("check_{kind}_{lanes}");
            writeln!(
                source,
                "fn {name}() {{
                let a: Simd[{kind}, {lanes}] = simd.splat({zero})
                let b = alter(a, {bytes}u64, 1u8)
                failure.check(a == a && !(a != a), TextInvariant())
                failure.check(a != b && !(a == b), TextInvariant())
                failure.check(b != a && !(b == a), TextInvariant())
                failure.check(b == b, TextInvariant())
            }}"
            )
            .unwrap();
            writeln!(calls, "{name}()").unwrap();
        }
    }
    writeln!(
        source,
        "pub fn main() -> Result[(), ()] {{ {calls} Ok(()) }}"
    )
    .unwrap();
    native_parity(&[("scalar-matrix", &source)]);
}

#[test]
#[ignore = "native: SIMD IEEE equality, wide integer bits and nested comparisons"]
fn ieee_and_wide_integer_semantics() {
    use std::fmt::Write;
    let mut source = ALTER.to_owned();
    let mut calls = String::new();
    for (kind, nan_byte) in [("f32", 192), ("f64", 248)] {
        for lanes in [1, 3, 17] {
            let name = format!("ieee_{kind}_{lanes}");
            writeln!(
                source,
                "fn {name}() {{
                let zero: Simd[{kind}, {lanes}] = simd.splat(0.0{kind})
                let neg_zero: Simd[{kind}, {lanes}] = simd.splat(-0.0{kind})
                let nan: Simd[{kind}, {lanes}] = simd.splat(0.0{kind} / 0.0{kind})
                let inf: Simd[{kind}, {lanes}] = simd.splat(1.0{kind} / 0.0{kind})
                let neg_inf: Simd[{kind}, {lanes}] = simd.splat(-1.0{kind} / 0.0{kind})
                let last_nan = alter(inf, 2u64, {nan_byte}u8)
                let last_neg_zero = alter(zero, 1u64, 128u8)
                failure.check(zero == neg_zero && !(zero != neg_zero), TextInvariant())
                failure.check(zero == last_neg_zero, TextInvariant())
                failure.check(nan != nan && !(nan == nan), TextInvariant())
                failure.check(zero != nan && nan != zero, TextInvariant())
                failure.check(inf == inf && neg_inf == neg_inf && inf != neg_inf, TextInvariant())
                failure.check(last_nan != last_nan && !(last_nan == last_nan), TextInvariant())
                failure.check((zero, 7u8) == (neg_zero, 7u8), TextInvariant())
                let xs: List[Simd[{kind}, {lanes}]] = [zero, inf]
                let ys: List[Simd[{kind}, {lanes}]] = [zero, neg_inf]
                failure.check(xs != ys, TextInvariant())
            }}"
            )
            .unwrap();
            writeln!(calls, "{name}()").unwrap();
        }
    }
    source.push_str(
        r#"
fn wide() {
    let signed: Simd[i128, 3] = simd.splat(-170141183460469231731687303715884105728i128)
    let unsigned: Simd[u128, 3] = simd.splat(340282366920938463463374607431768211455u128)
    failure.check(signed != alter(signed, 8u64, 1u8), TextInvariant())
    failure.check(unsigned != alter(unsigned, 8u64, 0u8), TextInvariant())
}
"#,
    );
    writeln!(
        source,
        "pub fn main() -> Result[(), ()] {{ {calls} wide()\n Ok(()) }}"
    )
    .unwrap();
    native_parity(&[("ieee-wide", &source)]);
}

fn native_parity(cases: &[(&str, &str)]) {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let options = Options::new(target());
    let matrix = vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: false },
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
    let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-simd-eq-17");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("simd.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
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
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            eprintln!("{name}: interpreter exit0");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let mut executions = 0;
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
                eprintln!("{name}: {configuration}: agrees");
                executions += 1;
            }
            assert_eq!(executions, 4);
        });
    }
}

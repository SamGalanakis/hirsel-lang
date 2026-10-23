//! Checked byte reads and byte-vector operations across reference, H and both native backends.
//! Fixed-width endian vectors follow Rust uint_macros.rs:4131/4174 (a7c37f9f),
//! Zig mem.zig:1953/1957 (bff490a2), and Go binary.go:115/201 (ebe2feef).
use crate::rc_support::target;
use std::fmt::Write as _;

fn literal(data: &[u8]) -> String {
    let mut s = String::from("b\"");
    for byte in data {
        write!(s, "\\x{byte:02x}").unwrap();
    }
    s.push('"');
    s
}

#[test]
#[ignore = "native: six total scalar reads, truncation and unaligned offsets on four engines"]
fn scalar_matrix() {
    let mut source = String::new();
    let mut calls = String::new();
    for width in [2, 4, 8] {
        let bits = width * 8;
        for be in [false, true] {
            let order = if be { "be" } else { "le" };
            writeln!(source, "fn check_{bits}_{order}(data: Bytes, at: u64, expected: Option[u{bits}]) {{ failure.check(bytes.try_read_u{bits}_{order}(data, at) == expected, TextInvariant()) }}").unwrap();
            for len in 0..=40 {
                let data: Vec<u8> = (0..len).map(|i| (i * 53 + 129) as u8).collect();
                for offset in (0..=32).chain([u64::MAX]) {
                    let expected = if offset <= len && width <= len - offset {
                        let mut value = 0u64;
                        for i in 0..width {
                            value |= (data[(offset + i) as usize] as u64)
                                << (8 * if be { width - 1 - i } else { i });
                        }
                        format!("Some({value}u{bits})")
                    } else {
                        "None()".into()
                    };
                    writeln!(
                        calls,
                        "check_{bits}_{order}({}, {offset}u64, {expected})",
                        literal(&data)
                    )
                    .unwrap();
                }
            }
        }
    }
    // Split input invocations by shape to stay under the default interpreter budget.
    let lines: Vec<_> = calls.lines().collect();
    for (i, chunk) in lines.chunks(1394).enumerate() {
        let program = format!(
            "{source}\npub fn main() -> Result[(), ()] {{ {}\nOk(()) }}",
            chunk.join("\n")
        );
        native_parity(&[(&format!("scalar-{i}"), &program)]);
    }
}

#[test]
#[ignore = "native: odd/wide byte vectors, every lane high bit, exact short and unaligned input"]
fn vector_matrix() {
    for n in [1usize, 3, 8, 16, 17, 32, 64] {
        let mut source = String::new();
        writeln!(source, "fn load(data: Bytes, at: u64) -> Simd[u8, {n}] {{ let value: Option[Simd[u8, {n}]] = simd.try_load_bytes(data, at)\nmatch value {{ Some(v) => v, None() => {{ failure.check(false, TextInvariant()); simd.splat(0u8) }}, }} }}").unwrap();
        writeln!(source, "pub fn main() -> Result[(), ()] {{").unwrap();
        let all = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        writeln!(source,"let zero: Simd[u8, {n}] = simd.splat(0u8)\nlet ones: Simd[u8, {n}] = simd.splat(255u8)\nfailure.check(simd.bitmask(simd.eq_bytes(zero, zero)) == {all}u64, TextInvariant())\nfailure.check(simd.bitmask(simd.eq_bytes(zero, ones)) == 0u64, TextInvariant())").unwrap();
        for lane in 0..n {
            // Other low bits are nonzero, so testing the low bit instead of the high fails.
            let mut data = vec![127; n];
            data[lane] = 128;
            let mut unaligned = vec![42];
            unaligned.extend(&data);
            writeln!(source,"let v{lane} = load({}, 1u64)\nfailure.check(simd.bitmask(v{lane}) == {}u64, TextInvariant())\nfailure.check(simd.bitmask(simd.bit_or(v{lane}, zero)) == {}u64, TextInvariant())\nfailure.check(simd.eq_bytes(v{lane}, v{lane}) == ones, TextInvariant())",literal(&unaligned),1u64<<lane,1u64<<lane).unwrap();
        }
        for len in [0, n - 1, n]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            let data = literal(&vec![0; len]);
            for at in [1, u64::MAX] {
                writeln!(source,"let absent_{len}_{at}: Option[Simd[u8, {n}]] = simd.try_load_bytes({data}, {at}u64)\nfailure.check(absent_{len}_{at} == None(), TextInvariant())").unwrap();
            }
        }
        writeln!(source, "Ok(()) }}").unwrap();
        native_parity(&[(&format!("vector-{n}"), &source)]);
    }
}

#[test]
fn byte_vector_width_is_restricted_without_narrowing_simd() {
    use hirsel_driver::compiler_adapters;
    use hirsel_engine::*;
    for n in [0, 65] {
        for expression in [
            format!("let a: Option[Simd[u8, {n}]] = simd.try_load_bytes(b\"\", 0u64)"),
            format!("let a: Simd[u8, {n}] = simd.splat(0u8)\nlet _ = simd.bitmask(a)"),
            format!("let a: Simd[u8, {n}] = simd.splat(0u8)\nlet _ = simd.eq_bytes(a, a)"),
            format!("let a: Simd[u8, {n}] = simd.splat(0u8)\nlet _ = simd.bit_or(a, a)"),
        ] {
            with_compiler_stack(|| {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine
                    .sources
                    .add("bad.hir", format!("pub fn main() {{ {expression} }}"))
                    .unwrap();
                let checked = check_progress(&mut engine, file, &default_context()).unwrap();
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == "MEM-8"),
                    "{:?}",
                    checked.diagnostics
                );
            });
        }
    }
}

#[test]
#[ignore = "native: core byte operations also work as function values"]
fn function_values() {
    native_parity(&[(
        "function-values",
        r#"
fn read(data: Bytes, at: u64, f: fn(Bytes, u64) -> Option[u16]) -> Option[u16] { f(data, at) }
fn mask(a: Simd[u8, 16], f: fn(Simd[u8, 16]) -> u64) -> u64 { f(a) }
pub fn main() -> Result[(), ()] {
  failure.check(read(b"\x01\x02", 0u64, bytes.try_read_u16_le) == Some(513u16), TextInvariant())
  failure.check(read(b"\x01\x02", 1u64, bytes.try_read_u16_be) == None(), TextInvariant())
  let load: fn(Bytes, u64) -> Option[Simd[u8, 16]] = simd.try_load_bytes
  let loaded: Option[Simd[u8, 16]] = load(b"abcdefghijklmnop", 0u64)
  let Some(v) = loaded else { failure.check(false, TextInvariant()); return Ok(()) }
  let eq: fn(Simd[u8, 16], Simd[u8, 16]) -> Simd[u8, 16] = simd.eq_bytes
  let both: fn(Simd[u8, 16], Simd[u8, 16]) -> Simd[u8, 16] = simd.bit_or
  let ones: Simd[u8, 16] = eq(v, v)
  let zero: Simd[u8, 16] = simd.splat(0u8)
  let combined: Simd[u8, 16] = both(ones, zero)
  failure.check(mask(combined, simd.bitmask) == 65535u64, TextInvariant())
  Ok(())
}
"#,
    )]);
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
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("word-primitives-21");
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

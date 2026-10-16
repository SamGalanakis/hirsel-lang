//! IEEE bit observation in optimized LLVM IR: `f32.bits`, `f64.bits` and their
//! `from_bits` inverses are a bitcast and a NaN select in the caller, with no
//! runtime entry, no status branch and no result slot, so a checksum loop over
//! a float list is a reduction the vectorizer can take, as C's `memcpy` read is.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

/// The optimized IR of `source` for `target`, whole.
fn module_ir(source: &str, target: TargetSpec) -> String {
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("bits.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(&validated, &Options::new(target)).unwrap();
        emit_ir(&ready, OptLevel::Speed).unwrap()
    })
}

/// The lines of the one defined function whose symbol contains `name`.
fn function<'a>(ir: &'a str, name: &str) -> Vec<&'a str> {
    let mut lines = ir
        .lines()
        .skip_while(|l| !(l.starts_with("define ") && l.contains(name) && !l.contains("@_H1T")));
    let header = lines
        .next()
        .unwrap_or_else(|| panic!("{name} is defined\n{ir}"));
    std::iter::once(header)
        .chain(lines.take_while(|l| *l != "}"))
        .collect()
}

/// Whether `body` calls anything but an intrinsic, which emits no call.
fn calls(body: &str) -> bool {
    body.lines()
        .any(|l| l.contains("call ") && !l.contains("@llvm."))
}

fn targets() -> [TargetSpec; 2] {
    [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ]
}

const CHECKSUM: &str = "fn checksum32(xs: List[f32]) -> u32 {
  let mut sum: u32 = 0
  for i in 0u64..list.len(xs) { sum = int.wrapping_add(sum, f32.bits(xs[i])) }
  sum
}
fn checksum64(xs: List[f64]) -> u64 {
  let mut sum: u64 = 0
  for i in 0u64..list.len(xs) { sum = int.wrapping_add(sum, f64.bits(xs[i])) }
  sum
}
fn round32(word: u32) -> u32 { f32.bits(f32.from_bits(word)) }
fn round64(word: u64) -> u64 { f64.bits(f64.from_bits(word)) }
fn observe32(x: f32) -> u32 { f32.bits(x) }
fn observe64(x: f64) -> u64 { f64.bits(x) }
fn make32(word: u32) -> f32 { f32.from_bits(word) }
fn make64(word: u64) -> f64 { f64.from_bits(word) }
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let words: List[f32] = [1.5f32, 0.0f32 / 0.0f32]
  let doubles: List[f64] = [2.5f64, -0.0f64]
  failure.check(checksum32(words) != round32(7u32), TextInvariant())
  failure.check(checksum64(doubles) != round64(7u64), TextInvariant())
  failure.check(observe32(make32(7u32)) != 7u32, TextInvariant())
  failure.check(observe64(make64(7u64)) != 7u64, TextInvariant())
  Ok(())
}
";

/// No function calls a bits runtime entry, and each observation is a bitcast
/// with an unordered self-compare choosing the canonical NaN (SEM-2).
#[test]
fn bits_and_from_bits_are_bitcasts_with_a_canonical_nan_select() {
    for target in targets() {
        let ir = module_ir(CHECKSUM, target);
        assert!(
            !ir.contains("_bits("),
            "a bits runtime entry is called:\n{ir}"
        );
        for (name, word, nan) in [
            ("round32", "i32", "2143289344"),
            ("round64", "i64", "9221120237041090560"),
        ] {
            let body = function(&ir, name).join("\n");
            assert!(!calls(&body), "{name} calls:\n{body}");
            assert!(body.contains("bitcast"), "{name} has no bitcast:\n{body}");
            assert!(
                body.contains("fcmp uno") || body.contains("fcmp une"),
                "{body}"
            );
            assert!(
                body.contains(&format!("{word} {nan}")),
                "{name}: no canonical NaN\n{body}"
            );
        }
    }
}

/// Each side canonicalizes a NaN on its own, as the runtime entries each did, so code
/// that consumes a `from_bits` result without passing it back through `bits`, or hands
/// `bits` a payload NaN of its own arithmetic, never sees the payload: a round trip alone
/// would let either side drop it, the other masking the loss. Each function's own
/// body is one side alone.
#[test]
fn each_side_canonicalizes_nan_alone() {
    for target in targets() {
        let ir = module_ir(CHECKSUM, target);
        // The canonical NaN as an integer of any width the result is returned in, or as
        // LLVM's float literal (which prints both kinds' canonical NaN as this double).
        for (name, nan) in [
            ("observe32", "2143289344"),
            ("observe64", "9221120237041090560"),
            ("make32", "2143289344"),
            ("make64", "9221120237041090560"),
        ] {
            let body = function(&ir, name).join("\n");
            assert!(!calls(&body), "{name} calls:\n{body}");
            assert!(
                body.contains("fcmp uno") || body.contains("fcmp une"),
                "{name} does not test for NaN:\n{body}"
            );
            assert!(
                body.contains(&format!(" {nan},")) || body.contains("0x7FF8000000000000"),
                "{name}: no canonical NaN\n{body}"
            );
        }
    }
}

/// The checksum loops read, add and branch on their exit only: no call and no
/// status test per element, and the reduction is vectorized like C's.
#[test]
fn a_float_checksum_loop_is_a_vector_reduction() {
    for target in targets() {
        let ir = module_ir(CHECKSUM, target);
        for (name, lanes) in [("checksum32", "x i32>"), ("checksum64", "x i64>")] {
            let body = function(&ir, name).join("\n");
            assert!(!calls(&body), "{name} calls:\n{body}");
            assert!(body.contains(lanes), "{name} is not vectorized:\n{body}");
        }
    }
}

//! Links emitted objects with C and the `hirsel_rt_v2` runtime and runs them: x86-64
//! natively, AArch64 under `qemu-aarch64-static`. These need the pinned clang, the AArch64
//! cross toolchain and qemu, so `tools/check-native.sh` runs them (`--include-ignored`) and
//! `tools/gate` does not.

use crate::support;

use std::fmt::Write as _;

use hirsel_lir::emission::{EmitInputs, OptLevel, site_code, symbol_name};
use hirsel_lir::ir::{BinaryOp, UnaryOp};
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_llvm::{c_definition, compile_ir, emit_object};
use support::native::*;
use support::programs::*;
use support::*;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tools/native-toolchain/fixtures"
);
const EXPECTED: &str = include_str!("../../../../tools/native-toolchain/fixtures/smoke.expected");

const OPTS: [OptLevel; 2] = [OptLevel::None, OptLevel::Speed];

/// Compiles the smoke fixture, drops the IR text, links the object with its C harness and
/// runs it.
fn run_smoke(target: &TargetSpec, opt: OptLevel) -> String {
    let dir = scratch(&format!("smoke-{}-{opt:?}", target.triple));
    let object = {
        let ir = std::fs::read_to_string(format!("{FIXTURES}/smoke.ll")).unwrap();
        compile_ir(&ir, target, opt).unwrap()
    };
    let harness = format!("{FIXTURES}/smoke_harness.c").into();
    let executable = link(target, &dir, &[("smoke.o", &object.bytes)], &[harness]);
    drop(object);
    run(target, &executable)
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn every_target_executes_the_smoke_fixture() {
    for target in TargetSpec::executed() {
        for opt in OPTS {
            assert_eq!(
                run_smoke(&target, opt),
                EXPECTED,
                "{} {opt:?}",
                target.triple
            );
        }
    }
}

// The scalar boundary matrix.

fn width(kind: ScalarKind) -> Option<(u32, bool)> {
    use ScalarKind as K;
    Some(match kind {
        K::U8 => (8, false),
        K::U16 => (16, false),
        K::U32 => (32, false),
        K::U64 => (64, false),
        K::U128 => (128, false),
        K::I8 => (8, true),
        K::I16 => (16, true),
        K::I32 => (32, true),
        K::I64 => (64, true),
        K::I128 => (128, true),
        _ => return None,
    })
}

fn mask(w: u32) -> u128 {
    if w == 128 { u128::MAX } else { (1 << w) - 1 }
}

fn signed(bits: u128, w: u32) -> i128 {
    ((bits << (128 - w)) as i128) >> (128 - w)
}

/// The boundary operands of a kind, as canonical bit patterns.
fn operands(kind: ScalarKind) -> Vec<u128> {
    let mut values = match kind {
        ScalarKind::Bool => vec![0, 1],
        ScalarKind::F32 => [
            0.0f32,
            -0.0,
            1.0,
            -1.5,
            3.0,
            1e38,
            f32::MAX,
            f32::MIN_POSITIVE,
            1e-45,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ]
        .iter()
        .map(|v| u128::from(v.to_bits()))
        .collect(),
        ScalarKind::F64 => [
            0.0f64,
            -0.0,
            1.0,
            -1.5,
            3.0,
            1e308,
            f64::MAX,
            f64::MIN_POSITIVE,
            5e-324,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ]
        .iter()
        .map(|v| u128::from(v.to_bits()))
        .collect(),
        _ => {
            let (w, s) = width(kind).unwrap();
            let m = mask(w);
            let min = if s { 1u128 << (w - 1) } else { 0 };
            let max = if s { m >> 1 } else { m };
            vec![
                0,
                1,
                2,
                3,
                7,
                u128::from(w - 1),
                u128::from(w),
                min,
                min + 1,
                max,
                max - 1,
                m,
                1 << (w / 2),
            ]
        }
    };
    let mut seen = Vec::new();
    values.retain(|v| {
        let fresh = !seen.contains(v);
        seen.push(*v);
        fresh
    });
    values
}

/// The mathematical result of a checked integer operation (SEM-1), computed independently of
/// the emitter; None is a SEM-1 failure.
fn integer_oracle(
    op: Result<BinaryOp, UnaryOp>,
    kind: ScalarKind,
    a: u128,
    b: u128,
) -> Option<u128> {
    let (w, s) = width(kind).unwrap();
    let m = mask(w);
    let fit_signed = |v: Option<i128>| {
        v.filter(|v| w == 128 || (-(1i128 << (w - 1))..(1i128 << (w - 1))).contains(v))
            .map(|v| v as u128 & m)
    };
    let fit_unsigned = |v: Option<u128>| v.filter(|v| *v <= m);
    let (sa, sb) = (signed(a, w), signed(b, w));
    let boolean = |c: bool| Some(u128::from(c));
    use BinaryOp as B;
    match op {
        Ok(B::Add) if s => fit_signed(sa.checked_add(sb)),
        Ok(B::Add) => fit_unsigned(a.checked_add(b)),
        Ok(B::Sub) if s => fit_signed(sa.checked_sub(sb)),
        Ok(B::Sub) => fit_unsigned(a.checked_sub(b)),
        Ok(B::Mul) if s => fit_signed(sa.checked_mul(sb)),
        Ok(B::Mul) => fit_unsigned(a.checked_mul(b)),
        Ok(B::Div) if s => {
            if sb == 0 {
                None
            } else {
                fit_signed(sa.checked_div(sb))
            }
        }
        Ok(B::Div) => a.checked_div(b),
        // `%` takes the dividend's sign; signed MIN % -1 is zero.
        Ok(B::Rem) if s => match sb {
            0 => None,
            -1 => Some(0),
            _ => Some(sa.checked_rem(sb).unwrap() as u128 & m),
        },
        Ok(B::Rem) => a.checked_rem(b),
        Ok(B::Shl | B::Shr) if (s && sb < 0) || b >= u128::from(w) => None,
        Ok(B::Shl) => Some((a << b) & m),
        Ok(B::Shr) if s => Some((sa >> b) as u128 & m),
        Ok(B::Shr) => Some(a >> b),
        Ok(B::BitAnd) => Some(a & b),
        Ok(B::BitOr) => Some(a | b),
        Ok(B::BitXor) => Some(a ^ b),
        Ok(B::Eq) => boolean(a == b),
        Ok(B::Ne) => boolean(a != b),
        Ok(B::Lt) => boolean(if s { sa < sb } else { a < b }),
        Ok(B::Le) => boolean(if s { sa <= sb } else { a <= b }),
        Ok(B::Gt) => boolean(if s { sa > sb } else { a > b }),
        Ok(B::Ge) => boolean(if s { sa >= sb } else { a >= b }),
        Err(UnaryOp::Neg) => fit_signed(sa.checked_neg()),
        Err(UnaryOp::Not | UnaryOp::BitNot) => Some(!a & m),
        // The named library operations have their own matrix (`tests/it/scalar_library.rs`).
        Ok(other) => panic!("no operator oracle for {other:?}"),
    }
}

/// IEEE results; a NaN result prints as `nan` whatever its payload.
fn float_oracle(op: Result<BinaryOp, UnaryOp>, kind: ScalarKind, a: u128, b: u128) -> String {
    macro_rules! float {
        ($f:ty, $bits:ty) => {{
            let (x, y) = (<$f>::from_bits(a as $bits), <$f>::from_bits(b as $bits));
            let value = |v: $f| {
                if v.is_nan() {
                    "nan".to_string()
                } else {
                    hex(u128::from(v.to_bits()))
                }
            };
            let boolean = |c: bool| hex(u128::from(c));
            use BinaryOp as B;
            match op {
                Ok(B::Add) => value(x + y),
                Ok(B::Sub) => value(x - y),
                Ok(B::Mul) => value(x * y),
                Ok(B::Div) => value(x / y),
                Ok(B::Rem) => value(x % y),
                Ok(B::Eq) => boolean(x == y),
                Ok(B::Ne) => boolean(x != y),
                Ok(B::Lt) => boolean(x < y),
                Ok(B::Ge) => boolean(x >= y),
                Err(UnaryOp::Neg) => value(-x),
                other => panic!("no float oracle for {other:?}"),
            }
        }};
    }
    match kind {
        ScalarKind::F32 => float!(f32, u32),
        _ => float!(f64, u64),
    }
}

fn hex(bits: u128) -> String {
    format!("{:016x}{:016x}", (bits >> 64) as u64, bits as u64)
}

fn c_type(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::Bool | ScalarKind::U8 => "uint8_t",
        ScalarKind::U16 => "uint16_t",
        ScalarKind::U32 => "uint32_t",
        ScalarKind::U64 => "uint64_t",
        ScalarKind::U128 => "unsigned __int128",
        ScalarKind::I8 => "int8_t",
        ScalarKind::I16 => "int16_t",
        ScalarKind::I32 => "int32_t",
        ScalarKind::I64 => "int64_t",
        ScalarKind::I128 => "__int128",
        ScalarKind::F32 => "float",
        ScalarKind::F64 => "double",
        ScalarKind::Pointer => "void *",
    }
}

const C_PRELUDE: &str = r#"#include <inttypes.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "hirsel_rt_v2.h"

static float f32_of(uint32_t b) { float f; memcpy(&f, &b, 4); return f; }
static double f64_of(uint64_t b) { double f; memcpy(&f, &b, 8); return f; }
static void bits(unsigned __int128 v) { printf("%016" PRIx64 "%016" PRIx64, (uint64_t)(v >> 64), (uint64_t)v); }
static void print_f32(float v) { uint32_t b; memcpy(&b, &v, 4); if (isnan(v)) printf("nan"); else bits(b); }
static void print_f64(double v) { uint64_t b; memcpy(&b, &v, 8); if (isnan(v)) printf("nan"); else bits(b); }
static hirsel_rt_v2_ctx *ctx;
static void outcome(uint8_t status) {
  if (status == 0) return;
  hirsel_rt_v2_failure primary;
  if (status != 1 || !hirsel_rt_v2_failure_primary(ctx, &primary)) { printf("bad status %u\n", status); return; }
  printf("fail %u %u %016" PRIx64, primary.kind, primary.rule, primary.site);
  hirsel_rt_v2_failure_clear(ctx);
}
static void context(void) {
  hirsel_rt_v2_config config;
  memset(&config, 0, sizeof config);
  config.abi_version = HIRSEL_RT_V2_ABI_VERSION;
  if (hirsel_rt_v2_ctx_create(&config, &ctx) != 0) { printf("no context\n"); }
}
"#;

/// The oracle's outcome for one scalar case and operand pair, as the harness prints it.
fn scalar_outcome(case: &ScalarCase, a: u128, b: u128) -> Option<String> {
    match case.kind {
        ScalarKind::F32 | ScalarKind::F64 => Some(float_oracle(case.op, case.kind, a, b)),
        ScalarKind::Bool => {
            let r = match case.op {
                Ok(BinaryOp::Eq) => a == b,
                Ok(BinaryOp::BitAnd) => a & b == 1,
                Ok(BinaryOp::BitOr) => a | b == 1,
                Ok(BinaryOp::BitXor) => a ^ b == 1,
                Err(UnaryOp::Not) => a == 0,
                other => panic!("no bool oracle for {other:?}"),
            };
            Some(hex(u128::from(r)))
        }
        _ => integer_oracle(case.op, case.kind, a, b).map(|r| {
            if case.result == ScalarKind::Bool {
                hex(r)
            } else {
                // C prints a signed result sign-extended to 128 bits.
                let (w, s) = width(case.result).unwrap();
                hex(if s { signed(r, w) as u128 } else { r })
            }
        }),
    }
}

/// The scalar harness and the output the oracle expects of it. Each case loops over its
/// kind's operand table (canonical bits as high and low words).
fn scalar_harness(program: &Program) -> (String, String) {
    let codes = failure_codes();
    let sem1 = codes.codes["SEM-1"];
    let mut c = String::from(C_PRELUDE);
    let mut kinds: Vec<ScalarKind> = Vec::new();
    let cases = scalar_cases();
    for case in &cases {
        if !kinds.contains(&case.kind) {
            kinds.push(case.kind);
            let values = operands(case.kind);
            let words = |f: fn(u128) -> u64| {
                values
                    .iter()
                    .map(|v| format!("0x{:016x}ull", f(*v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let name = kind_name(case.kind);
            let _ = writeln!(
                c,
                "static const uint64_t {name}_hi[] = {{{}}};\nstatic const uint64_t {name}_lo[] = {{{}}};\n#define {name}_n {}",
                words(|v| (v >> 64) as u64),
                words(|v| v as u64),
                values.len()
            );
        }
    }
    let mut main = String::from("int main(void) {\n  context();\n");
    let mut expected = String::new();
    for case in &cases {
        let (arg, result) = (c_type(case.kind), c_type(case.result));
        let name = kind_name(case.kind);
        let binary = case.op.is_ok();
        c.push_str(&program.c_call(case.name, &format!("call_{}", case.name)));
        let value = |index: &str| match case.kind {
            ScalarKind::F32 => format!("f32_of((uint32_t){name}_lo[{index}])"),
            ScalarKind::F64 => format!("f64_of({name}_lo[{index}])"),
            _ => format!(
                "({arg})(((unsigned __int128){name}_hi[{index}] << 64) | {name}_lo[{index}])"
            ),
        };
        let print = match case.result {
            ScalarKind::F32 => "print_f32(r)",
            ScalarKind::F64 => "print_f64(r)",
            _ => "bits((unsigned __int128)r)",
        };
        let (rights, arguments) = if binary {
            (
                format!("{name}_n"),
                format!("{}, {}", value("i"), value("j")),
            )
        } else {
            ("1".to_string(), value("i"))
        };
        let call = format!(
            "{result} r = 0;\n    uint8_t s = call_{}(ctx, &r, {arguments});",
            case.name
        );
        let _ = writeln!(
            main,
            "  for (int i = 0; i < {name}_n; i++) for (int j = 0; j < {rights}; j++) {{\n    {call}\n    printf(\"{} \"); bits(((unsigned __int128){name}_hi[i] << 64) | {name}_lo[i]); printf(\" \");\n    bits({});\n    printf(\" \"); if (s == 0) {print}; else outcome(s); printf(\"\\n\");\n  }}",
            case.name,
            if binary {
                format!("((unsigned __int128){name}_hi[j] << 64) | {name}_lo[j]")
            } else {
                "0".into()
            },
        );
        let site = site_code(
            &path(case.name),
            &revision(case.name),
            hirsel_lir::ir::SiteRange { start: 10, end: 20 },
        );
        let values = operands(case.kind);
        let rights = if binary { values.clone() } else { vec![0] };
        for &a in &values {
            for &b in &rights {
                let outcome = scalar_outcome(case, a, b)
                    .unwrap_or_else(|| format!("fail {} {} {site:016x}", sem1.kind, sem1.rule));
                let _ = writeln!(expected, "{} {} {} {outcome}", case.name, hex(a), hex(b));
            }
        }
    }
    main.push_str("  hirsel_rt_v2_ctx_destroy(ctx);\n  return 0;\n}\n");
    c.push_str(&main);
    (c, expected)
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn every_target_executes_the_scalar_boundary_matrix() {
    let codes = failure_codes();
    let input = input();
    for target in TargetSpec::executed() {
        let program = scalar_program(&target);
        let (harness, expected) = scalar_harness(&program);
        for opt in OPTS {
            let dir = scratch(&format!("scalar-{}-{opt:?}", target.triple));
            let bytes = {
                let inputs = EmitInputs {
                    input: &input,
                    target: &program.table.target,
                    layouts: &program.table.table,
                    manifest: &program.manifest,
                    failures: &codes,
                    statics: &program.statics,
                    finalizers: &program.finalizers,
                    inline_instances: &std::collections::BTreeSet::new(),
                    available_externally: &std::collections::BTreeSet::new(),
                    callbacks: &hirsel_lir::emission::NO_CALLBACKS,
                    registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                        &program.table.table,
                        &program.finalizers,
                    ),
                    opt,
                    measurement: false,
                    fuel: false,
                };
                emit_object(&program.units(), &inputs).unwrap().bytes
            };
            let source = dir.join("harness.c");
            std::fs::write(&source, &harness).unwrap();
            let executable = link(&target, &dir, &[("scalar.o", &bytes)], &[source]);
            let actual = run(&target, &executable);
            if actual != expected {
                let first = actual
                    .lines()
                    .zip(expected.lines())
                    .find(|(a, e)| a != e)
                    .map(|(a, e)| format!("native `{a}`\noracle `{e}`"))
                    .unwrap_or_else(|| "the outputs differ in length".into());
                panic!("{} {opt:?}: first difference:\n{first}", target.triple);
            }
        }
    }
}

// The ABI probes: C calls Hirsel entries, and Hirsel calls a C-defined entry and a C closure.

const ABI_HARNESS: &str = include_str!("../native/abi_harness.c");

/// The entries the harness calls, as `names.h` macro and unit name.
const ABI_ENTRIES: [&str; 16] = [
    "inc",
    "twice_inc",
    "host_call",
    "pair_sum",
    "pair_take",
    "pair_make",
    "wire_value",
    "wire_make",
    "opt_get",
    "opt_some",
    "arr_get",
    "sum_below",
    "apply",
    "positive",
    "choose",
    "fused",
];

/// `names.h`: the layout facts the harness's C structs assert.
fn abi_names(program: &Program) -> String {
    use hirsel_lir::layout_types::SizeSpec;
    let table = &program.table;
    let record = |name: &str| table.table.layout(table.layout(name)).unwrap();
    let size = |name: &str| match record(name).size {
        SizeSpec::Fixed(size) => size,
        SizeSpec::Dynamic(_) => panic!("{name} is dynamic"),
    };
    let mut h = String::from("/* Generated by crates/hirsel-llvm/tests/it/native.rs. */\n");
    let opt = record("opt");
    let some = &opt
        .tag
        .as_ref()
        .unwrap()
        .variants
        .iter()
        .find(|v| v.ordinal == 1)
        .unwrap()
        .fields[0];
    for (macro_, value) in [
        ("PAIR_SIZE", size("pair")),
        ("PAIR_ALIGN", u64::from(record("pair").align)),
        ("PAIR_B", record("pair").fields[1].offset),
        ("WIRE_SIZE", size("wire")),
        ("WIRE_B", record("wire").fields[1].offset),
        ("WIRE_C", record("wire").fields[2].offset),
        ("OPT_SIZE", size("opt")),
        ("OPT_VALUE", some.offset),
        ("ARR_SIZE", size("arr")),
    ] {
        let _ = writeln!(h, "#define {macro_} {value}");
    }
    h
}

/// `entries.h`: a `call_<entry>` wrapper of each entry's ABI v1 signature, and the two
/// entries C defines, each as emission lowers its signature.
fn abi_entries(program: &Program) -> String {
    let mut h = String::from("/* Generated by crates/hirsel-llvm/tests/it/native.rs. */\n");
    for name in ABI_ENTRIES {
        h.push_str(&program.c_call(name, &format!("call_{name}")));
    }
    let table = &program.table;
    let abi = |name: &str| &table.table.fn_abis[table.abi(name).0 as usize];
    let host = symbol_name(&path(HOST_SCALE), Some(&instance(HOST_SCALE)));
    h.push_str(&c_definition(
        &table.table,
        abi("i64 -> i64"),
        &host,
        "host_scale_body",
    ));
    h.push_str(&c_definition(
        &table.table,
        abi("callback"),
        "scale_thunk",
        "scale_body",
    ));
    h
}

/// What the harness prints: computed here from SEM-1, the fixtures' failure codes and each
/// failure's site, independently of emission.
fn abi_expected() -> String {
    let codes = failure_codes();
    let fail = |rule: &str, unit: &str, start: u32, end: u32| {
        let code = codes.codes[rule];
        let site = site_code(
            &path(unit),
            &revision(unit),
            hirsel_lir::ir::SiteRange { start, end },
        );
        format!("fail {} {} {site:016x}", code.kind, code.rule)
    };
    let sem1 = |unit: &str| fail("SEM-1", unit, 10, 20);
    [
        "inc 41 ok 42".to_string(),
        format!("inc max {}", sem1("inc")),
        "twice_inc 40 ok 42".into(),
        // The second call overflows; its failure propagates with the callee's site.
        format!("twice_inc max-1 {}", sem1("inc")),
        "host_call 5 ok 16".into(),
        "host_call -1 fail 5 50 0000000000000055".into(),
        "host_call max/3 ok 9223372036854775807".into(),
        "pair_sum 2^126+2^126-1 ok 7fffffffffffffffffffffffffffffff".into(),
        format!("pair_sum 2^126+2^126 {}", sem1("pair_sum")),
        "pair_take -5-7 ok fffffffffffffffffffffffffffffff4".into(),
        "pair_make ok fffffffffffffffffffffffffffffffe 40000000000000000000000000000000".into(),
        "wire_value ok 0102030405060709".into(),
        format!("wire_value max {}", sem1("wire_value")),
        "wire_make ok fe 1122334455667788 9988".into(),
        "opt_get some ok -77".into(),
        format!("opt_get none {}", fail("MEM-13", "opt_get", 40, 50)),
        "opt_some ok 1 123".into(),
        "arr_get 3 ok 40".into(),
        format!("arr_get 4 {}", fail("MEM-14", "arr_get", 60, 70)),
        format!("arr_get max {}", fail("MEM-14", "arr_get", 60, 70)),
        "sum_below 0 ok 0".into(),
        "sum_below 100 ok 4950".into(),
        "sum_below 100000 ok 4999950000".into(),
        "apply 7 ok 43".into(),
        "apply 1001 fail 7 70 0000000000000077".into(),
        "positive 3 ok 3".into(),
        format!("positive 0 {}", fail("LANG-9", "positive", 30, 40)),
        "choose true -128 ok -128".into(),
        "choose false 5 ok -5".into(),
        format!("choose false -128 {}", sem1("choose")),
        // 0.1 * 10 - 1 rounded once: the double nearest 0.1 times 10 exceeds 1 by 2^-54.
        "fused ok 0x1p-54".into(),
    ]
    .iter()
    .map(|line| format!("{line}\n"))
    .collect()
}

/// C calls every entry with the prototype the Hirsel ABI v1 gives it, and the object calls a
/// C-defined entry and a C closure; the object links and runs after every emission input is
/// dropped.
#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn every_target_executes_the_abi_probes() {
    let expected = abi_expected();
    for target in TargetSpec::executed() {
        for opt in OPTS {
            let dir = scratch(&format!("abi-{}-{opt:?}", target.triple));
            let (bytes, names) = {
                let codes = failure_codes();
                let input = input();
                let program = abi_program(&target);
                let inputs = EmitInputs {
                    input: &input,
                    target: &program.table.target,
                    layouts: &program.table.table,
                    manifest: &program.manifest,
                    failures: &codes,
                    statics: &program.statics,
                    finalizers: &program.finalizers,
                    inline_instances: &std::collections::BTreeSet::new(),
                    available_externally: &std::collections::BTreeSet::new(),
                    callbacks: &hirsel_lir::emission::NO_CALLBACKS,
                    registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                        &program.table.table,
                        &program.finalizers,
                    ),
                    opt,
                    measurement: false,
                    fuel: false,
                };
                // The harness's prototypes assume these hidden slots: only the nominal
                // `@packed` result takes a caller destination, and only the closure shape
                // takes an environment.
                for (name, id) in &program.table.abis {
                    let abi = &program.table.table.fn_abis[id.0 as usize];
                    assert_eq!(abi.destination, *name == "u8 u64 u16 -> wire", "{name}");
                    assert_eq!(abi.environment, *name == "callback", "{name}");
                }
                let artifact = emit_object(&program.units(), &inputs).unwrap();
                // The imports are the C-defined entry, the runtime and toolchain routines.
                let host = symbol_name(&path(HOST_SCALE), Some(&instance(HOST_SCALE)));
                assert!(artifact.imports.iter().any(|i| i.name == host));
                (artifact.bytes, (abi_names(&program), abi_entries(&program)))
            };
            std::fs::write(dir.join("names.h"), names.0).unwrap();
            std::fs::write(dir.join("entries.h"), names.1).unwrap();
            let source = dir.join("abi_harness.c");
            std::fs::write(&source, ABI_HARNESS).unwrap();
            let executable = link(&target, &dir, &[("abi.o", &bytes)], &[source]);
            assert_eq!(
                run(&target, &executable),
                expected,
                "{} {opt:?}",
                target.triple
            );
        }
    }
}

// The heap probes: allocation, release through descriptor step programs, failure-path
// releases and reset/reuse, against the runtime's live block count.

const HEAP_HARNESS: &str = include_str!("../native/heap_harness.c");

const HEAP_ENTRIES: [&str; 4] = ["box_roundtrip", "nested", "list_get", "reuse"];

fn heap_expected() -> String {
    let codes = failure_codes();
    let bounds = codes.codes["MEM-14"];
    let site = site_code(
        &path("list_get"),
        &revision("list_get"),
        hirsel_lir::ir::SiteRange { start: 60, end: 70 },
    );
    // Over i in 0..10000: box_roundtrip gives i, nested gives i, reuse gives i, and
    // list_get gives i unless i % 4 == 3, when it fails.
    let n: i64 = 10000;
    let total = 3 * (n * (n - 1) / 2) + (0..n).filter(|i| i % 4 != 3).sum::<i64>();
    format!(
        "box_roundtrip 7 ok 7\n\
         box_roundtrip min ok -9223372036854775808\n\
         nested 3 4 ok 3\n\
         list_get 0 ok 10\n\
         list_get 2 ok 30\n\
         list_get 3 fail {} {} {site:016x}\n\
         reuse 1 2 ok 2\n\
         live after one round 0\n\
         rounds {total} failures {} live 0\n",
        bounds.kind,
        bounds.rule,
        n / 4
    )
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn every_target_executes_the_heap_probes() {
    let expected = heap_expected();
    for target in TargetSpec::executed() {
        for opt in OPTS {
            let dir = scratch(&format!("heap-{}-{opt:?}", target.triple));
            let (bytes, entries) = {
                let codes = failure_codes();
                let input = input();
                let program = heap_program(&target);
                let entries: String = HEAP_ENTRIES
                    .iter()
                    .map(|name| program.c_call(name, &format!("call_{name}")))
                    .collect();
                let inputs = EmitInputs {
                    input: &input,
                    target: &program.table.target,
                    layouts: &program.table.table,
                    manifest: &program.manifest,
                    failures: &codes,
                    statics: &program.statics,
                    finalizers: &program.finalizers,
                    inline_instances: &std::collections::BTreeSet::new(),
                    available_externally: &std::collections::BTreeSet::new(),
                    callbacks: &hirsel_lir::emission::NO_CALLBACKS,
                    registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                        &program.table.table,
                        &program.finalizers,
                    ),
                    opt,
                    measurement: false,
                    fuel: false,
                };
                (
                    emit_object(&program.units(), &inputs).unwrap().bytes,
                    entries,
                )
            };
            std::fs::write(dir.join("entries.h"), entries).unwrap();
            let source = dir.join("heap_harness.c");
            std::fs::write(&source, HEAP_HARNESS).unwrap();
            let executable = link(&target, &dir, &[("heap.o", &bytes)], &[source]);
            assert_eq!(
                run(&target, &executable),
                expected,
                "{} {opt:?}",
                target.triple
            );
        }
    }
}

//! SEM-1/SEM-2 conversion compositions: 96 shapes, each literal and parameterized.
//! Prior art: Rust f6c1107f8be13e48465e12c0fd26e6a39fe18e4f (MIT OR Apache-2.0),
//! library/core/src/convert/num.rs:20-110 (checked truncation thresholds), and
//! library/coretests/tests/num/float_conversions.rs:1-63 (cast/checked shapes).
//! Hirsel keeps NotFinite distinct from OutOfRange; saturating casts are no oracle.
//! Existing SEM float-resize/from-int corpora and rc_legalize controls remain intact.
//! The new boundary is composition and branch/generic/parameter transport on I/H/LLVM/CL.
use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
struct Format {
    fraction: u32,
    exponent: u32,
}
const F32: Format = Format {
    fraction: 23,
    exponent: 8,
};
const F64: Format = Format {
    fraction: 52,
    exponent: 11,
};
impl Format {
    fn bias(self) -> i32 {
        (1 << (self.exponent - 1)) - 1
    }
    fn sign(self) -> u64 {
        1 << (self.fraction + self.exponent)
    }
    fn infinity(self) -> u64 {
        ((1 << self.exponent) - 1) << self.fraction
    }
    fn nan(self) -> u64 {
        self.infinity() | (1 << (self.fraction - 1))
    }
}

/// Exact value is (-1)^negative * significand * 2^exponent. No host float arithmetic.
#[derive(Clone, Copy, Debug)]
enum Exact {
    Finite {
        negative: bool,
        significand: u128,
        exponent: i32,
    },
    Infinity(bool),
    Nan,
}
fn decode(bits: u64, f: Format) -> Exact {
    let negative = bits & f.sign() != 0;
    let exponent = (bits & (f.infinity())) >> f.fraction;
    let fraction = bits & ((1 << f.fraction) - 1);
    if exponent == (1 << f.exponent) - 1 {
        if fraction == 0 {
            Exact::Infinity(negative)
        } else {
            Exact::Nan
        }
    } else {
        Exact::Finite {
            negative,
            significand: u128::from(fraction | if exponent == 0 { 0 } else { 1 << f.fraction }),
            exponent: (exponent.max(1) as i32) - f.bias() - f.fraction as i32,
        }
    }
}

/// Integer rounding certificate: guard, round and sticky decide a single nearest-even
/// rounding at the destination quantum, including subnormal/zero and exponent carry.
fn rounded(value: Exact, f: Format) -> u64 {
    let (negative, significand, exponent) = match value {
        Exact::Nan => return f.nan(),
        Exact::Infinity(negative) => return f.infinity() | if negative { f.sign() } else { 0 },
        Exact::Finite {
            negative,
            significand,
            exponent,
        } => (negative, significand, exponent),
    };
    let sign = if negative { f.sign() } else { 0 };
    if significand == 0 {
        return sign;
    }
    let top = 127 - significand.leading_zeros() as i32 + exponent;
    let quantum = (top - f.fraction as i32).max(1 - f.bias() - f.fraction as i32);
    let shift = quantum - exponent;
    let mut retained = if shift <= 0 {
        significand << -shift
    } else if shift > 128 {
        0
    } else {
        let s = shift as u32;
        let head = if s == 128 { 0 } else { significand >> s };
        let guard = (significand >> (s - 1)) & 1 != 0;
        let round = s > 1 && (significand >> (s - 2)) & 1 != 0;
        let sticky = s > 2 && significand & ((1u128 << (s - 2)) - 1) != 0;
        head + u128::from(guard && (round || sticky || head & 1 != 0))
    };
    if retained == 0 {
        return sign;
    }
    let top = 127 - retained.leading_zeros() as i32 + quantum;
    if top > f.bias() {
        return sign | f.infinity();
    }
    if top < 1 - f.bias() {
        return sign | retained as u64;
    }
    if retained >= 1 << (f.fraction + 1) {
        retained >>= 1;
    }
    sign | (((top + f.bias()) as u64) << f.fraction) | (retained as u64 & ((1 << f.fraction) - 1))
}

/// Error arms and signed truncated value; Rust's saturating `as` is never consulted.
fn truncated(value: Exact) -> (u32, i64) {
    let Exact::Finite {
        negative,
        significand,
        exponent,
    } = value
    else {
        return (1, 0);
    };
    let magnitude = if exponent < 0 {
        if exponent <= -128 {
            0
        } else {
            significand >> -exponent
        }
    } else {
        let shift = exponent as u32;
        if shift >= 128 || significand > (u128::MAX >> shift) {
            return (2, 0);
        }
        significand << shift
    };
    if magnitude > (1u128 << 31) - u128::from(!negative) {
        return (2, 0);
    }
    (
        0,
        if negative {
            -(magnitude as i64)
        } else {
            magnitude as i64
        },
    )
}

#[derive(Clone, Debug)]
enum Case {
    Integer {
        kind: &'static str,
        negative: bool,
        magnitude: u128,
    },
    Narrow(u64),
    Widen(u32),
    Truncate(Format, u64),
}
impl Case {
    fn operand(&self) -> (String, String) {
        match *self {
            Self::Integer {
                kind,
                negative,
                magnitude,
            } => (
                kind.into(),
                format!("{}{magnitude}{kind}", if negative { "-" } else { "" }),
            ),
            Self::Narrow(bits) | Self::Truncate(Format { fraction: 52, .. }, bits) => {
                ("f64".into(), format!("f64.from_bits(0x{bits:x}u64)"))
            }
            Self::Widen(bits) => ("f32".into(), format!("f32.from_bits(0x{bits:x}u32)")),
            Self::Truncate(_, bits) => ("f32".into(), format!("f32.from_bits(0x{bits:x}u32)")),
        }
    }
    fn result_type(&self) -> &'static str {
        match self {
            Self::Integer { .. } => "(u32, u64, u32, u64)",
            Self::Narrow(_) => "(u32, u64)",
            Self::Widen(_) => "(u64, u32)",
            Self::Truncate(..) => "(u32, i32, u32, i32)",
        }
    }
    fn body(&self, x: &str) -> String {
        match self {
            Self::Integer { .. } => format!(
                "let a: f32 = float.from_int({x})\nlet b: f64 = float.from_int({x})\n(f32.bits(a), f64.bits(b), f32.bits(f32.from_f64(b)), f64.bits(f64.from_f32(a)))"
            ),
            Self::Narrow(_) => {
                format!("let a = f32.from_f64({x})\n(f32.bits(a), f64.bits(f64.from_f32(a)))")
            }
            Self::Widen(_) => {
                format!("let a = f64.from_f32({x})\n(f64.bits(a), f32.bits(f32.from_f64(a)))")
            }
            Self::Truncate(f, _) => {
                let resized = if f.fraction == 23 {
                    format!("f64.from_f32({x})")
                } else {
                    format!("f32.from_f64({x})")
                };
                format!(
                    "let (a, b) = code(float.to_i32({x}))\nlet (c, d) = code(float.to_i32({resized}))\n(a, b, c, d)"
                )
            }
        }
    }
    fn expected(&self) -> String {
        match *self {
            Self::Integer {
                negative,
                magnitude,
                ..
            } => {
                let value = Exact::Finite {
                    negative,
                    significand: magnitude,
                    exponent: 0,
                };
                let a = rounded(value, F32);
                let b = rounded(value, F64);
                format!(
                    "{a} {b} {} {}",
                    rounded(decode(b, F64), F32),
                    rounded(decode(a, F32), F64)
                )
            }
            Self::Narrow(bits) => {
                let a = rounded(decode(bits, F64), F32);
                format!("{a} {}", rounded(decode(a, F32), F64))
            }
            Self::Widen(bits) => {
                let a = rounded(decode(u64::from(bits), F32), F64);
                format!("{a} {}", rounded(decode(a, F64), F32))
            }
            Self::Truncate(f, bits) => {
                let (status, value) = truncated(decode(bits, f));
                let other = if f.fraction == 23 { F64 } else { F32 };
                let (composed_status, composed_value) =
                    truncated(decode(rounded(decode(bits, f), other), other));
                format!("{status} {value} {composed_status} {composed_value}")
            }
        }
    }
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (kind, values) in [
        ("i8", vec![-127i128, -65, 65, 127]),
        ("u8", vec![63, 127, 129, 255]),
        ("i16", vec![-32767, -16385, 16385, 32767]),
        ("u16", vec![16383, 32767, 32769, 65535]),
        ("i32", vec![-16777217, -33554431, 16777219, 33554431]),
        ("u32", vec![16777217, 16777219, 33554431, 4294967295]),
        (
            "i64",
            vec![
                -9007199254740993,
                -9007199791611905,
                9007199254740995,
                18014398509481983,
            ],
        ),
        (
            "u64",
            vec![
                9007199254740993,
                9007199791611905,
                9007199791611903,
                18014398509481983,
            ],
        ),
        (
            "i128",
            vec![
                -((1 << 100) + (1 << 76) + 1),
                -((1 << 100) + (1 << 47)),
                (1 << 100) + (1 << 76) + 1,
                (1 << 101) - 1,
            ],
        ),
    ] {
        out.extend(values.into_iter().map(|value| Case::Integer {
            kind,
            negative: value < 0,
            magnitude: value.unsigned_abs(),
        }));
    }
    out.extend(
        [
            (1 << 100) + (1 << 47),
            (1 << 100) + 3 * (1 << 47),
            u128::MAX - (1 << 103),
            u128::MAX - (1 << 103) + 1,
        ]
        .into_iter()
        .map(|magnitude| Case::Integer {
            kind: "u128",
            negative: false,
            magnitude,
        }),
    );
    // Both signs at four halfway boundaries; adjacent f64 words bracket each exact tie.
    // Zero/minsub, subnormal/normal, exponent carry at 2, finite/infinity overflow.
    for halfway in [
        0x3690_0000_0000_0000u64,
        0x380f_ffff_e000_0000,
        0x3fff_ffff_f000_0000,
        0x47ef_ffff_f000_0000,
    ] {
        for sign in [0, 1 << 63] {
            for bits in [halfway - 1, halfway, halfway + 1] {
                out.push(Case::Narrow(bits | sign));
            }
        }
    }
    out.extend(
        [0x0080_0000, 0x7f7f_ffff, 0x807f_ffff, 0xff80_0001]
            .into_iter()
            .map(Case::Widen),
    );
    let low = 0xc1e0_0000_0020_0000u64; // -2147483649: first forbidden truncated lower bound.
    let high = 0x41e0_0000_0000_0000u64; // +2147483648: first forbidden upper bound.
    out.extend(
        [
            low - 1,
            low,
            low + 1,
            high - 1,
            high,
            high + 1,
            0xc1e0_0000_0018_0000,
            0x41df_ffff_fff0_0000, // -.75 / +.75 at bounds
            0,
            1 << 63,
            0x7ff8_0000_0000_0001,
            0x7ff0_0000_0000_0000,
            0xfff0_0000_0000_0000,
            0x3fe8_0000_0000_0000,
            0xbfe8_0000_0000_0000,
            0x8000_0000_0000_0001,
        ]
        .into_iter()
        .map(|bits| Case::Truncate(F64, bits)),
    );
    out.extend(
        [
            0xcf00_0001,
            0xcf00_0000,
            0xceff_ffff,
            0x4eff_ffff,
            0x4f00_0000,
            0x4f00_0001,
            0,
            0x8000_0000,
            0x7f80_0001,
            0x7f80_0000,
            0xff80_0000,
            0x8000_0001,
        ]
        .into_iter()
        .map(|bits| Case::Truncate(F32, bits)),
    );
    assert_eq!(out.len(), 96);
    out
}

/// Four logical shapes per partition, eight IDs. Seed is reproduction metadata only.
/// The order alternates arms deterministically (seed 20261026 / float-conversion-26).
fn fixture(partition: usize) -> (String, String, Vec<String>) {
    let all = cases();
    let group = &all[partition * 4..partition * 4 + 4];
    let mut source = String::from(
        "fn route[T: plain](x: T, y: T, pick: bool) -> T { if pick { x } else { y } }\n",
    );
    source.push_str("fn code(x: Result[i32, FloatIntError]) -> (u32, i32) { match x { Ok(n) => (0u32, n), Error(NotFinite) => (1u32, 0i32), Error(OutOfRange) => (2u32, 0i32) } }\n");
    let mut main = String::from(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\nlet output = io.stdout(rt) ?\n",
    );
    let mut expected = String::new();
    let mut ids = Vec::new();
    for (offset, case) in group.iter().enumerate() {
        let index = partition * 4 + offset;
        let (kind, literal) = case.operand();
        let ty = case.result_type();
        for runtime in [false, true] {
            let id = format!(
                "fc26-{index:02}-{}",
                if runtime { "parameter" } else { "literal" }
            );
            ids.push(id.clone());
            let name = format!("case_{index}_{}", u8::from(runtime));
            let invocation = if runtime {
                writeln!(source, "fn {name}(x: {kind}, y: {kind}, pick: bool) -> {ty} {{\nlet v = route(x, y, pick)\n{}\n}}", case.body("v")).unwrap();
                if (index + 20261026) % 2 == 0 {
                    format!("{name}({literal}, 0{kind}, true)")
                } else {
                    format!("{name}(0{kind}, {literal}, false)")
                }
            } else {
                writeln!(
                    source,
                    "fn {name}() -> {ty} {{\n{}\n}}",
                    case.body(&literal)
                )
                .unwrap();
                format!("{name}()")
            };
            let arity = if matches!(case, Case::Integer { .. } | Case::Truncate(..)) {
                4
            } else {
                2
            };
            let fields: Vec<_> = (0..arity)
                .map(|i| format!("v_{index}_{}_{i}", u8::from(runtime)))
                .collect();
            let rendered = fields
                .iter()
                .map(|field| format!("{{{field}}}"))
                .collect::<Vec<_>>()
                .join(" ");
            writeln!(
                main,
                "let ({}) = {invocation}\nlet _ = out.print(output, f\"{id} {rendered}\\n\")",
                fields.join(", ")
            )
            .unwrap();
            writeln!(expected, "{id} {}", case.expected()).unwrap();
        }
    }
    source.push_str(&main);
    source.push_str("Ok(())\n}\n");
    (source, expected, ids)
}

#[test]
fn oracle_golden_certificates() {
    for (word, expected) in [
        (0x3690_0000_0000_0000, 0),
        (0xb690_0000_0000_0000, 0x8000_0000),
        (0x3ff0_0000_1000_0000, 0x3f80_0000),
        (0x3ff0_0000_3000_0000, 0x3f80_0002),
        (0x47ef_ffff_f000_0000, 0x7f80_0000),
        (0x47ef_ffff_efff_ffff, 0x7f7f_ffff),
    ] {
        assert_eq!(rounded(decode(word, F64), F32), expected);
    }
    assert_eq!(
        rounded(
            Exact::Finite {
                negative: false,
                significand: 16777217,
                exponent: 0
            },
            F32
        ),
        0x4b80_0000
    );
    assert_eq!(
        truncated(decode(0xc1e0_0000_0018_0000, F64)),
        (0, -2147483648)
    );
    // A false direct/composed equality would erase the purpose of these controls.
    assert_ne!(
        rounded(
            Exact::Finite {
                negative: false,
                significand: 9007199791611905,
                exponent: 0
            },
            F32
        ),
        rounded(
            decode(
                rounded(
                    Exact::Finite {
                        negative: false,
                        significand: 9007199791611905,
                        exponent: 0
                    },
                    F64
                ),
                F64
            ),
            F32
        )
    );
}

// Four-engine harness (the pure prefix is independently executable by the evidence adapter).
fn partition(index: usize) {
    use crate::rc_support::history::{Category, Runner};
    use hirsel_driver::native::{
        Configuration, Engine, OptLevel, Options, Partition, Runtime, TargetSpec,
    };
    let (source, expected, ids) = fixture(index);
    let source_hash = format!(
        "{:?}",
        hirsel_syntax::ContentHash::of_bytes(source.as_bytes())
    );
    let expected_hash = format!(
        "{:?}",
        hirsel_syntax::ContentHash::of_bytes(expected.as_bytes())
    );
    eprintln!(
        "FLOAT_CONVERSION_START {}",
        serde_json::json!({"partition":index,"ids":ids,"source":source,"expected":expected,"source_hash":source_hash,"expected_hash":expected_hash})
    );
    let directory = tempfile::tempdir().unwrap();
    let matrix = [
        Engine::Heap { campaign: false },
        Engine::Native {
            opt: OptLevel::None,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    ]
    .into_iter()
    .map(|engine| Configuration {
        options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        engine,
    })
    .collect();
    let runner = Runner::with_matrix(directory.path(), matrix).unwrap();
    let report = hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program_source(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../hirsel-driver/tests/fixtures/std_numeric.hir"),
            Some(&source),
        )
        .unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let execution = hirsel_engine::run_observed_snapshot(
            &mut loaded.engine,
            &hirsel_engine::RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        let reference = hirsel_driver::native::Observation::of_reference(
            execution.observation.as_ref().unwrap(),
        );
        runner.compare(
            &validated,
            &loaded.entry,
            &reference,
            &format!("float-conversion-26-{index}"),
        )
    });
    let mut valid = report.validated && report.cells.len() == 4;
    // Every engine is compared directly with the integer oracle's output, independently.
    for cell in &report.cells {
        let pass = cell.category == Category::Observation
            && cell.observation.as_ref().is_some_and(|o| {
                o.output == expected.as_bytes()
                    && o.exit_code == Some(0)
                    && o.failures.is_empty()
                    && !o.blocked
                    && o.budget.is_none()
                    && o.signaled.is_none()
                    && o.roots.is_empty()
            });
        valid &= pass;
        eprintln!(
            "FLOAT_CONVERSION_CELL {}",
            serde_json::json!({"partition":index,"ids":ids,"source_hash":source_hash,"expected_hash":expected_hash,"engine":cell.engine,"category":format!("{:?}",cell.category),"pass":pass,"observation":cell.observation.as_ref().map(|o|o.render()),"detail":cell.detail})
        );
    }
    assert!(
        valid,
        "{ids:?}\n{source}\nexpected:\n{expected}\n{report:#?}"
    );
}

macro_rules! partitions {
    ($($name:ident: $index:literal),* $(,)?) => { $(#[test] fn $name() { partition($index); })* };
}
partitions! {
    p00:0,p01:1,p02:2,p03:3,p04:4,p05:5,p06:6,p07:7,p08:8,p09:9,p10:10,p11:11,
    p12:12,p13:13,p14:14,p15:15,p16:16,p17:17,p18:18,p19:19,p20:20,p21:21,p22:22,p23:23,
}

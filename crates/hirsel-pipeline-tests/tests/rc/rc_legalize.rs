//! Scalar legalization over real driver-derived programs. Every Ready unit
//! passes C, and the heap executor runs it with a clean audit and the
//! interpreter's outcome: the same returned scalar bits, or the same primary
//! failure rule on the same source line.
use crate::rc_support as support;

use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_heapcheck::{Config, NoOperations, Observed};
use hirsel_interp::observation::ObservedNode;
use hirsel_lir::ir::{BinaryOp, Callee, ConstantValue, Inst, LirUnit, ProjectionStep, Terminator};
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::Ready;
use hirsel_lir::validate::Inputs;
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::InstanceId;
use hirsel_rc::placement::Options;
use support::*;

/// The entry both executors run: the interpreter's entry returns
/// `Result[(), E]`, so `value()` comes back as the `Err` payload. It goes last,
/// so the source lines above it keep their numbers.
fn entry(t: &str) -> String {
    format!("pub fn main() -> Result[(), {t}] {{\n  Error(value())\n}}\n")
}

/// A scalar outcome both executors report.
#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    /// Returned bits (integers, floats, Bool as 0/1).
    Returned(u128),
    /// Primary failure rule and its 1-based source line.
    Failed(String, u32),
}

struct Ready_ {
    table: LayoutTable,
    units: Vec<LirUnit<Ready>>,
}

/// Rc (regions on, elision on), then `ready::ready`; C must accept every unit.
fn build(source: &str) -> Ready_ {
    let program = compile(source);
    let lowered = hirsel_rc::import::lower(&program, &target())
        .unwrap_or_else(|e| panic!("{source}\nlower: {e:?}"));
    let table = lowered.table.clone();
    let mut rc = Vec::new();
    for (i, u) in lowered.units.iter().enumerate() {
        let id = InstanceId(i as u32);
        let options = Options::for_instance(true, &lowered.instances, id);
        let placed = hirsel_rc::placement::place(u.clone(), &table, &options).unwrap();
        let proofs = Proofs::for_instance(&lowered.instances, id);
        let r = hirsel_rc::insert::insert(placed, &table, &proofs).unwrap();
        rc.push(hirsel_rc::elide::elide(r, &table).unwrap());
    }
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let units: Vec<_> = rc
        .into_iter()
        .map(|u| {
            hirsel_rc::ready::ready(u, &table, &finalizers)
                .unwrap_or_else(|e| panic!("{source}\nready: {e:?}"))
        })
        .collect();
    let manifest = manifest(&program);
    let target = target();
    let inputs = Inputs {
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    for u in &units {
        let v = hirsel_lir::validate::check(u, &inputs);
        assert!(v.is_empty(), "{source}\n{}\n{v:#?}", dump(u));
    }
    Ready_ { table, units }
}

fn line_of(source: &str, byte: u32) -> u32 {
    1 + source[..byte as usize].matches('\n').count() as u32
}

/// H's outcome of `main` in the Ready program, with a clean terminal audit,
/// and every fault-injected failure path audited too.
fn heap(source: &str, b: &Ready_) -> Outcome {
    let program = hirsel_heapcheck::Program {
        target: target(),
        layouts: b.table.clone(),
        units: b.units.clone(),
        statics: Vec::new(),
    };
    let campaign = hirsel_heapcheck::fault_campaign(
        &program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config::default(),
    );
    if let Some((site, v)) = campaign.first_violation() {
        let dumps: Vec<String> = b.units.iter().map(dump).collect();
        panic!("{source}\nsite {site:?}: {v}\n{}", dumps.join("\n"));
    }
    observed(source, campaign.clean.result.unwrap().observation)
}

/// H's outcome of `main` with a clean terminal audit, without a fault
/// campaign: for long straight-line programs, whose campaign reruns the
/// program once per injection site.
fn heap_clean(source: &str, b: &Ready_) -> Outcome {
    let program = hirsel_heapcheck::Program {
        target: target(),
        layouts: b.table.clone(),
        units: b.units.clone(),
        statics: Vec::new(),
    };
    let run = hirsel_heapcheck::run(
        &program,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    );
    let audit = run.result.unwrap_or_else(|v| {
        let dumps: Vec<String> = b.units.iter().map(dump).collect();
        panic!("{source}\n{v}\n{}", dumps.join("\n"))
    });
    observed(source, audit.observation)
}

fn observed(source: &str, o: hirsel_heapcheck::Observation) -> Outcome {
    if let Some(f) = o.failures.first() {
        let base = item_bases(source)[&f.definition];
        return Outcome::Failed(f.rule.clone(), line_of(source, base + f.site.0));
    }
    let payload = match o.returned {
        Some(Observed::Variant { fields, .. }) => fields.into_iter().next().flatten(),
        other => panic!("{source}\nreturned {other:?}"),
    };
    Outcome::Returned(match payload {
        Some(Observed::Integer { bits, .. }) => bits,
        Some(Observed::F64(bits)) => f64_bits(bits),
        Some(Observed::F32(bits)) => f32_bits(bits),
        Some(Observed::Bool(b)) => u128::from(b),
        other => panic!("{source}\nreturned {other:?}"),
    })
}

/// The interpreter's outcome of `main`.
fn reference(source: &str) -> Outcome {
    let source = source.to_string();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source.clone()).unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let config = RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
        let o = run.observation.expect("an observation");
        if let Some(f) = o.failures.first() {
            return Outcome::Failed(f.rule.as_str().to_string(), f.stable_site.line);
        }
        let value = o.returned.expect("a returned value");
        let payload = match &value.nodes[0] {
            ObservedNode::Aggregate { fields, .. } => fields[0].expect("an Err payload"),
            other => panic!("{source}\nreturned {other:?}"),
        };
        Outcome::Returned(match &value.nodes[payload] {
            ObservedNode::Integer(i) => i.bits(),
            ObservedNode::F64(bits) => f64_bits(*bits),
            ObservedNode::F32(bits) => f32_bits(*bits),
            ObservedNode::Bool(b) => u128::from(*b),
            other => panic!("{source}\nreturned {other:?}"),
        })
    })
}

/// Float bits as observed: each float type has one observable NaN (SEM-2),
/// the positive quiet canonical one.
fn f64_bits(bits: u64) -> u128 {
    u128::from(if f64::from_bits(bits).is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        bits
    })
}

fn f32_bits(bits: u32) -> u128 {
    u128::from(if f32::from_bits(bits).is_nan() {
        0x7fc0_0000
    } else {
        bits
    })
}

/// `agree` with one clean H run instead of a fault campaign.
fn agree_clean(source: &str) -> Outcome {
    let b = build(source);
    let h = heap_clean(source, &b);
    let i = reference(source);
    assert_eq!(h, i, "{source}\nH disagrees with I");
    h
}

/// C accepts, H and I agree; the agreed outcome.
fn agree(source: &str) -> Outcome {
    let b = build(source);
    let h = heap(source, &b);
    let i = reference(source);
    assert_eq!(h, i, "{source}\nH disagrees with I");
    h
}

#[test]
fn flat_part_copies_audit_every_failure_after_materialization() {
    let source = r#"
flat type Message { text: String, bytes: Bytes, id: u64 }
flat type Messages { items: List[Message] }
fn copied(message: Message) -> Message in message { copy message }
fn extract_text(take x: Message) -> String { copy x.text }
fn extract_bytes(take x: Message) -> Bytes { copy x.bytes }
fn checksum(xs: Messages) -> u64 {
  let mut sum = 0u64
  for message in xs.items {
    let mut changed = copied(copy message)
    changed.id ^= 291u64
    sum += changed.id
  }
  sum
}
fn value() -> u64 {
  let xs = Messages(items: [Message(text: "hello", bytes: b"abc", id: 7)])
  let sum = checksum(xs)
  let text = extract_text(Message(text: "owned", bytes: b"", id: 1))
  let bytes = extract_bytes(Message(text: "", bytes: b"bytes", id: 2))
  sum + 7u64 + string.byte_len(text) + bytes.len(bytes)
}
pub fn main() -> Result[(), u64] { Error(value()) }
"#;
    assert_eq!(agree(source), Outcome::Returned(309));
}

const SIMD: &str = "fn value() -> bool {
  let a: Simd[u8, 4] = simd.splat(LEFT)
  let b: Simd[u8, 4] = simd.splat(RIGHT)
  let c: Simd[u8, 4] = simd.add(a, b)
  let d: Simd[u8, 4] = simd.add(c, b)
  d == simd.splat(EXPECTED)
}
";

fn simd(left: u8, right: u8, expected: u8) -> String {
    SIMD.replace("LEFT", &format!("{left}u8"))
        .replace("RIGHT", &format!("{right}u8"))
        .replace("EXPECTED", &format!("{expected}u8"))
        + &entry("bool")
}

#[test]
fn simd_lanes_add_in_ascending_order_with_checked_lanes() {
    assert_eq!(agree(&simd(1, 2, 5)), Outcome::Returned(1));
    let b = build(&simd(1, 2, 5));
    let main = b
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "value")
        .unwrap();
    // No SIMD operation call is left, and each add is four checked lane adds
    // whose lane indices ascend.
    for t in &main.terminators {
        if let Terminator::Call { call, .. } = *t {
            assert!(!matches!(
                main.tables.calls[call.index()].callee,
                Callee::Operation(_)
            ));
        }
    }
    let index_of = |v: hirsel_lir::ir::ValueId| {
        main.instructions.iter().find_map(|i| match *i {
            Inst::Const { result, constant } if result == v => {
                match main.tables.constants[constant.index()].value {
                    ConstantValue::Integer(n) => Some(n),
                    _ => None,
                }
            }
            _ => None,
        })
    };
    let lane_of = |v: hirsel_lir::ir::ValueId| {
        main.instructions.iter().find_map(|i| match *i {
            Inst::Read {
                result, address, ..
            } if result == v => {
                let info = &main.addresses[address.index()];
                match info.projection.get(&main.pools.projections).unwrap() {
                    [ProjectionStep::Index(i)] => index_of(*i),
                    _ => None,
                }
            }
            _ => None,
        })
    };
    let lanes: Vec<_> = main
        .instructions
        .iter()
        .filter_map(|i| match *i {
            Inst::Binary {
                op: BinaryOp::Add,
                left,
                right,
                ..
            } => {
                let lane = lane_of(left);
                assert_eq!(lane, lane_of(right));
                lane
            }
            _ => None,
        })
        .collect();
    assert_eq!(lanes, [0, 1, 2, 3, 0, 1, 2, 3]);
}

#[test]
fn simd_overflow_fails_sem_1_at_the_first_overflowing_site() {
    // The second add overflows (100 + 100 + 100); the first does not.
    assert_eq!(
        agree(&simd(100, 100, 0)),
        Outcome::Failed("SEM-1".into(), 5)
    );
    // Both overflow: the first site is reported.
    assert_eq!(
        agree(&simd(200, 100, 0)),
        Outcome::Failed("SEM-1".into(), 4)
    );
}

const WIDTHS: [(&str, u32, bool); 10] = [
    ("u8", 8, false),
    ("u16", 16, false),
    ("u32", 32, false),
    ("u64", 64, false),
    ("u128", 128, false),
    ("i8", 8, true),
    ("i16", 16, true),
    ("i32", 32, true),
    ("i64", 64, true),
    ("i128", 128, true),
];

/// `value` returning `x op y` of type `t`, computed through a function so no
/// operand is a literal at the operation.
fn binary(t: &str, x: &str, op: &str, y: &str) -> String {
    format!(
        "fn f(x: {t}, y: {t}) -> {t} {{
  x {op} y
}}
fn value() -> {t} {{
  f({x}{t}, {y}{t})
}}
{}",
        entry(t)
    )
}

/// Every width's division, remainder and shift at the edges where the
/// instruction alone would be poison or undefined: H and I agree, failing
/// SEM-1 on line 2 or returning the same bits.
#[test]
fn every_integer_width_divides_and_shifts_without_poison() {
    let fail = || Outcome::Failed("SEM-1".into(), 2);
    for (t, width, signed) in WIDTHS {
        let max = if signed {
            format!("{}", (1u128 << (width - 1)) - 1)
        } else {
            format!("{}", u128::MAX >> (128 - width))
        };
        let min = if signed {
            format!("-{}", 1u128 << (width - 1))
        } else {
            "0".into()
        };
        assert_eq!(agree(&binary(t, "7", "/", "0")), fail(), "{t} / 0");
        assert_eq!(agree(&binary(t, "7", "%", "0")), fail(), "{t} % 0");
        assert_eq!(
            agree(&binary(t, &max, "/", "3")),
            Outcome::Returned(max.parse::<u128>().unwrap() / 3)
        );
        let w = width.to_string();
        assert_eq!(agree(&binary(t, "1", "<<", &w)), fail(), "{t} << {w}");
        assert_eq!(agree(&binary(t, "1", ">>", &w)), fail(), "{t} >> {w}");
        let last = (width - 1).to_string();
        let mask = u128::MAX >> (128 - width);
        assert_eq!(
            agree(&binary(t, "1", "<<", &last)),
            Outcome::Returned((1u128 << (width - 1)) & mask)
        );
        if signed {
            assert_eq!(agree(&binary(t, &min, "/", "-1")), fail(), "{t} MIN / -1");
            // MIN % -1 is zero, never the remainder instruction's overflow.
            assert_eq!(agree(&binary(t, &min, "%", "-1")), Outcome::Returned(0));
            assert_eq!(agree(&binary(t, "1", "<<", "-1")), fail(), "{t} << -1");
            // An arithmetic shift of MIN by width - 1 is all ones.
            assert_eq!(
                agree(&binary(t, &min, ">>", &last)),
                Outcome::Returned(mask)
            );
        }
    }
}

/// `value` returning `x op y` over two `t` bit patterns.
fn float(t: &str, x: u64, op: &str, y: u64) -> String {
    format!(
        "fn f(x: {t}, y: {t}) -> {t} {{
  x {op} y
}}
fn value() -> {t} {{
  f({}, {})
}}
{}",
        literal(t, x),
        literal(t, y),
        entry(t)
    )
}

/// A source literal for a float bit pattern: finite values in exact
/// scientific form, specials by expression.
fn literal(t: &str, bits: u64) -> String {
    let (value, negative) = match t {
        "f64" => {
            let v = f64::from_bits(bits);
            (v, v.is_sign_negative())
        }
        _ => {
            let v = f64::from(f32::from_bits(bits as u32));
            (v, v.is_sign_negative())
        }
    };
    if value.is_nan() {
        return format!("(0.0{t} / 0.0{t})");
    }
    if value.is_infinite() {
        return format!("({}1.0{t} / 0.0{t})", if negative { "-" } else { "" });
    }
    if value == 0.0 && negative {
        return format!("(-0.0{t})");
    }
    let text = match t {
        "f64" => format!("{:e}", value),
        _ => format!("{:e}", value as f32),
    };
    let text = if text.contains('.') {
        text
    } else {
        text.replacen('e', ".0e", 1)
    };
    if let Some(rest) = text.strip_prefix('-') {
        format!("(-{rest}{t})")
    } else {
        format!("{text}{t}")
    }
}

/// IEEE results bit for bit: rounding, signed zeros, infinities, NaN,
/// subnormals and remainder, in both widths. Expected bits are Rust's IEEE
/// operations; NaN results compare as any NaN (the payload is not observable
/// in source).
#[test]
fn floats_match_exact_vectors() {
    let f64s: [f64; 10] = [
        0.1,
        0.2,
        -0.0,
        0.0,
        1.0,
        -3.5,
        f64::INFINITY,
        f64::MIN_POSITIVE / 4.0,
        1e308,
        7.0,
    ];
    type Op = fn(f64, f64) -> f64;
    let ops: [(&str, Op); 5] = [
        ("+", |a, b| a + b),
        ("-", |a, b| a - b),
        ("*", |a, b| a * b),
        ("/", |a, b| a / b),
        ("%", |a, b| a % b),
    ];
    let pairs = [
        (0, 1),
        (2, 3),
        (3, 2),
        (4, 2),
        (6, 6),
        (7, 4),
        (8, 8),
        (5, 9),
        (9, 5),
        (1, 3),
    ];
    for (t, narrow) in [("f64", false), ("f32", true)] {
        for &(i, j) in &pairs {
            for (op, apply) in ops {
                let (x, y) = (f64s[i], f64s[j]);
                let (xb, yb, expected) = if narrow {
                    let (x, y) = (x as f32, y as f32);
                    let r = apply(f64::from(x), f64::from(y)) as f32;
                    // Each f32 operation is exact then rounded once; f64
                    // evaluation of + - * / then one rounding is exact for f32
                    // (double rounding is innocuous at 53 >= 2*24+2 bits).
                    (
                        u64::from(x.to_bits()),
                        u64::from(y.to_bits()),
                        (r.is_nan(), u128::from(r.to_bits())),
                    )
                } else {
                    let r = apply(x, y);
                    (
                        x.to_bits(),
                        y.to_bits(),
                        (r.is_nan(), u128::from(r.to_bits())),
                    )
                };
                let source = float(t, xb, op, yb);
                let outcome = agree(&source);
                let Outcome::Returned(bits) = outcome else {
                    panic!("{source}\n{outcome:?}");
                };
                if expected.0 {
                    let nan = if narrow {
                        f32::from_bits(bits as u32).is_nan()
                    } else {
                        f64::from_bits(bits as u64).is_nan()
                    };
                    assert!(nan, "{source}\nexpected NaN, got {bits:#x}");
                } else {
                    assert_eq!(bits, expected.1, "{source}");
                }
            }
        }
    }
}

/// Integer `+ - *` on i128 and u128: checked at their own width.
#[test]
fn i128_arithmetic_is_checked_at_its_own_width() {
    let max = i128::MAX.to_string();
    assert_eq!(
        agree(&binary("i128", &max, "+", "1")),
        Outcome::Failed("SEM-1".into(), 2)
    );
    assert_eq!(
        agree(&binary(
            "i128",
            "-9223372036854775808",
            "*",
            "9223372036854775807"
        )),
        Outcome::Returned((i128::from(i64::MIN) * i128::from(i64::MAX)) as u128)
    );
    assert_eq!(
        agree(&binary("u128", "0", "-", "1")),
        Outcome::Failed("SEM-1".into(), 2)
    );
    assert_eq!(
        agree(&binary(
            "u128",
            "18446744073709551616",
            "*",
            "18446744073709551615"
        )),
        Outcome::Returned((1u128 << 64) * u128::from(u64::MAX))
    );
}

/// C admits a lane of the Simd element's scalar kind under another layout
/// id (the type's own `u8` beside the Simd layout's), and only that: the
/// same construction with one lane of another kind is rejected.
#[test]
fn a_simd_lane_of_another_scalar_kind_is_rejected() {
    let source = simd(1, 2, 5);
    let b = build(&source);
    let program = compile(&source);
    let manifest = manifest(&program);
    let target = target();
    let inputs = Inputs {
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: &b.table,
        manifest: &manifest,
    };
    let mut unit = b
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "value")
        .unwrap()
        .clone();
    let kind = |u: &LirUnit<Ready>, v: hirsel_lir::ir::ValueId| {
        let layout = u.types[u.values[v.index()].ty.index()].layout;
        b.table.layout(layout).unwrap().kind.clone()
    };
    let flag = (0..unit.values.len() as u32)
        .map(hirsel_lir::ir::ValueId)
        .find(|&v| {
            kind(&unit, v)
                == hirsel_lir::layout_types::LayoutKind::Scalar(
                    hirsel_lir::layout_types::ScalarKind::Bool,
                )
        })
        .expect("the comparison's Bool");
    let splat = unit
        .tables
        .constructs
        .iter()
        .position(|c| c.constructor == hirsel_lir::ir::ConstructorKind::Sequence)
        .unwrap();
    let fields = unit.tables.constructs[splat].fields;
    let first = fields.start as usize;
    unit.pools.fields[first].value = flag;
    let v = hirsel_lir::validate::check(&unit, &inputs);
    assert!(
        v.iter()
            .any(|v| v.kind == hirsel_lir::validate::ViolationKind::Type("constructed fields")),
        "{v:#?}"
    );
}

fn ledger() -> hirsel_rc::legalize::Ledger {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../native-tests/rewrites/ledger.tsv");
    hirsel_rc::legalize::Ledger::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Reruns the evidence the ledger's proved rows cite: every operand of the
/// 8- and 16-bit widths, and the boundary vectors of 32 and 64 bits.
#[test]
fn mul_two_ledger_evidence() {
    macro_rules! exhaustive {
        ($($t:ty),*) => {$(
            for x in <$t>::MIN..=<$t>::MAX {
                assert_eq!(x.checked_mul(2), x.checked_add(x), "{x}");
            }
        )*};
    }
    exhaustive!(u8, i8, u16, i16);
    macro_rules! boundary {
        ($($t:ty),*) => {$(
            let half = <$t>::MAX / 2;
            let min_half = <$t>::MIN / 2;
            for x in [
                <$t>::MIN,
                min_half.wrapping_sub(1),
                min_half,
                (0 as $t).wrapping_sub(1),
                0,
                1,
                half,
                half.wrapping_add(1),
                <$t>::MAX,
            ] {
                assert_eq!(x.checked_mul(2), x.checked_add(x), "{x}");
            }
        )*};
    }
    boundary!(u32, i32, u64, i64);
    let ledger = ledger();
    use hirsel_lir::layout_types::ScalarKind as S;
    use hirsel_rc::legalize::{Domain, MUL_TWO, Status};
    for k in [S::U8, S::I8, S::U16, S::I16, S::U32, S::I32, S::U64, S::I64] {
        assert_eq!(ledger.status(MUL_TWO, Domain::Integer(k)), Status::Proved);
    }
    for d in [
        Domain::Integer(S::U128),
        Domain::Integer(S::I128),
        Domain::Float(S::F32),
        Domain::Float(S::F64),
    ] {
        assert_eq!(ledger.status(MUL_TWO, d), Status::Unknown, "{d:?}");
    }
}

/// `value` returning `f(x)` where `f` doubles its argument of type `t`.
fn double(t: &str, x: &str) -> String {
    format!(
        "fn f(x: {t}) -> {t} {{
  x * 2{t}
}}
fn value() -> {t} {{
  f({x})
}}
{}",
        entry(t)
    )
}

/// Discover, prove against `ledger`, commit; C validates every committed
/// unit. The plans committed and the plans left off.
type Rewritten = (
    Ready_,
    Vec<hirsel_rc::legalize::Plan>,
    Vec<(hirsel_rc::legalize::Plan, hirsel_rc::legalize::Status)>,
);

fn rewrite(b: Ready_, source: &str, ledger: &hirsel_rc::legalize::Ledger) -> Rewritten {
    let program = compile(source);
    let manifest = manifest(&program);
    let target = target();
    let (mut on, mut off) = (Vec::new(), Vec::new());
    let units = b
        .units
        .into_iter()
        .map(|u| {
            let before = u.digest();
            let plans = hirsel_rc::legalize::discover(&u, &b.table);
            assert_eq!(u.digest(), before, "discovery changed the unit");
            let (proved, left) = hirsel_rc::legalize::prove(plans, ledger);
            let u = hirsel_rc::legalize::commit(u, &proved);
            let inputs = Inputs {
                discharge: None,
                operations: &program.operation_registry(),
                target: &target,
                layouts: &b.table,
                manifest: &manifest,
            };
            let v = hirsel_lir::validate::check(&u, &inputs);
            assert!(v.is_empty(), "{source}\n{}\n{v:#?}", dump(&u));
            on.extend(proved);
            off.extend(left);
            u
        })
        .collect();
    (
        Ready_ {
            table: b.table,
            units,
        },
        on,
        off,
    )
}

#[test]
fn a_proved_rewrite_commits_revalidates_and_keeps_the_failure_site() {
    let ledger = ledger();
    for (t, x, expected) in [
        ("u32", "7u32", Outcome::Returned(14)),
        ("u32", "3000000000u32", Outcome::Failed("SEM-1".into(), 2)),
        ("i64", "-5i64", Outcome::Returned((-10i64) as u64 as u128)),
        ("i8", "-65i8", Outcome::Failed("SEM-1".into(), 2)),
    ] {
        let source = double(t, x);
        let (b, on, off) = rewrite(build(&source), &source, &ledger);
        assert_eq!(on.len(), 1, "{source}");
        assert!(off.is_empty());
        let f = b
            .units
            .iter()
            .find(|u| u.header.definition.0.name == "f")
            .unwrap();
        assert!(
            !f.instructions.iter().any(|i| matches!(
                i,
                Inst::Binary {
                    op: BinaryOp::Mul,
                    ..
                }
            )),
            "{source}"
        );
        let h = heap(&source, &b);
        assert_eq!(h, reference(&source), "{source}");
        assert_eq!(h, expected, "{source}");
    }
}

#[test]
fn an_unproved_domain_keeps_its_rewrite_off() {
    use hirsel_rc::legalize::Status;
    let ledger = ledger();
    for (t, x) in [("i128", "7i128"), ("u128", "7u128"), ("f64", "1.5f64")] {
        let source = double(t, x);
        let built = build(&source);
        let digests: Vec<_> = built.units.iter().map(|u| u.digest()).collect();
        let (b, on, off) = rewrite(built, &source, &ledger);
        assert!(on.is_empty(), "{source}");
        assert_eq!(off.len(), 1, "{source}");
        assert_eq!(off[0].1, Status::Unknown);
        assert_eq!(
            b.units.iter().map(|u| u.digest()).collect::<Vec<_>>(),
            digests
        );
        assert_eq!(heap(&source, &b), reference(&source), "{source}");
    }
    // A refuted row, or a ledger that lists nothing, keeps a proved-elsewhere
    // domain off too.
    let source = double("u32", "7u32");
    for text in [
        "rule\tdomain\tfailure\tlayout\tevidence\tstatus\nmul-two-as-add\tint:u32\t-\t-\tcounterexample\trefuted\n",
        "rule\tdomain\tfailure\tlayout\tevidence\tstatus\n",
    ] {
        let ledger = hirsel_rc::legalize::Ledger::parse(text).unwrap();
        let (_, on, off) = rewrite(build(&source), &source, &ledger);
        assert!(on.is_empty());
        assert_eq!(off.len(), 1);
    }
}

#[test]
fn a_malformed_ledger_is_rejected() {
    use hirsel_rc::legalize::Ledger;
    assert!(Ledger::parse("rule\tdomain\n").is_err());
    let header = "rule\tdomain\tfailure\tlayout\tevidence\tstatus\n";
    assert!(Ledger::parse(&format!("{header}mul-two-as-add\tint:u8\t-\t-\t-\tmaybe\n")).is_err());
    let row = "mul-two-as-add\tint:u8\t-\t-\t-\tproved\n";
    assert!(Ledger::parse(&format!("{header}{row}{row}")).is_err());
    assert!(Ledger::parse(&format!("{header}{row}")).is_ok());
}

// Scalar intrinsics (LIR schema revision 6): each call becomes its
// instruction. Every program checks under C, and H and the interpreter agree
// on it; Rust's own integer conversions and arithmetic are the oracle. The
// exhaustive edge matrix per instruction (H, interpreter kernels and native
// code) is `tests/front/llvm_scalar_library.rs`; these programs cover the
// lowering through the real pipeline.

/// `value() -> bool` asserting every term of `terms`, after `helpers`. The
/// terms are split into small functions joined pairwise up a tree: the front
/// end's cost grows steeply with the length of one `&&` chain (as one
/// function these tests take about 1.6 times as long; the native middle end
/// is near-linear, `tests/it/scaling.rs`).
fn all_of(helpers: &str, terms: &[String]) -> String {
    const GROUP: usize = 6;
    let mut source = helpers.to_string();
    let mut level: Vec<String> = terms.to_vec();
    let mut next = 0;
    while level.len() > GROUP {
        level = level
            .chunks(GROUP)
            .map(|chunk| {
                source += &format!("fn g{next}() -> bool {{\n  {}\n}}\n", chunk.join(" && "));
                next += 1;
                format!("g{}()", next - 1)
            })
            .collect();
    }
    format!(
        "{source}fn value() -> bool {{\n  {}\n}}\n{}",
        level.join(" && "),
        entry("bool")
    )
}

/// One integer source pair: the operand literals, `try_narrow`'s expected
/// result, `x as J` and whether widen and truncate apply.
struct Conversion {
    from: &'static str,
    to: &'static str,
    cases: Vec<(String, Option<String>, String)>,
    widens: bool,
    truncates: bool,
}

/// The conversions out of `$i`: its own bounds and -1 (or MAX), and each
/// destination bound that `$i` holds with its neighbour outside.
macro_rules! conversions {
    ($i:ident; $($j:ident),*) => {vec![$({
        let mut xs: Vec<$i> = vec![<$i>::MIN, <$i>::MAX, (0 as $i).wrapping_sub(1)];
        for bound in [<$i>::try_from(<$j>::MAX).ok(), <$i>::try_from(<$j>::MIN).ok()].into_iter().flatten() {
            xs.extend([Some(bound), bound.checked_add(1), bound.checked_sub(1)].into_iter().flatten());
        }
        xs.sort();
        xs.dedup();
        Conversion {
            from: stringify!($i),
            to: stringify!($j),
            cases: xs
                .iter()
                .map(|&x| (x.to_string(), <$j>::try_from(x).ok().map(|v| v.to_string()), (x as $j).to_string()))
                .collect(),
            widens: <$j>::try_from(<$i>::MIN).is_ok() && <$j>::try_from(<$i>::MAX).is_ok(),
            truncates: std::mem::size_of::<$j>() <= std::mem::size_of::<$i>(),
        }
    }),*]};
}

/// `try_narrow`, `narrow`, `widen` and `truncate` from one integer kind into
/// every integer kind (SEM-1, docs/spec/core-library.md): `Some`/`None` built by the lowering's
/// branch, and `narrow` failing SEM-1 at its call when the value is not
/// representable.
fn conversions_lower(conversions: Vec<Conversion>) {
    for c in conversions {
        let (i, j) = (c.from, c.to);
        let mut helpers = format!(
            "fn t(x: {i}) -> Option[{j}] {{\n  int.try_narrow(x)\n}}\nfn n(x: {i}) -> {j} {{\n  int.narrow(x)\n}}\n"
        );
        if c.widens {
            helpers += &format!("fn w(x: {i}) -> {j} {{\n  int.widen(x)\n}}\n");
        }
        if c.truncates {
            helpers += &format!("fn c(x: {i}) -> {j} {{\n  int.truncate(x)\n}}\n");
        }
        let mut terms = Vec::new();
        for (x, exact, wrapped) in &c.cases {
            match exact {
                Some(v) => {
                    terms.push(format!("t({x}{i}) == Some({v}{j})"));
                    terms.push(format!("n({x}{i}) == {v}{j}"));
                    if c.widens {
                        terms.push(format!("w({x}{i}) == {v}{j}"));
                    }
                }
                None => terms.push(format!("t({x}{i}) == None")),
            }
            if c.truncates {
                terms.push(format!("c({x}{i}) == {wrapped}{j}"));
            }
        }
        let source = all_of(&helpers, &terms);
        assert_eq!(
            agree_clean(&source),
            Outcome::Returned(1),
            "{i} -> {j}\n{source}"
        );
        if let Some((x, _, _)) = c.cases.iter().find(|(_, exact, _)| exact.is_none()) {
            let source = format!(
                "fn n(x: {i}) -> {j} {{\n  int.narrow(x)\n}}\nfn value() -> {j} {{\n  n({x}{i})\n}}\n{}",
                entry(j)
            );
            assert_eq!(
                agree(&source),
                Outcome::Failed("SEM-1".into(), 2),
                "narrow {x}{i} into {j}"
            );
        }
    }
}

/// The integer edges of one kind and every library operation over each pair:
/// the expected wrapping, saturating and checked sums, floor quotient,
/// floor remainder and exact quotient, or None where the operation fails.
struct Arithmetic {
    kind: &'static str,
    min: String,
    /// (a, b, wrapping, saturating, checked, div_floor, mod, div_exact)
    cases: Vec<[Option<String>; 8]>,
}

macro_rules! arithmetic {
    ($t:ident) => {{
        let xs: Vec<$t> = {
            let mut xs: Vec<$t> = vec![
                0,
                1,
                7,
                <$t>::MIN,
                <$t>::MAX,
                (0 as $t).wrapping_sub(1),
                (0 as $t).wrapping_sub(7),
            ];
            xs.sort();
            xs.dedup();
            xs
        };
        let s = |v: $t| Some(v.to_string());
        let mut cases = Vec::new();
        for &a in &xs {
            for &b in &xs {
                // The quotient rounded toward negative infinity and its remainder.
                let floor = a.checked_div(b).zip(a.checked_rem(b)).map(|(q, r)| {
                    if r != 0 && ((r < (0 as $t)) != (b < (0 as $t))) {
                        (q - 1, r + b)
                    } else {
                        (q, r)
                    }
                });
                cases.push([
                    s(a),
                    s(b),
                    s(a.wrapping_add(b)),
                    s(a.saturating_add(b)),
                    a.checked_add(b).map(|v| v.to_string()),
                    floor.map(|(q, _)| q.to_string()),
                    floor.map(|(_, r)| r.to_string()),
                    floor.filter(|&(_, r)| r == 0).map(|(q, _)| q.to_string()),
                ]);
            }
        }
        Arithmetic {
            kind: stringify!($t),
            min: <$t>::MIN.to_string(),
            cases,
        }
    }};
}

const ARITHMETIC_HELPERS: &str = "fn wa(a: T, b: T) -> T {
  int.wrapping_add(a, b)
}
fn sa(a: T, b: T) -> T {
  int.saturating_add(a, b)
}
fn ca(a: T, b: T) -> Option[T] {
  int.checked_add(a, b)
}
fn df(a: T, b: T) -> T {
  int.div_floor(a, b)
}
fn md(a: T, b: T) -> T {
  int.mod(a, b)
}
fn de(a: T, b: T) -> T {
  int.div_exact(a, b)
}
";

/// The line of each failing helper's call in `ARITHMETIC_HELPERS`.
const DIVISION_LINES: [(&str, u32); 3] = [("df", 11), ("md", 14), ("de", 17)];

/// Wrapping, saturating and checked addition and the three named divisions
/// of one kind over every pair of edges (SEM-1, docs/spec/core-library.md), and each division's
/// failures: a zero divisor, signed `MIN / -1` and an inexact `div_exact`.
fn arithmetic_lowers(a: Arithmetic) {
    let t = a.kind;
    let helpers = ARITHMETIC_HELPERS.replace('T', t);
    let mut terms = Vec::new();
    for [x, y, wrapping, saturating, checked, floor, modulo, exact] in &a.cases {
        let (x, y) = (x.as_deref().unwrap(), y.as_deref().unwrap());
        let call = |f: &str| format!("{f}({x}{t}, {y}{t})");
        terms.push(format!(
            "{} == {}{t}",
            call("wa"),
            wrapping.as_deref().unwrap()
        ));
        terms.push(format!(
            "{} == {}{t}",
            call("sa"),
            saturating.as_deref().unwrap()
        ));
        terms.push(match checked {
            Some(v) => format!("{} == Some({v}{t})", call("ca")),
            None => format!("{} == None", call("ca")),
        });
        for (f, v) in [("df", floor), ("md", modulo), ("de", exact)] {
            if let Some(v) = v {
                terms.push(format!("{} == {v}{t}", call(f)));
            }
        }
    }
    let source = all_of(&helpers, &terms);
    assert_eq!(agree_clean(&source), Outcome::Returned(1), "{t}\n{source}");
    let mut failing = vec![
        ("df", "7", "0"),
        ("md", "7", "0"),
        ("de", "7", "0"),
        ("de", "7", "2"),
    ];
    if a.min != "0" {
        for f in ["df", "md", "de"] {
            failing.push((f, a.min.as_str(), "-1"));
        }
    }
    for (f, x, y) in failing {
        let source = format!(
            "{helpers}fn value() -> {t} {{\n  {f}({x}{t}, {y}{t})\n}}\n{}",
            entry(t)
        );
        let line = DIVISION_LINES.iter().find(|(g, _)| *g == f).unwrap().1;
        assert_eq!(
            agree(&source),
            Outcome::Failed("SEM-1".into(), line),
            "{f}({x}{t}, {y}{t})"
        );
    }
}

/// One test per integer kind, so the kinds run in parallel.
macro_rules! integer_tests {
    ($($t:ident: $convert:ident, $arith:ident;)*) => {$(
        #[test]
        fn $convert() {
            conversions_lower(conversions!($t; u8, u16, u32, u64, u128, i8, i16, i32, i64, i128));
        }
        #[test]
        fn $arith() {
            arithmetic_lowers(arithmetic!($t));
        }
    )*};
}
integer_tests! {
    u8: conversions_from_u8_lower, u8_arithmetic_lowers;
    u16: conversions_from_u16_lower, u16_arithmetic_lowers;
    u32: conversions_from_u32_lower, u32_arithmetic_lowers;
    u64: conversions_from_u64_lower, u64_arithmetic_lowers;
    u128: conversions_from_u128_lower, u128_arithmetic_lowers;
    i8: conversions_from_i8_lower, i8_arithmetic_lowers;
    i16: conversions_from_i16_lower, i16_arithmetic_lowers;
    i32: conversions_from_i32_lower, i32_arithmetic_lowers;
    i64: conversions_from_i64_lower, i64_arithmetic_lowers;
    i128: conversions_from_i128_lower, i128_arithmetic_lowers;
}

/// The float edges as bit patterns: zeros, 1, a negative fraction, a
/// subnormal, the infinities and NaN.
fn float_edges(t: &str) -> Vec<u64> {
    if t == "f64" {
        [
            0.0f64,
            -0.0,
            1.0,
            -1.5,
            5e-324,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ]
        .iter()
        .map(|v| v.to_bits())
        .collect()
    } else {
        [
            0.0f32,
            -0.0,
            1.0,
            -1.5,
            1e-45,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ]
        .iter()
        .map(|v| u64::from(v.to_bits()))
        .collect()
    }
}

/// IEEE totalOrder of two canonical-NaN values of `t` (SEM-2), computed on
/// the bits: the order the min/max oracle breaks ties with.
fn total_order(t: &str, a: u64, b: u64) -> i32 {
    let key = |bits: u64| -> i64 {
        if t == "f64" {
            let v = if f64::from_bits(bits).is_nan() {
                0x7ff8_0000_0000_0000
            } else {
                bits
            } as i64;
            v ^ (((v >> 63) as u64) >> 1) as i64
        } else {
            let v = if f32::from_bits(bits as u32).is_nan() {
                0x7fc0_0000
            } else {
                bits as u32
            } as i32;
            i64::from(v ^ (((v >> 31) as u32) >> 1) as i32)
        }
    };
    key(a).cmp(&key(b)) as i32
}

/// IEEE 754-2019 minimum/maximum (`number`: minimumNumber/maximumNumber)
/// over bit patterns of `t`, with -0 < +0 (SEM-2).
fn min_max(t: &str, a: u64, b: u64, min: bool, number: bool) -> u64 {
    let nan = |x: u64| {
        if t == "f64" {
            f64::from_bits(x).is_nan()
        } else {
            f32::from_bits(x as u32).is_nan()
        }
    };
    match (nan(a), nan(b)) {
        (true, true) => a,
        (true, false) => {
            if number {
                b
            } else {
                a
            }
        }
        (false, true) => {
            if number {
                a
            } else {
                b
            }
        }
        _ => {
            if (total_order(t, a, b) < 0) == min {
                a
            } else {
                b
            }
        }
    }
}

/// `min`, `max`, `min_number`, `max_number` and `mul_add` of one float kind
/// over every pair of edges (`mul_add` over triples of a smaller set), each
/// result compared exactly through its NaN-canonical bits (SEM-2).
fn float_operations_lower(t: &str) {
    let edges = float_edges(t);
    let lit = |bits: u64| literal(t, bits);
    let helpers = format!("fn same(a: {t}, b: {t}) -> bool {{\n  {t}.bits(a) == {t}.bits(b)\n}}\n");
    let mut terms = Vec::new();
    for &a in &edges {
        for &b in &edges {
            for (name, min, number) in [
                ("min", true, false),
                ("max", false, false),
                ("min_number", true, true),
                ("max_number", false, true),
            ] {
                let r = min_max(t, a, b, min, number);
                terms.push(format!(
                    "same(float.{name}({}, {}), {})",
                    lit(a),
                    lit(b),
                    lit(r)
                ));
            }
        }
    }
    // mul_add rounds once: 0.1 * 10 - 1 is not zero, unlike two roundings.
    let fused: Vec<u64> = [0.1f64, 10.0, -1.0, f64::INFINITY]
        .iter()
        .map(|&v| {
            if t == "f64" {
                v.to_bits()
            } else {
                u64::from((v as f32).to_bits())
            }
        })
        .collect();
    for &a in &fused {
        for &b in &fused {
            for &c in &fused {
                let r = if t == "f64" {
                    f64::from_bits(a)
                        .mul_add(f64::from_bits(b), f64::from_bits(c))
                        .to_bits()
                } else {
                    let f = |x: u64| f32::from_bits(x as u32);
                    u64::from(f(a).mul_add(f(b), f(c)).to_bits())
                };
                terms.push(format!(
                    "same(float.mul_add({}, {}, {}), {})",
                    lit(a),
                    lit(b),
                    lit(c),
                    lit(r)
                ));
            }
        }
    }
    let source = all_of(&helpers, &terms);
    assert_eq!(agree_clean(&source), Outcome::Returned(1), "{t}\n{source}");
}

#[test]
fn f32_operations_lower() {
    float_operations_lower("f32");
}

#[test]
fn f64_operations_lower() {
    float_operations_lower("f64");
}

/// `float.to_i32` truncates toward zero into `Ok`, and returns
/// `Error(NotFinite)` for NaN and infinities and `Error(OutOfRange)` outside
/// i32 (docs/spec/core-library.md): the lowering's three-way switch at both widths.
#[test]
fn float_to_i32_lowers_into_its_result() {
    for t in ["f32", "f64"] {
        let values: Vec<f64> = vec![
            0.0,
            -0.0,
            -3.9,
            3.9,
            2147483647.0,
            2147483647.9,
            2147483648.0,
            -2147483648.0,
            -2147483648.9,
            -2147483649.0,
            5e-324,
            1e300,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        let mut terms = Vec::new();
        for v in values {
            let (bits, x) = if t == "f64" {
                (v.to_bits(), v)
            } else {
                let v = v as f32;
                (u64::from(v.to_bits()), f64::from(v))
            };
            let expected = if !x.is_finite() {
                "Error(NotFinite)".to_string()
            } else if x.trunc() < f64::from(i32::MIN) || x.trunc() > f64::from(i32::MAX) {
                "Error(OutOfRange)".to_string()
            } else {
                format!("Ok({}i32)", x.trunc() as i32)
            };
            terms.push(format!("float.to_i32({}) == {expected}", literal(t, bits)));
        }
        let source = all_of("", &terms);
        assert_eq!(agree_clean(&source), Outcome::Returned(1), "{t}\n{source}");
    }
}

#[test]
fn float_from_int_legalizes_signed_unsigned_limits_and_nearest_even_ties() {
    for (source_type, literals) in [
        (
            "i64",
            vec![
                "0",
                "1",
                "-1",
                "-9223372036854775808",
                "16777217",
                "9007199254740993",
            ],
        ),
        (
            "u64",
            vec![
                "0",
                "1",
                "18446744073709551615",
                "16777219",
                "9007199254740995",
            ],
        ),
        ("u128", vec!["340282366920938463463374607431768211455"]),
    ] {
        for destination in ["f32", "f64"] {
            for n in &literals {
                let (f32_bits, f64_bits) = if source_type == "i64" {
                    let n: i64 = n.parse().unwrap();
                    ((n as f32).to_bits(), (n as f64).to_bits())
                } else {
                    let n: u128 = n.parse().unwrap();
                    ((n as f32).to_bits(), (n as f64).to_bits())
                };
                let bits = if destination == "f32" {
                    u64::from(f32_bits)
                } else {
                    f64_bits
                };
                let source = format!(
                    "fn value() -> {destination} {{ float.from_int({n}{source_type}) }}\n{}",
                    entry(destination)
                );
                assert_eq!(
                    agree_clean(&source),
                    Outcome::Returned(u128::from(bits)),
                    "{source}"
                );
                let b = build(&source);
                assert!(
                    b.units
                        .iter()
                        .any(|u| u.instructions.iter().any(|i| matches!(
                            i,
                            Inst::Convert {
                                mode: hirsel_lir::ir::ConvertMode::IntToFloat,
                                flag: None,
                                ..
                            }
                        ))),
                    "conversion was not legalized"
                );
            }
        }
    }
}

#[test]
fn float_bits_legalize_to_reinterpret_and_canonicalize_every_nan() {
    for (t, body, bits) in [
        (
            "u32",
            "f32.bits(f32.from_bits(2147483648u32))",
            0x8000_0000u128,
        ),
        ("u32", "f32.bits(f32.from_bits(1u32))", 1),
        ("u32", "f32.bits(1.5f32)", u128::from(1.5f32.to_bits())),
        ("u32", "f32.bits(f32.from_bits(4290847557u32))", 0x7fc0_0000),
        ("u32", "f32.bits(f32.from_bits(2139095041u32))", 0x7fc0_0000),
        ("u32", "f32.bits(0.0f32 / 0.0f32)", 0x7fc0_0000),
        (
            "u64",
            "f64.bits(f64.from_bits(9223372036854775808u64))",
            1 << 63,
        ),
        (
            "u64",
            "f64.bits(f64.from_bits(18444512289894207488u64))",
            0x7ff8_0000_0000_0000,
        ),
        (
            "u64",
            "f64.bits(f64.from_bits(9218868437227405313u64))",
            0x7ff8_0000_0000_0000,
        ),
        ("u64", "f64.bits(0.0f64 / 0.0f64)", 0x7ff8_0000_0000_0000),
        ("u64", "f64.bits(-2.5f64)", u128::from((-2.5f64).to_bits())),
    ] {
        let source = format!("fn value() -> {t} {{ {body} }}\n{}", entry(t));
        assert_eq!(agree_clean(&source), Outcome::Returned(bits), "{source}");
        let b = build(&source);
        let reinterprets = b
            .units
            .iter()
            .flat_map(|u| &u.instructions)
            .filter(|i| {
                matches!(
                    i,
                    Inst::Convert {
                        mode: hirsel_lir::ir::ConvertMode::Reinterpret,
                        flag: None,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            reinterprets,
            body.matches("bits(").count(),
            "every bits operation is legalized: {source}"
        );
    }
}

//! The emission fixture programs, each built for one target.

use hirsel_core_contract::Manifest;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::{Ready, ValidatedUnit};
use hirsel_syntax::PassingMode;
use hirsel_types::Primitive;

use std::collections::BTreeMap;

use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::statics::StaticData;
use hirsel_syntax::ContentHash;

use super::*;

/// A sealed program: its table, manifest and units.
pub struct Program {
    pub table: Table,
    pub manifest: Manifest,
    pub units: Vec<ValidatedUnit<Ready>>,
    pub statics: Vec<StaticData>,
    /// Per layout with a finalizer, its drop body's instance digest.
    pub finalizers: BTreeMap<LayoutId, ContentHash>,
}

impl Program {
    pub fn units(&self) -> Vec<&ValidatedUnit<Ready>> {
        self.units.iter().collect()
    }

    /// The program's glue object (`hirsel_lir::glue`) with the fixture failure codes: the
    /// registry, cells, codecs, statics, literals and callbacks its unit objects import.
    pub fn glue(&self) -> Vec<u8> {
        let mut glue = hirsel_lir::glue::UnitGlue::default();
        for unit in &self.units {
            let needs = hirsel_lir::glue::UnitGlue::of(
                unit.unit(),
                &self.table.table,
                &self.table.keys,
                &self.manifest,
            )
            .unwrap();
            glue.extend(&needs);
        }
        let callbacks: Vec<_> = glue
            .callbacks
            .iter()
            .map(|(definition, instance)| {
                let header = self
                    .units
                    .iter()
                    .map(|u| &u.unit().header)
                    .find(|h| {
                        h.definition == *definition && instance.is_none_or(|i| h.instance == i)
                    })
                    .expect("a callback's unit");
                hirsel_lir::emission::GlueCallback {
                    definition: definition.clone(),
                    instance: Some(header.instance),
                    abi: header.abi,
                }
            })
            .collect();
        let failures = failure_codes();
        let inputs = hirsel_lir::emission::GlueInputs {
            target: &self.table.target,
            layouts: &self.table.table,
            keys: &self.table.keys,
            registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &self.table.table,
                &self.finalizers,
            ),
            finalizers: &self.finalizers,
            glue: &glue,
            failures: &failures,
            statics: &self.statics,
            callbacks: &callbacks,
            foreign: &hirsel_lir::emission::NO_FOREIGN,
            aliasing_unstated: !callbacks.is_empty(),
        };
        hirsel_llvm::emit_glue(&inputs)
            .unwrap_or_else(|e| panic!("{}: {e}", self.table.target.triple))
            .bytes
    }

    /// The ABI of the unit defining `name`.
    pub fn abi_of(&self, name: &str) -> &hirsel_lir::abi_types::FnAbi {
        let unit = self
            .units
            .iter()
            .map(|u| u.unit())
            .find(|u| u.header.definition.0.name == name)
            .unwrap_or_else(|| panic!("no unit {name}"));
        &self.table.table.fn_abis[unit.header.abi.0 as usize]
    }

    /// The C wrapper `wrapper` of the ABI v1 signature calling the unit `name`'s entry
    /// (`hirsel_llvm::c_call`).
    pub fn c_call(&self, name: &str, wrapper: &str) -> String {
        let symbol = hirsel_lir::emission::symbol_name(&path(name), Some(&instance(name)));
        hirsel_llvm::c_call(&self.table.table, self.abi_of(name), &symbol, wrapper)
    }
}

pub const INTEGERS: [ScalarKind; 10] = [
    ScalarKind::U8,
    ScalarKind::U16,
    ScalarKind::U32,
    ScalarKind::U64,
    ScalarKind::U128,
    ScalarKind::I8,
    ScalarKind::I16,
    ScalarKind::I32,
    ScalarKind::I64,
    ScalarKind::I128,
];

pub fn primitive(kind: ScalarKind) -> Primitive {
    match kind {
        ScalarKind::Bool => Primitive::Bool,
        ScalarKind::U8 => Primitive::U8,
        ScalarKind::U16 => Primitive::U16,
        ScalarKind::U32 => Primitive::U32,
        ScalarKind::U64 => Primitive::U64,
        ScalarKind::U128 => Primitive::U128,
        ScalarKind::I8 => Primitive::I8,
        ScalarKind::I16 => Primitive::I16,
        ScalarKind::I32 => Primitive::I32,
        ScalarKind::I64 => Primitive::I64,
        ScalarKind::I128 => Primitive::I128,
        ScalarKind::F32 => Primitive::F32,
        ScalarKind::F64 => Primitive::F64,
        ScalarKind::Pointer => panic!("no pointer primitive"),
    }
}

pub fn kind_name(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::Bool => "bool",
        ScalarKind::U8 => "u8",
        ScalarKind::U16 => "u16",
        ScalarKind::U32 => "u32",
        ScalarKind::U64 => "u64",
        ScalarKind::U128 => "u128",
        ScalarKind::I8 => "i8",
        ScalarKind::I16 => "i16",
        ScalarKind::I32 => "i32",
        ScalarKind::I64 => "i64",
        ScalarKind::I128 => "i128",
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::Pointer => "ptr",
    }
}

pub fn op_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
        BinaryOp::Shl => "shl",
        BinaryOp::Shr => "shr",
        BinaryOp::BitAnd => "and",
        BinaryOp::BitOr => "or",
        BinaryOp::BitXor => "xor",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::DivFloor => "div_floor",
        BinaryOp::Mod => "mod",
        BinaryOp::DivExact => "div_exact",
        BinaryOp::Min => "min",
        BinaryOp::Max => "max",
        BinaryOp::MinNumber => "min_number",
        BinaryOp::MaxNumber => "max_number",
    }
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// One scalar operation exported as `name(a, b) -> result`.
#[derive(Clone, Copy, Debug)]
pub struct ScalarCase {
    pub name: &'static str,
    pub op: Result<BinaryOp, UnaryOp>,
    pub kind: ScalarKind,
    pub result: ScalarKind,
}

fn comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

/// Every checked integer operation on every width, float
/// arithmetic and comparisons, and the unary operators.
pub fn scalar_cases() -> Vec<ScalarCase> {
    use BinaryOp as B;
    let mut cases = Vec::new();
    let integer_ops = [
        B::Add,
        B::Sub,
        B::Mul,
        B::Div,
        B::Rem,
        B::Shl,
        B::Shr,
        B::BitAnd,
        B::BitXor,
        B::Lt,
        B::Ge,
    ];
    for kind in INTEGERS {
        for op in integer_ops {
            cases.push(ScalarCase {
                name: leak(format!("{}_{}", op_name(op), kind_name(kind))),
                op: Ok(op),
                kind,
                result: if comparison(op) {
                    ScalarKind::Bool
                } else {
                    kind
                },
            });
        }
        let signed = matches!(
            kind,
            ScalarKind::I8 | ScalarKind::I16 | ScalarKind::I32 | ScalarKind::I64 | ScalarKind::I128
        );
        let unary: &[(UnaryOp, &str)] = if signed {
            &[(UnaryOp::Neg, "neg"), (UnaryOp::BitNot, "bitnot")]
        } else {
            &[(UnaryOp::Not, "not"), (UnaryOp::BitNot, "bitnot")]
        };
        for &(op, name) in unary {
            cases.push(ScalarCase {
                name: leak(format!("{name}_{}", kind_name(kind))),
                op: Err(op),
                kind,
                result: kind,
            });
        }
    }
    for kind in [ScalarKind::F32, ScalarKind::F64] {
        for op in [
            B::Add,
            B::Sub,
            B::Mul,
            B::Div,
            B::Rem,
            B::Eq,
            B::Ne,
            B::Lt,
            B::Ge,
        ] {
            cases.push(ScalarCase {
                name: leak(format!("{}_{}", op_name(op), kind_name(kind))),
                op: Ok(op),
                kind,
                result: if comparison(op) {
                    ScalarKind::Bool
                } else {
                    kind
                },
            });
        }
        cases.push(ScalarCase {
            name: leak(format!("neg_{}", kind_name(kind))),
            op: Err(UnaryOp::Neg),
            kind,
            result: kind,
        });
    }
    for op in [B::Eq, B::BitAnd, B::BitOr, B::BitXor] {
        cases.push(ScalarCase {
            name: leak(format!("{}_bool", op_name(op))),
            op: Ok(op),
            kind: ScalarKind::Bool,
            result: ScalarKind::Bool,
        });
    }
    cases.push(ScalarCase {
        name: "not_bool",
        op: Err(UnaryOp::Not),
        kind: ScalarKind::Bool,
        result: ScalarKind::Bool,
    });
    cases
}

/// Each scalar case as an exported unit taking its operands by value.
pub fn scalar_program(target: &TargetSpec) -> Program {
    let cases = scalar_cases();
    let mut schema = Schema::new();
    for case in &cases {
        let a = schema.types.p(primitive(case.kind));
        let r = schema.types.p(primitive(case.result));
        let parameters = match case.op {
            Ok(_) => {
                let b = schema.types.p(primitive(case.kind));
                vec![(PassingMode::Borrow, a), (PassingMode::Borrow, b)]
            }
            Err(_) => vec![(PassingMode::Borrow, a)],
        };
        schema.signature(case.name, &parameters, r);
    }
    let table = schema.build(target);
    let manifest = manifest();
    let units = cases
        .iter()
        .map(|case| {
            let mut u = Unit::new(&table, case.name, case.name);
            let a = u.param(table.scalar(case.kind));
            let result = match case.op {
                Ok(op) => {
                    let b = u.param(table.scalar(case.kind));
                    u.binary(ENTRY, op, a, b)
                }
                Err(op) => u.unary(ENTRY, op, a),
            };
            u.builder.ret(ENTRY, Some(result));
            seal(&table, &manifest, u.finish())
        })
        .collect();
    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: BTreeMap::new(),
    }
}

/// Names of the C side of the ABI probes: Hirsel ABI v1 entries the harness defines.
pub const HOST_SCALE: &str = "host_scale";

/// The C ABI, call, packed, i128 and status probes: Hirsel entries the C harness calls,
/// which call each other, a C-defined Hirsel entry and a C closure thunk.
pub fn abi_program(target: &TargetSpec) -> Program {
    use PassingMode::{Borrow, Take};
    let mut schema = Schema::new();
    let t = &mut schema.types;
    let (i64_, i128_, u8_, u16_, u64_, bool_, i8_, f64_) = (
        t.p(Primitive::I64),
        t.p(Primitive::I128),
        t.p(Primitive::U8),
        t.p(Primitive::U16),
        t.p(Primitive::U64),
        t.p(Primitive::Bool),
        t.p(Primitive::I8),
        t.p(Primitive::F64),
    );
    let unit = t.unit();
    let pair = t.record(&[i128_, i128_]);
    let opt = t.variant(&[&[], &[i64_]]);
    let arr = t.inline(i64_, 4);
    let wire_path = wire_owner(&mut schema.owners);
    let wire = schema.types.named(&wire_path, Vec::new());
    let callback =
        schema
            .types
            .function(&[(Borrow, i64_)], i64_, hirsel_types::Multiplicity::Many, 0);
    schema.name("pair", pair);
    schema.name("opt", opt);
    schema.name("arr", arr);
    schema.name("wire", wire);
    schema.name("callback", callback);
    schema.name("unit", unit);
    schema.signature("i64 -> i64", &[(Borrow, i64_)], i64_);
    schema.signature("pair -> i128", &[(Borrow, pair)], i128_);
    schema.signature("take pair -> i128", &[(Take, pair)], i128_);
    schema.signature(
        "i128 i128 -> pair",
        &[(Borrow, i128_), (Borrow, i128_)],
        pair,
    );
    schema.signature("wire -> u64", &[(Borrow, wire)], u64_);
    schema.signature(
        "u8 u64 u16 -> wire",
        &[(Borrow, u8_), (Borrow, u64_), (Borrow, u16_)],
        wire,
    );
    schema.signature("opt -> i64", &[(Borrow, opt)], i64_);
    schema.signature("i64 -> opt", &[(Borrow, i64_)], opt);
    schema.signature("arr u64 -> i64", &[(Borrow, arr), (Borrow, u64_)], i64_);
    schema.signature("u64 -> u64", &[(Borrow, u64_)], u64_);
    schema.signature(
        "callback i64 -> i64",
        &[(Borrow, callback), (Borrow, i64_)],
        i64_,
    );
    schema.shaped(
        "callback",
        hirsel_lir::abi_types::CallableShape::Closure,
        &[(Borrow, i64_)],
        i64_,
    );
    schema.signature("bool i8 -> i8", &[(Borrow, bool_), (Borrow, i8_)], i8_);
    schema.signature(
        "f64 f64 f64 -> f64",
        &[(Borrow, f64_), (Borrow, f64_), (Borrow, f64_)],
        f64_,
    );
    let table = schema.build(target);
    let manifest = manifest();
    let i64_l = table.scalar(ScalarKind::I64);
    let i128_l = table.scalar(ScalarKind::I128);
    let u64_l = table.scalar(ScalarKind::U64);
    let mut units = Vec::new();
    let mut add = |u: Unit<'_>| units.push(seal(&table, &manifest, u.finish()));

    // inc(x) = x + 1
    let mut u = Unit::new(&table, "inc", "i64 -> i64");
    let x = u.param(i64_l);
    let one = u.int(ENTRY, ScalarKind::I64, 1);
    let r = u.binary(ENTRY, BinaryOp::Add, x, one);
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // twice_inc(x) = inc(inc(x)): direct calls inside the object; a callee failure
    // propagates.
    let mut u = Unit::new(&table, "twice_inc", "i64 -> i64");
    let x = u.param(i64_l);
    let second = u.builder.block(BlockKind::Body, &[]);
    let done = u.builder.block(BlockKind::Body, &[]);
    let inc = Callee::Direct(u.symbol(SymbolKind::Function, "inc"));
    let f1 = u.fails(None);
    let y = u
        .call(
            ENTRY,
            inc,
            "i64 -> i64",
            &[(Passing::Borrow, ArgumentValue::Value(x))],
            second,
            &[],
            f1,
        )
        .unwrap();
    let f2 = u.fails(None);
    let z = u
        .call(
            second,
            inc,
            "i64 -> i64",
            &[(Passing::Borrow, ArgumentValue::Value(y))],
            done,
            &[],
            f2,
        )
        .unwrap();
    u.builder.ret(done, Some(z));
    add(u);

    // host_call(x) = host_scale(x) + 1: a call into C, whose failure propagates.
    let mut u = Unit::new(&table, "host_call", "i64 -> i64");
    let x = u.param(i64_l);
    let after = u.builder.block(BlockKind::Body, &[]);
    let host = Callee::Direct(u.symbol(SymbolKind::Function, HOST_SCALE));
    let f = u.fails(None);
    let h = u
        .call(
            ENTRY,
            host,
            "i64 -> i64",
            &[(Passing::Borrow, ArgumentValue::Value(x))],
            after,
            &[],
            f,
        )
        .unwrap();
    let one = u.int(after, ScalarKind::I64, 1);
    let r = u.binary(after, BinaryOp::Add, h, one);
    u.builder.ret(after, Some(r));
    add(u);

    // pair_sum(borrow p) = p.0 + p.1: a borrowed aggregate by read-only address.
    let pair_l = table.layout("pair");
    let mut u = Unit::new(&table, "pair_sum", "pair -> i128");
    let p = u.param(pair_l);
    let slot = u.slot(pair_l, SlotKind::Parameter(0));
    let a0 = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(0)], i128_l);
    let a1 = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(1)], i128_l);
    u.prepare(ENTRY, a0, None, SourceAnchorId(0));
    u.prepare(ENTRY, a1, None, SourceAnchorId(0));
    let v0 = u.read(ENTRY, a0, p);
    let v1 = u.read(ENTRY, a1, p);
    let r = u.binary(ENTRY, BinaryOp::Add, v0, v1);
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // pair_take(take p) = p.0 - p.1: a taken aggregate by transferring address.
    let mut u = Unit::new(&table, "pair_take", "take pair -> i128");
    let p = u.param(pair_l);
    let slot = u.slot(pair_l, SlotKind::Parameter(0));
    let a0 = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(0)], i128_l);
    let a1 = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(1)], i128_l);
    u.prepare(ENTRY, a0, None, SourceAnchorId(0));
    u.prepare(ENTRY, a1, None, SourceAnchorId(0));
    let v0 = u.read(ENTRY, a0, p);
    let v1 = u.read(ENTRY, a1, p);
    let r = u.binary(ENTRY, BinaryOp::Sub, v0, v1);
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // pair_make(a, b) = Pair { a, b }: an aggregate result through the output pointer.
    let mut u = Unit::new(&table, "pair_make", "i128 i128 -> pair");
    let a = u.param(i128_l);
    let b = u.param(i128_l);
    let symbol = u.symbol(SymbolKind::Type, "Pair");
    let r = u.construct(
        ENTRY,
        ConstructorKind::Record(symbol),
        pair_l,
        &[(0, a), (1, b)],
    );
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // wire_value(borrow w) = w.1 + 1: an unaligned u64 read in a `@packed` record.
    let wire_l = table.layout("wire");
    let mut u = Unit::new(&table, "wire_value", "wire -> u64");
    let w = u.param(wire_l);
    let slot = u.slot(wire_l, SlotKind::Parameter(0));
    let a1 = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(1)], u64_l);
    u.prepare(ENTRY, a1, None, SourceAnchorId(0));
    let v = u.read(ENTRY, a1, w);
    let one = u.int(ENTRY, ScalarKind::U64, 1);
    let r = u.binary(ENTRY, BinaryOp::Add, v, one);
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // wire_make(a, b, c) = Wire { a, b, c }: a packed construction returned at alignment 1.
    let mut u = Unit::new(&table, "wire_make", "u8 u64 u16 -> wire");
    let a = u.param(table.scalar(ScalarKind::U8));
    let b = u.param(u64_l);
    let c = u.param(table.scalar(ScalarKind::U16));
    let symbol = u.symbol(SymbolKind::Type, "Wire");
    let r = u.construct(
        ENTRY,
        ConstructorKind::Record(symbol),
        wire_l,
        &[(0, a), (1, b), (2, c)],
    );
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // opt_get(borrow o) = o.Some.0, failing MEM-13 on None.
    let opt_l = table.layout("opt");
    let mut u = Unit::new(&table, "opt_get", "opt -> i64");
    let o = u.param(opt_l);
    let slot = u.slot(opt_l, SlotKind::Parameter(0));
    let field = u.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        i64_l,
    );
    let tag_anchor = u.anchor(40, 50);
    u.prepare(ENTRY, field, Some("MEM-13"), tag_anchor);
    let v = u.read(ENTRY, field, o);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    // opt_some(x) = Some(x).
    let mut u = Unit::new(&table, "opt_some", "i64 -> opt");
    let x = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Opt");
    let r = u.construct(
        ENTRY,
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        opt_l,
        &[(0, x)],
    );
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // arr_get(borrow a, i) = a[i], failing MEM-14 out of bounds.
    let arr_l = table.layout("arr");
    let mut u = Unit::new(&table, "arr_get", "arr u64 -> i64");
    let a = u.param(arr_l);
    let i = u.param(u64_l);
    let slot = u.slot(arr_l, SlotKind::Parameter(0));
    let element = u.address(AddressBase::Slot(slot), &[ProjectionStep::Index(i)], i64_l);
    let bounds_anchor = u.anchor(60, 70);
    u.prepare(ENTRY, element, Some("MEM-14"), bounds_anchor);
    let v = u.read(ENTRY, element, a);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    // sum_below(n) = 0 + 1 + ... + (n - 1): a loop through block parameters.
    let mut u = Unit::new(&table, "sum_below", "u64 -> u64");
    let n = u.param(u64_l);
    let i = u.plain(u64_l);
    let acc = u.plain(u64_l);
    let head = u.builder.block(BlockKind::Body, &[i, acc]);
    let body = u.builder.block(BlockKind::Body, &[]);
    let exit = u.builder.block(BlockKind::Body, &[]);
    let zero = u.int(ENTRY, ScalarKind::U64, 0);
    u.builder.jump(ENTRY, head, &[zero, zero]);
    let more = u.binary(head, BinaryOp::Lt, i, n);
    let yes = u.builder.edge(body, &[]);
    let no = u.builder.edge(exit, &[]);
    u.builder.term(
        head,
        Terminator::Branch {
            condition: more,
            yes,
            no,
        },
    );
    let next_acc = u.binary(body, BinaryOp::Add, acc, i);
    let one = u.int(body, ScalarKind::U64, 1);
    let next_i = u.binary(body, BinaryOp::Add, i, one);
    u.builder.jump(body, head, &[next_i, next_acc]);
    u.builder.ret(exit, Some(acc));
    add(u);

    // apply(f, x) = f(x) + 1: an indirect call through a callable value from C.
    let callback_l = table.layout("callback");
    let mut u = Unit::new(&table, "apply", "callback i64 -> i64");
    let f = u.param(callback_l);
    let x = u.param(i64_l);
    let after = u.builder.block(BlockKind::Body, &[]);
    let failure = u.fails(None);
    let y = u
        .call(
            ENTRY,
            Callee::Indirect(f),
            "callback",
            &[(Passing::Borrow, ArgumentValue::Value(x))],
            after,
            &[],
            failure,
        )
        .unwrap();
    let one = u.int(after, ScalarKind::I64, 1);
    let r = u.binary(after, BinaryOp::Add, y, one);
    u.builder.ret(after, Some(r));
    add(u);

    // positive(x) = x, failing LANG-9 unless x > 0: a source check at its own site.
    let mut u = Unit::new(&table, "positive", "i64 -> i64");
    let x = u.param(i64_l);
    let zero = u.int(ENTRY, ScalarKind::I64, 0);
    let ok = u.binary(ENTRY, BinaryOp::Gt, x, zero);
    let check_anchor = u.anchor(30, 40);
    let p = u.builder.propagate();
    let failure = u.failure_to(p, Some("LANG-9"), check_anchor);
    u.builder.push(
        ENTRY,
        Inst::Check {
            condition: ok,
            failure,
        },
    );
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // choose(b, x) = if b { x } else { -x }: narrow arguments and a join.
    let i8_l = table.scalar(ScalarKind::I8);
    let mut u = Unit::new(&table, "choose", "bool i8 -> i8");
    let b = u.param(table.scalar(ScalarKind::Bool));
    let x = u.param(i8_l);
    let joined = u.plain(i8_l);
    let negate = u.builder.block(BlockKind::Body, &[]);
    let join = u.builder.block(BlockKind::Body, &[joined]);
    let yes = u.builder.edge(join, &[x]);
    let no = u.builder.edge(negate, &[]);
    u.builder.term(
        ENTRY,
        Terminator::Branch {
            condition: b,
            yes,
            no,
        },
    );
    let negated = u.unary(negate, UnaryOp::Neg, x);
    u.builder.jump(negate, join, &[negated]);
    u.builder.ret(join, Some(joined));
    add(u);

    // fused(a, b, c) = mul_add(a, b, c): `MulAdd`, one rounding (LIR schema revision 6).
    let f64_l = table.scalar(ScalarKind::F64);
    let mut u = Unit::new(&table, "fused", "f64 f64 f64 -> f64");
    let (a, b, c) = (u.param(f64_l), u.param(f64_l), u.param(f64_l));
    let r = u.plain(f64_l);
    u.builder.push(
        ENTRY,
        Inst::MulAdd {
            result: r,
            left: a,
            right: b,
            addend: c,
        },
    );
    u.builder.ret(ENTRY, Some(r));
    add(u);

    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: BTreeMap::new(),
    }
}

/// The payload layout of a block layout.
pub fn payload(table: &Table, block: &str) -> LayoutId {
    let id = table.layout(block);
    match table.table.layout(id).map(|r| &r.kind) {
        Some(
            hirsel_lir::layout_types::LayoutKind::Block { payload }
            | hirsel_lir::layout_types::LayoutKind::Box { payload },
        ) => *payload,
        other => panic!("{block} is not a block layout: {other:?}"),
    }
}

/// Heap blocks: allocation, payload reads, release through drop glue, a
/// release on a failure path, and reset/reuse. Each entry frees everything it allocates.
pub fn heap_program(target: &TargetSpec) -> Program {
    use PassingMode::Borrow;
    let mut schema = Schema::new();
    let t = &mut schema.types;
    let (i64_, u64_) = (t.p(Primitive::I64), t.p(Primitive::U64));
    let box_i64 = t.boxed(i64_);
    let rec = t.record(&[i64_, box_i64]);
    let box_rec = t.boxed(rec);
    let list = t.list(i64_);
    schema.name("box_i64", box_i64);
    schema.name("rec", rec);
    schema.name("box_rec", box_rec);
    schema.name("list", list);
    // A list's layouts include another U64 scalar, so the parameter's is found by name.
    schema.name("u64", u64_);
    schema.signature("i64 -> i64", &[(Borrow, i64_)], i64_);
    schema.signature("i64 i64 -> i64", &[(Borrow, i64_), (Borrow, i64_)], i64_);
    schema.signature(
        "i64 i64 i64 u64 -> i64",
        &[
            (Borrow, i64_),
            (Borrow, i64_),
            (Borrow, i64_),
            (Borrow, u64_),
        ],
        i64_,
    );
    let table = schema.build(target);
    let manifest = manifest();
    let i64_l = table.scalar(ScalarKind::I64);
    let u64_l = table.layout("u64");
    let mut units = Vec::new();
    let mut add = |u: Unit<'_>| units.push(seal(&table, &manifest, u.finish()));

    // box_roundtrip(x) = *Box(x)
    let (box_l, box_payload) = (table.layout("box_i64"), payload(&table, "box_i64"));
    let mut u = Unit::new(&table, "box_roundtrip", "i64 -> i64");
    let x = u.param(i64_l);
    let b = u.construct_block(ENTRY, ConstructorKind::Box, box_payload, box_l, &[(0, x)]);
    let at = u.address(AddressBase::Value(b), &[], i64_l);
    let failure = u.releasing(&[(b, box_payload)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(ENTRY, at, b);
    u.release(ENTRY, b, box_payload);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    // nested(x, y) = Box(Rec { x, Box(y) }).0: releasing the outer block releases the inner
    // one through the record's drop glue.
    let (rec_l, outer_l) = (payload(&table, "box_rec"), table.layout("box_rec"));
    let mut u = Unit::new(&table, "nested", "i64 i64 -> i64");
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let inner = u.construct_block(ENTRY, ConstructorKind::Box, box_payload, box_l, &[(0, y)]);
    let symbol = u.symbol(SymbolKind::Type, "Rec");
    // A failing construction has consumed its fields.
    let outer = u.construct_block(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec_l,
        outer_l,
        &[(0, x), (1, inner)],
    );
    let at = u.address(
        AddressBase::Value(outer),
        &[ProjectionStep::Field(0)],
        i64_l,
    );
    let failure = u.releasing(&[(outer, rec_l)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(ENTRY, at, outer);
    u.release(ENTRY, outer, rec_l);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    // list_get(a, b, c, i) = [a, b, c][i], failing MEM-14 out of bounds after releasing the
    // list on the failure path.
    let (list_l, list_payload) = (table.layout("list"), payload(&table, "list"));
    let mut u = Unit::new(&table, "list_get", "i64 i64 i64 u64 -> i64");
    let (a, b, c) = (u.param(i64_l), u.param(i64_l), u.param(i64_l));
    let i = u.param(u64_l);
    let l = u.construct_block(
        ENTRY,
        ConstructorKind::Sequence,
        list_payload,
        list_l,
        &[(0, a), (1, b), (2, c)],
    );
    let bounds = u.anchor(60, 70);
    let failure = u.releasing(&[(l, list_payload)], Some("MEM-14"), bounds);
    let at = u.address(AddressBase::Value(l), &[ProjectionStep::Index(i)], i64_l);
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(ENTRY, at, l);
    u.release(ENTRY, l, list_payload);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    // reuse(x, y) = *Box(y), built in the storage of a released Box(x).
    let mut u = Unit::new(&table, "reuse", "i64 i64 -> i64");
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let first = u.construct_block(ENTRY, ConstructorKind::Box, box_payload, box_l, &[(0, x)]);
    let token_ty = u.ty(ValueRepr::Token, table.header);
    u.builder.unit.values.push(ValueInfo {
        ty: token_ty,
        holding: Holding::Owned,
    });
    let token = ValueId(u.builder.unit.values.len() as u32 - 1);
    let anchor = u.cleanup_anchor(80, 90);
    let failure = u.fails(None);
    u.builder.push(
        ENTRY,
        Inst::Reset {
            result: token,
            value: first,
            layout: box_payload,
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    let failure = u.fails(None);
    let second = u.construct_block_to(
        ENTRY,
        ConstructorKind::Box,
        box_payload,
        box_l,
        &[(0, y)],
        failure,
        Some((token, None)),
    );
    let at = u.address(AddressBase::Value(second), &[], i64_l);
    let failure = u.releasing(&[(second, box_payload)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(ENTRY, at, second);
    u.release(ENTRY, second, box_payload);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: BTreeMap::new(),
    }
}

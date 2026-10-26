//! The Ready-construct probes: closures, function references, statics, comparison, the
//! uniqueness test, weakening, finalizers, regions, copies and partial masks. Every entry
//! takes two `i64`s and returns an `i64`, so one C table drives them natively and the heap
//! checker runs the same units for the expected observations.

use std::collections::BTreeMap;

use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::ScalarKind;
use hirsel_lir::statics::StaticData;
use hirsel_syntax::PassingMode;
use hirsel_types::{Modes, Multiplicity, Primitive};

use super::programs::{Program, payload};
use super::*;

/// The probe entries in harness order, with the argument pairs each runs on.
pub const READY_CASES: &[(&str, &[(i64, i64)])] = &[
    ("payload_write", &[(3, 4)]),
    ("nested_payload_write", &[(5, 6)]),
    ("closure_take", &[(3, 4), (i64::MAX, 1)]),
    ("closure_copy", &[(5, 6)]),
    ("function_ref", &[(10, 20), (i64::MAX, 0)]),
    ("is_unique", &[(1, 0)]),
    ("weaken", &[(8, 9)]),
    ("compare_boxes", &[(1, 2), (2, 2), (3, 2)]),
    ("compare_options", &[(1, 2), (2, 2), (-1, 0)]),
    ("compare_floats", &[(0, 0)]),
    (
        "compare_chain",
        &[(3, 0), (542, 0), (543, 0), (544, 0), (545, 0)],
    ),
    ("read_static", &[(2, 0)]),
    ("static_box", &[(5, 0)]),
    ("unique_copy", &[(4, 5)]),
    ("finalize_box", &[(7, 0), (0, 0)]),
    ("finalize_trace", &[(7, 0), (0, 0)]),
    ("region_sum", &[(3, 4), (i64::MAX, 1)]),
    ("mut_caller", &[(41, 100), (i64::MAX, 0)]),
    ("niche_option", &[(5, 0), (-3, 0)]),
    ("region_resource", &[(7, 0), (0, 0)]),
    ("compare_strings", &[(0, 0)]),
    ("literal_byte", &[(0, 0), (2, 0), (5, 0)]),
    ("finalize_explicit", &[(7, 0), (0, 0)]),
    ("finalize_trace_explicit", &[(7, 0), (0, 0)]),
    ("release_held", &[(7, 0), (0, 0)]),
    ("release_closure", &[(7, 0), (0, 0)]),
    ("release_variant", &[(7, 0), (0, 0)]),
    ("copy_pair", &[(5, 0)]),
    ("partial_release", &[(3, 4)]),
    ("reuse_kept", &[(3, 4)]),
    ("closure_reset", &[(3, 4), (i64::MAX, 1)]),
    ("flat_item", &[(3, 2), (3, 1), (-6, 4), (3, 5)]),
    ("flat_point", &[(3, 4), (-1, i64::MAX)]),
    ("flat_outer", &[(3, 4), (4, 4), (5, 4)]),
    ("flat_items", &[(0, 0), (1, 0), (2, 0)]),
    ("simd_lanes", &[(3, 4), (4, 3)]),
    ("seq_push", &[(3, 4), (5, 5), (-1, i64::MAX)]),
    ("flat_walk", &[(1, 0), (2, 0)]),
];

/// The entry signature.
pub const ENTRY_ABI: &str = "i64 i64 -> i64";

/// `close_held`'s signature: a taken handle and an `i64`.
pub const CLOSE_ABI: &str = "File i64 -> i64";

impl Unit<'_> {
    /// `cond ? yes : no` as an `i64`, in a join block returned with the value.
    pub fn select(&mut self, b: BlockId, cond: ValueId, yes: i64, no: i64) -> (BlockId, ValueId) {
        let i64_l = self.table.scalar(ScalarKind::I64);
        let result = self.plain(i64_l);
        let join = self.builder.block(BlockKind::Body, &[result]);
        let (y, n) = (
            self.builder.block(BlockKind::Body, &[]),
            self.builder.block(BlockKind::Body, &[]),
        );
        for (block, value) in [(y, yes), (n, no)] {
            let v = self.int(block, ScalarKind::I64, u128::from(value as u64));
            self.builder.jump(block, join, &[v]);
        }
        let (yes, no) = (self.builder.edge(y, &[]), self.builder.edge(n, &[]));
        self.builder.term(
            b,
            Terminator::Branch {
                condition: cond,
                yes,
                no,
            },
        );
        (join, result)
    }

    /// The sum of `bits` weighted by powers of two, most significant first: each a Bool.
    pub fn weigh(&mut self, b: BlockId, bits: &[ValueId]) -> (BlockId, ValueId) {
        let mut block = b;
        let mut total = self.int(block, ScalarKind::I64, 0);
        for (i, &bit) in bits.iter().enumerate() {
            let weight = 1i64 << (bits.len() - 1 - i);
            let (join, v) = self.select(block, bit, weight, 0);
            total = self.binary(join, BinaryOp::Add, total, v);
            block = join;
        }
        (block, total)
    }

    fn structural(&mut self, layout: LayoutId) {
        if !self
            .builder
            .unit
            .tables
            .structural
            .iter()
            .any(|plan| plan.root_layout() == layout)
        {
            self.builder.unit.tables.structural.push(
                self.table
                    .structural
                    .get(&layout)
                    .expect("declared structural operand plan")
                    .clone(),
            );
        }
    }

    /// A structural comparison of two values of `layout`.
    pub fn compare(
        &mut self,
        b: BlockId,
        op: CompareOp,
        left: ValueId,
        right: ValueId,
        layout: LayoutId,
        failure: FailureId,
    ) -> ValueId {
        self.structural(layout);
        let bool_l = self.table.scalar(ScalarKind::Bool);
        let result = self.plain(bool_l);
        self.builder.push(
            b,
            Inst::Compare {
                result,
                op,
                left,
                right,
                layout,
                failure,
            },
        );
        result
    }

    /// A construction of `payload` in local region `region`: an uncounted `block` reference.
    pub fn region_construct(
        &mut self,
        b: BlockId,
        constructor: ConstructorKind,
        payload: LayoutId,
        block: LayoutId,
        fields: &[(u32, ValueId)],
        failure: FailureId,
    ) -> ValueId {
        let fields = PoolRange::push(
            &mut self.builder.unit.pools.fields,
            fields
                .iter()
                .map(|&(field, value)| FieldValue { field, value }),
        );
        self.builder.unit.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor,
            layout: payload,
            fields,
            destination: DestinationClass::Region,
            placement: Some(Placement::Fixed(StorageOwner::LocalRegion(
                RegionInstanceId(0),
            ))),
            anchor: SourceAnchorId(0),
        });
        let construct = ConstructId(self.builder.unit.tables.constructs.len() as u32 - 1);
        let ty = self.ty(ValueRepr::Uncounted, block);
        self.builder.unit.values.push(ValueInfo {
            ty,
            holding: Holding::Plain,
        });
        let result = ValueId(self.builder.unit.values.len() as u32 - 1);
        self.builder.push(
            b,
            Inst::Construct {
                result,
                construct,
                failure,
            },
        );
        result
    }

    /// Enters the unit's one local region; returns its mark.
    pub fn enter_region(&mut self, b: BlockId) -> ValueId {
        let anchor = self.anchor(0, 5);
        self.builder.unit.tables.regions.push(RegionInstance {
            scope: ScopeId(0),
            anchor,
            seal_demand: false,
        });
        let i64_l = self.table.scalar(ScalarKind::I64);
        let ty = self.ty(ValueRepr::RegionMark, i64_l);
        self.builder.unit.values.push(ValueInfo {
            ty,
            holding: Holding::Owned,
        });
        let mark = ValueId(self.builder.unit.values.len() as u32 - 1);
        self.builder.push(
            b,
            Inst::RegionEnter {
                result: mark,
                region: RegionInstanceId(0),
            },
        );
        mark
    }

    /// A failure edge that exits the region of `mark` and propagates.
    pub fn exits(&mut self, mark: ValueId) -> FailureId {
        let cleanup = self.builder.block(BlockKind::FailureCleanup, &[]);
        self.exit_region(cleanup, mark);
        let p = self.builder.propagate();
        self.builder.jump(cleanup, p, &[]);
        self.failure_to(cleanup, None, SourceAnchorId(0))
    }

    pub fn exit_region(&mut self, b: BlockId, mark: ValueId) {
        let failure = self.fails(None);
        self.builder.push(
            b,
            Inst::RegionExit {
                mark,
                region: RegionInstanceId(0),
                failure,
            },
        );
    }

    /// A constant whose result holds a credit: a literal as a static block.
    pub fn owned_constant(
        &mut self,
        b: BlockId,
        layout: LayoutId,
        value: ConstantValue,
    ) -> ValueId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.builder
            .unit
            .tables
            .constants
            .push(ConstantData { ty, value });
        let constant = ConstantId(self.builder.unit.tables.constants.len() as u32 - 1);
        let result = self.value(layout, Holding::Owned);
        self.builder.push(b, Inst::Const { result, constant });
        result
    }

    pub fn is_unique(&mut self, b: BlockId, value: ValueId) -> ValueId {
        let bool_l = self.table.scalar(ScalarKind::Bool);
        let result = self.plain(bool_l);
        self.builder.push(b, Inst::IsUnique { result, value });
        result
    }

    /// A closure of `code` over `environment`, capturing `captures`, on the heap.
    #[allow(clippy::too_many_arguments)]
    pub fn closure(
        &mut self,
        b: BlockId,
        code: &str,
        abi: &str,
        environment: LayoutId,
        callable: LayoutId,
        captures: &[(ValueId, CaptureUse)],
        failure: FailureId,
    ) -> ValueId {
        let code = self.symbol(SymbolKind::Function, code);
        let captures = PoolRange::push(
            &mut self.builder.unit.pools.captures,
            captures
                .iter()
                .map(|&(value, usage)| CaptureValue { value, usage }),
        );
        self.builder.unit.tables.closures.push(ClosureSite {
            receipts: None,
            scoped: false,
            code: Some(ClosureCode {
                symbol: code,
                abi: self.table.abi(abi),
            }),
            environment,
            captures,
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
            anchor: SourceAnchorId(0),
        });
        let closure = ClosureId(self.builder.unit.tables.closures.len() as u32 - 1);
        let result = self.value(callable, Holding::Owned);
        self.builder.push(
            b,
            Inst::Closure {
                result,
                closure,
                failure,
            },
        );
        result
    }

    /// A capture slot of `layout` for environment field `ordinal`, and its entry version.
    pub fn capture(&mut self, ordinal: u32, layout: LayoutId) -> (SlotId, ValueId) {
        let slot = self.slot(layout, SlotKind::Capture(ordinal));
        let version = self.version(slot);
        self.builder.unit.captures.push(version);
        (slot, version)
    }

    /// Boxes the callable `value` and releases the box: a callable's environment is
    /// released through its holder. The `live` blocks are released if either step fails.
    pub fn release_callable(
        &mut self,
        b: BlockId,
        value: ValueId,
        boxed: LayoutId,
        live: &[(ValueId, LayoutId)],
    ) {
        let payload = payload(self.table, "box_fun");
        let failure = self.releasing(live, None, SourceAnchorId(0));
        let holder = self.construct_block_to(
            b,
            ConstructorKind::Box,
            payload,
            boxed,
            &[(0, value)],
            failure,
            None,
        );
        let failure = self.releasing(live, None, SourceAnchorId(0));
        self.release_to(b, holder, payload, failure);
    }
}

/// The nominal recursive `Chain = Nil | Cons(i64, Box<Chain>)`.
fn chain_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    let definition = path("Chain");
    let mut t = super::session_types();
    let i64_ = t.p(Primitive::I64);
    let own = t.named(&definition, Vec::new());
    let link = t.boxed(own);
    let shape = t.variant(&[&[], &[i64_, link]]);
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition.clone()),
        hirsel_layout::layout::OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("Chain revision"),
            generics: Vec::new(),
            private_shape: shape,
            layout_promises: hirsel_types::Layout::default(),
            resource: false,
            finalizer: None,
            unsafe_ffi_package: false,
        }),
    );
    definition
}

/// The resource `Res { i64 }` whose drop body `Res_drop` fails LANG-9 on a zero field.
fn resource_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    let definition = path("Res");
    let mut t = super::session_types();
    let i64_ = t.p(Primitive::I64);
    let shape = t.record(&[i64_]);
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition.clone()),
        hirsel_layout::layout::OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("Res revision"),
            generics: Vec::new(),
            private_shape: shape,
            layout_promises: hirsel_types::Layout::default(),
            resource: true,
            finalizer: Some((path("Res_drop"), hirsel_tir::BodyId(0))),
            unsafe_ffi_package: false,
        }),
    );
    definition
}

/// `resource type Trace { text: String, count: Box<Rec1> }` (DROP-2-reverse-fields' shape
/// with counted fields): its drop body `Trace_drop` fails LANG-9 when count.0 is zero, and
/// its fields are released after it, last first.
fn trace_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    let definition = path("Trace");
    let mut t = super::session_types();
    let i64_ = t.p(Primitive::I64);
    let rec1 = t.record(&[i64_]);
    let count = t.boxed(rec1);
    let text = t.p(Primitive::String);
    let shape = t.record(&[text, count]);
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition.clone()),
        hirsel_layout::layout::OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("Trace revision"),
            generics: Vec::new(),
            private_shape: shape,
            layout_promises: hirsel_types::Layout::default(),
            resource: true,
            finalizer: Some((path("Trace_drop"), hirsel_tir::BodyId(0))),
            unsafe_ffi_package: false,
        }),
    );
    definition
}

/// A flat nominal record of `fields`: FlatFormatV0 bytes (`flat type`).
fn flat_owner(
    owners: &mut hirsel_layout::layout::Owners,
    name: &str,
    fields: impl FnOnce(&mut Types) -> Vec<hirsel_types::TypeId>,
) -> DefPath {
    let definition = path(name);
    let mut t = super::session_types();
    let fields = fields(&mut t);
    let shape = t.record(&fields);
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition.clone()),
        hirsel_layout::layout::OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash(&format!("{name} revision")),
            generics: Vec::new(),
            private_shape: shape,
            layout_promises: hirsel_types::Layout {
                flat: true,
                ..hirsel_types::Layout::default()
            },
            resource: false,
            finalizer: None,
            unsafe_ffi_package: false,
        }),
    );
    definition
}

/// `flat type Item { text: String, n: i64 }`.
fn item_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    flat_owner(owners, "Item", |t| {
        vec![t.p(Primitive::String), t.p(Primitive::I64)]
    })
}

/// `flat type Items { items: List[Item] }`.
fn items_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    flat_owner(owners, "Items", |t| {
        let item = t.named(&path("Item"), Vec::new());
        vec![t.list(item)]
    })
}

/// `flat type Point { a: i64, b: i64 }`: fixed, so an inline value.
fn point_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    flat_owner(owners, "Point", |t| {
        vec![t.p(Primitive::I64), t.p(Primitive::I64)]
    })
}

/// `flat type Outer { item: Item, k: i64 }`: a variable flat record inside another.
fn outer_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    flat_owner(owners, "Outer", |t| {
        vec![t.named(&path("Item"), Vec::new()), t.p(Primitive::I64)]
    })
}

/// The run-context handle type `File`, closed through the Io adapter when dropped.
fn file_owner(owners: &mut hirsel_layout::layout::Owners) -> DefPath {
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&path("File")),
        hirsel_layout::layout::OwnerRepresentation::Handle {
            revision: hash("File revision"),
            resource: true,
        },
    );
    path("File")
}

pub fn ready_program(target: &TargetSpec) -> Program {
    use PassingMode::Borrow;
    let mut schema = Schema::new();
    let t = &mut schema.types;
    let i64_ = t.p(Primitive::I64);
    let rec1 = t.record(&[i64_]);
    let box_rec1 = t.boxed(rec1);
    let fun = t.function(&[(Borrow, i64_)], i64_, hirsel_types::Multiplicity::Many, 0);
    let box_fun = t.boxed(fun);
    let box_i64 = schema.types.boxed(i64_);
    schema.name("box_i64", box_i64);
    let box_box_i64 = schema.types.boxed(box_i64);
    schema.name("box_box_i64", box_box_i64);
    schema.name("rec1", rec1);
    schema.name("box_rec1", box_rec1);
    schema.name("fun", fun);
    schema.name("box_fun", box_fun);
    let t = &mut schema.types;
    let once_fun = {
        let kind = t.store.get(fun).unwrap().kind.clone();
        t.add_with(
            kind,
            Modes {
                multiplicity: Multiplicity::Once,
                ..Modes::global()
            },
        )
    };
    let box_once = t.boxed(once_fun);
    let f64_ = t.p(Primitive::F64);
    let rec_f = t.record(&[f64_, f64_]);
    let opt = t.variant(&[&[], &[i64_]]);
    let rec2 = t.record(&[i64_, box_rec1]);
    let opt_box = t.variant(&[&[], &[box_rec1]]);
    let box_opt = t.boxed(opt_box);
    let box_rec2 = t.boxed(rec2);
    let u8_ = t.p(Primitive::U8);
    let bytes = t.list(u8_);
    let string = t.p(Primitive::String);
    let chain_path = chain_owner(&mut schema.owners);
    let t = &mut schema.types;
    let chain = t.named(&chain_path, Vec::new());
    let box_chain = t.boxed(chain);
    schema.name("once_fun", once_fun);
    schema.name("box_once", box_once);
    schema.name("rec_f", rec_f);
    schema.name("opt", opt);
    schema.name("rec2", rec2);
    schema.name("opt_box", opt_box);
    schema.name("box_opt", box_opt);
    schema.name("box_rec2", box_rec2);
    schema.name("bytes", bytes);
    schema.name("string", string);
    schema.name("u8", u8_);
    let res_path = resource_owner(&mut schema.owners);
    let res = schema.types.named(&res_path, Vec::new());
    let box_res = schema.types.boxed(res);
    let unit_ = schema.types.unit();
    schema.name("res", res);
    schema.name("box_res", box_res);
    schema.signature("drop Res", &[(Borrow, res)], unit_);
    let trace_path = trace_owner(&mut schema.owners);
    let trace = schema.types.named(&trace_path, Vec::new());
    let box_trace = schema.types.boxed(trace);
    schema.name("trace", trace);
    schema.name("box_trace", box_trace);
    schema.signature("drop Trace", &[(Borrow, trace)], unit_);
    let t = &mut schema.types;
    let held = t.record(&[box_res, string]);
    let i64_ = t.p(Primitive::I64);
    let rel = t.variant(&[&[], &[i64_, box_res]]);
    let pair_s = t.record(&[box_rec1, string]);
    let pair = t.record(&[box_rec1, box_rec1]);
    let box_pair = t.boxed(pair);
    schema.name("held", held);
    schema.name("rel", rel);
    schema.name("pair_s", pair_s);
    schema.name("box_pair", box_pair);
    schema.environment("env_res", "env_res_add", &[(box_res, Passing::Take)]);
    let item_path = item_owner(&mut schema.owners);
    let items_path = items_owner(&mut schema.owners);
    let file_path = file_owner(&mut schema.owners);
    let t = &mut schema.types;
    let item = t.named(&item_path, Vec::new());
    let items = t.named(&items_path, Vec::new());
    let point_path = point_owner(&mut schema.owners);
    let t = &mut schema.types;
    let point = t.named(&point_path, Vec::new());
    let file = t.named(&file_path, Vec::new());
    let held_file = t.record(&[file, box_rec1]);
    // A dynamic flat value is a block reference to its FlatFormatV0 body.
    schema.name("item", item);
    schema.name("items", items);
    schema.name("point", point);
    let outer_path = outer_owner(&mut schema.owners);
    let outer = schema.types.named(&outer_path, Vec::new());
    schema.name("outer", outer);
    let simd4 = schema.types.simd(i64_, 4);
    schema.name("simd4", simd4);
    schema.name("file", file);
    schema.name("held_file", held_file);
    schema.signature(
        CLOSE_ABI,
        &[(PassingMode::Take, file), (Borrow, i64_)],
        i64_,
    );
    let i64_ = schema.types.p(Primitive::I64);
    schema.signature("mut i64", &[(PassingMode::Mut, i64_)], unit_);
    let bool_ = schema.types.p(Primitive::Bool);
    schema.name("bool", bool_);
    schema.name("chain", chain);
    schema.name("box_chain", box_chain);
    schema.environment("env_box", "env_add", &[(box_rec1, Passing::Take)]);
    schema.signature(ENTRY_ABI, &[(Borrow, i64_), (Borrow, i64_)], i64_);
    schema.signature("i64 -> i64", &[(Borrow, i64_)], i64_);
    schema.shaped(
        "closure i64 -> i64",
        CallableShape::Closure,
        &[(Borrow, i64_)],
        i64_,
    );
    let list64 = schema.types.list(i64_);
    schema.name("list64", list64);
    schema.signature(
        "push list64",
        &[(PassingMode::Mut, list64), (Borrow, i64_)],
        unit_,
    );
    let mut table = schema.build(target);
    table.structural = schema.structural_plans(
        &table,
        &[
            "box_rec1",
            "opt",
            "rec_f",
            "box_chain",
            "string",
            "box_opt",
            "list64",
        ],
    );

    let manifest = manifest();
    let i64_l = table.scalar(ScalarKind::I64);
    let (rec1_l, box_rec1_l) = (table.layout("rec1"), table.layout("box_rec1"));
    let (fun_l, box_fun_l) = (table.layout("fun"), table.layout("box_fun"));
    let env_box = table.layout("env_box");
    let box_i64_l = table.layout("box_i64");
    let (once_l, box_once_l) = (table.layout("once_fun"), table.layout("box_once"));
    let f64_l = table.scalar(ScalarKind::F64);
    let (rec_f_l, opt_l) = (table.layout("rec_f"), table.layout("opt"));
    let (rec2_l, box_rec2_l) = (table.layout("rec2"), table.layout("box_rec2"));
    let (bytes_l, string_l) = (table.layout("bytes"), table.layout("string"));
    let u8_l = table.layout("u8");
    let bool_l = table.scalar(ScalarKind::Bool);
    let mut statics = Vec::new();
    let mut units = Vec::new();
    let mut add = |u: Unit<'_>| units.push(seal(&table, &manifest, u.finish()));

    // Whole scalar payload transfers and writes, through one or two physical boxes.
    for (name, nested) in [("payload_write", false), ("nested_payload_write", true)] {
        let mut u = Unit::new(&table, name, ENTRY_ABI);
        let x = u.param(i64_l);
        let y = u.param(i64_l);
        let outer_l = if nested {
            table.layout("box_box_i64")
        } else {
            box_i64_l
        };
        let slot = u.slot(outer_l, SlotKind::Local);
        let whole = u.address(AddressBase::Slot(slot), &[], outer_l);
        u.prepare(ENTRY, whole, None, SourceAnchorId(0));
        let inner = u.construct_block(ENTRY, ConstructorKind::Box, i64_l, box_i64_l, &[(0, x)]);
        let (holder, _, outer_payload) = if nested {
            let outer_l = table.layout("box_box_i64");
            (
                u.construct_block(
                    ENTRY,
                    ConstructorKind::Box,
                    box_i64_l,
                    outer_l,
                    &[(0, inner)],
                ),
                outer_l,
                box_i64_l,
            )
        } else {
            (inner, box_i64_l, i64_l)
        };
        u.builder.push(ENTRY, Inst::StorageLive { slot });
        let version = u.store(ENTRY, whole, holder, slot);
        let steps = if nested {
            vec![ProjectionStep::Payload, ProjectionStep::Payload]
        } else {
            vec![ProjectionStep::Payload]
        };
        let at = u.address(AddressBase::Slot(slot), &steps, i64_l);
        let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
        let pointer = u.value(outer_l, Holding::Owned);
        u.builder.push(
            cleanup,
            Inst::Read {
                result: pointer,
                address: whole,
                reaching: version,
                mode: ReadMode::Take,
            },
        );
        u.builder.push(
            cleanup,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        u.release(cleanup, pointer, outer_payload);
        u.builder.term(cleanup, Terminator::Propagate);
        let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: at,
                failure,
            },
        );
        let old = u.plain(i64_l);
        u.builder.push(
            ENTRY,
            Inst::Read {
                result: old,
                address: at,
                reaching: version,
                mode: ReadMode::Take,
            },
        );
        let version = u.store(ENTRY, at, y, slot);
        let read = u.read(ENTRY, at, version);
        let pointer = u.value(outer_l, Holding::Owned);
        u.builder.push(
            ENTRY,
            Inst::Read {
                result: pointer,
                address: whole,
                reaching: version,
                mode: ReadMode::Take,
            },
        );
        u.builder.push(
            ENTRY,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        u.release(ENTRY, pointer, outer_payload);
        u.builder.ret(ENTRY, Some(read));
        add(u);
    }

    // env_add(y) = env.0.0 + y: the capture slot is the environment's boxed record.
    let mut u = Unit::new(&table, "env_add", "closure i64 -> i64");
    let y = u.param(i64_l);
    let (slot, version) = u.capture(0, box_rec1_l);
    let at = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(0)], i64_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let x = u.read(ENTRY, at, version);
    let sum = u.binary(ENTRY, BinaryOp::Add, x, y);
    u.builder.ret(ENTRY, Some(sum));
    add(u);

    // inc(x) = x + 1
    let mut u = Unit::new(&table, "inc", "i64 -> i64");
    let x = u.param(i64_l);
    let one = u.int(ENTRY, ScalarKind::I64, 1);
    let r = u.binary(ENTRY, BinaryOp::Add, x, one);
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // closure_take(x, y) = (|v| Box(Rec { x }).0 + v)(y), the box taken into the
    // environment and the callable released through a box on both edges.
    // closure_copy is the same with the box copied in and released after.
    for (name, usage) in [
        ("closure_take", CaptureUse::Take),
        ("closure_copy", CaptureUse::Copy),
    ] {
        let mut u = Unit::new(&table, name, ENTRY_ABI);
        let x = u.param(i64_l);
        let y = u.param(i64_l);
        let symbol = u.symbol(SymbolKind::Type, "Rec1");
        let b = u.construct_block(
            ENTRY,
            ConstructorKind::Record(symbol),
            rec1_l,
            box_rec1_l,
            &[(0, x)],
        );
        // A failing closure has consumed a taken capture but not a copied one.
        let failure = match usage {
            CaptureUse::Copy => u.releasing(&[(b, rec1_l)], None, SourceAnchorId(0)),
            _ => u.fails(None),
        };
        let f = u.closure(
            ENTRY,
            "env_add",
            "closure i64 -> i64",
            env_box,
            fun_l,
            &[(b, usage)],
            failure,
        );
        let normal = u.builder.block(BlockKind::Body, &[]);
        // The copied box outlives the callable.
        let copied: &[(ValueId, LayoutId)] = match usage {
            CaptureUse::Copy => &[(b, rec1_l)],
            _ => &[],
        };
        let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
        u.release_callable(cleanup, f, box_fun_l, copied);
        if usage == CaptureUse::Copy {
            u.release(cleanup, b, rec1_l);
        }
        let p = u.builder.propagate();
        u.builder.jump(cleanup, p, &[]);
        let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
        let r = u
            .call(
                ENTRY,
                Callee::Indirect(f),
                "closure i64 -> i64",
                &[(Passing::Borrow, ArgumentValue::Value(y))],
                normal,
                &[],
                failure,
            )
            .unwrap();
        u.release_callable(normal, f, box_fun_l, copied);
        if usage == CaptureUse::Copy {
            u.release(normal, b, rec1_l);
        }
        u.builder.ret(normal, Some(r));
        add(u);
    }

    // function_ref(x, y) = (inc)(x) + y through the function-reference thunk.
    let mut u = Unit::new(&table, "function_ref", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Function, "inc");
    // An owned callable over the static empty environment, released like any other.
    let f = u.value(fun_l, Holding::Owned);
    u.builder
        .push(ENTRY, Inst::FunctionRef { result: f, symbol });
    let normal = u.builder.block(BlockKind::Body, &[]);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    u.release_callable(cleanup, f, box_fun_l, &[]);
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    let r = u
        .call(
            ENTRY,
            Callee::Indirect(f),
            "closure i64 -> i64",
            &[(Passing::Borrow, ArgumentValue::Value(x))],
            normal,
            &[],
            failure,
        )
        .unwrap();
    u.release_callable(normal, f, box_fun_l, &[]);
    let sum = u.binary(normal, BinaryOp::Add, r, y);
    u.builder.ret(normal, Some(sum));
    add(u);

    // is_unique(x, _): one holder is unique, two are not: 2 * unique + unique-after-dup.
    let mut u = Unit::new(&table, "is_unique", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Rec1");
    let b = u.construct_block(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let one = u.is_unique(ENTRY, b);
    u.builder.push(ENTRY, Inst::Dup { value: b });
    let two = u.is_unique(ENTRY, b);
    let failure = u.releasing(&[(b, rec1_l)], None, SourceAnchorId(0));
    u.release_to(ENTRY, b, rec1_l, failure);
    u.release(ENTRY, b, rec1_l);
    let (end, r) = u.weigh(ENTRY, &[one, two]);
    u.builder.ret(end, Some(r));
    add(u);

    // weaken(x, y): the many-call closure weakened to a once-call one, called and released.
    let mut u = Unit::new(&table, "weaken", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Rec1");
    let b = u.construct_block(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let failure = u.fails(None);
    let many = u.closure(
        ENTRY,
        "env_add",
        "closure i64 -> i64",
        env_box,
        fun_l,
        &[(b, CaptureUse::Take)],
        failure,
    );
    let once = u.value(once_l, Holding::Owned);
    u.builder.push(
        ENTRY,
        Inst::Weaken {
            result: once,
            value: many,
        },
    );
    let normal = u.builder.block(BlockKind::Body, &[]);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    let once_payload = payload(&table, "box_once");
    let release_once = |u: &mut Unit<'_>, block: BlockId| {
        let failure = u.fails(None);
        let holder = u.construct_block_to(
            block,
            ConstructorKind::Box,
            once_payload,
            box_once_l,
            &[(0, once)],
            failure,
            None,
        );
        u.release(block, holder, once_payload);
    };
    release_once(&mut u, cleanup);
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    let r = u
        .call(
            ENTRY,
            Callee::Indirect(once),
            "closure i64 -> i64",
            &[(Passing::Borrow, ArgumentValue::Value(y))],
            normal,
            &[],
            failure,
        )
        .unwrap();
    release_once(&mut u, normal);
    u.builder.ret(normal, Some(r));
    add(u);

    // compare_boxes(x, y): Box(Rec1 { x }) against Box(Rec1 { y }) by <, == and >.
    let mut u = Unit::new(&table, "compare_boxes", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Rec1");
    let a = u.construct_block(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let failure = u.releasing(&[(a, rec1_l)], None, SourceAnchorId(0));
    let b = u.construct_block_to(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec1_l,
        box_rec1_l,
        &[(0, y)],
        failure,
        None,
    );
    let mut bits = Vec::new();
    for op in [CompareOp::Lt, CompareOp::Eq, CompareOp::Gt] {
        let failure = u.releasing(&[(a, rec1_l), (b, rec1_l)], None, SourceAnchorId(0));
        bits.push(u.compare(ENTRY, op, a, b, box_rec1_l, failure));
    }
    let failure = u.releasing(&[(b, rec1_l)], None, SourceAnchorId(0));
    u.release_to(ENTRY, a, rec1_l, failure);
    u.release(ENTRY, b, rec1_l);
    let (end, r) = u.weigh(ENTRY, &bits);
    u.builder.ret(end, Some(r));
    add(u);

    // compare_options(x, y): Some(x) against Some(y), and None below Some(y).
    let mut u = Unit::new(&table, "compare_options", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Opt");
    let some = |variant| ConstructorKind::Variant {
        definition: symbol,
        variant,
    };
    let a = u.construct(ENTRY, some(1), opt_l, &[(0, x)]);
    let b = u.construct(ENTRY, some(1), opt_l, &[(0, y)]);
    let none = u.construct(ENTRY, some(0), opt_l, &[]);
    let mut bits = Vec::new();
    for (op, l, r) in [
        (CompareOp::Lt, a, b),
        (CompareOp::Eq, a, b),
        (CompareOp::Gt, a, b),
        (CompareOp::Lt, none, b),
        (CompareOp::Le, a, none),
    ] {
        let failure = u.fails(None);
        bits.push(u.compare(ENTRY, op, l, r, opt_l, failure));
    }
    let (end, r) = u.weigh(ENTRY, &bits);
    u.builder.ret(end, Some(r));
    add(u);

    // compare_floats: records of f64 under IEEE order, NaN unordered at its first field.
    let mut u = Unit::new(&table, "compare_floats", ENTRY_ABI);
    let _ = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "RecF");
    let float =
        |u: &mut Unit<'_>, v: f64| u.constant(ENTRY, f64_l, ConstantValue::F64(v.to_bits()));
    let (nan, one, zero, neg_zero) = (
        float(&mut u, f64::NAN),
        float(&mut u, 1.0),
        float(&mut u, 0.0),
        float(&mut u, -0.0),
    );
    let pair = |u: &mut Unit<'_>, a, b| {
        u.construct(
            ENTRY,
            ConstructorKind::Record(symbol),
            rec_f_l,
            &[(0, a), (1, b)],
        )
    };
    let nan_one = pair(&mut u, nan, one);
    let one_nan = pair(&mut u, one, nan);
    let one_zero = pair(&mut u, one, zero);
    let one_neg_zero = pair(&mut u, one, neg_zero);
    let mut bits = Vec::new();
    for (op, l, r) in [
        (CompareOp::Eq, nan_one, nan_one),
        (CompareOp::Ne, nan_one, nan_one),
        (CompareOp::Lt, nan_one, one_zero),
        (CompareOp::Ge, one_zero, one_nan),
        (CompareOp::Eq, one_zero, one_neg_zero),
        (CompareOp::Lt, one_neg_zero, one_zero),
    ] {
        let failure = u.fails(None);
        bits.push(u.compare(ENTRY, op, l, r, rec_f_l, failure));
    }
    let (end, r) = u.weigh(ENTRY, &bits);
    u.builder.ret(end, Some(r));
    add(u);

    // compare_chain(n, _): a chain of n links (each a box and a variant field, two levels)
    // compared with itself; past COMPARE_MAX_DEPTH levels it is the MEM-3 Depth failure.
    let (chain_l, box_chain_l) = (payload(&table, "box_chain"), table.layout("box_chain"));
    let mut u = Unit::new(&table, "compare_chain", ENTRY_ABI);
    let n = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Chain");
    let link = |variant| ConstructorKind::Variant {
        definition: symbol,
        variant,
    };
    let nil = u.construct_block(ENTRY, link(0), chain_l, box_chain_l, &[]);
    let zero = u.int(ENTRY, ScalarKind::I64, 0);
    let (i, acc) = (u.plain(i64_l), u.value(box_chain_l, Holding::Owned));
    let head = u.builder.block(BlockKind::Body, &[i, acc]);
    u.builder.jump(ENTRY, head, &[zero, nil]);
    let more = u.plain(bool_l);
    let failure = u.releasing(&[(acc, chain_l)], None, SourceAnchorId(0));
    u.builder.push(
        head,
        Inst::Binary {
            result: more,
            op: BinaryOp::Lt,
            left: i,
            right: n,
            failure,
        },
    );
    let (body, done) = (
        u.builder.block(BlockKind::Body, &[]),
        u.builder.block(BlockKind::Body, &[]),
    );
    let (yes, no) = (u.builder.edge(body, &[]), u.builder.edge(done, &[]));
    u.builder.term(
        head,
        Terminator::Branch {
            condition: more,
            yes,
            no,
        },
    );
    let failure = u.releasing(&[(acc, chain_l)], None, SourceAnchorId(0));
    let one = u.int(body, ScalarKind::I64, 1);
    let next = u.plain(i64_l);
    u.builder.push(
        body,
        Inst::Binary {
            result: next,
            op: BinaryOp::Add,
            left: i,
            right: one,
            failure,
        },
    );
    let failure_cons = u.fails(None);
    let cons = u.construct_block_to(
        body,
        link(1),
        chain_l,
        box_chain_l,
        &[(0, i), (1, acc)],
        failure_cons,
        None,
    );
    u.builder.jump(body, head, &[next, cons]);
    let depth = u.anchor(100, 110);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    u.release(cleanup, acc, chain_l);
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, Some("MEM-3"), depth);
    let eq = u.compare(done, CompareOp::Eq, acc, acc, box_chain_l, failure);
    u.release(done, acc, chain_l);
    let (end, r) = u.weigh(done, &[eq]);
    u.builder.ret(end, Some(r));
    add(u);

    // read_static(x, _) = x + BASE, an i64 static.
    statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("BASE"),
            instance: Some(instance("BASE")),
        },
        i64_l,
        40i64.to_le_bytes().to_vec(),
    ));
    let mut u = Unit::new(&table, "read_static", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Constant, "BASE");
    let block = u.value(box_i64_l, Holding::Owned);
    u.builder.push(
        ENTRY,
        Inst::ReadStatic {
            result: block,
            symbol,
        },
    );
    let at = u.address(AddressBase::Value(block), &[], i64_l);
    let failure = u.releasing(&[(block, i64_l)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let base = u.read(ENTRY, at, block);
    u.release(ENTRY, block, i64_l);
    let r = u.binary(ENTRY, BinaryOp::Add, x, base);
    u.builder.ret(ENTRY, Some(r));
    add(u);

    // static_box(x, _) = BOXED.0 + x + 2 * unique(BOXED): an immortal static block.
    statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("BOXED"),
            instance: Some(instance("BOXED")),
        },
        rec1_l,
        1000i64.to_le_bytes().to_vec(),
    ));
    let mut u = Unit::new(&table, "static_box", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Constant, "BOXED");
    let boxed = u.value(box_rec1_l, Holding::Owned);
    u.builder.push(
        ENTRY,
        Inst::ReadStatic {
            result: boxed,
            symbol,
        },
    );
    let at = u.address(
        AddressBase::Value(boxed),
        &[ProjectionStep::Field(0)],
        i64_l,
    );
    let failure = u.releasing(&[(boxed, rec1_l)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(ENTRY, at, boxed);
    let unique = u.is_unique(ENTRY, boxed);
    // Releasing the immortal block leaves it alone.
    u.release(ENTRY, boxed, rec1_l);
    let sum = u.binary(ENTRY, BinaryOp::Add, v, x);
    let (end, w) = u.weigh(ENTRY, &[unique]);
    let r = u.binary(end, BinaryOp::Add, sum, w);
    u.builder.ret(end, Some(r));
    add(u);

    // literal_byte(i, _) = b"hello"[i] == 'l', failing MEM-14 past the end: a bytes literal
    // as a static immortal block.
    let mut u = Unit::new(&table, "literal_byte", ENTRY_ABI);
    let i = u.param(i64_l);
    let _ = u.param(i64_l);
    let lit = u.owned_constant(ENTRY, bytes_l, ConstantValue::Bytes(b"hello".to_vec()));
    let at = u.address(AddressBase::Value(lit), &[ProjectionStep::Index(i)], u8_l);
    let bounds = u.anchor(60, 70);
    let failure = u.releasing(&[(lit, payload(&table, "bytes"))], Some("MEM-14"), bounds);
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let byte = u.read(ENTRY, at, lit);
    u.release(ENTRY, lit, payload(&table, "bytes"));
    let l = u.int(ENTRY, ScalarKind::U8, u128::from(b'l'));
    let eq = u.binary(ENTRY, BinaryOp::Eq, byte, l);
    let (end, r) = u.weigh(ENTRY, &[eq]);
    u.builder.ret(end, Some(r));
    add(u);

    // unique_copy(x, y): a deep copy of Box(Rec2 { x, Box(Rec1 { y }) }), both unique,
    // reading x + y through the copy: 4 * (x + y) + 2 * unique(copy) + unique(inner copy).
    let mut u = Unit::new(&table, "unique_copy", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let rec2_symbol = u.symbol(SymbolKind::Type, "Rec2");
    let inner = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, y)],
    );
    let outer = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec2_symbol),
        rec2_l,
        box_rec2_l,
        &[(0, x), (1, inner)],
    );
    u.builder.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor: ConstructorKind::Record(rec2_symbol),
        layout: rec2_l,
        fields: PoolRange::EMPTY,
        destination: DestinationClass::Heap,
        placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        anchor: SourceAnchorId(0),
    });
    let construct = ConstructId(u.builder.unit.tables.constructs.len() as u32 - 1);
    let copy = u.value(box_rec2_l, Holding::Owned);
    let failure = u.releasing(&[(outer, rec2_l)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::UniqueCopy {
            result: copy,
            value: outer,
            construct,
            deep: true,
            failure,
        },
    );
    let failure = u.releasing(&[(copy, rec2_l)], None, SourceAnchorId(0));
    u.release_to(ENTRY, outer, rec2_l, failure);
    let field = |u: &mut Unit<'_>, steps: &[ProjectionStep], layout| {
        let at = u.address(AddressBase::Value(copy), steps, layout);
        let failure = u.releasing(&[(copy, rec2_l)], None, SourceAnchorId(0));
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: at,
                failure,
            },
        );
        u.read(ENTRY, at, copy)
    };
    let a = field(&mut u, &[ProjectionStep::Field(0)], i64_l);
    let b = field(
        &mut u,
        &[ProjectionStep::Field(1), ProjectionStep::Field(0)],
        i64_l,
    );
    let unique = u.is_unique(ENTRY, copy);
    let failure = u.fails(None);
    u.release_to(ENTRY, copy, rec2_l, failure);
    let sum = u.binary(ENTRY, BinaryOp::Add, a, b);
    let four = u.int(ENTRY, ScalarKind::I64, 4);
    let scaled = u.binary(ENTRY, BinaryOp::Mul, sum, four);
    let (end, w) = u.weigh(ENTRY, &[unique]);
    let r = u.binary(end, BinaryOp::Add, scaled, w);
    u.builder.ret(end, Some(r));
    add(u);

    // compare_strings: "abc" against "abd" and "ab" by <, ==, > over static literal slices
    // (native only: the heap checker has no string slices).
    let mut u = Unit::new(&table, "compare_strings", ENTRY_ABI);
    let _ = u.param(i64_l);
    let _ = u.param(i64_l);
    let text =
        |u: &mut Unit<'_>, t: &str| u.constant(ENTRY, string_l, ConstantValue::String(t.into()));
    let (abc, abd, ab) = (text(&mut u, "abc"), text(&mut u, "abd"), text(&mut u, "ab"));
    let mut bits = Vec::new();
    for (op, l, r) in [
        (CompareOp::Lt, abc, abd),
        (CompareOp::Gt, abc, ab),
        (CompareOp::Eq, abc, abd),
    ] {
        let failure = u.fails(None);
        bits.push(u.compare(ENTRY, op, l, r, string_l, failure));
    }
    let (end, r) = u.weigh(ENTRY, &bits);
    u.builder.ret(end, Some(r));
    add(u);
    let _ = bool_l;

    // Res_drop(self): fails LANG-9 when self.0 is zero.
    let (res_l, box_res_l) = (table.layout("res"), table.layout("box_res"));
    let mut u = Unit::new(&table, "Res_drop", "drop Res");
    let this = u.param(res_l);
    let slot = u.slot(res_l, SlotKind::Parameter(0));
    let _ = this;
    let at = u.address(AddressBase::Slot(slot), &[ProjectionStep::Field(0)], i64_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let v = u.read(ENTRY, at, this);
    let zero = u.int(ENTRY, ScalarKind::I64, 0);
    let nonzero = u.binary(ENTRY, BinaryOp::Ne, v, zero);
    let check = u.anchor(30, 40);
    let p = u.builder.propagate();
    let failure = u.failure_to(p, Some("LANG-9"), check);
    u.builder.push(
        ENTRY,
        Inst::Check {
            condition: nonzero,
            failure,
        },
    );
    u.builder.ret(ENTRY, None);
    add(u);

    // finalize_box(x, _) = x after releasing Box(Res { x }), whose drop body runs through
    // the descriptor's drop thunk and fails on zero.
    let mut u = Unit::new(&table, "finalize_box", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Res");
    let r = u.construct(ENTRY, ConstructorKind::Record(symbol), res_l, &[(0, x)]);
    // A resource holds its finalization obligation.
    u.builder.unit.values[r.index()].holding = Holding::Owned;
    let b = u.construct_block(ENTRY, ConstructorKind::Box, res_l, box_res_l, &[(0, r)]);
    u.release(ENTRY, b, res_l);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // finalize_explicit(x, _) = x after finalizing a local Res { x } explicitly: begin, the
    // drop body called directly, end, on its normal and failure edges alike, then the slot's
    // storage ends.
    let mut u = Unit::new(&table, "finalize_explicit", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Res");
    let slot = u.slot(res_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let at = u.address(AddressBase::Slot(slot), &[], res_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let r = u.construct(ENTRY, ConstructorKind::Record(symbol), res_l, &[(0, x)]);
    u.builder.unit.values[r.index()].holding = Holding::Owned;
    let v = u.store(ENTRY, at, r, slot);
    let anchor = u.cleanup_anchor(50, 60);
    u.builder.push(
        ENTRY,
        Inst::FinalizeBegin {
            address: at,
            anchor,
        },
    );
    let normal = u.builder.block(BlockKind::Body, &[]);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    for block in [normal, cleanup] {
        u.builder.push(
            block,
            Inst::EndLoan {
                loan: LoanId(u.builder.unit.tables.loans.len() as u32),
                if_active: false,
            },
        );
        u.builder.push(
            block,
            Inst::FinalizeEnd {
                address: at,
                anchor,
            },
        );
        // Res has no owning field, so its finalized storage is empty (C's slot-emptiness
        // predicate): nothing is taken or forgotten.
        u.builder.push(
            block,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    u.builder.unit.tables.loans.push(LoanInfo {
        address: at,
        mutable: false,
    });
    let loan = LoanId(u.builder.unit.tables.loans.len() as u32 - 1);
    u.builder.push(
        ENTRY,
        Inst::BeginLoan {
            loan,
            address: at,
            mutable: false,
            if_inactive: false,
        },
    );
    let this = u.value(res_l, Holding::Borrowed(loan));
    u.builder.push(
        ENTRY,
        Inst::Read {
            result: this,
            address: at,
            reaching: v,
            mode: ReadMode::Borrow,
        },
    );
    let drop = u.symbol(SymbolKind::Function, "Res_drop");
    u.call(
        ENTRY,
        Callee::Direct(drop),
        "drop Res",
        &[(Passing::Borrow, ArgumentValue::Value(this))],
        normal,
        &[],
        failure,
    );
    u.builder.ret(normal, Some(x));
    add(u);

    // region_sum(x, y) = 2 * (x + y) + (Box(Rec1 { x }) < Box(Rec1 { y })), both boxes in
    // a local region, read and compared through their uncounted references.
    let mut u = Unit::new(&table, "region_sum", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Rec1");
    let mark = u.enter_region(ENTRY);
    let failure = u.exits(mark);
    let a = u.region_construct(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
        failure,
    );
    let failure = u.exits(mark);
    let b = u.region_construct(
        ENTRY,
        ConstructorKind::Record(symbol),
        rec1_l,
        box_rec1_l,
        &[(0, y)],
        failure,
    );
    let read = |u: &mut Unit<'_>, v| {
        let at = u.address(AddressBase::Value(v), &[ProjectionStep::Field(0)], i64_l);
        let failure = u.exits(mark);
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: at,
                failure,
            },
        );
        u.read(ENTRY, at, v)
    };
    let (va, vb) = (read(&mut u, a), read(&mut u, b));
    let failure = u.exits(mark);
    let lt = u.compare(ENTRY, CompareOp::Lt, a, b, box_rec1_l, failure);
    u.exit_region(ENTRY, mark);
    // 2 * (x + y) + (a < b), after the region has exited.
    let sum = u.binary(ENTRY, BinaryOp::Add, va, vb);
    let doubled = u.binary(ENTRY, BinaryOp::Add, sum, sum);
    let (end, w) = u.weigh(ENTRY, &[lt]);
    let r = u.binary(end, BinaryOp::Add, doubled, w);
    u.builder.ret(end, Some(r));
    add(u);

    // region_resource(x, _) = x, with Box(Res { x }) in a local region: its drop body runs
    // when the region exits, failing on zero.
    let mut u = Unit::new(&table, "region_resource", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Type, "Res");
    let mark = u.enter_region(ENTRY);
    let fields = PoolRange::push(
        &mut u.builder.unit.pools.fields,
        [FieldValue { field: 0, value: x }],
    );
    u.builder.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor: ConstructorKind::Record(symbol),
        layout: res_l,
        fields,
        destination: DestinationClass::Heap,
        placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        anchor: SourceAnchorId(0),
    });
    let construct = ConstructId(u.builder.unit.tables.constructs.len() as u32 - 1);
    let r = u.value(res_l, Holding::Owned);
    let failure = u.exits(mark);
    u.builder.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct,
            failure,
        },
    );
    let failure = u.exits(mark);
    let _ = u.region_construct(
        ENTRY,
        ConstructorKind::Box,
        res_l,
        box_res_l,
        &[(0, r)],
        failure,
    );
    u.exit_region(ENTRY, mark);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // bump(mut x): x += 1 through its exclusive address.
    let mut u = Unit::new(&table, "bump", "mut i64");
    let this = u.param(i64_l);
    let slot = u.slot(i64_l, SlotKind::Parameter(0));
    let at = u.address(AddressBase::Slot(slot), &[], i64_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let v = u.read(ENTRY, at, this);
    let one = u.int(ENTRY, ScalarKind::I64, 1);
    let next = u.binary(ENTRY, BinaryOp::Add, v, one);
    // The old content moves to a displaced slot, which dies with nothing to release.
    let displaced = u.slot(i64_l, SlotKind::Displaced);
    u.builder.push(ENTRY, Inst::StorageLive { slot: displaced });
    let version = u.version(slot);
    let displaced_version = u.version(displaced);
    let failure = u.fails(None);
    u.builder.push(
        ENTRY,
        Inst::Replace {
            address: at,
            value: next,
            displaced,
            version,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    u.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: displaced,
            if_live: false,
        },
    );
    u.builder.ret(ENTRY, None);
    add(u);

    // mut_caller(x, y) = { let mut m = x; bump(&mut m); m + y }.
    let mut u = Unit::new(&table, "mut_caller", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let slot = u.slot(i64_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let at = u.address(AddressBase::Slot(slot), &[], i64_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    u.store(ENTRY, at, x, slot);
    let written = u.version(slot);
    let normal = u.builder.block(BlockKind::Body, &[]);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    u.builder.push(
        cleanup,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    let bump = u.symbol(SymbolKind::Function, "bump");
    u.call(
        ENTRY,
        Callee::Direct(bump),
        "mut i64",
        &[(
            Passing::Mut,
            ArgumentValue::Address {
                address: at,
                version: written,
            },
        )],
        normal,
        &[],
        failure,
    );
    let m = u.read(normal, at, written);
    u.builder.push(
        normal,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let r = u.binary(normal, BinaryOp::Add, m, y);
    u.builder.ret(normal, Some(r));
    add(u);

    // niche_option(x, _) = 4 * x + 2 * (Box(Some(Box(Rec1 { x }))) > Box(None))
    // + unique(copy): Option<Box<_>> is a null-pointer niche, read, compared, released and
    // deep-copied through its pointer word.
    let (opt_l, box_opt_l) = (table.layout("opt_box"), table.layout("box_opt"));
    assert!(matches!(
        table.table.layouts[opt_l.0 as usize]
            .tag
            .as_ref()
            .map(|t| t.form),
        Some(hirsel_lir::layout_types::TagForm::Niche { .. })
    ));
    let mut u = Unit::new(&table, "niche_option", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let opt_symbol = u.symbol(SymbolKind::Type, "OptBox");
    let variant = |variant| ConstructorKind::Variant {
        definition: opt_symbol,
        variant,
    };
    let inner = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let some = u.construct_block(ENTRY, variant(1), opt_l, box_opt_l, &[(0, inner)]);
    let failure = u.releasing(&[(some, opt_l)], None, SourceAnchorId(0));
    let none = u.construct_block_to(ENTRY, variant(0), opt_l, box_opt_l, &[], failure, None);
    let both = [(some, opt_l), (none, opt_l)];
    let failure = u.releasing(&both, None, SourceAnchorId(0));
    let gt = u.compare(ENTRY, CompareOp::Gt, some, none, box_opt_l, failure);
    u.builder.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor: variant(1),
        layout: opt_l,
        fields: PoolRange::EMPTY,
        destination: DestinationClass::Heap,
        placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        anchor: SourceAnchorId(0),
    });
    let construct = ConstructId(u.builder.unit.tables.constructs.len() as u32 - 1);
    let copy = u.value(box_opt_l, Holding::Owned);
    let failure = u.releasing(&both, None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::UniqueCopy {
            result: copy,
            value: some,
            construct,
            deep: true,
            failure,
        },
    );
    let all = [(some, opt_l), (none, opt_l), (copy, opt_l)];
    let at = u.address(
        AddressBase::Value(copy),
        &[
            ProjectionStep::VariantField {
                variant: 1,
                field: 0,
            },
            ProjectionStep::Field(0),
        ],
        i64_l,
    );
    let failure = u.releasing(&all, None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(ENTRY, at, copy);
    let unique = u.is_unique(ENTRY, copy);
    u.release_all(ENTRY, &all);
    let four = u.int(ENTRY, ScalarKind::I64, 4);
    let scaled = u.binary(ENTRY, BinaryOp::Mul, v, four);
    let (end, w) = u.weigh(ENTRY, &[gt, unique]);
    let r = u.binary(end, BinaryOp::Add, scaled, w);
    u.builder.ret(end, Some(r));
    add(u);

    // Trace_drop(self): fails LANG-9 when self.count.0 is zero.
    let (trace_l, box_trace_l) = (table.layout("trace"), table.layout("box_trace"));
    let mut u = Unit::new(&table, "Trace_drop", "drop Trace");
    let this = u.param(trace_l);
    let slot = u.slot(trace_l, SlotKind::Parameter(0));
    let at = u.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(1), ProjectionStep::Field(0)],
        i64_l,
    );
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let v = u.read(ENTRY, at, this);
    let zero = u.int(ENTRY, ScalarKind::I64, 0);
    let nonzero = u.binary(ENTRY, BinaryOp::Ne, v, zero);
    let check = u.anchor(30, 40);
    let p = u.builder.propagate();
    let failure = u.failure_to(p, Some("LANG-9"), check);
    u.builder.push(
        ENTRY,
        Inst::Check {
            condition: nonzero,
            failure,
        },
    );
    u.builder.ret(ENTRY, None);
    add(u);

    // finalize_trace(x, _) = x after releasing Box(Trace { "trace", Box(Rec1 { x }) }): the
    // drop body runs through the drop thunk, then the counted fields are released, on its
    // failure too.
    let mut u = Unit::new(&table, "finalize_trace", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let trace_symbol = u.symbol(SymbolKind::Type, "Trace");
    let text = u.constant(ENTRY, string_l, ConstantValue::String("trace".into()));
    let count = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let t = u.construct(
        ENTRY,
        ConstructorKind::Record(trace_symbol),
        trace_l,
        &[(0, text), (1, count)],
    );
    u.builder.unit.values[t.index()].holding = Holding::Owned;
    let b = u.construct_block(ENTRY, ConstructorKind::Box, trace_l, box_trace_l, &[(0, t)]);
    u.release(ENTRY, b, trace_l);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // env_res_add(y) = y: a closure body whose environment holds a Box<Res>.
    let (held_l, rel_l) = (table.layout("held"), table.layout("rel"));
    let env_res = table.layout("env_res");
    let mut u = Unit::new(&table, "env_res_add", "closure i64 -> i64");
    let y = u.param(i64_l);
    let _ = u.capture(0, box_res_l);
    u.builder.ret(ENTRY, Some(y));
    add(u);

    // release_held(x, _) = x after an inline Release of { Box(Res { x }), "held" }: the box's
    // drop body fails on zero, the slice's owner is released all the same.
    let mut u = Unit::new(&table, "release_held", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let res_symbol = u.symbol(SymbolKind::Type, "Res");
    let held_symbol = u.symbol(SymbolKind::Type, "Held");
    let r = u.construct(ENTRY, ConstructorKind::Record(res_symbol), res_l, &[(0, x)]);
    u.builder.unit.values[r.index()].holding = Holding::Owned;
    let b = u.construct_block(ENTRY, ConstructorKind::Box, res_l, box_res_l, &[(0, r)]);
    let text = u.constant(ENTRY, string_l, ConstantValue::String("held".into()));
    let h = u.construct(
        ENTRY,
        ConstructorKind::Record(held_symbol),
        held_l,
        &[(0, b), (1, text)],
    );
    u.builder.unit.values[h.index()].holding = Holding::Owned;
    u.release(ENTRY, h, held_l);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // release_closure(x, _) = x after an inline Release of a closure over Box(Res { x }):
    // the environment is released, and with it the box, whose drop body fails on zero.
    let mut u = Unit::new(&table, "release_closure", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let res_symbol = u.symbol(SymbolKind::Type, "Res");
    let r = u.construct(ENTRY, ConstructorKind::Record(res_symbol), res_l, &[(0, x)]);
    u.builder.unit.values[r.index()].holding = Holding::Owned;
    let b = u.construct_block(ENTRY, ConstructorKind::Box, res_l, box_res_l, &[(0, r)]);
    let failure = u.fails(None);
    let f = u.closure(
        ENTRY,
        "env_res_add",
        "closure i64 -> i64",
        env_res,
        fun_l,
        &[(b, CaptureUse::Take)],
        failure,
    );
    u.release(ENTRY, f, fun_l);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // release_variant(x, _) = x after inline Releases of None and Some(x, Box(Res { x })),
    // a null-pointer niche whose dataful arm releases the box.
    let mut u = Unit::new(&table, "release_variant", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let res_symbol = u.symbol(SymbolKind::Type, "Res");
    let rel_symbol = u.symbol(SymbolKind::Type, "Rel");
    let variant = |variant| ConstructorKind::Variant {
        definition: rel_symbol,
        variant,
    };
    let none = u.construct(ENTRY, variant(0), rel_l, &[]);
    u.builder.unit.values[none.index()].holding = Holding::Owned;
    u.release(ENTRY, none, rel_l);
    let r = u.construct(ENTRY, ConstructorKind::Record(res_symbol), res_l, &[(0, x)]);
    u.builder.unit.values[r.index()].holding = Holding::Owned;
    let b = u.construct_block(ENTRY, ConstructorKind::Box, res_l, box_res_l, &[(0, r)]);
    let some = u.construct(ENTRY, variant(1), rel_l, &[(0, x), (1, b)]);
    u.builder.unit.values[some.index()].holding = Holding::Owned;
    u.release(ENTRY, some, rel_l);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // copy_pair(x, _) = 4 * x + 2 * unique(box copy) + unique(taken box): a Ready Copy of
    // a slot's { Box(Rec1 { x }), "pair" } (Fields recipe) and of its box (Dup recipe), both
    // released, then each field taken out of the slot, which ends empty.
    let pair_s_l = table.layout("pair_s");
    let mut u = Unit::new(&table, "copy_pair", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let pair_symbol = u.symbol(SymbolKind::Type, "PairS");
    let slot = u.slot(pair_s_l, SlotKind::Local);
    let whole = u.address(AddressBase::Slot(slot), &[], pair_s_l);
    let first = u.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0)],
        box_rec1_l,
    );
    let second = u.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(1)],
        string_l,
    );
    for at in [whole, first, second] {
        u.prepare(ENTRY, at, None, SourceAnchorId(0));
    }
    let inner = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let text = u.constant(ENTRY, string_l, ConstantValue::String("pair".into()));
    let p = u.construct(
        ENTRY,
        ConstructorKind::Record(pair_symbol),
        pair_s_l,
        &[(0, inner), (1, text)],
    );
    u.builder.unit.values[p.index()].holding = Holding::Owned;
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let v = u.store(ENTRY, whole, p, slot);
    let take = |u: &mut Unit<'_>, b: BlockId, address, layout| {
        let result = u.value(layout, Holding::Owned);
        u.builder.push(
            b,
            Inst::Read {
                result,
                address,
                reaching: v,
                mode: ReadMode::Take,
            },
        );
        result
    };
    // A failure edge while the slot is full: its content is taken and released with `live`.
    let emptying = |u: &mut Unit<'_>, live: &[(ValueId, LayoutId)]| {
        let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
        let content = take(u, cleanup, whole, pair_s_l);
        u.builder.push(
            cleanup,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        let mut all = vec![(content, pair_s_l)];
        all.extend_from_slice(live);
        u.release_all(cleanup, &all);
        let p = u.builder.propagate();
        u.builder.jump(cleanup, p, &[]);
        u.failure_to(cleanup, None, SourceAnchorId(0))
    };
    let copy = |u: &mut Unit<'_>, address, layout, failure| {
        let result = u.value(layout, Holding::Owned);
        u.builder.push(
            ENTRY,
            Inst::Copy {
                result,
                address,
                reaching: v,
                failure,
            },
        );
        result
    };
    let failure = emptying(&mut u, &[]);
    let pair_copy = copy(&mut u, whole, pair_s_l, failure);
    let failure = emptying(&mut u, &[(pair_copy, pair_s_l)]);
    let box_copy = copy(&mut u, first, box_rec1_l, failure);
    let shared = u.is_unique(ENTRY, box_copy);
    let boxed = take(&mut u, ENTRY, first, box_rec1_l);
    let text = take(&mut u, ENTRY, second, string_l);
    u.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let rest = [(box_copy, rec1_l), (boxed, rec1_l), (text, string_l)];
    let failure = u.releasing(&rest, None, SourceAnchorId(0));
    u.release_to(ENTRY, pair_copy, pair_s_l, failure);
    let failure = u.releasing(&rest[1..], None, SourceAnchorId(0));
    u.release_to(ENTRY, box_copy, rec1_l, failure);
    let unique = u.is_unique(ENTRY, boxed);
    let at = u.address(
        AddressBase::Value(boxed),
        &[ProjectionStep::Field(0)],
        i64_l,
    );
    let failure = u.releasing(&rest[1..], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let read = u.read(ENTRY, at, boxed);
    u.release_all(ENTRY, &rest[1..]);
    let four = u.int(ENTRY, ScalarKind::I64, 4);
    let scaled = u.binary(ENTRY, BinaryOp::Mul, read, four);
    let (end, w) = u.weigh(ENTRY, &[shared, unique]);
    let r = u.binary(end, BinaryOp::Add, scaled, w);
    u.builder.ret(end, Some(r));
    add(u);

    // finalize_trace_explicit(x, _) = x after finalizing a local Trace { "trace",
    // Box(Rec1 { x }) } explicitly: begin, the drop body called directly, end, then each
    // counted field taken and released, last first, on its normal and failure edges alike.
    // The slot is empty by C's predicate once both fields are out (no Forget).
    let mut u = Unit::new(&table, "finalize_trace_explicit", ENTRY_ABI);
    let x = u.param(i64_l);
    let _ = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let trace_symbol = u.symbol(SymbolKind::Type, "Trace");
    let slot = u.slot(trace_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let at = u.address(AddressBase::Slot(slot), &[], trace_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let count_at = u.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(1)],
        box_rec1_l,
    );
    u.prepare(ENTRY, count_at, None, SourceAnchorId(0));
    let text_at = u.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0)],
        string_l,
    );
    u.prepare(ENTRY, text_at, None, SourceAnchorId(0));
    let text = u.constant(ENTRY, string_l, ConstantValue::String("trace".into()));
    let count = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let t = u.construct(
        ENTRY,
        ConstructorKind::Record(trace_symbol),
        trace_l,
        &[(0, text), (1, count)],
    );
    u.builder.unit.values[t.index()].holding = Holding::Owned;
    let v = u.store(ENTRY, at, t, slot);
    let anchor = u.cleanup_anchor(50, 60);
    u.builder.push(
        ENTRY,
        Inst::FinalizeBegin {
            address: at,
            anchor,
        },
    );
    let normal = u.builder.block(BlockKind::Body, &[]);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    for block in [normal, cleanup] {
        u.builder.push(
            block,
            Inst::EndLoan {
                loan: LoanId(u.builder.unit.tables.loans.len() as u32),
                if_active: false,
            },
        );
        u.builder.push(
            block,
            Inst::FinalizeEnd {
                address: at,
                anchor,
            },
        );
        let take = |u: &mut Unit<'_>, address, layout| {
            let result = u.value(layout, Holding::Owned);
            u.builder.push(
                block,
                Inst::Read {
                    result,
                    address,
                    reaching: v,
                    mode: ReadMode::Take,
                },
            );
            result
        };
        let count = take(&mut u, count_at, box_rec1_l);
        let text = take(&mut u, text_at, string_l);
        u.builder.push(
            block,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        let failure = u.releasing(&[(text, string_l)], None, SourceAnchorId(0));
        u.release_to(block, count, rec1_l, failure);
        u.release(block, text, string_l);
    }
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    u.builder.unit.tables.loans.push(LoanInfo {
        address: at,
        mutable: false,
    });
    let loan = LoanId(u.builder.unit.tables.loans.len() as u32 - 1);
    u.builder.push(
        ENTRY,
        Inst::BeginLoan {
            loan,
            address: at,
            mutable: false,
            if_inactive: false,
        },
    );
    let this = u.value(trace_l, Holding::Borrowed(loan));
    u.builder.push(
        ENTRY,
        Inst::Read {
            result: this,
            address: at,
            reaching: v,
            mode: ReadMode::Borrow,
        },
    );
    let drop = u.symbol(SymbolKind::Function, "Trace_drop");
    u.call(
        ENTRY,
        Callee::Direct(drop),
        "drop Trace",
        &[(Passing::Borrow, ArgumentValue::Value(this))],
        normal,
        &[],
        failure,
    );
    u.builder.ret(normal, Some(x));
    add(u);

    // Revision 5: a unique block's field moved out by value, a masked release and reset, a
    // reuse that keeps a field in place, and a closure's environment as a reuse donor.
    let (pair_l, box_pair_l) = (payload(&table, "box_pair"), table.layout("box_pair"));
    let mask = |u: &mut Unit<'_>, bits: u64| {
        let words = PoolRange::push(&mut u.builder.unit.pools.mask_words, [bits]);
        u.builder
            .unit
            .tables
            .masks
            .push(FieldMask { fields: 2, words });
        MaskId(u.builder.unit.tables.masks.len() as u32 - 1)
    };
    // Box(Pair { Box(Rec1 { x }), Box(Rec1 { y }) }) and its first field taken out by value.
    let split = |u: &mut Unit<'_>, x, y| {
        let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
        let pair_symbol = u.symbol(SymbolKind::Type, "Pair");
        let record = ConstructorKind::Record(rec1_symbol);
        let a = u.construct_block(ENTRY, record, rec1_l, box_rec1_l, &[(0, x)]);
        let failure = u.releasing(&[(a, rec1_l)], None, SourceAnchorId(0));
        let b = u.construct_block_to(ENTRY, record, rec1_l, box_rec1_l, &[(0, y)], failure, None);
        // A failing construction has consumed its fields.
        let failure = u.fails(None);
        let d = u.construct_block_to(
            ENTRY,
            ConstructorKind::Record(pair_symbol),
            pair_l,
            box_pair_l,
            &[(0, a), (1, b)],
            failure,
            None,
        );
        let first = u.address(
            AddressBase::Value(d),
            &[ProjectionStep::Field(0)],
            box_rec1_l,
        );
        let failure = u.releasing(&[(d, pair_l)], None, SourceAnchorId(0));
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: first,
                failure,
            },
        );
        let taken = u.value(box_rec1_l, Holding::Owned);
        u.builder.push(
            ENTRY,
            Inst::Read {
                result: taken,
                address: first,
                reaching: d,
                mode: ReadMode::Take,
            },
        );
        (d, taken)
    };
    let field0 = |u: &mut Unit<'_>, block, live: &[(ValueId, LayoutId)]| {
        let at = u.address(
            AddressBase::Value(block),
            &[ProjectionStep::Field(0)],
            i64_l,
        );
        let failure = u.releasing(live, None, SourceAnchorId(0));
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: at,
                failure,
            },
        );
        u.read(ENTRY, at, block)
    };

    // partial_release(x, y) = x: the box's first field moved out, the box released with a
    // static mask that skips it, then the moved box read and released.
    let mut u = Unit::new(&table, "partial_release", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let (d, taken) = split(&mut u, x, y);
    let live = mask(&mut u, 0b10);
    let anchor = u.cleanup_anchor(80, 90);
    let failure = u.releasing(&[(taken, rec1_l)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Release {
            value: d,
            layout: pair_l,
            live: MaskRef::Static(live),
            anchor,
            failure,
        },
    );
    let v = field0(&mut u, taken, &[(taken, rec1_l)]);
    u.release(ENTRY, taken, rec1_l);
    u.builder.ret(ENTRY, Some(v));
    add(u);

    // reuse_kept(x, y) = 4 * (x + y + 1) + 2 * same + unique: Box(Pair { a, b }) is reset
    // with a moved out (b released), and Box(Pair { a, Box(Rec1 { y + 1 }) }) reuses its
    // storage keeping a in place; `same` is whether the reused block is the donor.
    let mut u = Unit::new(&table, "reuse_kept", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let one = u.int(ENTRY, ScalarKind::I64, 1);
    let next = u.binary(ENTRY, BinaryOp::Add, y, one);
    let (d, taken) = split(&mut u, x, y);
    let live = mask(&mut u, 0b10);
    let kept = mask(&mut u, 0b01);
    let token_ty = u.ty(ValueRepr::Token, u.table.header);
    u.builder.unit.values.push(ValueInfo {
        ty: token_ty,
        holding: Holding::Owned,
    });
    let token = ValueId(u.builder.unit.values.len() as u32 - 1);
    let anchor = u.cleanup_anchor(80, 90);
    let failure = u.releasing(&[(taken, rec1_l)], None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Reset {
            result: token,
            value: d,
            layout: pair_l,
            live: MaskRef::Static(live),
            anchor,
            failure,
        },
    );
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let pair_symbol = u.symbol(SymbolKind::Type, "Pair");
    // A failure with the token live disposes of it on the edge.
    let failure = u.releasing(&[(taken, rec1_l)], None, SourceAnchorId(0));
    u.builder.unit.tables.failures[failure.index()].dispose =
        PoolRange::push(&mut u.builder.unit.pools.values, [token]);
    let fresh = u.construct_block_to(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, next)],
        failure,
        None,
    );
    let failure = u.fails(None);
    let r = u.construct_block_to(
        ENTRY,
        ConstructorKind::Record(pair_symbol),
        pair_l,
        box_pair_l,
        &[(0, taken), (1, fresh)],
        failure,
        Some((token, Some(kept))),
    );
    let unique = u.is_unique(ENTRY, r);
    let mut through = |k: u32| {
        let steps = [ProjectionStep::Field(k), ProjectionStep::Field(0)];
        let at = u.address(AddressBase::Value(r), &steps, i64_l);
        let failure = u.releasing(&[(r, pair_l)], None, SourceAnchorId(0));
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: at,
                failure,
            },
        );
        u.read(ENTRY, at, r)
    };
    let (a, b) = (through(0), through(1));
    u.release(ENTRY, r, pair_l);
    let sum = u.binary(ENTRY, BinaryOp::Add, a, b);
    let four = u.int(ENTRY, ScalarKind::I64, 4);
    let scaled = u.binary(ENTRY, BinaryOp::Mul, sum, four);
    let (end, w) = u.weigh(ENTRY, &[unique]);
    let r = u.binary(end, BinaryOp::Add, scaled, w);
    u.builder.ret(end, Some(r));
    add(u);

    // closure_reset(x, y) = x + y: the closure over Box(Rec1 { x }) is called, then its
    // environment is reset (the box released) and reused as Box(Rec1 { x + y }).
    let mut u = Unit::new(&table, "closure_reset", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let b = u.construct_block(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
    );
    let failure = u.fails(None);
    let f = u.closure(
        ENTRY,
        "env_add",
        "closure i64 -> i64",
        env_box,
        fun_l,
        &[(b, CaptureUse::Take)],
        failure,
    );
    let normal = u.builder.block(BlockKind::Body, &[]);
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    u.release(cleanup, f, fun_l);
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    let sum = u
        .call(
            ENTRY,
            Callee::Indirect(f),
            "closure i64 -> i64",
            &[(Passing::Borrow, ArgumentValue::Value(y))],
            normal,
            &[],
            failure,
        )
        .unwrap();
    let token_ty = u.ty(ValueRepr::Token, u.table.header);
    u.builder.unit.values.push(ValueInfo {
        ty: token_ty,
        holding: Holding::Owned,
    });
    let token = ValueId(u.builder.unit.values.len() as u32 - 1);
    let anchor = u.cleanup_anchor(80, 90);
    let failure = u.fails(None);
    u.builder.push(
        normal,
        Inst::Reset {
            result: token,
            value: f,
            layout: env_box,
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    let failure = u.fails(None);
    let n = u.construct_block_to(
        normal,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, sum)],
        failure,
        Some((token, None)),
    );
    let at = u.address(AddressBase::Value(n), &[ProjectionStep::Field(0)], i64_l);
    let failure = u.releasing(&[(n, rec1_l)], None, SourceAnchorId(0));
    u.builder.push(
        normal,
        Inst::Address {
            address: at,
            failure,
        },
    );
    let v = u.read(normal, at, n);
    u.release(normal, n, rec1_l);
    u.builder.ret(normal, Some(v));
    add(u);

    // Revision 4: FlatFormatV0 values built, viewed, counted, indexed and copied.
    let (item_l, box_item_l) = (payload(&table, "item"), table.layout("item"));
    let (items_l, box_items_l) = (payload(&table, "items"), table.layout("items"));
    let field = |layout: LayoutId, ordinal: u32| {
        table.table.layouts[layout.0 as usize]
            .fields
            .iter()
            .find(|f| f.ordinal == ordinal)
            .expect("a flat field")
            .layout
    };
    let (text_l, n_l) = (field(item_l, 0), field(item_l, 1));
    let byte_l = field(text_l, 0);
    let u64_l = table.scalar(ScalarKind::U64);
    let list_l = field(items_l, 0);
    let element_l = field(list_l, 0);

    // flat_item(x, i) = n + 4 * (view == "hello") + 2 * (text[i] == 'l') + (len(text) == 5)
    // over Item { "hello", x }: the String is encoded behind its prefix, n read at its
    // cursor, and the text viewed, counted and indexed (MEM-14 past its end).
    let mut u = Unit::new(&table, "flat_item", ENTRY_ABI);
    let x = u.param(i64_l);
    let i = u.param(i64_l);
    let item_symbol = u.symbol(SymbolKind::Type, "Item");
    let hello = u.constant(ENTRY, string_l, ConstantValue::String("hello".into()));
    let b = u.construct_block(
        ENTRY,
        ConstructorKind::Record(item_symbol),
        item_l,
        box_item_l,
        &[(0, hello), (1, x)],
    );
    let live = [(b, item_l)];
    let prepared = |u: &mut Unit<'_>, base, steps: &[ProjectionStep], layout, live: &[_]| {
        let at = u.address(AddressBase::Value(base), steps, layout);
        let bounds = u.anchor(60, 70);
        let failure = u.releasing(live, Some("MEM-14"), bounds);
        u.builder.push(
            ENTRY,
            Inst::Address {
                address: at,
                failure,
            },
        );
        at
    };
    let n_at = prepared(&mut u, b, &[ProjectionStep::Field(1)], n_l, &live);
    let n = u.read(ENTRY, n_at, b);
    let text_at = prepared(&mut u, b, &[ProjectionStep::Field(0)], text_l, &live);
    let len = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::FlatLength {
            result: len,
            address: text_at,
        },
    );
    let index = [ProjectionStep::Field(0), ProjectionStep::Index(i)];
    let byte_at = prepared(&mut u, b, &index, byte_l, &live);
    let byte = u.read(ENTRY, byte_at, b);
    u.builder.unit.tables.loans.push(LoanInfo {
        address: text_at,
        mutable: false,
    });
    let loan = LoanId(u.builder.unit.tables.loans.len() as u32 - 1);
    u.builder.push(
        ENTRY,
        Inst::BeginLoan {
            loan,
            address: text_at,
            mutable: false,
            if_inactive: false,
        },
    );
    let view = u.value(string_l, Holding::Borrowed(loan));
    u.builder.push(
        ENTRY,
        Inst::FlatView {
            result: view,
            address: text_at,
        },
    );
    // The comparison's failure edge ends the view's loan before releasing its block.
    let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
    u.builder.push(
        cleanup,
        Inst::EndLoan {
            loan,
            if_active: false,
        },
    );
    u.release_all(cleanup, &live);
    let p = u.builder.propagate();
    u.builder.jump(cleanup, p, &[]);
    let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
    let other = u.constant(ENTRY, string_l, ConstantValue::String("hello".into()));
    let eq = u.compare(ENTRY, CompareOp::Eq, view, other, string_l, failure);
    u.builder.push(
        ENTRY,
        Inst::EndLoan {
            loan,
            if_active: false,
        },
    );
    u.release(ENTRY, b, item_l);
    let ell = u.constant(ENTRY, byte_l, ConstantValue::Integer(u128::from(b'l')));
    let is_l = u.binary(ENTRY, BinaryOp::Eq, byte, ell);
    let five = u.int(ENTRY, ScalarKind::U64, 5);
    let is_five = u.binary(ENTRY, BinaryOp::Eq, len, five);
    let (end, w) = u.weigh(ENTRY, &[eq, is_l, is_five]);
    let r = u.binary(end, BinaryOp::Add, n, w);
    u.builder.ret(end, Some(r));
    add(u);

    // flat_point(x, y) = 4 * x + 2 * (copy.a == x) + (copy.b == y): the fixed flat record
    // Point { x, y } built inline in a slot and copied by its bytes.
    let point_l = table.layout("point");
    let (pa_l, pb_l) = (field(point_l, 0), field(point_l, 1));
    let mut u = Unit::new(&table, "flat_point", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let point_symbol = u.symbol(SymbolKind::Type, "Point");
    let p = u.construct(
        ENTRY,
        ConstructorKind::Record(point_symbol),
        point_l,
        &[(0, x), (1, y)],
    );
    let slot = u.slot(point_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let at = u.address(AddressBase::Slot(slot), &[], point_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let v = u.store(ENTRY, at, p, slot);
    let copy = u.plain(point_l);
    let failure = u.fails(None);
    u.builder.push(
        ENTRY,
        Inst::FlatCopy {
            result: copy,
            address: at,
            layout: point_l,
            failure,
        },
    );
    let _ = v;
    u.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let other = u.slot(point_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot: other });
    let at = u.address(AddressBase::Slot(other), &[], point_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let w = u.store(ENTRY, at, copy, other);
    let a_at = u.address(AddressBase::Slot(other), &[ProjectionStep::Field(0)], pa_l);
    u.prepare(ENTRY, a_at, None, SourceAnchorId(0));
    let a = u.read(ENTRY, a_at, w);
    let b_at = u.address(AddressBase::Slot(other), &[ProjectionStep::Field(1)], pb_l);
    u.prepare(ENTRY, b_at, None, SourceAnchorId(0));
    let b = u.read(ENTRY, b_at, w);
    u.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: other,
            if_live: false,
        },
    );
    let is_x = u.binary(ENTRY, BinaryOp::Eq, a, x);
    let is_y = u.binary(ENTRY, BinaryOp::Eq, b, y);
    let four = u.int(ENTRY, ScalarKind::I64, 4);
    let scaled = u.binary(ENTRY, BinaryOp::Mul, x, four);
    let (end, w) = u.weigh(ENTRY, &[is_x, is_y]);
    let r = u.binary(end, BinaryOp::Add, scaled, w);
    u.builder.ret(end, Some(r));
    add(u);

    // flat_outer(x, y) = 4 * (len(outer.item.text) == 3) + 2 * (outer.item.n < outer.k)
    // + (outer.item.n == outer.k) over Outer { Item { "hey", x }, y }: the item block is
    // encoded behind its prefix, its extent read by the item's size program, and k lies at
    // its cursor past it.
    let (outer_l, box_outer_l) = (payload(&table, "outer"), table.layout("outer"));
    let encoded_l = field(outer_l, 0);
    assert_eq!(field(encoded_l, 0), item_l);
    assert_eq!(field(outer_l, 1), n_l);
    let mut u = Unit::new(&table, "flat_outer", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let item_symbol = u.symbol(SymbolKind::Type, "Item");
    let outer_symbol = u.symbol(SymbolKind::Type, "Outer");
    let hey = u.constant(ENTRY, string_l, ConstantValue::String("hey".into()));
    let item = u.construct_block(
        ENTRY,
        ConstructorKind::Record(item_symbol),
        item_l,
        box_item_l,
        &[(0, hey), (1, x)],
    );
    // The encoded item is only read: it stays owned and is released afterwards.
    let failure = u.releasing(&[(item, item_l)], None, SourceAnchorId(0));
    let o = u.construct_block_to(
        ENTRY,
        ConstructorKind::Record(outer_symbol),
        outer_l,
        box_outer_l,
        &[(0, item), (1, y)],
        failure,
        None,
    );
    let live = [(o, outer_l)];
    let failure = u.releasing(&live, None, SourceAnchorId(0));
    u.release_to(ENTRY, item, item_l, failure);
    let inner = [ProjectionStep::Field(0), ProjectionStep::Field(0)];
    let n_steps = [inner.as_slice(), &[ProjectionStep::Field(1)]].concat();
    let n_at = prepared(&mut u, o, &n_steps, n_l, &live);
    let n = u.read(ENTRY, n_at, o);
    let k_at = prepared(&mut u, o, &[ProjectionStep::Field(1)], n_l, &live);
    let k = u.read(ENTRY, k_at, o);
    let text_steps = [inner.as_slice(), &[ProjectionStep::Field(0)]].concat();
    let text_at = prepared(&mut u, o, &text_steps, text_l, &live);
    let len = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::FlatLength {
            result: len,
            address: text_at,
        },
    );
    u.release(ENTRY, o, outer_l);
    let three = u.int(ENTRY, ScalarKind::U64, 3);
    let is_three = u.binary(ENTRY, BinaryOp::Eq, len, three);
    let below = u.binary(ENTRY, BinaryOp::Lt, n, k);
    let same = u.binary(ENTRY, BinaryOp::Eq, n, k);
    let (end, r) = u.weigh(ENTRY, &[is_three, below, same]);
    u.builder.ret(end, Some(r));
    add(u);

    // flat_items(i, _) = 8 * (len(items) == 2) + 4 * (items[i].n == 7) + 2 * (items[i].n == 9)
    // + (len(items[i].text) == 2)
    // over the static Items { [Item { "ab", 7 }, Item { "xyz", 9 }] }: a list of variable
    // elements, counted after its prefix and walked to element i (MEM-14 past its end).
    let item_bytes = |text: &str, n: i64| {
        let mut bytes = (text.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(&n.to_le_bytes());
        let mut element = (bytes.len() as u64).to_le_bytes().to_vec();
        element.extend(bytes);
        element
    };
    let mut content = 2u64.to_le_bytes().to_vec();
    content.extend(item_bytes("ab", 7));
    content.extend(item_bytes("xyz", 9));
    let mut items = (content.len() as u64).to_le_bytes().to_vec();
    items.extend(content);
    statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("ITEMS"),
            instance: Some(instance("ITEMS")),
        },
        items_l,
        items,
    ));
    let mut u = Unit::new(&table, "flat_items", ENTRY_ABI);
    let i = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Constant, "ITEMS");
    let st = u.value(box_items_l, Holding::Owned);
    u.builder
        .push(ENTRY, Inst::ReadStatic { result: st, symbol });
    let live = [(st, items_l)];
    let list_at = prepared(&mut u, st, &[ProjectionStep::Field(0)], list_l, &live);
    let count = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::FlatLength {
            result: count,
            address: list_at,
        },
    );
    let element = [
        ProjectionStep::Field(0),
        ProjectionStep::Index(i),
        ProjectionStep::Field(0),
    ];
    let n_steps = [element.as_slice(), &[ProjectionStep::Field(1)]].concat();
    let n_at = prepared(&mut u, st, &n_steps, n_l, &live);
    let n = u.read(ENTRY, n_at, st);
    let text_steps = [element.as_slice(), &[ProjectionStep::Field(0)]].concat();
    let text_at = prepared(&mut u, st, &text_steps, text_l, &live);
    let text_len = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::FlatLength {
            result: text_len,
            address: text_at,
        },
    );
    u.release(ENTRY, st, items_l);
    let two = u.int(ENTRY, ScalarKind::U64, 2);
    let pair = u.binary(ENTRY, BinaryOp::Eq, count, two);
    let short = u.binary(ENTRY, BinaryOp::Eq, text_len, two);
    let seven = u.constant(ENTRY, n_l, ConstantValue::Integer(7));
    let nine = u.constant(ENTRY, n_l, ConstantValue::Integer(9));
    let is_seven = u.binary(ENTRY, BinaryOp::Eq, n, seven);
    let is_nine = u.binary(ENTRY, BinaryOp::Eq, n, nine);
    let (end, r) = u.weigh(ENTRY, &[pair, is_seven, is_nine, short]);
    u.builder.ret(end, Some(r));
    add(u);
    let _ = element_l;

    // close_held(h, x) = x after an inline Release of { h, Box(Rec1 { x }) }: the handle is
    // closed through the Io adapter (the harness fails handle 13), the box released all the
    // same. C calls it with each handle; the heap checker has no handle arguments.
    let held_file_l = table.layout("held_file");
    let file_l = table.layout("file");
    let mut u = Unit::new(&table, "close_held", CLOSE_ABI);
    let h = u.param(file_l);
    u.builder.unit.values[h.index()].holding = Holding::Owned;
    let x = u.param(i64_l);
    let rec1_symbol = u.symbol(SymbolKind::Type, "Rec1");
    let held_symbol = u.symbol(SymbolKind::Type, "HeldFile");
    let failure = u.releasing(&[(h, file_l)], None, SourceAnchorId(0));
    let b = u.construct_block_to(
        ENTRY,
        ConstructorKind::Record(rec1_symbol),
        rec1_l,
        box_rec1_l,
        &[(0, x)],
        failure,
        None,
    );
    let held = u.construct(
        ENTRY,
        ConstructorKind::Record(held_symbol),
        held_file_l,
        &[(0, h), (1, b)],
    );
    u.builder.unit.values[held.index()].holding = Holding::Owned;
    u.release(ENTRY, held, held_file_l);
    u.builder.ret(ENTRY, Some(x));
    add(u);

    // simd_lanes(x, y) = 4 * (lane 0 == lane 2) + 2 * (lane 1 == lane 3) + (lane 0 < lane 1)
    // of Simd[i64, 4](x, y, x, y): K's legalization builds a Simd value as an inline
    // sequence of its lanes and reads them by index.
    let simd_l = table.layout("simd4");
    let hirsel_lir::layout_types::LayoutKind::Simd {
        element: lane_l, ..
    } = table.table.layouts[simd_l.0 as usize].kind
    else {
        panic!("simd4 is not a Simd layout")
    };
    let mut u = Unit::new(&table, "simd_lanes", ENTRY_ABI);
    let x = u.param(i64_l);
    let y = u.param(i64_l);
    let v = u.construct(
        ENTRY,
        ConstructorKind::Sequence,
        simd_l,
        &[(0, x), (1, y), (2, x), (3, y)],
    );
    let slot = u.slot(simd_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let at = u.address(AddressBase::Slot(slot), &[], simd_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let version = u.store(ENTRY, at, v, slot);
    let mut lane = |i: u128| {
        let index = u.int(ENTRY, ScalarKind::U64, i);
        let at = u.address(
            AddressBase::Slot(slot),
            &[ProjectionStep::Index(index)],
            lane_l,
        );
        u.prepare(ENTRY, at, Some("MEM-14"), SourceAnchorId(0));
        u.read(ENTRY, at, version)
    };
    let lanes = [lane(0), lane(1), lane(2), lane(3)];
    u.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let same_even = u.binary(ENTRY, BinaryOp::Eq, lanes[0], lanes[2]);
    let same_odd = u.binary(ENTRY, BinaryOp::Eq, lanes[1], lanes[3]);
    let rising = u.binary(ENTRY, BinaryOp::Lt, lanes[0], lanes[1]);
    let (end, r) = u.weigh(ENTRY, &[same_even, same_odd, rising]);
    u.builder.ret(end, Some(r));
    add(u);

    // LIR schema revision 7. push_i64(mut xs, x): a library body appending as
    // `list.push` does: count and capacity, `SeqReserve` when full, the next slot through
    // a `Slot` step, then `SeqSetLen`. Its caller holds the only reference.
    let (list_l, seq_l) = (table.layout("list64"), payload(&table, "list64"));
    let mut u = Unit::new(&table, "push_i64", "push list64");
    u.builder.unit.header.origin = UnitOrigin::Library;
    let this = u.param(list_l);
    let x = u.param(i64_l);
    let slot = u.slot(list_l, SlotKind::Parameter(0));
    let at = u.address(AddressBase::Slot(slot), &[], list_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    u.builder.unit.tables.loans.push(LoanInfo {
        address: at,
        mutable: false,
    });
    let loan = LoanId(u.builder.unit.tables.loans.len() as u32 - 1);
    u.builder.push(
        ENTRY,
        Inst::BeginLoan {
            loan,
            address: at,
            mutable: false,
            if_inactive: false,
        },
    );
    let view = u.value(list_l, Holding::Borrowed(loan));
    u.builder.push(
        ENTRY,
        Inst::Read {
            result: view,
            address: at,
            reaching: this,
            mode: ReadMode::Borrow,
        },
    );
    let len = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::SeqLen {
            result: len,
            value: view,
        },
    );
    let capacity = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::SeqCapacity {
            result: capacity,
            value: view,
        },
    );
    u.builder.push(
        ENTRY,
        Inst::EndLoan {
            loan,
            if_active: false,
        },
    );
    let full = u.binary(ENTRY, BinaryOp::Ge, len, capacity);
    let grow = u.builder.block(BlockKind::Body, &[]);
    let joined = u.version(slot);
    let append = u.builder.block(BlockKind::Body, &[joined]);
    let yes = u.builder.edge(grow, &[]);
    let no = u.builder.edge(append, &[this]);
    u.builder.term(
        ENTRY,
        Terminator::Branch {
            condition: full,
            yes,
            no,
        },
    );
    let two = u.int(grow, ScalarKind::U64, 2);
    let twice = u.binary(grow, BinaryOp::Mul, capacity, two);
    let four = u.int(grow, ScalarKind::U64, 4);
    let wanted = u.binary(grow, BinaryOp::Add, twice, four);
    let grown = u.version(slot);
    u.builder.push(
        grow,
        Inst::SeqReserve {
            address: at,
            capacity: wanted,
            version: grown,
        },
    );
    u.builder.jump(grow, append, &[grown]);
    let element = u.address(AddressBase::Slot(slot), &[ProjectionStep::Slot(len)], i64_l);
    u.prepare(append, element, None, SourceAnchorId(0));
    u.store(append, element, x, slot);
    let one = u.int(append, ScalarKind::U64, 1);
    let count = u.binary(append, BinaryOp::Add, len, one);
    let counted = u.version(slot);
    u.builder.push(
        append,
        Inst::SeqSetLen {
            address: at,
            len: count,
            version: counted,
        },
    );
    u.builder.ret(append, None);
    add(u);

    // seq_push(a, b) = 16 * (len == 4) + 8 * (capacity >= 4) + 4 * (hash(xs) == hash(ys))
    // + 2 * (xs[0] == a) + (xs[3] == b) for xs = [a] pushed b three times and
    // ys = [a, b, b, b]: the pushes grow the one-element block across size classes, and
    // the hash and the indexed reads see the slots they wrote.
    let mut u = Unit::new(&table, "seq_push", ENTRY_ABI);
    let a = u.param(i64_l);
    let b = u.param(i64_l);
    let slot = u.slot(list_l, SlotKind::Local);
    u.builder.push(ENTRY, Inst::StorageLive { slot });
    let at = u.address(AddressBase::Slot(slot), &[], list_l);
    u.prepare(ENTRY, at, None, SourceAnchorId(0));
    let xs = u.construct_block(ENTRY, ConstructorKind::Sequence, seq_l, list_l, &[(0, a)]);
    u.store(ENTRY, at, xs, slot);
    let push = u.symbol(SymbolKind::Function, "push_i64");
    let mut current = ENTRY;
    let mut version = ValueId(0);
    for _ in 0..3 {
        // A `mut` argument names the slot's version after the call, on both edges.
        version = u.version(slot);
        let normal = u.builder.block(BlockKind::Body, &[]);
        // A failed push leaves the list in the slot: take and release it.
        let cleanup = u.builder.block(BlockKind::FailureCleanup, &[]);
        let held = u.value(list_l, Holding::Owned);
        u.builder.push(
            cleanup,
            Inst::Read {
                result: held,
                address: at,
                reaching: version,
                mode: ReadMode::Take,
            },
        );
        u.release(cleanup, held, seq_l);
        u.builder.push(
            cleanup,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        let p = u.builder.propagate();
        u.builder.jump(cleanup, p, &[]);
        let failure = u.failure_to(cleanup, None, SourceAnchorId(0));
        u.call(
            current,
            Callee::Direct(push),
            "push list64",
            &[
                (
                    Passing::Mut,
                    ArgumentValue::Address {
                        address: at,
                        version,
                    },
                ),
                (Passing::Borrow, ArgumentValue::Value(b)),
            ],
            normal,
            &[],
            failure,
        );
        current = normal;
    }
    let pushed = u.value(list_l, Holding::Owned);
    u.builder.push(
        current,
        Inst::Read {
            result: pushed,
            address: at,
            reaching: version,
            mode: ReadMode::Take,
        },
    );
    u.builder.push(
        current,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let failure = u.releasing(&[(pushed, seq_l)], None, SourceAnchorId(0));
    let ys = u.construct_block_to(
        current,
        ConstructorKind::Sequence,
        seq_l,
        list_l,
        &[(0, a), (1, b), (2, b), (3, b)],
        failure,
        None,
    );
    let live = [(pushed, seq_l), (ys, seq_l)];
    let len = u.plain(u64_l);
    u.builder.push(
        current,
        Inst::SeqLen {
            result: len,
            value: pushed,
        },
    );
    let capacity = u.plain(u64_l);
    u.builder.push(
        current,
        Inst::SeqCapacity {
            result: capacity,
            value: pushed,
        },
    );
    let hash = |u: &mut Unit<'_>, value| {
        u.structural(list_l);
        let result = u.plain(u64_l);
        let failure = u.releasing(&live, Some("MEM-3"), SourceAnchorId(0));
        u.builder.push(
            current,
            Inst::Hash {
                result,
                value,
                layout: list_l,
                failure,
            },
        );
        result
    };
    let hx = hash(&mut u, pushed);
    let hy = hash(&mut u, ys);
    // The written slots, read back by checked index.
    let element = |u: &mut Unit<'_>, i: u128| {
        let index = u.int(current, ScalarKind::U64, i);
        let at = u.address(
            AddressBase::Value(pushed),
            &[ProjectionStep::Index(index)],
            i64_l,
        );
        let failure = u.releasing(&live, Some("MEM-14"), SourceAnchorId(0));
        u.builder.push(
            current,
            Inst::Address {
                address: at,
                failure,
            },
        );
        u.read(current, at, pushed)
    };
    let x0 = element(&mut u, 0);
    let x3 = element(&mut u, 3);
    u.release_all(current, &live);
    let four = u.int(current, ScalarKind::U64, 4);
    let counted = u.binary(current, BinaryOp::Eq, len, four);
    let room = u.binary(current, BinaryOp::Ge, capacity, four);
    let hashed = u.binary(current, BinaryOp::Eq, hx, hy);
    let first = u.binary(current, BinaryOp::Eq, x0, a);
    let last = u.binary(current, BinaryOp::Eq, x3, b);
    let (end, r) = u.weigh(current, &[counted, room, hashed, first, last]);
    u.builder.ret(end, Some(r));
    add(u);

    // flat_walk(i, _) = 2 * (items[0].n == 7) + (items[i].n == 9) over the static Items:
    // element 0 at offset 0, its end by `FlatNext`, and element 1 at that offset.
    // The arbitrary parameter checks bounds through Index before the proved walk.
    let mut u = Unit::new(&table, "flat_walk", ENTRY_ABI);
    let i = u.param(i64_l);
    let _ = u.param(i64_l);
    let symbol = u.symbol(SymbolKind::Constant, "ITEMS");
    let st = u.value(box_items_l, Holding::Owned);
    u.builder
        .push(ENTRY, Inst::ReadStatic { result: st, symbol });
    let live = [(st, items_l)];
    let zero = u.int(ENTRY, ScalarKind::U64, 0);
    let first = [
        ProjectionStep::Field(0),
        ProjectionStep::FlatAt {
            index: zero,
            offset: zero,
        },
    ];
    let first_at = prepared(&mut u, st, &first, element_l, &live);
    let next = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::FlatNext {
            result: next,
            address: first_at,
        },
    );
    let n_of = |at: &[ProjectionStep]| {
        [at, &[ProjectionStep::Field(0), ProjectionStep::Field(1)]].concat()
    };
    let n0_at = prepared(&mut u, st, &n_of(&first), n_l, &live);
    let n0 = u.read(ENTRY, n0_at, st);
    // The index as a u64: i64 to u64 keeps the bits.
    let index = u.plain(u64_l);
    u.builder.push(
        ENTRY,
        Inst::Convert {
            result: index,
            flag: None,
            operand: i,
            mode: ConvertMode::Truncate,
        },
    );
    let bounds = [ProjectionStep::Field(0), ProjectionStep::Index(index)];
    prepared(&mut u, st, &bounds, element_l, &live);
    let index = u.int(ENTRY, ScalarKind::U64, 1);
    let nth = [
        ProjectionStep::Field(0),
        ProjectionStep::FlatAt {
            index,
            offset: next,
        },
    ];
    let ni_at = prepared(&mut u, st, &n_of(&nth), n_l, &live);
    let ni = u.read(ENTRY, ni_at, st);
    u.release(ENTRY, st, items_l);
    let seven = u.constant(ENTRY, n_l, ConstantValue::Integer(7));
    let nine = u.constant(ENTRY, n_l, ConstantValue::Integer(9));
    let is_seven = u.binary(ENTRY, BinaryOp::Eq, n0, seven);
    let is_nine = u.binary(ENTRY, BinaryOp::Eq, ni, nine);
    let (end, r) = u.weigh(ENTRY, &[is_seven, is_nine]);
    u.builder.ret(end, Some(r));
    add(u);

    let mut finalizers = BTreeMap::new();
    finalizers.insert(res_l, instance("Res_drop"));
    finalizers.insert(trace_l, instance("Trace_drop"));

    Program {
        table,
        manifest,
        units,
        statics,
        finalizers,
    }
}

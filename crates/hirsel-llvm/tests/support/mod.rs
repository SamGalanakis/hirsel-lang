//! Hand-built Ready programs for emission tests.
//!
//! Layouts and `FnAbi`s come from the `LayoutBuilder` over a small type store, so the
//! emitter sees the same physical facts real programs will have. Every unit is sealed by
//! the LIR validator (`validate::<Ready>`); a fixture the validator rejects fails its test.

pub mod glue;
pub mod native;
pub mod programs;
pub mod ready;

use std::collections::BTreeMap;

use hirsel_core_contract::{
    Disposition, EngineDispositions, Manifest, OperationId, OperationRow, Passing, SemanticEffects,
};
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::*;
use hirsel_lir::layout::{EnvironmentShape, LayoutBuilder, OwnerRepresentation, Owners};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind, TargetSpec};
use hirsel_lir::stage::{InputIdentity, Ready, StageKind, ValidatedUnit};
use hirsel_lir::validate::{self, Inputs};
use hirsel_llvm::{FailureCode, FailureCodes};
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId, PassingMode};
use hirsel_tir::RepresentationRecord;
use hirsel_types::*;

pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "0".into(),
        },
        modules: vec!["emit".into()],
        name: name.into(),
    })
}

pub fn hash(label: &str) -> ContentHash {
    ContentHash::of_bytes(label.as_bytes())
}

/// The instance digest of fixture unit `name`; direct calls name it.
pub fn instance(name: &str) -> ContentHash {
    hash(&format!("{name} instance"))
}

pub fn revision(name: &str) -> ContentHash {
    hash(&format!("{name} source"))
}

pub fn input() -> InputIdentity {
    InputIdentity {
        interface_version: hirsel_lir::INTERFACE_VERSION,
        digest: hash("emission fixture input"),
    }
}

/// The rules fixtures fail with and their runtime codes.
pub fn failure_codes() -> FailureCodes {
    let mut codes = BTreeMap::new();
    for (rule, kind, code) in [
        ("SEM-1", 1, 101),
        ("MEM-3", 1, 103),
        ("MEM-13", 1, 113),
        ("MEM-14", 1, 114),
        ("LANG-9", 2, 9),
    ] {
        codes.insert(rule.to_owned(), FailureCode { kind, rule: code });
    }
    FailureCodes { codes }
}

/// A type store under construction.
#[derive(Default)]
pub struct Types {
    pub store: TypeStore,
}

impl Types {
    pub fn add(&mut self, kind: TypeKind) -> TypeId {
        self.add_with(kind, Modes::global())
    }
    pub fn add_with(&mut self, kind: TypeKind, modes: Modes) -> TypeId {
        self.store.push(Type {
            modes,
            ..Type::with_kind(kind)
        })
    }
    pub fn p(&mut self, p: Primitive) -> TypeId {
        self.add(TypeKind::Primitive(p))
    }
    pub fn unit(&mut self) -> TypeId {
        self.add(TypeKind::Tuple(Vec::new().into()))
    }
    pub fn record(&mut self, fields: &[TypeId]) -> TypeId {
        self.add(TypeKind::Record(
            fields
                .iter()
                .enumerate()
                .map(|(i, &ty)| Field {
                    growable: false,
                    name: format!("f{i}"),
                    ty,
                })
                .collect(),
        ))
    }
    pub fn variant(&mut self, variants: &[&[TypeId]]) -> TypeId {
        self.add(TypeKind::Variant(
            variants
                .iter()
                .enumerate()
                .map(|(i, fields)| Variant {
                    name: format!("V{i}"),
                    fields: fields.iter().map(|&t| (None, t)).collect(),
                })
                .collect(),
        ))
    }
    pub fn named(&mut self, definition: &DefPath) -> TypeId {
        self.add(TypeKind::Named {
            definition: definition.clone(),
            arguments: Vec::new().into(),
        })
    }
    pub fn inline(&mut self, element: TypeId, n: u64) -> TypeId {
        self.add(TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Constant(n)),
            ]
            .into(),
        })
    }
    pub fn simd(&mut self, element: TypeId, lanes: u64) -> TypeId {
        self.add(TypeKind::Builtin {
            kind: BuiltinType::Simd,
            arguments: vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Constant(lanes)),
            ]
            .into(),
        })
    }
    pub fn boxed(&mut self, payload: TypeId) -> TypeId {
        self.add(TypeKind::Box(payload))
    }
    pub fn list(&mut self, element: TypeId) -> TypeId {
        self.add(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(element)].into(),
        })
    }
    pub fn function(&mut self, parameters: &[(PassingMode, TypeId)], result: TypeId) -> TypeId {
        self.add(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: Vec::new().into(),
            parameters: parameters
                .iter()
                .map(|&(passing, ty)| Parameter {
                    name: None,
                    label: None,
                    passing,
                    ty,
                })
                .collect(),
            result,
            error: None,
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::inert(),
            uses: Uses::Unrestricted,
            contracts: Vec::new().into(),
            incomplete: false,
            foreign: None,
        }))
    }
}

/// A `@packed` nominal record `Wire { u8, u64, u16 }`.
pub fn wire_owner(owners: &mut Owners) -> DefPath {
    let mut t = Types::default();
    let (a, b, c) = (t.p(Primitive::U8), t.p(Primitive::U64), t.p(Primitive::U16));
    let shape = t.record(&[a, b, c]);
    let definition = path("Wire");
    owners.nominals.insert(
        definition.clone(),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("Wire revision"),
            generics: Vec::new(),
            types: t.store,
            private_shape: shape,
            layout_promises: Layout {
                packed: true,
                ..Layout::default()
            },
            resource: false,
            finalizer: None,
            unsafe_ffi_package: false,
        }),
    );
    definition
}

/// The layout table of one program and the ids its units name.
pub struct Table {
    pub target: TargetSpec,
    pub table: LayoutTable,
    pub layouts: BTreeMap<&'static str, LayoutId>,
    pub abis: BTreeMap<&'static str, FnAbiId>,
    /// The block header layout: a reuse token's.
    pub header: LayoutId,
}

impl Table {
    pub fn layout(&self, name: &str) -> LayoutId {
        self.layouts[name]
    }
    pub fn abi(&self, name: &str) -> FnAbiId {
        self.abis[name]
    }
    pub fn scalar(&self, kind: ScalarKind) -> LayoutId {
        let found = self
            .table
            .layouts
            .iter()
            .position(|l| l.kind == LayoutKind::Scalar(kind))
            .unwrap_or_else(|| panic!("the table has no {kind:?} layout"));
        LayoutId(found as u32)
    }
}

/// A closure environment: its name, the closure body's definition and the captures.
type Environment = (&'static str, &'static str, Vec<(TypeId, Passing)>, Vec<u32>);

/// Declared types and callable signatures, laid out together for one target.
pub struct Schema {
    pub types: Types,
    pub owners: Owners,
    named: Vec<(&'static str, TypeId)>,
    signatures: Vec<(&'static str, TypeId, CallableShape)>,
    /// Closure environments: name, closure body definition, captures.
    environments: Vec<Environment>,
}

impl Default for Schema {
    fn default() -> Self {
        Self::new()
    }
}

impl Schema {
    pub fn new() -> Self {
        Self {
            types: Types::default(),
            owners: Owners::default(),
            named: Vec::new(),
            signatures: Vec::new(),
            environments: Vec::new(),
        }
    }
    /// Declares the environment of closure body `definition` capturing `captures`.
    pub fn environment(
        &mut self,
        name: &'static str,
        definition: &'static str,
        captures: &[(TypeId, Passing)],
    ) {
        self.environments
            .push((name, definition, captures.to_vec(), Vec::new()));
    }
    pub fn movable_environment(
        &mut self,
        name: &'static str,
        definition: &'static str,
        captures: &[(TypeId, Passing)],
        movable: &[u32],
    ) {
        self.environments
            .push((name, definition, captures.to_vec(), movable.to_vec()));
    }
    /// Names a type whose layout units refer to.
    pub fn name(&mut self, name: &'static str, ty: TypeId) -> TypeId {
        self.named.push((name, ty));
        ty
    }
    /// Declares a definition signature.
    pub fn signature(
        &mut self,
        name: &'static str,
        parameters: &[(PassingMode, TypeId)],
        result: TypeId,
    ) -> TypeId {
        self.shaped(name, CallableShape::Definition, parameters, result)
    }
    pub fn shaped(
        &mut self,
        name: &'static str,
        shape: CallableShape,
        parameters: &[(PassingMode, TypeId)],
        result: TypeId,
    ) -> TypeId {
        let ty = self.types.function(parameters, result);
        self.signatures.push((name, ty, shape));
        ty
    }
    pub fn build(&self, target: &TargetSpec) -> Table {
        let mut builder = LayoutBuilder::new(target, &self.owners);
        let mut layouts = BTreeMap::new();
        for &(name, ty) in &self.named {
            layouts.insert(
                name,
                builder.layout(&self.types.store, ty).expect("a layout"),
            );
        }
        for (name, definition, captures, movable) in &self.environments {
            let definition = path(definition);
            let shape = EnvironmentShape {
                definition: &definition,
                body: 0,
                store: &self.types.store,
                captures,
                sizes: &[],
                movable,
            };
            layouts.insert(*name, builder.environment(&shape).expect("an environment"));
        }
        let mut abis = BTreeMap::new();
        for &(name, ty, shape) in &self.signatures {
            abis.insert(
                name,
                builder
                    .fn_abi(&self.types.store, ty, shape)
                    .expect("an FnAbi"),
            );
        }
        let header = builder.header();
        let table = builder.finish();
        if let Err(errors) = hirsel_lir::layout_validate::validate_table(&table, target) {
            panic!("the fixture table fails validation: {errors:#?}");
        }
        Table {
            target: target.clone(),
            table,
            layouts,
            abis,
            header,
        }
    }
}

/// A fixture operation returning f64, for generic symbol-template tests.
pub fn operation() -> OperationId {
    OperationId {
        definition: path("sum"),
        declaration_digest: hash("sum declaration"),
    }
}

/// A manifest with one fixture runtime primitive whose symbol template is
/// resolved per call (`hirsel_lir::abi::resolve_symbol`).
pub fn manifest() -> Manifest {
    let mut m = Manifest::skeleton([operation()]);
    m.replace(OperationRow {
        id: operation(),
        ownership: None,
        effects: SemanticEffects::CONSERVATIVE,
        dispositions: EngineDispositions {
            reference: Disposition::Body,
            native: Disposition::RuntimePrimitive("hirsel_rt_v2_{F}_sum".into()),
            heap_check: Disposition::Body,
        },
        vectors: Vec::new(),
        failure: None,
    })
    .expect("a manifest row");
    m
}

/// The representation a value of `layout` has.
pub fn repr_of(table: &LayoutTable, layout: LayoutId) -> ValueRepr {
    match table.layouts[layout.0 as usize].kind {
        LayoutKind::Unit => ValueRepr::Unit,
        LayoutKind::Scalar(kind) => ValueRepr::Scalar(kind),
        LayoutKind::Block { .. } | LayoutKind::Box { .. } => ValueRepr::Block,
        LayoutKind::Callable { .. } => ValueRepr::Callable,
        LayoutKind::Header => ValueRepr::Token,
        LayoutKind::Handle => ValueRepr::Handle,
        LayoutKind::Path => ValueRepr::Scalar(ScalarKind::U64),
        _ => ValueRepr::Aggregate,
    }
}

/// One Ready unit under construction against a finished table.
pub struct Unit<'t> {
    pub table: &'t Table,
    pub lir: LirUnit<Ready>,
    insts: Vec<Vec<Inst>>,
    terms: Vec<Option<Terminator>>,
    propagate: Option<BlockId>,
}

pub const ENTRY: BlockId = BlockId(0);

impl<'t> Unit<'t> {
    /// A unit named `name` with the signature `abi`, and its entry block.
    pub fn new(table: &'t Table, name: &str, abi: &str) -> Self {
        let mut lir = LirUnit::<Ready>::new(UnitHeader {
            stage: StageKind::Ready,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: hirsel_lir::ir::UnitOrigin::Source,
            input: input(),
            definition: path(name),
            implementation_revision: revision(name),
            instance: instance(name),
            target: table.table.target,
            layouts: table.table.digest(),
            abi: table.abi(abi),
        });
        lir.tables.task_domains = 1;
        lir.scopes.push(ScopeInfo { parent: None });
        lir.tables.source_anchors.push(SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange { start: 10, end: 20 },
        });
        let mut unit = Self {
            table,
            lir,
            insts: Vec::new(),
            terms: Vec::new(),
            propagate: None,
        };
        unit.block(BlockKind::Body, &[]);
        unit
    }

    /// A source anchor at a byte range; failures and sites name it.
    pub fn anchor(&mut self, start: u32, end: u32) -> SourceAnchorId {
        self.lir.tables.source_anchors.push(SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange { start, end },
        });
        SourceAnchorId(self.lir.tables.source_anchors.len() as u32 - 1)
    }

    pub fn ty(&mut self, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
        let ty = LirType { repr, layout };
        if let Some(i) = self.lir.types.iter().position(|t| *t == ty) {
            return LirTypeId(i as u32);
        }
        self.lir.types.push(ty);
        LirTypeId(self.lir.types.len() as u32 - 1)
    }

    pub fn value(&mut self, layout: LayoutId, holding: Holding) -> ValueId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.lir.values.push(ValueInfo { ty, holding });
        ValueId(self.lir.values.len() as u32 - 1)
    }

    pub fn plain(&mut self, layout: LayoutId) -> ValueId {
        self.value(layout, Holding::Plain)
    }

    /// The value of the next declared parameter.
    pub fn param(&mut self, layout: LayoutId) -> ValueId {
        let v = self.plain(layout);
        self.lir.parameters.push(v);
        v
    }

    pub fn slot(&mut self, layout: LayoutId, kind: SlotKind) -> SlotId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.lir.slots.push(SlotInfo {
            ty,
            kind,
            scope: ScopeId(0),
        });
        SlotId(self.lir.slots.len() as u32 - 1)
    }

    pub fn address(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        layout: LayoutId,
    ) -> AddressId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        let projection = PoolRange::push(&mut self.lir.pools.projections, steps.iter().copied());
        self.lir.addresses.push(AddressInfo {
            base,
            projection,
            ty,
        });
        AddressId(self.lir.addresses.len() as u32 - 1)
    }

    pub fn block(&mut self, kind: BlockKind, params: &[ValueId]) -> BlockId {
        let parameters = PoolRange::push(&mut self.lir.pools.values, params.iter().copied());
        self.lir.blocks.push(Block {
            kind,
            parameters,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        });
        self.insts.push(Vec::new());
        self.terms.push(None);
        BlockId(self.lir.blocks.len() as u32 - 1)
    }

    pub fn push(&mut self, b: BlockId, inst: Inst) {
        self.insts[b.index()].push(inst);
    }

    pub fn term(&mut self, b: BlockId, term: Terminator) {
        self.terms[b.index()] = Some(term);
    }

    pub fn edge(&mut self, target: BlockId, args: &[ValueId]) -> EdgeId {
        let arguments = PoolRange::push(&mut self.lir.pools.values, args.iter().copied());
        self.lir.tables.edges.push(Edge { target, arguments });
        EdgeId(self.lir.tables.edges.len() as u32 - 1)
    }

    pub fn jump(&mut self, from: BlockId, to: BlockId, args: &[ValueId]) {
        let e = self.edge(to, args);
        self.term(from, Terminator::Jump(e));
    }

    /// The shared failure-cleanup block that only propagates.
    pub fn propagate(&mut self) -> BlockId {
        if let Some(b) = self.propagate {
            return b;
        }
        let b = self.block(BlockKind::FailureCleanup, &[]);
        self.term(b, Terminator::Propagate);
        self.propagate = Some(b);
        b
    }

    /// A failure edge into `target` that starts `rule` (None propagates a callee's).
    pub fn failure_to(
        &mut self,
        target: BlockId,
        rule: Option<&str>,
        anchor: SourceAnchorId,
    ) -> FailureId {
        let cleanup = self.edge(target, &[]);
        self.lir.tables.failures.push(FailureEdge {
            cleanup,
            rule: rule.map(str::to_owned),
            anchor,
            dispose: PoolRange::EMPTY,
        });
        FailureId(self.lir.tables.failures.len() as u32 - 1)
    }

    /// A failure edge to plain propagation.
    pub fn fails(&mut self, rule: Option<&str>) -> FailureId {
        let p = self.propagate();
        self.failure_to(p, rule, SourceAnchorId(0))
    }

    pub fn constant(&mut self, b: BlockId, layout: LayoutId, value: ConstantValue) -> ValueId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.lir.tables.constants.push(ConstantData { ty, value });
        let constant = ConstantId(self.lir.tables.constants.len() as u32 - 1);
        let result = self.plain(layout);
        self.push(b, Inst::Const { result, constant });
        result
    }

    pub fn int(&mut self, b: BlockId, kind: ScalarKind, bits: u128) -> ValueId {
        let layout = self.table.scalar(kind);
        self.constant(b, layout, ConstantValue::Integer(bits))
    }

    /// `left op right`, failing with `rule` at `anchor`.
    pub fn binary_at(
        &mut self,
        b: BlockId,
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
        rule: &str,
        anchor: SourceAnchorId,
    ) -> ValueId {
        let layout = if matches!(
            op,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        ) {
            self.table.scalar(ScalarKind::Bool)
        } else {
            self.lir.types[self.lir.values[left.index()].ty.index()].layout
        };
        let result = self.plain(layout);
        let p = self.propagate();
        let failure = self.failure_to(p, Some(rule), anchor);
        self.push(
            b,
            Inst::Binary {
                result,
                op,
                left,
                right,
                failure,
            },
        );
        result
    }

    pub fn binary(&mut self, b: BlockId, op: BinaryOp, left: ValueId, right: ValueId) -> ValueId {
        self.binary_at(b, op, left, right, "SEM-1", SourceAnchorId(0))
    }

    pub fn unary(&mut self, b: BlockId, op: UnaryOp, operand: ValueId) -> ValueId {
        let layout = self.lir.types[self.lir.values[operand.index()].ty.index()].layout;
        let result = self.plain(layout);
        let failure = self.fails(Some("SEM-1"));
        self.push(
            b,
            Inst::Unary {
                result,
                op,
                operand,
                failure,
            },
        );
        result
    }

    /// Prepares `address` in `b`; a failing check starts `rule` at `anchor`.
    pub fn prepare(
        &mut self,
        b: BlockId,
        address: AddressId,
        rule: Option<&str>,
        anchor: SourceAnchorId,
    ) {
        let p = self.propagate();
        let failure = self.failure_to(p, rule, anchor);
        self.push(b, Inst::Address { address, failure });
    }

    pub fn read(&mut self, b: BlockId, address: AddressId, reaching: ValueId) -> ValueId {
        let layout = self.lir.types[self.lir.addresses[address.index()].ty.index()].layout;
        let result = self.plain(layout);
        self.push(
            b,
            Inst::Read {
                result,
                address,
                reaching,
                mode: ReadMode::Bits,
            },
        );
        result
    }

    /// Stores into dead storage; returns the slot's new version.
    pub fn store(
        &mut self,
        b: BlockId,
        address: AddressId,
        value: ValueId,
        slot: SlotId,
    ) -> ValueId {
        let version = self.version(slot);
        self.push(
            b,
            Inst::Store {
                address,
                value,
                version,
            },
        );
        version
    }

    /// A fresh version name of `slot`'s type.
    pub fn version(&mut self, slot: SlotId) -> ValueId {
        let ty = self.lir.slots[slot.index()].ty;
        self.lir.values.push(ValueInfo {
            ty,
            holding: Holding::Plain,
        });
        ValueId(self.lir.values.len() as u32 - 1)
    }

    pub fn construct(
        &mut self,
        b: BlockId,
        constructor: ConstructorKind,
        layout: LayoutId,
        fields: &[(u32, ValueId)],
    ) -> ValueId {
        let fields = PoolRange::push(
            &mut self.lir.pools.fields,
            fields
                .iter()
                .map(|&(field, value)| FieldValue { field, value }),
        );
        self.lir.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor,
            layout,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
            anchor: SourceAnchorId(0),
        });
        let construct = ConstructId(self.lir.tables.constructs.len() as u32 - 1);
        let result = self.plain(layout);
        let failure = self.fails(None);
        self.push(
            b,
            Inst::Construct {
                result,
                construct,
                failure,
            },
        );
        result
    }

    /// A heap block construction of `payload` (an owned `block` reference).
    pub fn construct_block(
        &mut self,
        b: BlockId,
        constructor: ConstructorKind,
        payload: LayoutId,
        block: LayoutId,
        fields: &[(u32, ValueId)],
    ) -> ValueId {
        let failure = self.fails(None);
        self.construct_block_to(b, constructor, payload, block, fields, failure, None)
    }

    /// A heap block construction failing to `failure`, reusing `token` (with its kept-field
    /// mask) if given.
    #[allow(clippy::too_many_arguments)]
    pub fn construct_block_to(
        &mut self,
        b: BlockId,
        constructor: ConstructorKind,
        payload: LayoutId,
        block: LayoutId,
        fields: &[(u32, ValueId)],
        failure: FailureId,
        token: Option<(ValueId, Option<MaskId>)>,
    ) -> ValueId {
        let fields = PoolRange::push(
            &mut self.lir.pools.fields,
            fields
                .iter()
                .map(|&(field, value)| FieldValue { field, value }),
        );
        self.lir.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor,
            layout: payload,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
            anchor: SourceAnchorId(0),
        });
        let construct = ConstructId(self.lir.tables.constructs.len() as u32 - 1);
        let result = self.value(block, Holding::Owned);
        let inst = match token {
            None => Inst::Construct {
                result,
                construct,
                failure,
            },
            Some((token, kept)) => Inst::Reuse {
                result,
                token,
                construct,
                kept,
                failure,
            },
        };
        self.push(b, inst);
        result
    }

    /// A cleanup anchor at a byte range.
    pub fn cleanup_anchor(&mut self, start: u32, end: u32) -> CleanupAnchorId {
        self.lir.tables.cleanup_anchors.push(CleanupAnchor {
            body: 0,
            cleanup: self.lir.tables.cleanup_anchors.len() as u32,
            action: 0,
            site: SiteRange { start, end },
        });
        CleanupAnchorId(self.lir.tables.cleanup_anchors.len() as u32 - 1)
    }

    /// A failure edge (starting `rule` at `anchor`, if given) into a cleanup block that
    /// releases the live `blocks` (value, payload layout) and propagates.
    pub fn releasing(
        &mut self,
        blocks: &[(ValueId, LayoutId)],
        rule: Option<&str>,
        anchor: SourceAnchorId,
    ) -> FailureId {
        let cleanup = self.block(BlockKind::FailureCleanup, &[]);
        self.release_all(cleanup, blocks);
        let p = self.propagate();
        self.jump(cleanup, p, &[]);
        self.failure_to(cleanup, rule, anchor)
    }

    /// Releases `blocks` in order; a failing release still releases the rest.
    pub fn release_all(&mut self, b: BlockId, blocks: &[(ValueId, LayoutId)]) {
        for (i, &(value, payload)) in blocks.iter().enumerate() {
            let rest = &blocks[i + 1..];
            let failure = if rest.is_empty() {
                self.fails(None)
            } else {
                self.releasing(rest, None, SourceAnchorId(0))
            };
            self.release_to(b, value, payload, failure);
        }
    }

    /// Releases a fully initialized block, failing to `failure`.
    pub fn release_to(
        &mut self,
        b: BlockId,
        value: ValueId,
        payload: LayoutId,
        failure: FailureId,
    ) {
        let anchor = self.cleanup_anchor(80, 90);
        self.push(
            b,
            Inst::Release {
                value,
                layout: payload,
                live: MaskRef::All,
                anchor,
                failure,
            },
        );
    }

    /// Releases a fully initialized block whose payload is `payload`.
    pub fn release(&mut self, b: BlockId, value: ValueId, payload: LayoutId) {
        let anchor = self.cleanup_anchor(80, 90);
        let failure = self.fails(None);
        self.push(
            b,
            Inst::Release {
                value,
                layout: payload,
                live: MaskRef::All,
                anchor,
                failure,
            },
        );
    }

    pub fn symbol(&mut self, kind: SymbolKind, name: &str) -> SymbolId {
        let symbol = Symbol {
            kind,
            definition: path(name),
            instance: Some(instance(name)),
        };
        if let Some(i) = self.lir.tables.symbols.iter().position(|s| *s == symbol) {
            return SymbolId(i as u32);
        }
        self.lir.tables.symbols.push(symbol);
        SymbolId(self.lir.tables.symbols.len() as u32 - 1)
    }

    /// Terminates `b` with a call; returns the result defined on the normal edge.
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &mut self,
        b: BlockId,
        callee: Callee,
        abi: &str,
        args: &[(Passing, ArgumentValue)],
        normal: BlockId,
        normal_args: &[ValueId],
        failure: FailureId,
    ) -> Option<ValueId> {
        let abi_id = self.table.abi(abi);
        let fn_abi = self.table.table.fn_abis[abi_id.0 as usize].clone();
        let arguments = PoolRange::push(
            &mut self.lir.pools.call_arguments,
            args.iter()
                .enumerate()
                .map(|(i, &(passing, value))| CallArgument {
                    parameter: i as u32,
                    passing,
                    value,
                }),
        );
        let result = fn_abi.result.as_ref().map(|r| self.plain(r.layout));
        self.lir.tables.calls.push(CallSite {
            operation_arguments: Vec::new(),
            callee,
            abi: abi_id,
            arguments,
            result,
            destination: fn_abi.destination.then_some(DestinationClass::Heap),
            declared: Effects::ALL,
            anchor: SourceAnchorId(0),
        });
        let call = CallId(self.lir.tables.calls.len() as u32 - 1);
        let normal = self.edge(normal, normal_args);
        self.term(
            b,
            Terminator::Call {
                call,
                normal: Some(normal),
                failure,
            },
        );
        result
    }

    /// Terminates `b` with a diverging call (LIR schema revision 19): its ABI
    /// has a Never result, so only the failure edge continues.
    pub fn diverging_call(&mut self, b: BlockId, callee: Callee, abi: &str, failure: FailureId) {
        let abi = self.table.abi(abi);
        self.lir.tables.calls.push(CallSite {
            operation_arguments: Vec::new(),
            callee,
            abi,
            arguments: PoolRange::EMPTY,
            result: None,
            destination: None,
            declared: Effects::ALL,
            anchor: SourceAnchorId(0),
        });
        let call = CallId(self.lir.tables.calls.len() as u32 - 1);
        self.term(
            b,
            Terminator::Call {
                call,
                normal: None,
                failure,
            },
        );
    }

    pub fn ret(&mut self, b: BlockId, value: Option<ValueId>) {
        self.term(b, Terminator::Return(value));
    }

    pub fn finish(mut self) -> LirUnit<Ready> {
        for (b, insts) in self.insts.iter().enumerate() {
            let range = PoolRange::push(&mut self.lir.instructions, insts.iter().copied());
            self.lir.blocks[b].instructions = range;
        }
        self.lir.terminators = self
            .terms
            .iter()
            .enumerate()
            .map(|(b, t)| {
                t.clone()
                    .unwrap_or_else(|| panic!("block {b} has no terminator"))
            })
            .collect();
        self.lir
    }
}

/// Seals a unit with the Ready validator; a rejection fails the test with its violations.
pub fn seal(table: &Table, manifest: &Manifest, unit: LirUnit<Ready>) -> ValidatedUnit<Ready> {
    let name = unit.header.definition.0.name.clone();
    validate::validate(
        unit,
        &Inputs {
            operations: &Default::default(),
            target: &table.target,
            layouts: &table.table,
            manifest,
        },
    )
    .unwrap_or_else(|rejected| panic!("fixture `{name}` is rejected: {:#?}", rejected.violations))
}

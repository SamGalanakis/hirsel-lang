//! Hand-built Ready programs for emission tests.
//!
//! Layouts and `FnAbi`s come from the `LayoutBuilder` over a small type store, so the
//! emitter sees the same physical facts real programs will have. Every unit is sealed by
//! the LIR validator (`validate::<Ready>`); a fixture the validator rejects fails its test.

use hirsel_lir::testing::UnitBuilder;
pub mod glue;
pub mod native;
pub mod programs;
pub mod ready;

use std::collections::BTreeMap;

use hirsel_core_contract::{
    Disposition, EngineDispositions, Manifest, OperationId, OperationRow, Passing, SemanticEffects,
};
use hirsel_layout::layout::{EnvironmentShape, LayoutBuilder, OwnerRepresentation, Owners};
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::*;

use hirsel_lir::emission::{FailureCode, FailureCodes};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind, TargetSpec};
use hirsel_lir::stage::{InputIdentity, Ready, StageKind, ValidatedUnit};
use hirsel_lir::validate::{self, Inputs};
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

/// The rules fixtures fail with and their runtime codes, which glue binds; every rule's
/// kind is `KIND_BUG`, which units state themselves.
pub fn failure_codes() -> FailureCodes {
    let mut codes = BTreeMap::new();
    for (rule, kind, code) in [
        ("SEM-1", 1, 101),
        ("MEM-3", 1, 103),
        ("MEM-13", 1, 113),
        ("MEM-14", 1, 114),
        ("LANG-9", 1, 9),
    ] {
        codes.insert(rule.to_owned(), FailureCode { kind, rule: code });
    }
    FailureCodes { codes }
}

pub use hirsel_types::testing::Types;

thread_local! {
    /// A test's session interner: every row a test builds, owner shapes
    /// included, lives in it, as in a compile session.
    static SESSION: std::sync::Arc<hirsel_types::TypeInterner> =
        hirsel_types::TypeInterner::new(None);
}

/// A type builder over this test thread's session interner.
pub fn session_types() -> Types {
    Types {
        store: SESSION.with(|s| s.clone()),
    }
}

/// A `@packed` nominal record `Wire { u8, u64, u16 }`.
pub fn wire_owner(owners: &mut Owners) -> DefPath {
    let mut t = session_types();
    let (a, b, c) = (t.p(Primitive::U8), t.p(Primitive::U64), t.p(Primitive::U16));
    let shape = t.record(&[a, b, c]);
    let definition = path("Wire");
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition.clone()),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("Wire revision"),
            generics: Vec::new(),
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
    /// `table`'s content keys, which name the glue symbols units import.
    pub keys: hirsel_lir::LayoutKeys,
    pub layouts: BTreeMap<&'static str, LayoutId>,
    pub abis: BTreeMap<&'static str, FnAbiId>,
    /// The block header layout: a reuse token's.
    pub header: LayoutId,
    pub structural: BTreeMap<LayoutId, hirsel_lir::structural_access::AccessPlan>,
    pub declarations: hirsel_layout::operation::OperationRegistry,
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

thread_local! {
    static DECOYS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Builds the fixtures `f` makes with unrelated layouts requested before theirs, so
/// every layout the program names has another `LayoutId` than without.
pub fn with_decoys<T>(f: impl FnOnce() -> T) -> T {
    DECOYS.with(|d| d.set(true));
    let result = f();
    DECOYS.with(|d| d.set(false));
    result
}

impl Schema {
    pub fn new() -> Self {
        let mut schema = Self {
            types: session_types(),
            owners: Owners::default(),
            named: Vec::new(),
            signatures: Vec::new(),
            environments: Vec::new(),
        };
        if DECOYS.with(|d| d.get()) {
            let t = &mut schema.types;
            let (u16_, f32_, u8_) = (t.p(Primitive::U16), t.p(Primitive::F32), t.p(Primitive::U8));
            // Records of scalar types only: a sequence or box brings its own count and
            // header layouts, which fixtures that find a scalar by kind would pick up.
            let record = t.record(&[u16_, f32_]);
            let other = t.record(&[u8_, u16_, u8_]);
            schema
                .named
                .extend([("decoy record", record), ("decoy other", other)]);
        }
        schema
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
        let ty = self
            .types
            .function(parameters, result, Multiplicity::Many, 0);
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
            let shape = EnvironmentShape {
                definition: hirsel_syntax::DefId::intern(&path(definition)),
                body: 0,
                captures,
                sizes: &[],
                movable,
            };
            layouts.insert(
                *name,
                builder
                    .environment(&self.types.store, &shape)
                    .expect("an environment"),
            );
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
            keys: hirsel_lir::LayoutKeys::new(&table),
            table,
            layouts,
            abis,
            header,
            structural: BTreeMap::new(),
            declarations: hirsel_layout::operation::OperationRegistry {
                seal_sources: Default::default(),
                receipt_sources: Default::default(),
                types: self.types.store.clone(),
                owners: self.owners.clone(),
                type_declarations: self
                    .owners
                    .nominals
                    .keys()
                    .filter_map(|&definition| {
                        self.declaration(definition).map(|row| (definition, row))
                    })
                    .collect(),
                signatures: BTreeMap::new(),
                invocations: BTreeMap::new(),
                invocation_bodies: Vec::new(),
                pretransfer_adapters: Vec::new(),
                pretransfer_actuals: Vec::new(),
            },
        }
    }

    fn declaration(&self, definition: hirsel_syntax::DefId) -> Option<SignatureRef> {
        let Some(OwnerRepresentation::Defined(record)) = self.owners.nominals.get(&definition)
        else {
            return None;
        };
        Some(std::sync::Arc::new(Signature {
            metadata: SignatureMetadata::default(),
            definition,
            fingerprint: record.representation_revision,
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                destructor_allocation: Allocation::MayAllocate,
                generics: record.generics.clone(),
                representation: Some(record.private_shape),
                bounds: vec![],
                authority: AuthoritySummary::Known(Default::default()),
                capability: None,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: record.layout_promises.clone(),
                distinct: false,
                resource: record.resource,
                drop: record
                    .finalizer
                    .as_ref()
                    .map(|(definition, _)| definition.clone()),
                fact: None,
                captures: None,
            }),
        }))
    }

    /// Logical operand declarations belong to the original component schema,
    /// independently of the instructions later built against its table.
    pub fn structural_plans(
        &self,
        table: &Table,
        names: &[&str],
    ) -> BTreeMap<LayoutId, hirsel_lir::structural_access::AccessPlan> {
        let mut lookup = |definition| {
            Ok(table
                .declarations
                .type_declarations
                .get(&definition)
                .cloned())
        };
        names
            .iter()
            .map(|name| {
                let ty = self
                    .named
                    .iter()
                    .find(|(key, _)| key == name)
                    .unwrap_or_else(|| panic!("structural fixture source {name}"))
                    .1;
                let plan = hirsel_layout::structural_access::derive(
                    &self.types.store,
                    ty,
                    &mut lookup,
                    &self.owners,
                    &table.target,
                    &table.table,
                )
                .unwrap_or_else(|e| panic!("structural fixture {name}: {e:?}"));
                assert_eq!(plan.root_layout(), table.layout(name));
                (plan.root_layout(), plan)
            })
            .collect()
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
        LayoutKind::Handle | LayoutKind::HostCapability { .. } => ValueRepr::Handle,
        LayoutKind::Path => ValueRepr::Scalar(ScalarKind::U64),
        _ => ValueRepr::Aggregate,
    }
}

/// One Ready unit under construction against a finished table.
pub struct Unit<'t> {
    pub table: &'t Table,
    pub builder: UnitBuilder<Ready>,
}

pub const ENTRY: BlockId = BlockId(0);

impl<'t> Unit<'t> {
    /// A unit named `name` with the signature `abi`, and its entry block.
    pub fn new(table: &'t Table, name: &str, abi: &str) -> Self {
        let mut lir = LirUnit::<Ready>::new(UnitHeader {
            source_count: 1,
            key: hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash([0; 32])),
            track_caller: false,
            stage: StageKind::Ready,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: hirsel_lir::ir::UnitOrigin::Source,
            definition: path(name),
            implementation_revision: revision(name),
            instance: instance(name),
            target: table.table.target,
            abi: table.abi(abi),
        });
        lir.tables.task_domains = 1;
        lir.scopes.push(ScopeInfo { parent: None });
        lir.tables.source_anchors.push(SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange {
                source: 0,
                start: 10,
                end: 20,
            },
        });
        let mut unit = Self {
            table,
            builder: UnitBuilder::new(lir),
        };
        unit.builder.block(BlockKind::Body, &[]);
        unit
    }

    /// A source anchor at a byte range; failures and sites name it.
    pub fn anchor(&mut self, start: u32, end: u32) -> SourceAnchorId {
        self.builder.unit.tables.source_anchors.push(SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange {
                source: 0,
                start,
                end,
            },
        });
        SourceAnchorId(self.builder.unit.tables.source_anchors.len() as u32 - 1)
    }

    pub fn ty(&mut self, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
        let ty = LirType { repr, layout };
        if let Some(i) = self.builder.unit.types.iter().position(|t| *t == ty) {
            return LirTypeId(i as u32);
        }
        self.builder.unit.types.push(ty);
        LirTypeId(self.builder.unit.types.len() as u32 - 1)
    }

    pub fn value(&mut self, layout: LayoutId, holding: Holding) -> ValueId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.builder.unit.values.push(ValueInfo { ty, holding });
        ValueId(self.builder.unit.values.len() as u32 - 1)
    }

    pub fn plain(&mut self, layout: LayoutId) -> ValueId {
        self.value(layout, Holding::Plain)
    }

    /// The value of the next declared parameter.
    pub fn param(&mut self, layout: LayoutId) -> ValueId {
        let v = self.plain(layout);
        self.builder.unit.parameters.push(v);
        v
    }

    pub fn slot(&mut self, layout: LayoutId, kind: SlotKind) -> SlotId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.builder.unit.slots.push(SlotInfo {
            ty,
            kind,
            scope: ScopeId(0),
        });
        SlotId(self.builder.unit.slots.len() as u32 - 1)
    }

    pub fn address(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        layout: LayoutId,
    ) -> AddressId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        let projection = PoolRange::push(
            &mut self.builder.unit.pools.projections,
            steps.iter().copied(),
        );
        self.builder.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base,
            projection,
            ty,
        });
        AddressId(self.builder.unit.addresses.len() as u32 - 1)
    }

    /// A failure edge into `target` that starts `rule` (None propagates a callee's).
    pub fn failure_to(
        &mut self,
        target: BlockId,
        rule: Option<&str>,
        anchor: SourceAnchorId,
    ) -> FailureId {
        let cleanup = self.builder.edge(target, &[]);
        self.builder.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: rule.map(str::to_owned),
            anchor,
            dispose: PoolRange::EMPTY,
        });
        FailureId(self.builder.unit.tables.failures.len() as u32 - 1)
    }

    /// A failure edge to plain propagation.
    pub fn fails(&mut self, rule: Option<&str>) -> FailureId {
        let p = self.builder.propagate();
        self.failure_to(p, rule, SourceAnchorId(0))
    }

    pub fn constant(&mut self, b: BlockId, layout: LayoutId, value: ConstantValue) -> ValueId {
        let ty = self.ty(repr_of(&self.table.table, layout), layout);
        self.builder
            .unit
            .tables
            .constants
            .push(ConstantData { ty, value });
        let constant = ConstantId(self.builder.unit.tables.constants.len() as u32 - 1);
        let result = self.plain(layout);
        self.builder.push(b, Inst::Const { result, constant });
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
            self.builder.unit.types[self.builder.unit.values[left.index()].ty.index()].layout
        };
        let result = self.plain(layout);
        let p = self.builder.propagate();
        let failure = self.failure_to(p, Some(rule), anchor);
        self.builder.push(
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
        let layout =
            self.builder.unit.types[self.builder.unit.values[operand.index()].ty.index()].layout;
        let result = self.plain(layout);
        let failure = self.fails(Some("SEM-1"));
        self.builder.push(
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
        let p = self.builder.propagate();
        let failure = self.failure_to(p, rule, anchor);
        self.builder.push(b, Inst::Address { address, failure });
    }

    pub fn read(&mut self, b: BlockId, address: AddressId, reaching: ValueId) -> ValueId {
        let layout =
            self.builder.unit.types[self.builder.unit.addresses[address.index()].ty.index()].layout;
        let result = self.plain(layout);
        self.builder.push(
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
        self.builder.push(
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
        let ty = self.builder.unit.slots[slot.index()].ty;
        self.builder.unit.values.push(ValueInfo {
            ty,
            holding: Holding::Plain,
        });
        ValueId(self.builder.unit.values.len() as u32 - 1)
    }

    pub fn construct(
        &mut self,
        b: BlockId,
        constructor: ConstructorKind,
        layout: LayoutId,
        fields: &[(u32, ValueId)],
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
            layout,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
            anchor: SourceAnchorId(0),
        });
        let construct = ConstructId(self.builder.unit.tables.constructs.len() as u32 - 1);
        let result = self.plain(layout);
        let failure = self.fails(None);
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
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
            anchor: SourceAnchorId(0),
        });
        let construct = ConstructId(self.builder.unit.tables.constructs.len() as u32 - 1);
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
        self.builder.push(b, inst);
        result
    }

    /// An explicit instruction-drop anchor for this hand-built unit.
    pub fn cleanup_anchor(&mut self, start: u32, end: u32) -> CleanupAnchorId {
        self.builder
            .unit
            .tables
            .cleanup_anchors
            .push(CleanupAnchor {
                body: 0,
                cleanup: INSTRUCTION_DROP,
                action: 0,
                site: SiteRange {
                    source: 0,
                    start,
                    end,
                },
            });
        CleanupAnchorId(self.builder.unit.tables.cleanup_anchors.len() as u32 - 1)
    }

    /// A failure edge (starting `rule` at `anchor`, if given) into a cleanup block that
    /// releases the live `blocks` (value, payload layout) and propagates.
    pub fn releasing(
        &mut self,
        blocks: &[(ValueId, LayoutId)],
        rule: Option<&str>,
        anchor: SourceAnchorId,
    ) -> FailureId {
        let cleanup = self.builder.block(BlockKind::FailureCleanup, &[]);
        self.release_all(cleanup, blocks);
        let p = self.builder.propagate();
        self.builder.jump(cleanup, p, &[]);
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
        self.builder.push(
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
        self.builder.push(
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
        if let Some(i) = self
            .builder
            .unit
            .tables
            .symbols
            .iter()
            .position(|s| *s == symbol)
        {
            return SymbolId(i as u32);
        }
        self.builder.unit.tables.symbols.push(symbol);
        SymbolId(self.builder.unit.tables.symbols.len() as u32 - 1)
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
            &mut self.builder.unit.pools.call_arguments,
            args.iter()
                .enumerate()
                .map(|(i, &(passing, value))| CallArgument {
                    parameter: i as u32,
                    passing,
                    value,
                }),
        );
        let result = fn_abi.result.as_ref().map(|r| self.plain(r.layout));
        self.builder.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee,
            abi: abi_id,
            arguments,
            result,
            destination: fn_abi.destination.then_some(DestinationClass::Heap),
            declared: Effects::ALL,
            anchor: SourceAnchorId(0),
        });
        let call = CallId(self.builder.unit.tables.calls.len() as u32 - 1);
        let normal = self.builder.edge(normal, normal_args);
        self.builder.term(
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
        self.builder.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee,
            abi,
            arguments: PoolRange::EMPTY,
            result: None,
            destination: None,
            declared: Effects::ALL,
            anchor: SourceAnchorId(0),
        });
        let call = CallId(self.builder.unit.tables.calls.len() as u32 - 1);
        self.builder.term(
            b,
            Terminator::Call {
                call,
                normal: None,
                failure,
            },
        );
    }

    pub fn finish(self) -> LirUnit<Ready> {
        self.builder.finish()
    }
}

/// Seals a unit with the Ready validator; a rejection fails the test with its violations.
pub fn seal(table: &Table, manifest: &Manifest, unit: LirUnit<Ready>) -> ValidatedUnit<Ready> {
    let name = unit.header.definition.0.name.clone();
    validate::validate(
        unit,
        &Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &table.declarations,
            target: &table.target,
            layouts: &table.table,
            manifest,
        },
    )
    .unwrap_or_else(|rejected| panic!("fixture `{name}` is rejected: {:#?}", rejected.violations))
}

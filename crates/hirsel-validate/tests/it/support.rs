//! Hand-built TIR fixtures. The crate's unit tests include this file by path
//! (`src/lib.rs`), so it names the crate as `hirsel_validate` in both builds.
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId, DefPath, ItemPath, PackageId, PassingMode};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::{SignatureMap, ViolationKind, assert_valid, validate};

thread_local! {
    static SESSION: (std::sync::Arc<TypeInterner>, &'static TypeInterner) = {
        let t = TypeInterner::new(None);
        // Each test runs on its own thread; its interner lives as long as the test.
        let leaked = unsafe { &*std::sync::Arc::into_raw(t.clone()) };
        (t, leaked)
    };
}
/// This test thread's session interner.
pub(crate) fn session() -> std::sync::Arc<TypeInterner> {
    SESSION.with(|s| s.0.clone())
}
/// This test thread's session interner, borrowed for the test's lifetime.
pub(crate) fn interner() -> &'static TypeInterner {
    SESSION.with(|s| s.1)
}
/// A definition's closure table as fixtures grow it: every row a fixture adds
/// joins the table with the rows it reaches, as rows once joined a store.
pub(crate) trait Table {
    /// Interns `row` and adds it (with its closure) to the table.
    fn push(&mut self, row: Type) -> TypeId;
    /// Adds an interned row and its closure to the table.
    fn adopt(&mut self, root: TypeId) -> TypeId;
    /// The `index`th row of the table.
    fn id(&self, index: usize) -> Option<TypeId>;
}
impl Table for std::sync::Arc<[TypeId]> {
    fn push(&mut self, row: Type) -> TypeId {
        let id = interner().intern(row);
        self.adopt(id)
    }
    fn adopt(&mut self, root: TypeId) -> TypeId {
        let mut rows = self.to_vec();
        for &id in closure(interner(), [root]).iter() {
            if !rows.contains(&id) {
                rows.push(id);
            }
        }
        *self = rows.into();
        root
    }
    fn id(&self, index: usize) -> Option<TypeId> {
        self.get(index).copied()
    }
}
/// Rows a fixture interns outside any definition's table (signature rows).
pub(crate) trait Rows {
    fn push(&self, row: Type) -> TypeId;
}
impl Rows for TypeInterner {
    fn push(&self, row: Type) -> TypeId {
        self.intern(row)
    }
}
/// `definition` with the closure table lowering computes for it
/// (`hirsel_tir::definition_types`) after the rows the fixture listed itself.
pub(crate) fn tabled(definition: &Definition) -> Definition {
    let mut definition = definition.clone();
    let computed = hirsel_tir::definition_types(interner(), &definition);
    let mut rows = definition.types.to_vec();
    let listed: std::collections::HashSet<_> = rows.iter().copied().collect();
    rows.extend(computed.iter().copied().filter(|id| !listed.contains(id)));
    definition.types = rows.into();
    definition
}
fn kinds_of(definition: &Definition, signatures: &SignatureMap) -> Vec<ViolationKind> {
    validate(definition, interner(), signatures, &mut vec![])
        .violations
        .into_iter()
        .map(|v| v.kind)
        .collect()
}
/// The session id of a fixture item.
pub(crate) fn id(name: &str) -> DefId {
    DefId::intern(&path(name))
}
pub(crate) fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "validate".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
/// The lexical owner of the fixture definition's sites.
pub(crate) fn lexical() -> hirsel_syntax::LexicalOwner {
    hirsel_syntax::LexicalOwner::first(&path("main"))
}
/// A site of the fixture definition, qualified by its owner.
pub(crate) fn scoped(site: NameSite) -> SiteRef {
    lexical().site(site)
}
pub(crate) fn ty(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
pub(crate) fn operand(place: u32, usage: UseKind) -> Operand {
    Operand {
        place: PlaceId(place),
        usage,
    }
}
pub(crate) fn take(place: u32) -> Operand {
    operand(place, UseKind::Take)
}
pub(crate) fn copy(place: u32) -> Operand {
    operand(place, UseKind::Copy)
}
pub(crate) fn drop(place: u32) -> CleanupAction {
    CleanupAction::Drop {
        place: PlaceId(place),
        if_live: true,
    }
}
pub(crate) fn inst(kind: InstructionKind, cleanup: u32) -> Instruction {
    Instruction {
        kind,
        span: Span::default(),
        on_failure: CleanupId(cleanup),
    }
}
pub(crate) fn block(instructions: Vec<Instruction>, terminator: Terminator) -> Block {
    Block {
        instructions,
        terminator,
        span: Span::default(),
    }
}
pub(crate) fn ret(place: u32, cleanup: u32) -> Terminator {
    Terminator::Return {
        value: take(place),
        cleanup: CleanupId(cleanup),
    }
}

pub(crate) struct Fixture {
    pub(crate) definition: Definition,
    pub(crate) signatures: SignatureMap,
}
impl Fixture {
    pub(crate) fn unit() -> Self {
        let mut types: std::sync::Arc<[TypeId]> = std::sync::Arc::new([]);
        let unit = types.push(ty(TypeKind::Tuple(vec![].into())));
        Self {
            definition: Definition {
                caller_bodies: vec![],
                protocol_associations: vec![],
                types,
                path: id("main"),
                inferred_unique: Default::default(),
                signature_hash: ContentHash::default(),
                entry: BodyId(0),
                contracts: vec![],
                bodies: vec![Body {
                    arena_scope_grant: None,
                    scoped_parameters: Vec::new(),
                    parameter_scope: scoped(hirsel_types::NameSite::Item),
                    locals: vec![Local {
                        ty: unit,
                        storage: Storage::ReturnSlot,
                        scope: ScopeId(0),
                        span: Span::default(),
                    }],
                    places: vec![Place {
                        loan: None,
                        binding: None,
                        root: LocalId(0),
                        projection: vec![].into(),
                        ty: unit,
                    }],
                    scopes: vec![Scope {
                        parent: None,
                        span: Span::default(),
                    }],
                    blocks: vec![block(
                        vec![inst(
                            InstructionKind::Assign {
                                destination: PlaceId(0),
                                value: RvalueId(0),
                            },
                            0,
                        )],
                        ret(0, 0),
                    )],
                    rvalues: vec![Rvalue::Constant(Constant::Unit)],
                    cleanup_nodes: vec![],
                    cleanups: vec![Cleanup::default()],
                    entry: BlockId(0),
                    parameters: vec![],
                    result: unit,
                    borrow_cursors: vec![],
                    tail_sites: vec![],
                    matches: vec![],
                }],
            },
            signatures: SignatureMap::new(session()),
        }
    }
    pub(crate) fn body(&mut self) -> &mut Body {
        &mut self.definition.bodies[0]
    }
    pub(crate) fn slot(&mut self, kind: TypeKind, storage: Storage) -> u32 {
        let body = &mut self.definition.bodies[0];
        let t = self.definition.types.push(ty(kind));
        let t = map_generic_binders(interner(), t, |binder, nested| {
            Ok(match binder {
                GenericBinder::Bound(depth) if depth == nested => {
                    GenericBinder::Body(body.parameter_scope)
                }
                b => b,
            })
        })
        .unwrap();
        self.definition.types.adopt(t);
        let id = body.locals.len() as u32;
        body.locals.push(Local {
            ty: t,
            storage,
            scope: ScopeId(0),
            span: Span::default(),
        });
        body.places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(id),
            projection: vec![].into(),
            ty: t,
        });
        body.places.len() as u32 - 1
    }
    pub(crate) fn assign(&mut self, p: u32, r: Rvalue, cleanup: u32) {
        let body = self.body();
        let id = RvalueId(body.rvalues.len() as u32);
        body.rvalues.push(r);
        body.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(p),
                value: id,
            },
            cleanup,
        ));
    }
    pub(crate) fn emit(&mut self, kind: InstructionKind) {
        self.body().blocks[0].instructions.push(inst(kind, 0));
    }
    pub(crate) fn cleanup(&mut self, actions: Vec<CleanupAction>) -> u32 {
        let id = self.body().cleanups.len() as u32;
        self.body().add_cleanup(actions);
        id
    }
    pub(crate) fn valid(&self) {
        assert_valid(&tabled(&self.definition), interner(), &self.signatures);
    }
    pub(crate) fn kinds(&self) -> Vec<ViolationKind> {
        kinds_of(&tabled(&self.definition), &self.signatures)
    }
    pub(crate) fn broken(&self, expected: Vec<ViolationKind>) {
        assert_eq!(self.kinds(), expected);
    }
    /// As `broken`, validating the closure table exactly as the fixture built it.
    pub(crate) fn broken_as_built(&self, expected: Vec<ViolationKind>) {
        assert_eq!(kinds_of(&self.definition, &self.signatures), expected);
    }
    pub(crate) fn string() -> Self {
        let mut f = Self::unit();
        let p = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
        f.assign(p, Rvalue::Constant(Constant::String("live".into())), 0);
        let c = f.cleanup(vec![drop(p)]);
        f.body().blocks[0].terminator = ret(0, c);
        f
    }
    pub(crate) fn through_loan(&mut self, source: u32, loan: u32) -> u32 {
        let mut place = self.body().places[source as usize].clone();
        place.loan = Some(LoanId(loan));
        let id = self.body().places.len() as u32;
        self.body().places.push(place);
        id
    }
    pub(crate) fn loan() -> Self {
        let mut f = Self::string();
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(1),
            mutable: false,
            if_inactive: false,
        });
        f.emit(InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        });
        f
    }
    pub(crate) fn call() -> Self {
        let mut f = Self::unit();
        let arg = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        f.assign(arg, Rvalue::Constant(Constant::String("arg".into())), 0);
        let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(arg),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(result, 0)));
        let signature = function_signature(
            "callee",
            vec![(PassingMode::Take, TypeKind::Primitive(Primitive::String))],
            TypeKind::Tuple(vec![].into()),
        );
        f.signatures.insert(signature.definition.clone(), signature);
        f
    }
    /// `fn main() -> R { callee() }` for `fn callee() -> R`, lowered as a
    /// returning call into a result temporary or as a diverging call.
    pub(crate) fn result_call(result: TypeKind, returning: bool) -> Self {
        let mut f = Self::unit();
        let r = f.definition.types.push(ty(result.clone()));
        let body = f.body();
        body.result = r;
        body.locals[0].ty = r;
        body.places[0].ty = r;
        body.blocks[0].instructions.clear();
        body.rvalues.clear();
        let returns = returning.then(|| {
            let slot = f.slot(result.clone(), Storage::Temporary);
            f.body().blocks.push(block(vec![], ret(slot, 0)));
            CallReturn {
                destination: PlaceId(slot),
                next: BlockId(1),
            }
        });
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![],
            },
            arguments: vec![],
            returns,
            on_failure: CleanupId(0),
        };
        let signature = function_signature("callee", vec![], result);
        f.signatures.insert(signature.definition.clone(), signature);
        f
    }
}
pub(crate) fn function_signature(
    name: &str,
    params: Vec<(PassingMode, TypeKind)>,
    result: TypeKind,
) -> Signature {
    let types = interner();
    let result = types.intern(ty(result));
    let parameters = params
        .into_iter()
        .map(|(passing, kind)| Parameter {
            name: None,
            label: None,
            passing,
            ty: types.intern(ty(kind)),
        })
        .collect();
    Signature {
        metadata: SignatureMetadata::default(),
        definition: id(name),
        kind: SignatureKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters,
            result,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::inert(),
            uses: Uses::Only(AuthoritySummary::inert()),
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
        fingerprint: ContentHash::default(),
    }
}

pub(crate) fn owner_signature(f: &mut Fixture, passing: PassingMode) {
    let b = &f.definition.bodies[0];
    let mut s = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    if let SignatureKind::Function(fun) = &mut s.kind {
        let close = |root| {
            map_generic_binders(interner(), root, |binder, nested| {
                Ok(match binder {
                    GenericBinder::Body(scope) if scope == b.parameter_scope => {
                        GenericBinder::Bound(nested)
                    }
                    b => b,
                })
            })
            .unwrap()
        };
        fun.result = close(b.result);
        fun.parameters = b
            .parameters
            .iter()
            .map(|l| Parameter {
                name: None,
                label: None,
                passing,
                ty: close(b.locals[l.0 as usize].ty),
            })
            .collect();
    }
    if let SignatureKind::Function(fun) = &s.kind {
        f.definition.bodies[0].scoped_parameters = fun
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.passing != PassingMode::Take
                    && matches!(
                        interner().get(p.ty).map(|t| &t.kind),
                        Some(TypeKind::Function(_))
                    )
            })
            .map(|(i, _)| ScopedParameter {
                parameter: i as u32,
                fields: vec![],
            })
            .collect();
    }
    f.signatures.insert(id("main"), s);
}

/// Registers the registry Arena and adds a slot of that type.
pub(crate) fn arena_slot(f: &mut Fixture, storage: Storage) -> u32 {
    let roles = NominalRoles {
        heap: Some(HeapRole {
            arena: path("Arena"),
            operation: path("heap"),
        }),
        ..Default::default()
    };
    let mut arena = function_signature("Arena", vec![], TypeKind::Tuple(vec![].into()));
    arena.metadata.roles = roles.into();
    arena.kind = SignatureKind::Type(TypeDefinition {
        loop_source: None,
        format: None,
        generics: vec![],
        representation: None,
        bounds: vec![],
        authority: AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Arena"))].into()),
        capability: Some(CapabilityKind::Scoped),
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::NoAlloc,
    });
    f.signatures.insert(id("Arena"), arena);
    f.slot(
        TypeKind::Named {
            definition: id("Arena"),
            arguments: vec![].into(),
        },
        storage,
    )
}

pub(crate) fn param(f: &mut Fixture, kind: TypeKind) -> u32 {
    let p = f.slot(kind, Storage::Parameter);
    let l = f.body().places[p as usize].root;
    f.body().parameters.push(l);
    p
}

pub(crate) fn owner_sig(f: &mut Fixture, passing: &[PassingMode]) {
    owner_signature(f, PassingMode::Borrow);
    if let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind {
        for (p, m) in fun.parameters.iter_mut().zip(passing) {
            p.passing = *m;
        }
        f.definition.bodies[0].scoped_parameters = fun
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.passing != PassingMode::Take
                    && matches!(
                        interner().get(p.ty).map(|t| &t.kind),
                        Some(TypeKind::Function(_))
                    )
            })
            .map(|(i, _)| ScopedParameter {
                parameter: i as u32,
                fields: vec![],
            })
            .collect();
    }
}

pub(crate) fn capability_signature(name: &str) -> Signature {
    let mut s = function_signature(name, vec![], TypeKind::Tuple(vec![].into()));
    s.kind = SignatureKind::Type(TypeDefinition {
        loop_source: None,
        format: None,
        destructor_allocation: Allocation::NoAlloc,
        generics: vec![],
        representation: None,
        bounds: vec![Bound::Plain, Bound::NoallocDrop],
        authority: AuthoritySummary::Known(vec![AuthorityTerm::Capability(path(name))].into()),
        capability: Some(CapabilityKind::Global),
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
    });
    s
}

pub(crate) fn captured_generic_copy(outer_plain: bool, inner_plain: bool, alias: bool) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let x = param(
        &mut f,
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
    );
    let p = f.body().places[x as usize].ty;
    let mut fun = fn_type(vec![], unit);
    fun.generics = vec![GenericParameter::Type {
        name: "U".into(),
        bounds: if inner_plain {
            vec![Bound::Plain].into()
        } else {
            vec![].into()
        },
    }]
    .into();
    // Plain captures can carry a global capability; authority stays conditional.
    fun.authority = AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(TypeParameterRef::body(
            f.body().parameter_scope,
            TypeParamId(0),
        ))]
        .into(),
    );
    fun.uses = Uses::Unrestricted;
    let c = f.slot(TypeKind::Function(fun), Storage::Local);
    let ct = f.body().places[c as usize].ty;
    let mut row = interner().row(ct).clone();
    row.modes.relation = Relation::environment();
    let ct = f.definition.types.push(row);
    f.body().places[c as usize].ty = ct;
    let root = f.body().places[c as usize].root;
    f.body().locals[root.0 as usize].ty = ct;
    let mut nested = empty_body(&mut f);
    let capture = add_capture(&mut nested, p);
    let mut read = capture;
    if alias {
        read = add_capture(&mut nested, p);
        nested.locals[read as usize].storage = Storage::Local;
        let rv = RvalueId(nested.rvalues.len() as u32);
        nested
            .rvalues
            .push(Rvalue::Use(operand(capture, UseKind::Borrow)));
        nested.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(read),
                value: rv,
            },
            0,
        ));
    }
    let copied = add_capture(&mut nested, p);
    nested.locals[copied as usize].storage = Storage::Temporary;
    let rv = RvalueId(nested.rvalues.len() as u32);
    nested.rvalues.push(Rvalue::Use(copy(read)));
    nested.blocks[0].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(copied),
            value: rv,
        },
        0,
    ));
    edit_cleanup(&mut nested, 0, |actions| {
        *actions = vec![drop(copied)];
    });
    f.definition.bodies.push(nested);
    f.assign(
        c,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(x, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(c)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(f.definition.path).unwrap().kind
    else {
        panic!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: if outer_plain {
            vec![Bound::Plain].into()
        } else {
            vec![].into()
        },
    }]
    .into();
    f
}

pub(crate) fn fn_type(parameters: Vec<Parameter>, result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters.into(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}

pub(crate) fn unit_ty(f: &mut Fixture) -> TypeId {
    f.body().locals[0].ty
}

pub(crate) fn empty_body(f: &mut Fixture) -> Body {
    // Closure type IDs refer to the definition store.
    let unit = f.body().locals[0].ty;
    Body {
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        parameter_scope: scoped(hirsel_types::NameSite::Statement(hirsel_syntax::StmtId(
            f.definition.bodies.len() as u32,
        ))),
        locals: vec![Local {
            ty: unit,
            storage: Storage::ReturnSlot,
            scope: ScopeId(0),
            span: Span::default(),
        }],
        places: vec![Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: vec![].into(),
            ty: unit,
        }],
        scopes: vec![Scope {
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![block(
            vec![inst(
                InstructionKind::Assign {
                    destination: PlaceId(0),
                    value: RvalueId(0),
                },
                0,
            )],
            ret(0, 0),
        )],
        rvalues: vec![Rvalue::Constant(Constant::Unit)],
        cleanup_nodes: vec![],
        cleanups: vec![Cleanup::default()],
        entry: BlockId(0),
        parameters: vec![],
        result: unit,
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
    }
}

pub(crate) fn add_capture(body: &mut Body, ty: TypeId) -> u32 {
    let l = body.locals.len() as u32;
    body.locals.push(Local {
        ty,
        storage: Storage::Capture,
        scope: ScopeId(0),
        span: Span::default(),
    });
    body.places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(l),
        projection: vec![].into(),
        ty,
    });
    body.places.len() as u32 - 1
}

pub(crate) fn edit_cleanup(body: &mut Body, id: u32, edit: impl FnOnce(&mut Vec<CleanupAction>)) {
    let mut actions = body.cleanup_actions(CleanupId(id)).cloned().collect();
    edit(&mut actions);
    body.set_cleanup(CleanupId(id), actions);
    // Fixture mutation removes unreachable old nodes, just as replacing an arena
    // row used to remove its old actions. Production chains are immutable.
    let mut used = vec![false; body.cleanup_nodes.len()];
    for cleanup in &body.cleanups {
        if let Some(head) = cleanup.head {
            used[head.0 as usize] = true;
        }
    }
    for i in (0..used.len()).rev() {
        if used[i] {
            if let Some(parent) = body.cleanup_nodes[i].parent {
                used[parent.0 as usize] = true;
            }
        }
    }
    let mut remap = vec![None; used.len()];
    let old = std::mem::take(&mut body.cleanup_nodes);
    for (i, mut node) in old.into_iter().enumerate() {
        if used[i] {
            node.parent = node.parent.and_then(|p| remap[p.0 as usize]);
            remap[i] = Some(CleanupNodeId(body.cleanup_nodes.len() as u32));
            body.cleanup_nodes.push(node);
        }
    }
    for cleanup in &mut body.cleanups {
        cleanup.head = cleanup.head.and_then(|p| remap[p.0 as usize]);
    }
}

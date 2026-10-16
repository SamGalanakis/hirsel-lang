#![allow(dead_code)]
use hirsel_interp::{Execution, Limits, Value, run};
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::{BinaryOp, ContentHash, DefPath, ItemPath, PackageId, UnaryOp};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;

pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
pub fn site(line: u32) -> Span {
    Span {
        file: FileId(0),
        start: line - 1,
        end: line - 1,
    }
}
pub fn sources() -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("fixture.hir", "\n".repeat(100)).unwrap();
    sources
}
pub fn ty(kind: TypeKind) -> Type {
    Type {
        drop_allocation: DropAllocation::MayAllocate,
        kind,
        modes: Modes::global(),
        sendability_requirement: hirsel_types::SendabilityRequirement::None,
        allocation_target: None,
        erased_captures: None,
        brand: Brand::default(),
    }
}
pub struct Builder {
    pub types: TypeStore,
    pub body: Body,
    pub instructions: Vec<Instruction>,
    caller_owned: Vec<LocalId>,
}
impl Builder {
    pub fn new() -> Self {
        let mut types = TypeStore::default();
        let result = types.push(ty(TypeKind::Tuple(vec![].into())));
        Self {
            types,
            body: Body {
                parameter_scope: hirsel_types::NameSite::Item(hirsel_syntax::ItemId(0)),
                locals: vec![],
                places: vec![],
                scopes: vec![Scope {
                    parent: None,
                    span: site(1),
                }],
                blocks: vec![],
                rvalues: vec![],
                cleanup_nodes: vec![],
                cleanups: vec![Cleanup::default()],
                entry: BlockId(0),
                parameters: vec![],
                result,
                tail_sites: vec![],
                matches: vec![],
            },
            instructions: vec![],
            caller_owned: vec![],
        }
    }
    pub fn type_id(&mut self, kind: TypeKind) -> TypeId {
        self.types.push(ty(kind))
    }
    pub fn local(&mut self, kind: TypeKind, storage: Storage) -> PlaceId {
        let ty = self.type_id(kind);
        self.slot(ty, storage)
    }
    pub fn slot(&mut self, ty: TypeId, storage: Storage) -> PlaceId {
        let root = LocalId(self.body.locals.len() as u32);
        self.body.locals.push(Local {
            ty,
            storage,
            scope: ScopeId(0),
            span: site(1),
        });
        let place = PlaceId(self.body.places.len() as u32);
        self.body.places.push(Place {
            loan: None,
            binding: None,
            root,
            projection: vec![].into(),
            ty,
        });
        place
    }
    pub fn through_loan(&mut self, source: PlaceId, loan: LoanId) -> PlaceId {
        let mut place = self.body.places[source.0 as usize].clone();
        place.loan = Some(loan);
        let id = PlaceId(self.body.places.len() as u32);
        self.body.places.push(place);
        id
    }
    pub fn parameter(&mut self, kind: TypeKind) -> PlaceId {
        let p = self.local(kind, Storage::Parameter);
        self.body
            .parameters
            .push(self.body.places[p.0 as usize].root);
        p
    }
    pub fn borrowed_parameter(&mut self, kind: TypeKind) -> PlaceId {
        let p = self.parameter(kind);
        self.caller_owned.push(self.body.places[p.0 as usize].root);
        p
    }
    pub fn project(&mut self, p: PlaceId, projection: Projection, ty: TypeId) -> PlaceId {
        let mut place = self.body.places[p.0 as usize].clone();
        place.projection.push(projection);
        place.ty = ty;
        let id = PlaceId(self.body.places.len() as u32);
        self.body.places.push(place);
        id
    }
    pub fn emit(&mut self, kind: InstructionKind, line: u32, cleanup: CleanupId) {
        self.instructions.push(Instruction {
            kind,
            span: site(line),
            on_failure: cleanup,
        });
    }
    pub fn assign(&mut self, p: PlaceId, value: Rvalue, line: u32, cleanup: CleanupId) {
        let id = RvalueId(self.body.rvalues.len() as u32);
        self.body.rvalues.push(value);
        self.emit(
            InstructionKind::Assign {
                destination: p,
                value: id,
            },
            line,
            cleanup,
        );
    }
    pub fn replace(&mut self, destination: PlaceId, value: Operand, line: u32) {
        let displaced = self.slot(
            self.body.places[destination.0 as usize].ty,
            Storage::Temporary,
        );
        self.emit(
            InstructionKind::Replace {
                destination,
                value,
                displaced,
                if_live: false,
            },
            line,
            CleanupId(0),
        );
        self.emit(
            InstructionKind::Drop {
                place: displaced,
                if_live: false,
            },
            line,
            CleanupId(0),
        );
    }
    pub fn constant(&mut self, kind: TypeKind, c: Constant) -> PlaceId {
        let p = self.local(kind, Storage::Temporary);
        self.assign(p, Rvalue::Constant(c), 1, CleanupId(0));
        p
    }
    pub fn int(&mut self, k: Primitive, v: u128) -> PlaceId {
        self.constant(TypeKind::Primitive(k), Constant::Integer(v))
    }
    pub fn unit(&mut self) -> PlaceId {
        self.constant(TypeKind::Tuple(vec![].into()), Constant::Unit)
    }
    pub fn binary(
        &mut self,
        op: BinaryOp,
        a: PlaceId,
        b: PlaceId,
        kind: TypeKind,
        line: u32,
        c: CleanupId,
    ) -> PlaceId {
        let p = self.local(kind, Storage::Temporary);
        self.assign(
            p,
            Rvalue::Binary {
                op,
                left: copy(a),
                right: copy(b),
            },
            line,
            c,
        );
        p
    }
    pub fn unary(
        &mut self,
        op: UnaryOp,
        a: PlaceId,
        kind: TypeKind,
        line: u32,
        c: CleanupId,
    ) -> PlaceId {
        let p = self.local(kind, Storage::Temporary);
        self.assign(p, Rvalue::Unary { op, value: copy(a) }, line, c);
        p
    }
    pub fn cleanup(&mut self, actions: Vec<CleanupAction>) -> CleanupId {
        let id = CleanupId(self.body.cleanups.len() as u32);
        self.body.add_cleanup(actions);
        id
    }
    pub fn block(&mut self, terminator: Terminator, line: u32) -> BlockId {
        if let Terminator::Return { value, .. } = &terminator {
            if !self
                .body
                .blocks
                .iter()
                .any(|b| matches!(b.terminator, Terminator::Return { .. }))
            {
                self.body.result = self.body.places[value.place.0 as usize].ty;
            }
        }
        let id = BlockId(self.body.blocks.len() as u32);
        self.body.blocks.push(Block {
            instructions: std::mem::take(&mut self.instructions),
            terminator,
            span: site(line),
        });
        id
    }
    pub fn finish(mut self, name: &str, p: PlaceId) -> Definition {
        self.body.result = self.body.places[p.0 as usize].ty;
        self.block(
            Terminator::Return {
                value: take(p),
                cleanup: CleanupId(0),
            },
            2,
        );
        self.definition(name)
    }
    pub fn definition(mut self, name: &str) -> Definition {
        // This hand-lowering builder emits explicit conditional releases. The
        // interpreter still only executes lists; it never infers scope cleanup.
        let counted = |id| fixture_needs_release(&self.types, id);
        let mut roots = self
            .body
            .places
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.loan.is_none()
                    && p.projection.is_empty()
                    && counted(p.ty)
                    && !self.caller_owned.contains(&p.root)
            })
            .collect::<Vec<_>>();
        roots.sort_by_key(|(_, p)| {
            (
                matches!(
                    self.body.locals[p.root.0 as usize].storage,
                    Storage::Temporary | Storage::ReturnSlot
                ),
                p.root.0,
            )
        });
        if self.body.cleanups.first().is_some_and(|c| c.head.is_none()) {
            self.body.set_cleanup(
                CleanupId(0),
                roots
                    .into_iter()
                    .rev()
                    .map(|(i, _)| drop(PlaceId(i as u32)))
                    .collect(),
            );
        }
        Definition {
            protocol_associations: vec![],
            types: self.types,
            path: path(name),
            signature_hash: ContentHash::default(),
            bodies: vec![self.body],
            entry: BodyId(0),
            contracts: vec![],
        }
    }
}
pub fn copy(place: PlaceId) -> Operand {
    Operand {
        place,
        usage: UseKind::Copy,
    }
}
pub fn take(place: PlaceId) -> Operand {
    Operand {
        place,
        usage: UseKind::Take,
    }
}
pub fn drop(place: PlaceId) -> CleanupAction {
    CleanupAction::Drop {
        place,
        if_live: true,
    }
}
pub struct Provider {
    pub definitions: Vec<Definition>,
    pub queries: Vec<DefPath>,
}
impl TirProvider for Provider {
    fn constant(
        &mut self,
        _path: &hirsel_syntax::DefPath,
    ) -> Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        Err(hirsel_types::QueryError::NotImplemented)
    }
    fn constant_definitions(
        &mut self,
    ) -> Result<Vec<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        Ok(vec![])
    }

    fn definition(&mut self, p: &DefPath) -> Result<DefinitionRef, QueryError> {
        (|| {
            self.queries.push(p.clone());
            self.definitions
                .iter()
                .find(|d| d.path == *p)
                .cloned()
                .ok_or(QueryError::NotImplemented)
        })()
        .map(std::sync::Arc::new)
    }
}
pub fn execute(definitions: Vec<Definition>, arguments: Vec<Value>, limits: Limits) -> Execution {
    assert_fixtures(
        &definitions,
        &hirsel_interp::aggregate::RuntimeTypes::default(),
    );
    execute_unvalidated(definitions, arguments, limits)
}
pub fn execute_unvalidated(
    definitions: Vec<Definition>,
    arguments: Vec<Value>,
    limits: Limits,
) -> Execution {
    let configuration = RunConfiguration {
        entry: definitions[0].path.clone(),
        hash_seed: 42,
        inputs: vec![],
    };
    run(
        &mut Provider {
            definitions,
            queries: vec![],
        },
        &sources(),
        &configuration,
        arguments,
        limits,
    )
    .unwrap()
}

/// A concrete function type for hand-lowered closure fixtures.
pub fn function_type(result: TypeId, parameters: Vec<TypeId>) -> TypeKind {
    TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters
            .into_iter()
            .map(|ty| Parameter {
                name: None,
                label: None,
                passing: hirsel_syntax::PassingMode::Borrow,
                ty,
            })
            .collect(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::Known(vec![].into()),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })
}

fn fixture_needs_release(store: &TypeStore, id: TypeId) -> bool {
    match &store.get(id).unwrap().kind {
        TypeKind::Primitive(Primitive::String | Primitive::Bytes) => true,
        TypeKind::Primitive(_) => false,
        TypeKind::Tuple(ts) => ts.iter().any(|t| fixture_needs_release(store, *t)),
        TypeKind::Record(fs) => fs.iter().any(|f| fixture_needs_release(store, f.ty)),
        TypeKind::Variant(vs) => vs
            .iter()
            .any(|v| v.fields.iter().any(|f| fixture_needs_release(store, f.1))),
        _ => true,
    }
}

mod signatures;
pub use signatures::fixture_signatures;

pub fn assert_fixtures(
    definitions: &[Definition],
    runtime: &hirsel_interp::aggregate::RuntimeTypes,
) {
    let signatures = fixture_signatures(definitions, runtime);
    for definition in definitions {
        hirsel_validate::assert_valid(definition, &signatures);
    }
}

/// Import an independently constructed fixture body into its definition graph.
pub fn append_body(target: &mut Definition, source: &Definition) {
    assert_eq!(source.bodies.len(), 1);
    let mut body = source.bodies[0].clone();
    let mut mapped = std::collections::BTreeMap::new();
    let mut import = |id: &mut TypeId| {
        *id = *mapped.entry(*id).or_insert_with(|| {
            hirsel_types::import_type(&source.types, *id, &mut target.types).unwrap()
        });
    };
    for local in &mut body.locals {
        import(&mut local.ty);
    }
    for place in &mut body.places {
        import(&mut place.ty);
    }
    import(&mut body.result);
    for value in &mut body.rvalues {
        if let Rvalue::Shape(ty) = value {
            import(ty);
        }
    }
    for block in &mut body.blocks {
        if let Terminator::Call { callee, .. } = &mut block.terminator {
            let (Callee::Definition { arguments, .. } | Callee::Indirect { arguments, .. }) =
                callee;
            for argument in arguments {
                if let GenericArgument::Type(ty) = argument {
                    import(ty);
                }
            }
        }
    }
    target.bodies.push(body);
}

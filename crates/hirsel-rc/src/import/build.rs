use super::*;
use crate::PassError;

impl Importer<'_> {
    /// TIR reachability, predecessor counts and reverse postorder. A normalized
    /// self-tail site's back edge targets the entry block.
    pub(super) fn cfg(&mut self) -> R<()> {
        let n = self.body.blocks.len();
        let successors = |b: tir::BlockId| -> Vec<tir::BlockId> {
            if let Some(plan) = self.plan
                && plan.site(b).is_some()
            {
                return vec![self.body.entry];
            }
            self.body
                .successors(&self.body.blocks[b.0 as usize].terminator)
        };
        let adjacent: Vec<Vec<usize>> = (0..n)
            .map(|b| {
                successors(tir::BlockId(b as u32))
                    .iter()
                    .map(|s| s.0 as usize)
                    .collect()
            })
            .collect();
        if adjacent.iter().flatten().any(|&b| b >= n) {
            return malformed("successor outside the body");
        }
        self.rpo = hirsel_graph::reverse_postorder(&adjacent, self.body.entry.0 as usize)
            .into_iter()
            .map(|b| tir::BlockId(b as u32))
            .collect();
        self.tir_preds = vec![0; n];
        for &b in &self.rpo {
            for s in successors(b) {
                self.tir_preds[s.0 as usize] += 1;
            }
        }
        Ok(())
    }

    // --- types and values ---

    /// A TIR type substituted by the instance's actuals.
    pub(super) fn ty(&mut self, ty: TypeId) -> R<TypeId> {
        if let Some(&t) = self.type_memo.get(&ty) {
            return Ok(t);
        }
        let t = substitute(self.types, &self.program.memo, ty, self.actuals, false)
            .map_err(|e| InstanceError::Substitution(self.def.path.path().clone(), e))?;
        self.type_memo.insert(ty, t);
        Ok(t)
    }

    pub(super) fn structural_access(&mut self, source: TypeId, layout: LayoutId) -> R<()> {
        if self
            .out
            .tables
            .structural
            .iter()
            .any(|p| p.root_layout() == layout)
        {
            return Ok(());
        }
        let source = self.ty(source)?;
        let plan = hirsel_layout::structural_access::at_layout_with_owners(
            self.types,
            source,
            &mut |d| Ok(self.program.signatures.get(d.path()).cloned()),
            self.layouts.owners(),
            self.layouts.table(),
            layout,
        )
        .map_err(|e| PassError::Layout(self.def.path.path().clone(), e))?;
        self.out.tables.structural.push(plan);
        Ok(())
    }

    /// Whether a block of the TIR type `ty` whose payload is `payload` is one
    /// MEM-6 tracks in an arena: a List or Array of rows, a Map or a Set.
    pub(super) fn tracked_block(&mut self, ty: TypeId, payload: LayoutId) -> R<bool> {
        let ty = self.ty(ty)?;
        Ok(crate::library::tracked_block(
            self.types,
            ty,
            self.layouts.table(),
            payload,
        ))
    }

    pub(super) fn layout(&mut self, store_ty: TypeId) -> R<LayoutId> {
        let layout = self
            .layouts
            .layout(self.types, store_ty)
            .map_err(|e| PassError::Layout(self.def.path.path().clone(), e))?;
        self.library_finalizers(store_ty);
        Ok(layout)
    }

    /// Require the drop body of every library resource inside a type this
    /// unit lays out: cleanup lowering calls it where the value is dropped.
    pub(super) fn library_finalizers(&mut self, store_ty: TypeId) {
        let mut todo = vec![store_ty];
        while let Some(ty) = todo.pop() {
            if !self.finalizer_memo.insert(ty) {
                continue;
            }
            let Some(row) = self.types.get(ty) else {
                continue;
            };
            let arguments = |arguments: &[GenericArgument], todo: &mut Vec<TypeId>| {
                todo.extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                }));
            };
            match &row.kind {
                hirsel_types::TypeKind::Tuple(items) => todo.extend(items.iter().copied()),
                hirsel_types::TypeKind::Record(fields) => todo.extend(fields.iter().map(|f| f.ty)),
                hirsel_types::TypeKind::Variant(variants) => {
                    todo.extend(variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1)));
                }
                hirsel_types::TypeKind::Box(inner) => todo.push(*inner),
                hirsel_types::TypeKind::Builtin { arguments: a, .. } => arguments(a, &mut todo),
                hirsel_types::TypeKind::Named {
                    definition,
                    arguments: a,
                } => {
                    arguments(a, &mut todo);
                    if let Some(hirsel_layout::layout::OwnerRepresentation::Defined(record)) =
                        self.layouts.owners().nominals.get(definition)
                        && let Some((finalizer, _)) = &record.finalizer
                        && let Some(body) = crate::library::body_index(finalizer)
                    {
                        let actuals = Actuals {
                            binder: (!a.is_empty()).then(|| crate::library::binder(finalizer)),
                            arguments: a.to_vec(),
                            outer: Vec::new(),
                        };
                        self.required.push(crate::library::Request::finalizer(
                            self.types,
                            body,
                            finalizer,
                            definition.path(),
                            &actuals,
                        ));
                    }
                }
                hirsel_types::TypeKind::Primitive(_)
                | hirsel_types::TypeKind::Parameter(_)
                | hirsel_types::TypeKind::Function(_)
                | hirsel_types::TypeKind::Error => {}
            }
        }
    }

    /// Require a library instance and name it as a direct callee. `lower`
    /// registers the instance once this unit is imported.
    pub(super) fn require(
        &mut self,
        request: crate::library::Request,
    ) -> R<crate::library::builder::Direct> {
        let key = request.key(self.types, self.layouts)?;
        let abi = self
            .layouts
            .fn_abi(self.types, request.callable, request.shape)
            .map_err(|e| PassError::Layout(request.path.clone(), e))?;
        let abi = if request.path.is_core("region.with") {
            crate::library::sealed_region::scoped_abi(
                self.layouts,
                self.types,
                request.callable,
                abi,
            )
            .map_err(|e| PassError::Layout(request.path.clone(), e))?
        } else {
            abi
        };
        let abi = self
            .layouts
            .with_caller_site(abi)
            .map_err(|e| PassError::Layout(request.path.clone(), e))?;
        let abi = if let Some(source) = &request.receipt {
            self.layouts
                .receipt_fn_abi(abi, &source.contract)
                .map_err(|e| PassError::Layout(request.path.clone(), e))?
        } else {
            abi
        };
        let symbol = Symbol {
            kind: SymbolKind::Function,
            definition: request.path.clone(),
            instance: Some(key.digest()),
        };
        self.required.push(request);
        Ok(crate::library::builder::Direct { symbol, abi })
    }

    /// The instance of the library body of operation `path` under the
    /// call's generic arguments.
    pub(super) fn library_request(
        &mut self,
        body: usize,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> R<crate::library::Request> {
        let callee = crate::instance::callee_actuals(
            self.types,
            &self.program.memo,
            self.program.binders.get(path).copied(),
            arguments,
            self.actuals,
            false,
            path,
        )?;
        Ok(crate::library::Request::operation(
            self.program,
            body,
            path,
            callee.arguments,
        )?)
    }

    pub(super) fn intern(&mut self, layout: LayoutId) -> LirTypeId {
        let repr = repr_of(&self.layouts.record(layout).kind);
        self.intern_repr(repr, layout)
    }

    pub(super) fn intern_repr(&mut self, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
        let ty = LirType { repr, layout };
        if let Some(&id) = self.out.type_index.get(&ty) {
            return id;
        }
        let id = LirTypeId(self.out.types.len() as u32);
        self.out.types.push(ty.clone());
        self.out.type_index.insert(ty, id);
        id
    }

    /// The LIR type of a TIR type.
    pub(super) fn lir_type(&mut self, ty: TypeId) -> R<LirTypeId> {
        let t = self.ty(ty)?;
        let layout = self.layout(t)?;
        Ok(self.intern(layout))
    }

    pub(super) fn layout_of(&self, ty: LirTypeId) -> LayoutId {
        self.out.types[ty.index()].layout
    }

    pub(super) fn record(&self, ty: LirTypeId) -> &hirsel_lir::layout_types::LayoutRecord {
        self.layouts.record(self.layout_of(ty))
    }

    /// Whether values of this type carry a holder credit (the validator's rule).
    pub(super) fn owns(&self, ty: LirTypeId) -> bool {
        let t = &self.out.types[ty.index()];
        match t.repr {
            ValueRepr::PayloadReceipt => true,
            ValueRepr::Block | ValueRepr::Token | ValueRepr::RegionMark | ValueRepr::Hole => true,
            ValueRepr::Unit
            | ValueRepr::Scalar(_)
            | ValueRepr::InitMask { .. }
            | ValueRepr::Uncounted
            | ValueRepr::Arena => false,
            ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle => {
                let r = self.layouts.record(t.layout);
                !(r.drop.is_empty() && r.finalizer.is_none())
            }
        }
    }

    /// Plain bits: a bit copy, no credit.
    pub(super) fn bits(&self, ty: LirTypeId) -> bool {
        self.record(ty).copy == CopyRecipe::Bits && !self.owns(ty)
    }

    pub(super) fn value(&mut self, ty: LirTypeId, holding: Holding) -> ValueId {
        let id = ValueId(self.out.values.len() as u32);
        self.out.values.push(ValueInfo { ty, holding });
        id
    }

    /// A freshly produced value: owned if its type carries a credit.
    pub(super) fn fresh(&mut self, ty: LirTypeId) -> ValueId {
        let holding = if self.owns(ty) {
            Holding::Owned
        } else {
            Holding::Plain
        };
        self.value(ty, holding)
    }

    /// A many-call callable received where TIR expects the once-call callable of
    /// the same environment (MODE-3: a once binding holds whatever it
    /// received) is weakened explicitly, so LIR types stay exact. An owned one
    /// is consumed; a borrowed one is viewed under its loan (`task.scope`'s body).
    pub(super) fn weaken(&mut self, value: ValueId, ty: LirTypeId) -> ValueId {
        let from = self.value_type(value);
        let callable = |t: LirTypeId| {
            let record = self.record(t);
            match record.kind {
                LayoutKind::Callable { environment }
                    if self.out.types[t.index()].repr == ValueRepr::Callable =>
                {
                    Some((environment, record.copy))
                }
                _ => None,
            }
        };
        let weakens = matches!(
            (callable(from), callable(ty)),
            (Some((a, CopyRecipe::Dup)), Some((b, CopyRecipe::NotCopyable))) if a == b
        );
        if !weakens {
            return value;
        }
        // A borrowed view stays a view under its loan.
        let result = match self.out.values[value.index()].holding {
            Holding::Owned => self.fresh(ty),
            view => self.value(ty, view),
        };
        self.push(Inst::Weaken { result, value });
        for pending in &mut self.failure.pending {
            if *pending == value {
                *pending = result;
            }
        }
        result
    }

    pub(super) fn value_type(&self, v: ValueId) -> LirTypeId {
        self.out.values[v.index()].ty
    }

    pub(super) fn holds(&self, v: ValueId) -> bool {
        self.out.values[v.index()].holding == Holding::Owned
    }

    // --- blocks and edges ---

    pub(super) fn block(&mut self, kind: BlockKind) -> BlockId {
        let anchor = self.anchor_here();
        let id = BlockId(self.out.blocks.len() as u32);
        self.out.blocks.push(BBlock {
            kind,
            params: Vec::new(),
            phis: Vec::new(),
            view_phis: Vec::new(),
            insts: Vec::new(),
            term: None,
            anchor,
        });
        id
    }

    pub(super) fn push(&mut self, inst: Inst) {
        self.out.blocks[self.current.index()].insts.push(inst);
    }

    pub(super) fn terminate(&mut self, term: Terminator) {
        let block = &mut self.out.blocks[self.current.index()];
        debug_assert!(block.term.is_none(), "block terminated twice");
        block.term = Some(term);
    }

    pub(super) fn edge(&mut self, target: BlockId, args: Vec<ValueId>) -> EdgeId {
        let id = EdgeId(self.out.edges.len() as u32);
        self.out.edges.push(BEdge {
            target,
            args,
            versions: self.versions.clone(),
            views: self.views.clone(),
        });
        id
    }

    /// End the current block with a jump and continue in `next`.
    pub(super) fn goto(&mut self, next: BlockId) {
        let e = self.edge(next, Vec::new());
        self.terminate(Terminator::Jump(e));
        self.current = next;
    }

    /// Continue in a fresh block of `kind` (single predecessor).
    pub(super) fn split(&mut self, kind: BlockKind) {
        let next = self.block(kind);
        self.goto(next);
    }

    pub(super) fn symbol(
        &mut self,
        kind: SymbolKind,
        definition: &DefPath,
        instance: Option<ContentHash>,
    ) -> SymbolId {
        let s = Symbol {
            kind,
            definition: definition.clone(),
            instance,
        };
        if let Some(&id) = self.out.symbol_index.get(&s) {
            return id;
        }
        let id = SymbolId(self.out.tables.symbols.len() as u32);
        self.out.tables.symbols.push(s.clone());
        self.out.symbol_index.insert(s, id);
        id
    }

    pub(super) fn operation(&mut self, op: &OperationId) -> OperationRef {
        if let Some(&id) = self.out.operation_index.get(op) {
            return id;
        }
        let id = OperationRef(self.out.tables.operations.len() as u32);
        self.out.tables.operations.push(op.clone());
        self.out.operation_index.insert(op.clone(), id);
        id
    }

    pub(super) fn constant(&mut self, ty: LirTypeId, value: ConstantValue) -> ConstantId {
        let data = ConstantData { ty, value };
        if let Some(&id) = self.out.constant_index.get(&data) {
            return id;
        }
        let id = ConstantId(self.out.tables.constants.len() as u32);
        self.out.tables.constants.push(data.clone());
        self.out.constant_index.insert(data, id);
        id
    }

    /// Whether the statement being imported is a PS-13 site licensed in `form`.
    pub(super) fn discharged(&self, form: Form) -> bool {
        self.discharge.contains(&Discharged {
            body: self.body_id,
            block: self.statement.0,
            statement: self.statement.1,
            form,
        })
    }

    pub(super) fn source_anchor_here(&self) -> SourceAnchor {
        let (block, statement) = self.statement;
        SourceAnchor {
            body: self.body_id,
            block,
            statement,
            site: self.site(self.span),
        }
    }

    pub(super) fn anchor_here(&mut self) -> SourceAnchorId {
        let a = self.source_anchor_here();
        if let Some(&id) = self.out.source_anchor_index.get(&a) {
            return id;
        }
        let id = SourceAnchorId(self.out.tables.source_anchors.len() as u32);
        self.out.tables.source_anchors.push(a.clone());
        self.out.source_anchor_index.insert(a, id);
        id
    }

    pub(super) fn cleanup_anchor(
        &mut self,
        cleanup: u32,
        action: u32,
        span: Span,
    ) -> CleanupAnchorId {
        let a = CleanupAnchor {
            body: self.body_id,
            cleanup,
            action,
            site: self.site(span),
        };
        if let Some(&id) = self.out.cleanup_anchor_index.get(&a) {
            return id;
        }
        let id = CleanupAnchorId(self.out.tables.cleanup_anchors.len() as u32);
        self.out.tables.cleanup_anchors.push(a.clone());
        self.out.cleanup_anchor_index.insert(a, id);
        id
    }

    pub(super) fn new_slot(&mut self, ty: LirTypeId, kind: SlotKind, scope: ScopeId) -> SlotId {
        let id = SlotId(self.out.slots.len() as u32);
        self.out.slots.push(SlotInfo { ty, kind, scope });
        // Indexed by id, as `new_loan` is: a later pass's state may already
        // name the slot.
        if self.state.slots.len() <= id.index() {
            self.state.slots.resize(id.index() + 1, SlotState::DEAD);
        }
        self.state.slots[id.index()] = SlotState::DEAD;
        self.versions.push(None);
        self.versioned.push(false);
        self.constant_version.push(None);
        id
    }
}

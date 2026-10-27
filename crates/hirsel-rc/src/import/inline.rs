use super::*;
use crate::PassError;

impl Inline for Importer<'_> {
    fn construct_inline(
        &mut self,
        layout: LayoutId,
        payload: LayoutId,
        constructor: ConstructorKind,
        values: &[ValueId],
    ) -> R<ValueId> {
        let ty = self.intern(layout);
        let result = self.fresh(ty);
        let fields = PoolRange::push(
            &mut self.out.pools.fields,
            values.iter().enumerate().map(|(i, &value)| FieldValue {
                field: i as u32,
                value,
            }),
        );
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        let anchor = self.anchor_here();
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor,
            layout: payload,
            fields,
            destination: DestinationClass::Inferred,
            placement: None,
            anchor,
        });
        for &value in values {
            self.consumed(value);
        }
        let failure = self.fail(None)?;
        self.push(Inst::Construct {
            result,
            construct,
            failure,
        });
        self.construct_receipt(result, construct)?;
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        Ok(result)
    }
    fn container_len(&mut self, call: &crate::library::CallShape<'_>) -> R<()> {
        let argument = &call.arguments[0].value;
        // A joined view has no storage: its entries are a field of the
        // block it holds.
        let view = self.joined_view(argument).transpose()?;
        if let Some(view) = view {
            let value = self.read_field(view, 0)?;
            let layout = self.place_layout(call.destination)?;
            let ty = self.intern(layout);
            let result = self.value(ty, Holding::Plain);
            self.push(Inst::SeqLen { result, value });
            return self.finish(call.destination, call.next, result);
        }
        let mut place = self.place(argument.place)?;
        let layout = self.layout_of(place.ty);
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
            self.layouts.record(layout).kind
        else {
            return malformed("container length of a nonblock");
        };
        let entries = self.layouts.record(payload).fields[0].layout;
        place.steps.push(Step::Field(0));
        place.projection.push(ProjectionStep::Field(0));
        place.fixed = false;
        place.ty = self.intern(entries);
        let address = self.address(&place)?;
        let value = self.operand_at(UseKind::Borrow, &place, address, place.loan)?;
        let layout = self.place_layout(call.destination)?;
        let ty = self.intern(layout);
        let result = self.value(ty, Holding::Plain);
        self.push(Inst::SeqLen { result, value });
        self.finish(call.destination, call.next, result)
    }
    fn operand(&mut self, operand: &tir::Operand) -> R<ValueId> {
        Importer::operand(self, operand)
    }
    fn codec_schema(
        &mut self,
        ty: TypeId,
    ) -> R<Option<hirsel_types::serialization::SerializableSchema>> {
        let ty = self.ty(ty)?;
        let signatures = &self.program.signatures;
        hirsel_types::serialization::serializable_schema(
            self.types,
            ty,
            &hirsel_types::GenericDeclarations::default(),
            &mut |d| Ok(signatures.get(d.path()).cloned()),
        )
        .map_err(|e| PassError::Unsupported(format!("codec schema: {e:?}")))
    }
    fn layout_table(&self) -> &hirsel_lir::layout_types::LayoutTable {
        self.layouts.table()
    }
    fn add_codec(&mut self, plan: hirsel_lir::codec::CodecPlan) -> hirsel_lir::ir::CodecId {
        self.out.tables.codecs.push(plan);
        hirsel_lir::ir::CodecId(self.out.tables.codecs.len() as u32 - 1)
    }
    fn read_field(&mut self, value: ValueId, field: u32) -> R<ValueId> {
        let layout = self.layout_of(self.value_type(value));
        let shape = match self.layouts.record(layout).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => layout,
        };
        let layout = self
            .layouts
            .record(shape)
            .fields
            .iter()
            .find(|f| f.ordinal == field)
            .ok_or_else(|| PassError::Unsupported(format!("field {field} of {shape:?}")))?
            .layout;
        let ty = self.intern(layout);
        let projection = PoolRange::push(
            &mut self.out.pools.projections,
            [ProjectionStep::Field(field)],
        );
        let address = AddressId(self.out.addresses.len() as u32);
        let (base, reaching) = self.out.blocks[self.current.index()]
            .insts
            .iter()
            .find_map(|i| match *i {
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                } if result == value => Some((AddressBase::Address(address), reaching)),
                _ => None,
            })
            .unwrap_or((AddressBase::Value(value), value));
        self.out.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base,
            projection,
            ty,
        });
        let failure = self.fail(None)?;
        self.push(Inst::Address { address, failure });
        let holding = if self.owns(ty) {
            self.out.values[value.index()].holding
        } else {
            Holding::Plain
        };
        let result = self.value(ty, holding);
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        Ok(result)
    }
    fn place_address(&mut self, place: tir::PlaceId) -> R<(AddressId, ValueId)> {
        if let Some(view) = self.whole_joined(place) {
            let value = view?;
            return Ok((self.view_address(value)?, value));
        }
        let p = self.place(place)?;
        let address = self.address(&p)?;
        Ok((address, self.reaching(p.slot)?))
    }
    fn tir_layout(&mut self, ty: TypeId) -> R<LayoutId> {
        let t = self.ty(ty)?;
        self.layout(t)
    }
    fn place_layout(&mut self, place: tir::PlaceId) -> R<LayoutId> {
        let Some(p) = self.body.places.get(place.0 as usize) else {
            return malformed(format!("place {} outside the body", place.0));
        };
        self.tir_layout(p.ty)
    }
    fn layout_record(&self, layout: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord {
        self.layouts.record(layout)
    }
    fn new_value(&mut self, layout: LayoutId, holding: Option<Holding>) -> ValueId {
        let ty = self.intern(layout);
        match holding {
            Some(holding) => self.value(ty, holding),
            None => self.fresh(ty),
        }
    }
    fn literal(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId {
        let ty = self.intern(layout);
        let constant = self.constant(ty, value);
        let result = self.new_value(layout, Some(Holding::Plain));
        self.push(Inst::Const { result, constant });
        result
    }
    fn emit(&mut self, inst: Inst) {
        self.push(inst);
    }
    fn consume(&mut self, value: ValueId) {
        self.consumed(value);
    }
    fn pend(&mut self, value: ValueId) {
        if self.holds(value) {
            self.failure.pending.push(value);
        }
    }
    fn failure(&mut self, rule: Option<String>) -> R<FailureId> {
        self.fail(rule)
    }
    fn finish(&mut self, destination: tir::PlaceId, next: tir::BlockId, value: ValueId) -> R<()> {
        let dest = self.place(destination)?;
        let address = self.address(&dest)?;
        self.store(&dest, address, value)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }
    fn intern_symbol(&mut self, symbol: &Symbol) -> SymbolId {
        self.symbol(symbol.kind, &symbol.definition, symbol.instance)
    }
    fn iterator_closure(
        &mut self,
        call: &crate::library::CallShape<'_>,
        helper: &DefPath,
    ) -> R<ValueId> {
        let body = crate::library::body_index(helper).expect("registered iterator thunk");
        let mut request = self.library_request(
            body,
            &hirsel_syntax::DefPath::core("iter.next"),
            &call.generics[1..],
        )?;
        request.path = helper.clone();
        let mut callable = self.types.row(request.callable).clone();
        let hirsel_types::TypeKind::Function(ref mut function) = callable.kind else {
            unreachable!()
        };
        function.parameters = Vec::new().into();
        request.callable = self.types.intern(callable);
        request.shape = CallableShape::IterNext;
        let mut fields = Vec::new();
        let mut values = Vec::new();
        // Keep the actual captured callable type, including its invocation ABI,
        // rather than reconstructing it from the operation's receiving type.
        for argument in call.arguments {
            let source = self.body.places[argument.value.place.0 as usize].ty;
            let ty = self.ty(source)?;
            fields.push((ty, Passing::Take));
            values.push(CaptureValue {
                value: self.operand(&argument.value)?,
                usage: hirsel_lir::ir::CaptureUse::Take,
            });
        }
        // The capture types complete the helper instance's identity. The first
        // two actuals are the physical capture types; its result is already closed.
        request.actuals.arguments = fields
            .iter()
            .map(|(ty, _)| GenericArgument::Type(*ty))
            .collect();
        request.environment = Some(
            self.layouts
                .environment(
                    self.types,
                    &hirsel_layout::layout::EnvironmentShape {
                        definition: DefId::intern(helper),
                        body: 0,
                        captures: &fields,
                        sizes: &[],
                        movable: &[],
                    },
                )
                .map_err(|e| PassError::Layout(helper.clone(), e))?,
        );
        let environment = request.environment.unwrap();
        let direct = self.require(request)?;
        let code = self.symbol(SymbolKind::Function, helper, direct.symbol.instance);
        let anchor = self.anchor_here();
        let captures = PoolRange::push(&mut self.out.pools.captures, values.iter().copied());
        let closure = hirsel_lir::ir::ClosureId(self.out.tables.closures.len() as u32);
        self.out.tables.closures.push(ClosureSite {
            receipts: None,
            scoped: false,
            code: Some(ClosureCode {
                symbol: code,
                abi: direct.abi,
            }),
            environment,
            captures,
            destination: DestinationClass::Inferred,
            placement: None,
            anchor,
        });
        for c in values {
            self.consumed(c.value);
        }
        let layout = self.place_layout(call.destination)?;
        let result = self.new_value(layout, None);
        let failure = self.fail(None)?;
        self.push(Inst::Closure {
            result,
            closure,
            failure,
        });
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        Ok(result)
    }

    fn function(&mut self, path: &DefPath) -> R<(SymbolId, FnAbiId)> {
        self.direct(path, &[])
    }
    fn direct_call(
        &mut self,
        symbol: SymbolId,
        abi: FnAbiId,
        arguments: Vec<CallArgument>,
    ) -> R<Option<ValueId>> {
        let fn_abi = self.layouts.table().fn_abis[abi.0 as usize].clone();
        for a in &arguments {
            if let ArgumentValue::Value(v) = a.value {
                self.consumed(v);
            }
        }
        let result = fn_abi.result.as_ref().map(|r| {
            let ty = self.intern(r.layout);
            self.fresh(ty)
        });
        let failure = self.fail(None)?;
        let anchor = self.anchor_here();
        let arguments = PoolRange::push(&mut self.out.pools.call_arguments, arguments);
        let call = CallId(self.out.tables.calls.len() as u32);
        self.out.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee: Callee::Direct(symbol),
            abi,
            arguments,
            result,
            destination: fn_abi.destination.then_some(DestinationClass::Inferred),
            declared: CALL_EFFECTS,
            anchor,
        });
        let after = self.block(BlockKind::Body);
        let normal = self.edge(after, Vec::new());
        self.terminate(Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        });
        self.current = after;
        if let Some(v) = result
            && self.holds(v)
        {
            self.failure.pending.push(v);
        }
        Ok(result)
    }
}

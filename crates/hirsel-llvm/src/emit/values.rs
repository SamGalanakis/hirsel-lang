use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn new(
        module: &'m Module<'m>,
        inputs: &'a EmitInputs<'a>,
        unit: &'a LirUnit<Ready>,
        function: Function<'m>,
        signature: Signature<'m, 'a>,
        manifested: &'a mut Manifested,
    ) -> Self {
        let prologue_block = module.append_block(function);
        let prologue = Builder::new(module);
        prologue.position(prologue_block);
        let context = module.parameter(function, 0);
        let environment = signature
            .hidden_index(|h| matches!(h, Hidden::Environment))
            .map(|index| module.parameter(function, index));
        Self {
            module,
            inputs,
            unit,
            function,
            signature,
            manifested,
            builder: Builder::new(module),
            debug_sites: None,
            debug: None,
            measured: None,
            prologue,
            prologue_block,
            context,
            values: vec![None; unit.values.len()],
            versions: BTreeSet::new(),
            slots: vec![None; unit.slots.len()],
            sequence_states: vec![None; unit.slots.len()],
            read_roots: vec![None; unit.values.len()],
            preserved_lengths: vec![None; unit.values.len()],
            deferred: vec![false; unit.slots.len()],
            viewed: vec![false; unit.slots.len()],
            companion_slots: vec![false; unit.slots.len()],
            constant_slots: hirsel_lir::statics::constant_slots(unit, inputs.statics),
            defer_params: vec![None; unit.slots.len()],
            fresh_detaches: BTreeSet::new(),
            detach_untracked: BTreeMap::new(),
            known_count: None,
            at: (BlockId(0), 0),
            adopted: BTreeMap::new(),
            addresses: vec![None; unit.addresses.len()],
            flat_contents: vec![None; unit.addresses.len()],
            flat_content: None,
            blocks: Vec::new(),
            phis: Vec::new(),
            failures: BTreeMap::new(),
            environment,
            regions: Vec::new(),
            register_return: None,
            drop_status: None,
            ahead_state: None,
            ahead_counter: None,
            ownership: None,
            ownership_frame: None,
            ownership_cleanup: None,
            resume: None,
            room: Default::default(),
            no_room: None,
            counters: Default::default(),
            counter_cell: None,
            fuel_points: if inputs.fuel || inputs.alternate {
                hirsel_lir::fuel::fuel_points(unit)
            } else {
                Vec::new()
            },
        }
    }

    pub(super) fn definition(&self) -> &DefPath {
        &self.unit.header.definition
    }

    pub(super) fn invalid<T>(&self, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Invalid {
            definition: self.definition().clone(),
            what: what.into(),
        })
    }

    pub(super) fn unsupported<T>(&self, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Unsupported {
            definition: self.definition().clone(),
            what: what.into(),
        })
    }

    pub(super) fn layout(&self, id: LayoutId) -> Emitted<&'a LayoutRecord> {
        match self.inputs.layouts.layout(id) {
            Some(record) => Ok(record),
            None => self.invalid(format!("no layout {}", id.0)),
        }
    }

    /// The bytes a value of `id` occupies in storage: a Packed value's
    /// largest variant (`SizeSpec::storage`).
    pub(super) fn fixed_size(&self, id: LayoutId) -> Emitted<u64> {
        match self.layout(id)?.size.storage() {
            Some(size) => Ok(size),
            None => self.unsupported("a value of dynamic extent"),
        }
    }

    pub(super) fn lir_type(&self, id: LirTypeId) -> Emitted<&'a LirType> {
        match self.unit.types.get(id.index()) {
            Some(ty) => Ok(ty),
            None => self.invalid(format!("no type {}", id.0)),
        }
    }

    pub(super) fn value_type(&self, value: ValueId) -> Emitted<&'a LirType> {
        match self.unit.values.get(value.index()) {
            Some(info) => self.lir_type(info.ty),
            None => self.invalid(format!("no value {}", value.0)),
        }
    }

    pub(super) fn shape(&self, ty: &LirType) -> Emitted<Shape> {
        if matches!(ty.repr, ValueRepr::Token | ValueRepr::RegionMark) {
            return Ok(Shape::Scalar(ScalarKind::Pointer));
        }
        self.layout_shape(ty.layout)
    }

    pub(super) fn layout_shape(&self, layout: LayoutId) -> Emitted<Shape> {
        let record = self.layout(layout)?;
        if let Some(kind) = scalar_shape(&record.kind) {
            return Ok(Shape::Scalar(kind));
        }
        Ok(match record.kind {
            LayoutKind::Unit => Shape::Unit,
            LayoutKind::Callable { .. } => Shape::Callable,
            _ => Shape::Bytes {
                size: self.fixed_size(layout)?,
                align: record.align,
            },
        })
    }

    pub(super) fn value_shape(&self, value: ValueId) -> Emitted<Shape> {
        self.shape(self.value_type(value)?)
    }

    pub(super) fn llvm_type(&self, shape: Shape) -> Type<'m> {
        match shape {
            Shape::Unit => self.module.struct_type(&[]),
            Shape::Scalar(kind) => scalar::scalar_type(&self.builder, kind),
            Shape::Callable => self.module.struct_type(&[self.module.ptr_type(); 2]),
            Shape::Bytes { size, align } => {
                let unit = [8, 4, 2, 1]
                    .into_iter()
                    .find(|&unit| u64::from(align) % unit == 0 && size % unit == 0)
                    .unwrap_or(1);
                self.module
                    .array_type(self.module.int_type((unit * 8) as u32), size / unit)
            }
        }
    }

    pub(super) fn load(&self, shape: Shape, place: Place<'m>) -> Value<'m> {
        let b = &self.builder;
        let loaded = |ty| {
            // `block` proves that the place lies in a payload, past the counted
            // header. Unknown incoming addresses stay unclassified; they need
            // not inherit a source-level alias promise from their scalar type.
            if place.block.is_some() {
                b.load_access(ty, place.ptr, place.align, Access::Value)
            } else {
                b.load(ty, place.ptr, place.align)
            }
        };
        match shape {
            Shape::Unit => self.module.const_zero(self.llvm_type(Shape::Unit)),
            Shape::Scalar(ScalarKind::Bool) => {
                let byte = loaded(self.module.int_type(8));
                let zero = self.module.const_int(self.module.int_type(8), 0);
                b.icmp(IntPredicate::Ne, byte, zero)
            }
            Shape::Scalar(_) => loaded(self.llvm_type(shape)),
            _ => b.load(self.llvm_type(shape), place.ptr, place.align),
        }
    }

    pub(super) fn store(&self, shape: Shape, value: Value<'m>, place: Place<'m>) {
        let b = &self.builder;
        let stored = |value| {
            if place.block.is_some() {
                b.store_access(value, place.ptr, place.align, Access::Value);
            } else {
                b.store(value, place.ptr, place.align);
            }
        };
        match shape {
            Shape::Unit => {}
            Shape::Scalar(ScalarKind::Bool) => {
                stored(b.cast(Cast::ZExt, value, self.module.int_type(8)));
            }
            Shape::Scalar(_) => stored(value),
            _ => b.store(value, place.ptr, place.align),
        }
    }

    /// A stack slot in the prologue for a value of `layout`.
    pub(super) fn stack(&self, layout: LayoutId) -> Emitted<Place<'m>> {
        let size = self.fixed_size(layout)?;
        let align = self.layout(layout)?.align;
        let ty = self.module.array_type(self.module.int_type(8), size);
        Ok(Place {
            columns: None,
            ptr: self.prologue.alloca(ty, align),
            layout,
            align,
            block: None,
        })
    }

    pub(super) fn get(&self, value: ValueId) -> Emitted<Value<'m>> {
        match self.values.get(value.index()).copied().flatten() {
            Some(v) => Ok(v),
            None if self.versions.contains(&value) => {
                self.invalid(format!("storage version {} used as an operand", value.0))
            }
            None => self.invalid(format!("value {} used before its definition", value.0)),
        }
    }

    /// Define a scalar operation's flag, present exactly when the operation reports one.
    pub(super) fn define_flag(
        &mut self,
        flag: Option<ValueId>,
        llvm: Option<Value<'m>>,
    ) -> Emitted {
        match (flag, llvm) {
            (Some(flag), Some(llvm)) => self.define(flag, llvm),
            (None, None) => Ok(()),
            _ => self.invalid("a scalar flag the operation does not report"),
        }
    }

    pub(super) fn define(&mut self, value: ValueId, llvm: Value<'m>) -> Emitted {
        match self.values.get_mut(value.index()) {
            Some(slot @ None) => {
                *slot = Some(llvm);
                Ok(())
            }
            Some(Some(_)) => self.invalid(format!("value {} defined twice", value.0)),
            None => self.invalid(format!("no value {}", value.0)),
        }
    }

    pub(super) fn const_u64(&self, n: u64) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(64), u128::from(n))
    }

    pub(super) fn pool<T: Copy>(&self, range: PoolRange<T>, pool: &'a [T]) -> Emitted<&'a [T]> {
        match range.get(pool) {
            Some(items) => Ok(items),
            None => self.invalid("a pool range outside its pool"),
        }
    }

    pub(super) fn edge(&self, edge: EdgeId) -> Emitted<&'a Edge> {
        match self.unit.tables.edges.get(edge.index()) {
            Some(e) => Ok(e),
            None => self.invalid(format!("no edge {}", edge.0)),
        }
    }

    pub(super) fn failure_edge(&self, failure: FailureId) -> Emitted<&'a FailureEdge> {
        match self.unit.tables.failures.get(failure.index()) {
            Some(f) => Ok(f),
            None => self.invalid(format!("no failure edge {}", failure.0)),
        }
    }

    pub(super) fn runtime(&mut self, entry: RuntimeEntry) -> Emitted<Function<'m>> {
        let declaration = entry.declaration();
        let c_type = |ty: CType| -> Type<'m> {
            if ty.pointers > 0 {
                return self.module.ptr_type();
            }
            match ty.name {
                CName::Void => self.module.void_type(),
                CName::U8 => self.module.int_type(8),
                CName::U32 => self.module.int_type(32),
                CName::U64 => self.module.int_type(64),
                _ => self.module.ptr_type(),
            }
        };
        let parameters: Vec<_> = declaration
            .parameters
            .iter()
            .map(|p| c_type(p.ty))
            .collect();
        let ty = self
            .module
            .function_type(c_type(declaration.returns), &parameters);
        let function = match self.module.function(declaration.symbol, ty) {
            Ok(function) => function,
            Err(what) => return self.invalid(what),
        };
        crate::runtime::substrate(self.module, function, entry);
        self.manifested
            .imports
            .entry(declaration.symbol.to_owned())
            .or_insert_with(|| ObjectSymbol {
                name: declaration.symbol.to_owned(),
                kind: ObjectSymbolKind::Runtime,
                definition: None,
                abi: None,
            });
        Ok(function)
    }
}

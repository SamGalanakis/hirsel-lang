use super::*;

/// An LLVM parameter of a Hirsel ABI v3 entry.
#[derive(Clone, Copy, Debug)]
pub(super) enum Hidden {
    Context,
    Environment,
    CallerSite,
    OperationSite,
    Destination,
    Result,
}

/// What a Hirsel entry's declaration states about the memory its address parameters
/// reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Aliasing {
    /// No aliasing attribute: runtime primitives, whose bodies are the runtime's, and
    /// every entry of a program that hands C a callback.
    Unstated,
    /// A `mut` place's address is `noalias`. Both checkers prove it for Hirsel
    /// callers (OWN-3); the unit partition never changes it.
    ExclusiveMut,
}

impl Aliasing {
    /// The aliasing a program's Hirsel entries state. A program that names a C
    /// callback states none: C holds a callback context's address for the whole extern
    /// call and may hand it to a callback nested in an activation that still has it
    /// (PROG-4), so the context place is reached by two addresses at once.
    pub(super) fn of(callbacks: bool) -> Self {
        if callbacks {
            Self::Unstated
        } else {
            Self::ExclusiveMut
        }
    }
}

/// The LLVM signature of one `FnAbi`: hidden slots in ABI order, then each declared
/// parameter's physical slots, returning a zero-extended `u8` status. A Hirsel entry whose
/// result is a plain scalar (`register_result`) has no result pointer and returns
/// `{ value word, i8 status }` instead, in registers on x86-64 and AArch64.
pub(super) struct Signature<'m, 'a> {
    pub(super) abi: &'a FnAbi,
    pub(super) ty: Type<'m>,
    /// The LLVM parameter types, hidden slots first.
    pub(super) types: Vec<Type<'m>>,
    pub(super) hidden: Vec<Hidden>,
    /// Per declared parameter, the index of its first LLVM parameter.
    pub(super) first: Vec<u32>,
    pub(super) extensions: Vec<(u32, Attribute)>,
    /// Internal library roots travel by value and return on both status outcomes.
    pub(super) roots: Vec<usize>,
    pub(super) sequence_roots: Vec<bool>,
    /// Private sequence inputs: full length, must-unique, defer and must-untracked.
    pub(super) companions: Vec<Option<u32>>,
    /// Per root, the returned `i1` field holding its must-unique fact (sequence roots).
    pub(super) uniques: Vec<Option<u32>>,
    /// Per root, the returned `i1` field holding its must-untracked fact (sequence roots,
    /// MEM-6): its block is no tracked arena block.
    pub(super) untrackeds: Vec<Option<u32>>,
    /// The returned field holding the status.
    pub(super) status_field: u32,
    /// The plain scalar result returned in a register word instead of through a
    /// result pointer, and the returned field holding it.
    pub(super) value: Option<(ScalarKind, u32)>,
    pub(super) returns: Type<'m>,
    pub(super) ahead: Option<(u32, u32)>,
}

pub(super) fn physical<'m>(module: &'m Module<'m>, ty: PhysicalType) -> Type<'m> {
    match ty {
        PhysicalType::I8 => module.int_type(8),
        PhysicalType::I16 => module.int_type(16),
        PhysicalType::I32 => module.int_type(32),
        PhysicalType::I64 => module.int_type(64),
        PhysicalType::I128 => module.int_type(128),
        PhysicalType::F32 => module.f32_type(),
        PhysicalType::F64 => module.f64_type(),
        PhysicalType::Pointer => module.ptr_type(),
    }
}

impl<'m, 'a> Signature<'m, 'a> {
    /// The signature of a Hirsel entry: a plain scalar result returns in registers.
    pub(super) fn of(
        module: &'m Module<'m>,
        layouts: &LayoutTable,
        abi: &'a FnAbi,
        definition: &DefPath,
    ) -> Result<Self, CodegenError> {
        Self::with_result(module, abi, definition, register_result(layouts, abi))
    }

    /// The signature of a runtime primitive, whose every result is written through its
    /// result pointer.
    pub(super) fn runtime(
        module: &'m Module<'m>,
        abi: &'a FnAbi,
        definition: &DefPath,
    ) -> Result<Self, CodegenError> {
        Self::with_result(module, abi, definition, None)
    }

    pub(super) fn with_result(
        module: &'m Module<'m>,
        abi: &'a FnAbi,
        definition: &DefPath,
        value: Option<ScalarKind>,
    ) -> Result<Self, CodegenError> {
        if !abi.context {
            return Err(CodegenError::Unsupported {
                definition: definition.clone(),
                what: "a Hirsel entry without a run context".into(),
            });
        }
        let ptr = module.ptr_type();
        let mut hidden = vec![Hidden::Context];
        if abi.environment {
            hidden.push(Hidden::Environment);
        }
        if abi.caller_site {
            hidden.extend([Hidden::CallerSite, Hidden::OperationSite]);
        }
        if abi.destination {
            hidden.push(Hidden::Destination);
        }
        if abi.result.is_some() && value.is_none() {
            hidden.push(Hidden::Result);
        }
        let mut types = hidden
            .iter()
            .map(|h| match h {
                Hidden::CallerSite | Hidden::OperationSite => module.int_type(64),
                _ => ptr,
            })
            .collect::<Vec<_>>();
        let mut first = Vec::new();
        let mut extensions = Vec::new();
        for parameter in &abi.parameters {
            first.push(types.len() as u32);
            let attribute = match parameter.extension {
                ScalarExtension::None => None,
                ScalarExtension::Sign => Some(Attribute::SignExt),
                ScalarExtension::Zero => Some(Attribute::ZeroExt),
            };
            if let (Some(attribute), [_]) = (attribute, parameter.slots.as_slice()) {
                extensions.push((types.len() as u32, attribute));
            }
            types.extend(parameter.slots.iter().map(|s| physical(module, s.ty)));
        }
        let receipts = hirsel_lir::abi::receipt_slots(abi, types.len()).map_err(|what| {
            CodegenError::Invalid {
                definition: definition.clone(),
                what: what.into(),
            }
        })?;
        types.extend(std::iter::repeat_n(ptr, receipts.len()));
        let returns = match value {
            Some(kind) => pair_type(module, kind),
            None => module.int_type(8),
        };
        let ty = module.function_type(returns, &types);
        Ok(Self {
            abi,
            ty,
            types,
            hidden,
            first,
            extensions,
            roots: Vec::new(),
            sequence_roots: Vec::new(),
            companions: Vec::new(),
            uniques: Vec::new(),
            untrackeds: Vec::new(),
            status_field: if value.is_some() {
                PairField::Status.index()
            } else {
                1
            },
            value: value.map(|kind| (kind, PairField::Value.index())),
            returns,
            ahead: None,
        })
    }

    pub(super) fn with_roots(mut self, module: &'m Module<'m>, layouts: &LayoutTable) -> Self {
        self.roots = self
            .abi
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                (p.passing == ParameterPassing::ExclusiveAddress
                    && matches!(
                        layouts.layout(p.layout).map(|l| &l.kind),
                        Some(LayoutKind::Block { .. } | LayoutKind::Box { .. })
                    ))
                .then_some(i)
            })
            .collect();
        self.sequence_roots = self
            .roots
            .iter()
            .map(|&i| {
                match layouts
                    .layout(self.abi.parameters[i].layout)
                    .map(|l| &l.kind)
                {
                    Some(LayoutKind::Block { payload }) => matches!(
                        layouts.layout(*payload).map(|l| &l.kind),
                        Some(LayoutKind::Sequence { .. })
                    ),
                    _ => false,
                }
            })
            .collect();
        self.companions = self
            .sequence_roots
            .iter()
            .map(|&sequence| {
                sequence.then(|| {
                    // Length, must-unique, whether the caller defers this root's
                    // count store (`SequenceState::dirty`): a caller without a
                    // sequence state for it passes false, and gets the count stored;
                    // and must-untracked (MEM-6).
                    let first = self.types.len() as u32;
                    self.types.extend([
                        module.int_type(64),
                        module.int_type(1),
                        module.int_type(1),
                        module.int_type(1),
                    ]);
                    first
                })
            })
            .collect();
        if !self.roots.is_empty() {
            // Every root's first word is its pointer, typed `ptr` and never
            // tagged, so a loop that calls or inlines a root body carries the
            // pointer as itself: no mask or re-tag per iteration. Other roots can
            // be headerless regional payloads and return status in the second
            // word. A sequence root's second word is its full u64 length, and its
            // must-unique fact has a word of its own; its capacity is the block's
            // own header word (runtime ABI 5), so nothing caches it. When the first
            // root is a sequence, status is a last word of its own: every return
            // site supplies a constant there, so after inlining the caller's
            // status branch threads to the right continuation and the facts it
            // carries fold. That one-root return takes four words (memory on
            // x86-64 and AArch64); only uninlined calls pay for it.
            let mut fields = Vec::new();
            for _ in &self.roots {
                fields.extend([module.ptr_type(), module.int_type(64)]);
            }
            let mut extra = |ty: Type<'m>| {
                fields.push(ty);
                fields.len() as u32 - 1
            };
            let sequences = self.sequence_roots.clone();
            self.uniques = sequences
                .iter()
                .map(|&sequence| sequence.then(|| extra(module.int_type(1))))
                .collect();
            self.untrackeds = sequences
                .iter()
                .map(|&sequence| sequence.then(|| extra(module.int_type(1))))
                .collect();
            self.status_field = 1;
            if sequences[0] {
                self.status_field = extra(module.int_type(64));
            }
            // A register result is one more word, after every root's.
            if let Some((_, field)) = &mut self.value {
                *field = extra(module.int_type(64));
            }
            self.returns = module.struct_type(&fields);
            self.ty = module.function_type(self.returns, &self.types);
        }
        self
    }

    pub(super) fn with_ahead(mut self, module: &'m Module<'m>) -> Self {
        let first = self.types.len() as u32;
        self.types.extend([module.ptr_type(), module.ptr_type()]);
        self.ahead = Some((first, first + 1));
        self.ty = module.function_type(self.returns, &self.types);
        self
    }

    pub(super) fn root_pointer(
        &self,
        builder: &Builder<'m>,
        returned: Value<'m>,
        field: usize,
    ) -> Value<'m> {
        builder.extract(returned, field as u32 * 2)
    }

    /// A returned sequence root's must-unique fact.
    pub(super) fn root_unique(
        &self,
        builder: &Builder<'m>,
        returned: Value<'m>,
        field: usize,
    ) -> Value<'m> {
        builder.extract(
            returned,
            self.uniques[field].expect("a sequence root's must-unique word"),
        )
    }

    /// A returned sequence root's must-untracked fact, or its must-unique one where the
    /// signature has none.
    pub(super) fn root_untracked(
        &self,
        builder: &Builder<'m>,
        returned: Value<'m>,
        field: usize,
    ) -> Value<'m> {
        match self.untrackeds.get(field).copied().flatten() {
            Some(index) => builder.extract(returned, index),
            None => self.root_unique(builder, returned, field),
        }
    }

    /// The `u8` status a call of this signature returned.
    pub(super) fn status(&self, builder: &Builder<'m>, returned: Value<'m>) -> Value<'m> {
        let module = builder.module();
        if !self.roots.is_empty() {
            let word = builder.extract(returned, self.status_field);
            return builder.cast(Cast::Trunc, word, module.int_type(8));
        }
        if self.value.is_some() {
            return builder.extract(returned, self.status_field);
        }
        returned
    }

    /// The register result a successful call of this signature returned.
    pub(super) fn result(&self, builder: &Builder<'m>, returned: Value<'m>) -> Option<Value<'m>> {
        let (kind, field) = self.value?;
        let ty = if self.roots.is_empty() {
            value_word(builder.module(), kind)
        } else {
            builder.module().int_type(64)
        };
        Some(decode_word(
            builder,
            kind,
            builder.extract(returned, field),
            ty,
        ))
    }

    /// Whether the entry returns a bare status byte.
    pub(super) fn returns_status(&self) -> bool {
        self.roots.is_empty() && self.value.is_none()
    }

    /// What an entry of this signature returns: the `u8` `status` and, with a register
    /// result, `value` (a zero word when absent, as on failure). Root words are already
    /// in `returned`, which is otherwise zero.
    pub(super) fn returned(
        &self,
        builder: &Builder<'m>,
        mut returned: Value<'m>,
        status: Value<'m>,
        value: Option<Value<'m>>,
    ) -> Value<'m> {
        let module = builder.module();
        if self.returns_status() {
            return status;
        }
        if self.roots.is_empty() {
            returned = builder.insert(returned, status, self.status_field);
        }
        if let (Some((kind, field)), Some(value)) = (self.value, value) {
            let word = if self.roots.is_empty() {
                value_word(module, kind)
            } else {
                module.int_type(64)
            };
            returned = builder.insert(returned, encode_word(builder, kind, value, word), field);
        }
        returned
    }

    pub(super) fn hidden_index(&self, which: fn(&Hidden) -> bool) -> Option<u32> {
        self.hidden.iter().position(which).map(|i| i as u32)
    }

    /// The LLVM parameters that are a `mut` place's address (`ExclusiveAddress`): every
    /// one but a root, which travels as the block pointer the place holds.
    fn exclusive_addresses(&self) -> impl Iterator<Item = (u32, LayoutId)> + '_ {
        self.abi
            .parameters
            .iter()
            .enumerate()
            .filter(|&(i, p)| {
                p.passing == ParameterPassing::ExclusiveAddress && !self.roots.contains(&i)
            })
            .map(|(i, p)| (self.first[i], p.layout))
    }

    /// The function `name` with this signature. Emitted code provides argument and status
    /// extension but never assumes it, since an AAPCS64 C caller or callee does not extend
    /// narrow integers: only a definition's status return is marked extended (it returns a
    /// constant), and parameters are extended at call sites (`attribute_call`).
    ///
    /// With `aliasing` (`Aliasing::of`), each `mut` place address is `noalias`: for
    /// the whole call the callee reaches the place's bytes only through it
    /// (`docs/spec/implementation.md`, Aliasing facts). The blocks those bytes point to
    /// are loaded pointers, which the attribute does not cover. The live place also
    /// grants its layout's extent and alignment (`dereferenceable` and `align`);
    /// underaligned fields use aligned ABI temporaries through either outcome.
    pub(super) fn declare(
        &self,
        module: &'m Module<'m>,
        layouts: &LayoutTable,
        name: &str,
        definition: &DefPath,
        defining: bool,
        aliasing: Aliasing,
    ) -> Result<Function<'m>, CodegenError> {
        let function = module
            .function(name, self.ty)
            .map_err(|what| CodegenError::Invalid {
                definition: definition.clone(),
                what,
            })?;
        module.add_attribute(function, AttributeSlot::Function, Attribute::NoUnwind);
        if defining && self.returns_status() {
            module.add_attribute(function, AttributeSlot::Return, Attribute::ZeroExt);
        }
        if aliasing == Aliasing::ExclusiveMut {
            for (index, layout) in self.exclusive_addresses() {
                let record = layouts
                    .layout(layout)
                    .ok_or_else(|| CodegenError::Invalid {
                        definition: definition.clone(),
                        what: format!("no mut place layout {}", layout.0),
                    })?;
                // Fixed places have their whole layout's extent. A sequential Packed
                // element has only its active variant's bytes, so promise the minimum
                // variant extent, never the maximum used for standalone storage.
                let size = match &record.size {
                    SizeSpec::Fixed(size) => Some(*size),
                    SizeSpec::Dynamic(program) => match program.as_slice() {
                        [SizeTerm::ByTag { variants, .. }]
                            if record.kind == LayoutKind::Packed && !variants.is_empty() =>
                        {
                            variants
                                .iter()
                                .map(|(_, p)| match p.as_slice() {
                                    [SizeTerm::Constant(size)] => Some(*size),
                                    _ => None,
                                })
                                .try_fold(u64::MAX, |min, size| size.map(|s| min.min(s)))
                        }
                        _ => None,
                    },
                }
                .ok_or_else(|| CodegenError::Unsupported {
                    definition: definition.clone(),
                    what: "a mut place of variable extent".into(),
                })?;
                // Call ownership keeps the place live, and calls normalize packed
                // field addresses to aligned storage through both status outcomes.
                let slot = AttributeSlot::Parameter(index);
                module.add_attribute(function, slot, Attribute::NoAlias);
                module.add_attribute(function, slot, Attribute::Align(record.align));
                // LLVM requires a positive extent. A zero-size place grants no bytes.
                if size != 0 {
                    module.add_attribute(function, slot, Attribute::Dereferenceable(size));
                }
            }
        }
        Ok(function)
    }

    /// The caller extends each narrow integer argument as the `FnAbi` says.
    pub(super) fn attribute_call(&self, module: &'m Module<'m>, call: Value<'m>) {
        // Hirsel failure crosses this ABI as status and cleanup, never unwinding.
        // In particular, this is also true when the code pointer is erased.
        module.add_call_attribute(call, AttributeSlot::Function, Attribute::NoUnwind);
        for &(index, attribute) in &self.extensions {
            module.add_call_attribute(call, AttributeSlot::Parameter(index), attribute);
        }
    }
}

#[cfg(test)]
mod receipt_tests;

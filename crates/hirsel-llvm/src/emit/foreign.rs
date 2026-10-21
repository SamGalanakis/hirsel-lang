//! Calls of `extern "c"` functions (PROG-4): a direct call of the C symbol in
//! the platform convention, outside the Hirsel status protocol. Foreign
//! contract violations terminate before invalid pointers can enter Hirsel.
use super::*;
use hirsel_lir::layout_types::CConvention;

// Linux AAPCS64 assigns a register but leaves its high bits unspecified for
// narrow C scalars. Model that raw register explicitly at the boundary; then
// truncate/extend it to Hirsel's declared scalar before internal calls.
fn aarch64_narrow(kind: ScalarKind, convention: CConvention) -> Option<bool> {
    if convention != CConvention::Aapcs64 {
        return None;
    }
    if kind == ScalarKind::Bool {
        return Some(false);
    }
    scalar::integer(kind).and_then(|(width, signed)| (width < 64).then_some(signed))
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn foreign_call(
        &mut self,
        call: CallId,
        abi: &FnAbi,
        normal: Option<EdgeId>,
    ) -> Emitted {
        let unit = self.unit;
        let site = &unit.tables.calls[call.index()];
        let Callee::Direct(symbol) = site.callee else {
            return self.invalid("an extern called through a value");
        };
        let Some(symbol) = unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a callee outside the symbol table");
        };
        let name = symbol.definition.0.name.clone();
        let arguments = self.pool(site.arguments, &unit.pools.call_arguments)?;
        let pointer = self.module.ptr_type();
        let mut types = Vec::new();
        let mut args = Vec::new();
        let mut extensions = Vec::new();
        let mut written = Vec::new();
        for parameter in &abi.parameters {
            let Some(argument) = arguments.iter().find(|a| a.parameter == parameter.source) else {
                return self.invalid("a call without an argument for a parameter");
            };
            let kind = &self.layout(parameter.layout)?.kind;
            match (parameter.passing, argument.value, kind) {
                // A capability is authority, never data C receives.
                (_, _, LayoutKind::Handle) => {}
                (
                    ParameterPassing::ByValue,
                    ArgumentValue::Value(value),
                    LayoutKind::Scalar(scalar),
                ) => {
                    let value = self.get(value)?;
                    if let Some(signed) = aarch64_narrow(*scalar, self.inputs.target.c_convention) {
                        types.push(self.module.int_type(64));
                        args.push(self.builder.cast(
                            if signed { Cast::SExt } else { Cast::ZExt },
                            value,
                            self.module.int_type(64),
                        ));
                        continue;
                    }
                    match parameter.extension {
                        ScalarExtension::None => {}
                        ScalarExtension::Sign => {
                            extensions.push((args.len() as u32, Attribute::SignExt))
                        }
                        ScalarExtension::Zero => {
                            extensions.push((args.len() as u32, Attribute::ZeroExt))
                        }
                    }
                    types.push(scalar::scalar_type(&self.builder, *scalar));
                    args.push(value);
                }
                // A borrowed byte view: its first byte and its length.
                (
                    ParameterPassing::ByValue,
                    ArgumentValue::Value(value),
                    LayoutKind::Slice { .. },
                ) => {
                    let (data, length) = self.slice_bytes(value)?;
                    types.extend([pointer, self.module.int_type(64)]);
                    args.extend([data, length]);
                }
                // Option[Ptr/FnPtr] is an aggregate internally, one pointer in C.
                (
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::TransferAddress,
                    ArgumentValue::Value(value),
                    _,
                ) if hirsel_lir::abi::foreign_scalar(self.inputs.layouts, parameter.layout)
                    .is_some() =>
                {
                    let place = self.stack(parameter.layout)?;
                    self.store(self.value_shape(value)?, self.get(value)?, place);
                    types.push(pointer);
                    args.push(self.builder.load(pointer, place.ptr, place.align));
                }
                // A borrowed record: the address of a copy C only reads.
                (
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::PointerLength,
                    ArgumentValue::Value(value),
                    _,
                ) => {
                    let place = self.stack(parameter.layout)?;
                    self.store(self.value_shape(value)?, self.get(value)?, place);
                    types.push(pointer);
                    args.push(place.ptr);
                }
                // `mut`: the address of the caller's storage, valid for the call.
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { address, .. }, _) => {
                    types.push(pointer);
                    let place = self.place(address)?;
                    args.push(place.ptr);
                    for offset in hirsel_lir::abi::foreign_nonnull_offsets(
                        self.inputs.layouts,
                        parameter.layout,
                    ) {
                        let at = self.field_place(place, offset, parameter.layout, true)?;
                        written.push(at);
                    }
                }
                _ => return self.invalid("an extern argument outside the C convention"),
            }
        }
        let result = match &abi.result {
            Some(result) => Some(
                hirsel_lir::abi::foreign_scalar(self.inputs.layouts, result.layout).ok_or_else(
                    || CodegenError::Invalid {
                        definition: self.definition().clone(),
                        what: "an extern result that is not a C scalar".into(),
                    },
                )?,
            ),
            None => None,
        };
        let returns = match result {
            Some(scalar) if aarch64_narrow(scalar, self.inputs.target.c_convention).is_some() => {
                self.module.int_type(64)
            }
            Some(scalar) => scalar::scalar_type(&self.builder, scalar),
            None => self.module.void_type(),
        };
        let ty = self.module.function_type(returns, &types);
        let function = self
            .module
            .function(&name, ty)
            .map_err(|what| CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Foreign,
                definition: Some(symbol.definition.clone()),
                abi: Some(abi_digest(abi)),
            });
        // C may reach any storage it is handed and call back into Hirsel code:
        // deferred counts are stored and must-unique facts dropped first.
        self.flush_lengths(true)?;
        self.invalidate_uniqueness();
        // The runtime dispatches the call to its effective thread (S-13), where a callback C
        // makes during the call finds the run context.
        let word = self.module.int_type(64);
        let cell = self.glue_cell(
            &hirsel_lir::glue::foreign_dispatch_symbol(&symbol.definition),
            word,
        );
        let dispatch = self.builder.load(word, cell, 8);
        let enter = self.runtime(RuntimeEntry::ForeignEnter)?;
        let call_site = self.anchor_code(site.anchor)?;
        let previous = self.prologue.alloca(
            self.module.array_type(
                self.module.int_type(8),
                hirsel_lir::runtime_abi::FOREIGN_SCOPE_BYTES,
            ),
            hirsel_lir::runtime_abi::FOREIGN_SCOPE_ALIGN,
        );
        self.builder.call(
            enter,
            &[self.context, self.const_u64(call_site), previous, dispatch],
        );
        let returned = self.builder.call(function, &args);
        self.module
            .add_call_attribute(returned, AttributeSlot::Function, Attribute::NoUnwind);
        for (index, attribute) in extensions {
            self.module
                .add_call_attribute(returned, AttributeSlot::Parameter(index), attribute);
        }
        if let Some(scalar) = result
            && self.inputs.target.c_convention != CConvention::Aapcs64
        {
            match hirsel_lir::abi::extension_of(scalar) {
                ScalarExtension::None => {}
                ScalarExtension::Sign => self.module.add_call_attribute(
                    returned,
                    AttributeSlot::Return,
                    Attribute::SignExt,
                ),
                ScalarExtension::Zero => self.module.add_call_attribute(
                    returned,
                    AttributeSlot::Return,
                    Attribute::ZeroExt,
                ),
            }
        }
        for place in written {
            let pointer = self.builder.load(pointer, place.ptr, place.align);
            self.foreign_nonnull(pointer, call_site)?;
        }
        if abi.result.as_ref().is_some_and(|r| {
            self.inputs
                .layouts
                .layout(r.layout)
                .is_some_and(|r| r.kind == LayoutKind::Scalar(ScalarKind::Pointer))
        }) {
            self.foreign_nonnull(returned, call_site)?;
        }
        let leave = self.runtime(RuntimeEntry::ForeignLeave)?;
        self.builder.call(leave, &[previous]);
        if let Some(value) = site.result {
            if result.is_none() {
                return self.invalid("a call result without an extern result");
            }
            let returned = if let Some(kind) = result
                && aarch64_narrow(kind, self.inputs.target.c_convention).is_some()
            {
                self.builder.cast(
                    Cast::Trunc,
                    returned,
                    scalar::scalar_type(&self.builder, kind),
                )
            } else {
                returned
            };
            let shape = self.value_shape(value)?;
            let returned = if matches!(shape, Shape::Bytes { .. }) {
                let place = self.stack(self.value_type(value)?.layout)?;
                self.builder.store(returned, place.ptr, place.align);
                self.load(shape, place)
            } else {
                returned
            };
            self.define(value, returned)?;
        }
        let Some(normal) = normal else {
            return self.invalid("an extern call without a normal edge");
        };
        self.jump(normal)
    }

    fn foreign_nonnull(&mut self, pointer: Value<'m>, site: u64) -> Emitted {
        let violation = self.runtime(RuntimeEntry::ForeignViolation)?;
        nonnull(
            &self.builder,
            self.function,
            pointer,
            violation,
            self.const_u64(site),
        );
        Ok(())
    }
}

/// Check a C-owned pointer before any use as a Hirsel non-null value.
fn nonnull<'m>(
    builder: &Builder<'m>,
    function: Function<'m>,
    pointer: Value<'m>,
    violation: Function<'m>,
    site: Value<'m>,
) {
    let module = builder.module();
    let bad = module.append_block(function);
    let next = module.append_block(function);
    let zero = module.const_zero(module.ptr_type());
    let is_null = builder.icmp(IntPredicate::Eq, pointer, zero);
    builder.cold_cond_br(is_null, bad, next);
    builder.position(bad);
    builder.call(violation, &[site]);
    builder.unreachable();
    builder.position(next);
}

/// Declares the runtime entry `entry` for module-level code and records its import.
fn runtime_function<'m>(
    module: &'m Module<'m>,
    manifested: &mut Manifested,
    entry: RuntimeEntry,
    ty: Type<'m>,
) -> Function<'m> {
    let symbol = entry.declaration().symbol;
    let function = module
        .function(symbol, ty)
        .expect("a runtime entry is declared with its one type");
    crate::runtime::substrate(module, function, entry);
    manifested.import(symbol, ObjectSymbolKind::Runtime, None);
    function
}

/// Defines `name`, the C callback thunk of the definition entry `target` (PROG-4):
/// `R name(void *ctx, ...)` in the platform C convention. It takes the run context of its
/// thread's extern call, passes `ctx` back as the definition's `mut` context address and
/// the other arguments by value, and returns the definition's scalar result, or nothing.
/// A failure in the definition ends the process.
pub(super) fn define_callback_thunk<'m>(
    module: &'m Module<'m>,
    manifested: &mut Manifested,
    layouts: &LayoutTable,
    convention: CConvention,
    name: &str,
    target: Function<'m>,
    signature: &Signature<'m, '_>,
    definition: &DefPath,
) -> Result<Function<'m>, CodegenError> {
    let abi = signature.abi;
    let unsupported = |what: &str| CodegenError::Unsupported {
        definition: definition.clone(),
        what: format!("a C callback {what}"),
    };
    if abi.shape != CallableShape::Definition || abi.environment || !signature.roots.is_empty() {
        return Err(unsupported("of a definition with an environment or roots"));
    }
    let Some((context, rest)) = abi.parameters.split_first() else {
        return Err(unsupported("without a context parameter"));
    };
    if context.passing != ParameterPassing::ExclusiveAddress {
        return Err(unsupported("whose context is not a `mut` address"));
    }
    let ptr = module.ptr_type();
    let pointer_align = context.slots[0].align;
    let builder = Builder::new(module);
    let mut types = vec![ptr];
    for parameter in rest {
        if !matches!(
            parameter.passing,
            ParameterPassing::ByValue
                | ParameterPassing::ReadOnlyAddress
                | ParameterPassing::TransferAddress
        ) || parameter.slots.len() != 1
        {
            return Err(unsupported("parameter that is not one C scalar"));
        }
        let kind = hirsel_lir::abi::foreign_scalar(layouts, parameter.layout)
            .ok_or_else(|| unsupported("parameter that is not one C scalar"))?;
        let narrow = aarch64_narrow(kind, convention).is_some();
        types.push(if narrow {
            module.int_type(64)
        } else {
            scalar::scalar_type(&builder, kind)
        });
    }
    let c_result = abi
        .result
        .as_ref()
        .and_then(|r| hirsel_lir::abi::foreign_scalar(layouts, r.layout));
    let returns = match c_result {
        Some(kind) if aarch64_narrow(kind, convention).is_some() => module.int_type(64),
        Some(kind) => scalar::scalar_type(&builder, kind),
        None => module.void_type(),
    };
    let thunk = module
        .function(name, module.function_type(returns, &types))
        .map_err(|what| CodegenError::Invalid {
            definition: definition.clone(),
            what,
        })?;
    module.add_attribute(thunk, AttributeSlot::Function, Attribute::NoUnwind);
    let enter = runtime_function(
        module,
        manifested,
        RuntimeEntry::CallbackContext,
        module.function_type(ptr, &[]),
    );
    let failed = runtime_function(
        module,
        manifested,
        RuntimeEntry::CallbackFailed,
        module.function_type(module.void_type(), &[ptr]),
    );
    let violation = runtime_function(
        module,
        manifested,
        RuntimeEntry::ForeignViolation,
        module.function_type(module.void_type(), &[module.int_type(64)]),
    );
    let leave = runtime_function(
        module,
        manifested,
        RuntimeEntry::CallbackLeave,
        module.function_type(module.void_type(), &[ptr]),
    );
    builder.position(module.append_block(thunk));
    let run = builder.call(enter, &[]);
    let mut arguments = Vec::new();
    let mut result_storage = None;
    for hidden in &signature.hidden {
        match hidden {
            Hidden::Context => arguments.push(run),
            Hidden::CallerSite | Hidden::OperationSite => {
                arguments.push(module.const_zero(module.int_type(64)))
            }
            Hidden::Destination => arguments.push(module.const_zero(ptr)),
            // A unit result has no bytes; anything else returns in a register.
            Hidden::Result => match abi.result.as_ref().and_then(|r| layouts.layout(r.layout)) {
                Some(layout) if layout.kind == LayoutKind::Unit => {
                    arguments.push(builder.alloca(module.int_type(8), 1))
                }
                Some(_) if c_result == Some(ScalarKind::Pointer) => {
                    let storage = builder.alloca(ptr, pointer_align);
                    arguments.push(storage);
                    result_storage = Some(storage);
                }
                _ => return Err(unsupported("result that is not a scalar")),
            },
            Hidden::Environment => return Err(unsupported("with an environment")),
        }
    }
    let callback_site = module.const_int(module.int_type(64), 0);
    nonnull(
        &builder,
        thunk,
        module.parameter(thunk, 0),
        violation,
        callback_site,
    );
    arguments.push(module.parameter(thunk, 0));
    for (index, parameter) in rest.iter().enumerate() {
        let raw = module.parameter(thunk, index as u32 + 1);
        let kind = layouts.layout(parameter.layout).map(|l| &l.kind);
        let value = if let Some(LayoutKind::Scalar(kind)) = kind
            && aarch64_narrow(*kind, convention).is_some()
        {
            builder.cast(Cast::Trunc, raw, scalar::scalar_type(&builder, *kind))
        } else {
            raw
        };
        if matches!(kind, Some(LayoutKind::Scalar(ScalarKind::Pointer))) {
            nonnull(&builder, thunk, value, violation, callback_site);
        }
        let value = if matches!(
            parameter.passing,
            ParameterPassing::ReadOnlyAddress | ParameterPassing::TransferAddress
        ) {
            let storage = builder.alloca(ptr, pointer_align);
            builder.store(value, storage, pointer_align);
            storage
        } else {
            value
        };
        arguments.push(value);
    }
    let returned = builder.call(target, &arguments);
    signature.attribute_call(module, returned);
    let status = if signature.returns_status() {
        returned
    } else {
        builder.extract(returned, signature.status_field)
    };
    let failure = module.append_block(thunk);
    let normal = module.append_block(thunk);
    let zero = module.const_zero(module.int_type(8));
    let nonzero = builder.icmp(IntPredicate::Ne, status, zero);
    builder.cond_br(nonzero, failure, normal);
    builder.position(failure);
    builder.call(failed, &[run]);
    builder.unreachable();
    builder.position(normal);
    // The callback's Hirsel code is done: its compute permit goes back before C resumes.
    builder.call(leave, &[run]);
    match signature.value {
        Some((kind, field)) => {
            let word = builder.extract(returned, field);
            let word_ty = value_word(module, kind);
            let value = decode_word(&builder, kind, word, word_ty);
            let value = if let Some(signed) = aarch64_narrow(kind, convention) {
                builder.cast(
                    if signed { Cast::SExt } else { Cast::ZExt },
                    value,
                    module.int_type(64),
                )
            } else {
                value
            };
            builder.ret(value);
        }
        None => match result_storage {
            Some(storage) => builder.ret(builder.load(ptr, storage, pointer_align)),
            None => builder.ret_void(),
        },
    }
    Ok(thunk)
}

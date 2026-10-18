//! Calls of `extern "c"` functions (PROG-4): a direct call of the C symbol in
//! the platform convention, outside the Hirsel status protocol. A foreign call
//! cannot fail in Hirsel terms, so it always continues on its normal edge.
use super::*;

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
                    args.push(self.get(value)?);
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
                    args.push(self.place(address)?.ptr);
                }
                _ => return self.invalid("an extern argument outside the C convention"),
            }
        }
        let result = match &abi.result {
            Some(result) => match self.layout(result.layout)?.kind {
                LayoutKind::Scalar(scalar) => Some(scalar),
                _ => return self.invalid("an extern result that is not a scalar"),
            },
            None => None,
        };
        let returns = match result {
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
        // A callback C makes during the call finds this thread's run context.
        let enter = self.runtime(RuntimeEntry::ForeignEnter)?;
        self.builder.call(enter, &[self.context]);
        let returned = self.builder.call(function, &args);
        self.module
            .add_call_attribute(returned, AttributeSlot::Function, Attribute::NoUnwind);
        for (index, attribute) in extensions {
            self.module
                .add_call_attribute(returned, AttributeSlot::Parameter(index), attribute);
        }
        if let Some(scalar) = result {
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
        if let Some(value) = site.result {
            if result.is_none() {
                return self.invalid("a call result without an extern result");
            }
            self.define(value, returned)?;
        }
        let Some(normal) = normal else {
            return self.invalid("an extern call without a normal edge");
        };
        self.jump(normal)
    }
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
    let mut types = vec![ptr];
    for (index, parameter) in rest.iter().enumerate() {
        if parameter.passing != ParameterPassing::ByValue || parameter.slots.len() != 1 {
            return Err(unsupported("parameter that is not one scalar"));
        }
        types.push(signature.types[signature.first[index + 1] as usize]);
    }
    let builder = Builder::new(module);
    let returns = match signature.value {
        Some((kind, _)) => scalar::scalar_type(&builder, kind),
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
    builder.position(module.append_block(thunk));
    let run = builder.call(enter, &[]);
    let mut arguments = Vec::new();
    for hidden in &signature.hidden {
        match hidden {
            Hidden::Context => arguments.push(run),
            Hidden::Destination => arguments.push(module.const_zero(ptr)),
            // A unit result has no bytes; anything else returns in a register.
            Hidden::Result => match abi.result.as_ref().and_then(|r| layouts.layout(r.layout)) {
                Some(layout) if layout.kind == LayoutKind::Unit => {
                    arguments.push(builder.alloca(module.int_type(8), 1))
                }
                _ => return Err(unsupported("result that is not a scalar")),
            },
            Hidden::Environment => return Err(unsupported("with an environment")),
        }
    }
    arguments.push(module.parameter(thunk, 0));
    arguments.extend((1..types.len() as u32).map(|i| module.parameter(thunk, i)));
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
    match signature.value {
        Some((kind, field)) => {
            let word = builder.extract(returned, field);
            let word_ty = value_word(module, kind);
            builder.ret(decode_word(&builder, kind, word, word_ty));
        }
        None => builder.ret_void(),
    }
    Ok(thunk)
}

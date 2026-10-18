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

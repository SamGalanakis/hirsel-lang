use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// A callable value: the code pointer, then the environment block.
    pub(super) fn callable(
        &self,
        layout: LayoutId,
        code: Value<'m>,
        environment: Value<'m>,
    ) -> Emitted<Value<'m>> {
        if self.layout_shape(layout)? == Shape::Callable {
            let value = self.module.const_zero(self.llvm_type(Shape::Callable));
            let value = self.builder.insert(value, code, 0);
            return Ok(self.builder.insert(value, environment, 1));
        }
        let place = self.stack(layout)?;
        let word = u64::from(self.inputs.target.pointer_bytes);
        self.builder
            .memset(place.ptr, 0, self.fixed_size(layout)?, place.align);
        let iterator = matches!(self.layout(layout)?.kind, LayoutKind::Iterator { .. });
        let code_at = self.field_place(place, if iterator { word } else { 0 }, layout, false)?;
        let env_at = self.field_place(place, if iterator { 0 } else { word }, layout, false)?;
        self.builder.store(code, code_at.ptr, code_at.align);
        self.builder.store(environment, env_at.ptr, env_at.align);
        Ok(self.load(self.layout_shape(layout)?, place))
    }

    /// The code pointer and environment block of a callable value.
    pub(super) fn callable_parts(&self, callable: ValueId) -> Emitted<(Value<'m>, Value<'m>)> {
        let layout = self.value_type(callable)?.layout;
        if self.layout_shape(layout)? == Shape::Callable {
            let value = self.get(callable)?;
            return Ok((
                self.builder.extract(value, 0),
                self.builder.extract(value, 1),
            ));
        }
        let place = self.stack(layout)?;
        self.store(self.value_shape(callable)?, self.get(callable)?, place);
        let word = u64::from(self.inputs.target.pointer_bytes);
        let ptr_ty = self.module.ptr_type();
        let iterator = matches!(self.layout(layout)?.kind, LayoutKind::Iterator { .. });
        let code_at = self.field_place(place, if iterator { word } else { 0 }, layout, false)?;
        let code = self.builder.load(ptr_ty, code_at.ptr, code_at.align);
        let env_at = self.field_place(place, if iterator { 0 } else { word }, layout, false)?;
        let environment = self.builder.load(ptr_ty, env_at.ptr, env_at.align);
        Ok((code, environment))
    }

    /// Imports a function symbol the object calls or refers to, unless the object defines it.
    pub(super) fn import_function(
        &mut self,
        name: &str,
        definition: &DefPath,
        abi: Option<ContentHash>,
    ) {
        self.manifested
            .imports
            .entry(name.to_owned())
            .or_insert_with(|| ObjectSymbol {
                name: name.to_owned(),
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi,
            });
    }

    /// `FunctionRef`: the definition's function-reference thunk with the static empty
    /// environment, a Plain static callable.
    pub(super) fn function_ref(&mut self, result: ValueId, symbol: SymbolId) -> Emitted<Value<'m>> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a function reference outside the symbol table");
        };
        // At a pointer scalar, the definition's C callback thunk (PROG-4).
        if self.layout(self.value_type(result)?.layout)?.kind
            == LayoutKind::Scalar(ScalarKind::Pointer)
        {
            let name = callback_thunk_name(&symbol.definition, symbol.instance.as_ref());
            self.import_function(&name, &symbol.definition, None);
            self.manifested
                .kept
                .insert(symbol_name(&symbol.definition, symbol.instance.as_ref()));
            return Ok(self.module.function_address(&name));
        }
        let name = function_thunk_name(&symbol.definition, symbol.instance.as_ref());
        self.import_function(&name, &symbol.definition, None);
        let code = self.module.function_address(&name);
        let environment = self.empty_environment()?;
        let layout = self.value_type(result)?.layout;
        self.callable(layout, code, environment)
    }

    /// `Closure`: a counted heap environment at the site's `environment()` layout holding the
    /// captures (a borrowed capture as its view, a `mut` capture as its address, a copied one
    /// with a new credit), or the static empty environment when it has no bytes.
    pub(super) fn closure(&mut self, result: ValueId, closure: ClosureId) -> Emitted<Value<'m>> {
        let unit = self.unit;
        let Some(site) = unit.tables.closures.get(closure.index()) else {
            return self.invalid("a closure outside the closure table");
        };
        let wrapper = if let Some(receipts) = &site.receipts {
            let code = site.code.ok_or_else(|| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "receipt closure has no code ABI".into(),
            })?;
            let abi = self
                .inputs
                .layouts
                .fn_abis
                .get(code.abi.0 as usize)
                .ok_or_else(|| crate::CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: "receipt closure has a foreign code ABI".into(),
                })?;
            receipts
                .check_transport(site, abi, unit, self.inputs.layouts)
                .map_err(|what| crate::CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: what.into(),
                })?;
            Some(
                hirsel_lir::receipt_closure::transport_environment(
                    self.inputs.layouts,
                    site.environment,
                    abi.receipts.as_ref().unwrap(),
                )
                .map_err(|what| crate::CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: what.into(),
                })?,
            )
        } else {
            None
        };
        // A generic nested function's value has no code of its own (LIR
        // schema revision 9): its code word is null, and bound calls name
        // each instantiation's code.
        let code = match site.code {
            Some(c) => {
                let Some(symbol) = unit.tables.symbols.get(c.symbol.index()) else {
                    return self.invalid("closure code outside the symbol table");
                };
                let name = symbol_name(&symbol.definition, symbol.instance.as_ref());
                let abi = fn_abi(self.inputs.layouts, c.abi, self.definition())?;
                self.import_function(&name, &symbol.definition, Some(abi_digest(abi)));
                // A bound call may name this same entry later in the unit. Declare
                // its actual ABI now, as Cranelift does, rather than introducing
                // an untyped address placeholder that conflicts with that call.
                Signature::of(self.module, self.inputs.layouts, abi, self.definition())?
                    .declare(
                        self.module,
                        self.inputs.layouts,
                        &name,
                        self.definition(),
                        false,
                        Aliasing::of(self.inputs.callbacks),
                    )?
                    .value
            }
            None => self.module.const_zero(self.module.ptr_type()),
        };
        let layout = self.value_type(result)?.layout;
        let size = self.fixed_size(site.environment)?;
        if size == 0 {
            let environment = self.empty_environment()?;
            return self.callable(layout, code, environment);
        }
        if !matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::Heap(_)))
        ) {
            return self.unsupported("a closure environment outside the heap");
        }
        let record = self.layout(site.environment)?;
        let align = record.align;
        let block = self.alloc_block(site.environment, self.const_u64(size), align, site.anchor)?;
        let payload = self.payload_of(block, site.environment)?;
        self.builder.memset(payload.ptr, 0, size, align);
        let ordinary = if let Some((base, _)) = wrapper {
            self.layout(base)?
        } else {
            record
        };
        let captures = self.pool(site.captures, &unit.pools.captures)?;
        for (i, capture) in captures.iter().enumerate() {
            let Some(field) = ordinary.fields.iter().find(|f| f.ordinal == i as u32) else {
                return self.invalid("a capture without its environment field");
            };
            let at = self.field_place(payload, field.offset, field.layout, field.unaligned)?;
            self.store(
                self.value_shape(capture.value)?,
                self.get(capture.value)?,
                at,
            );
            // A copied uncounted reference (region storage) holds no credit to duplicate.
            if capture.usage == CaptureUse::Copy
                && self.value_type(capture.value)?.repr != ValueRepr::Uncounted
            {
                self.dup_holders(at)?;
            }
        }
        if let LayoutKind::Environment { movable } = &ordinary.kind {
            let field = ordinary.fields.last().expect("live word");
            let at = self.field_place(payload, field.offset, field.layout, field.unaligned)?;
            let bits = u64::MAX >> (64 - movable.len());
            self.store(Shape::Scalar(ScalarKind::U64), self.const_u64(bits), at);
        }
        if let (Some(receipts), Some((_, fields))) = (&site.receipts, wrapper) {
            for (binding, field) in receipts.captures.iter().zip(fields) {
                let at = self.field_place(payload, field.offset, field.layout, field.unaligned)?;
                self.store(
                    Shape::Scalar(ScalarKind::Pointer),
                    self.get(binding.receipt)?,
                    at,
                );
            }
        }
        self.callable(layout, code, block)
    }
}

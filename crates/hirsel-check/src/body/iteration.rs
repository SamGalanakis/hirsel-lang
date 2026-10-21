use super::*;
impl Checker<'_, '_> {
    pub(super) fn cursor_plan(
        &mut self,
        statement: ast::StmtId,
        source: ast::ExprId,
        source_type: TypeId,
        element: TypeId,
        protocol: &ForProtocol,
    ) -> Result<Option<CursorPlan>, QueryError> {
        if self.invalid(element) {
            return Ok(None);
        }
        let written_take = matches!(
            self.input.syntax.tree.expressions[source.0 as usize].kind,
            ast::ExprKind::Unary {
                op: ast::UnaryOp::Take,
                ..
            }
        );
        if matches!(protocol, ForProtocol::Map(_)) && !written_take {
            return Ok(None);
        }
        let mut association = None;
        let declared = if let ForProtocol::BorrowedSource(definition) = protocol {
            let signature = self.snapshot(definition)?;
            match &signature.kind {
                SignatureKind::Type(nominal) => nominal.loop_source.clone().map(|a| {
                    association = Some(ProtocolAssociation {
                        source: definition.clone(),
                        fingerprint: signature.fingerprint,
                    });
                    (a.open, a.next)
                }),
                _ => None,
            }
        } else {
            None
        };
        let (open, next) = if let Some(paths) = declared {
            paths
        } else {
            let label = match protocol {
                ForProtocol::ConsumingSequence(BuiltinType::Array) => "ArrayConsume",
                ForProtocol::ConsumingSequence(BuiltinType::List) => "ListConsume",
                ForProtocol::ConsumingSequence(BuiltinType::Inline) => "InlineConsume",
                ForProtocol::Map(BuiltinType::Map) => "MapInsertionOrder",
                ForProtocol::Set => "SetElements",
                ForProtocol::Map(BuiltinType::SortedMap) => "SortedMapOrder",
                ForProtocol::BorrowedSource(_) => return Ok(None),
                _ => return Ok(None),
            };
            let mut entries = self
                .loop_protocols
                .iter()
                .filter(|(name, _, _)| *name == label);
            let Some((_, open, next)) = entries.next() else {
                return Ok(None);
            };
            if entries.next().is_some() {
                return Err(protocol_error("duplicate cursor protocol mapping"));
            }
            (open.clone(), next.clone())
        };
        let open_signature = self.snapshot(&open)?;
        let SignatureKind::Function(open_function) = &open_signature.kind else {
            return Err(protocol_error("cursor open operation is not a function"));
        };
        let open_function = self.import_function(open_function)?;
        let consuming = matches!(
            protocol,
            ForProtocol::ConsumingSequence(_) | ForProtocol::Map(_)
        );
        let (cursor, open_arguments) = self.specialize_cursor_call(
            &open_function,
            source_type,
            if consuming {
                ast::PassingMode::Take
            } else {
                ast::PassingMode::Borrow
            },
        )?;
        let next_signature = self.snapshot(&next)?;
        let SignatureKind::Function(next_function) = &next_signature.kind else {
            return Err(protocol_error("cursor next operation is not a function"));
        };
        let next_function = self.import_function(next_function)?;
        let (result, next_arguments) =
            self.specialize_cursor_call(&next_function, cursor, ast::PassingMode::Mut)?;
        let TypeKind::Named { arguments, .. } = self.kind(result) else {
            return Err(protocol_error("cursor next must return Option"));
        };
        let [GenericArgument::Type(item)] = arguments.as_slice() else {
            return Err(protocol_error("cursor next must return one item"));
        };
        let item = *item;
        if !self.compatible(item, element) {
            return Err(protocol_error(
                "cursor item does not match the loop element",
            ));
        }
        let step = self.option_layout(result, item)?;
        // Both edges are recorded, including monomorphic opens. Call actuals are
        // the checked source and the open result, never guessed by K.
        self.body.instantiations.extend([
            Instantiation {
                expression: source,
                callee: open.clone(),
                arguments: open_arguments.clone(),
            },
            Instantiation {
                expression: source,
                callee: next.clone(),
                arguments: next_arguments.clone(),
            },
        ]);
        Ok(Some(CursorPlan {
            association,
            statement,
            source_use: if consuming {
                UseKind::Take
            } else {
                UseKind::Borrow
            },
            source,
            open,
            open_arguments,
            next,
            next_arguments,
            cursor,
            step,
        }))
    }
    pub(super) fn declared_loop_item(
        &mut self,
        source_type: TypeId,
    ) -> Result<Option<TypeId>, QueryError> {
        let TypeKind::Named { definition, .. } = self.kind(source_type) else {
            return Ok(None);
        };
        let signature = self.snapshot(&definition)?;
        let SignatureKind::Type(nominal) = &signature.kind else {
            return Ok(None);
        };
        let Some(association) = &nominal.loop_source else {
            return Ok(None);
        };
        let open = self.snapshot(&association.open)?;
        let next = self.snapshot(&association.next)?;
        let (SignatureKind::Function(open_f), SignatureKind::Function(next_f)) =
            (&open.kind, &next.kind)
        else {
            return Err(protocol_error("invalid published association"));
        };
        let open_f = self.import_function(open_f)?;
        let next_f = self.import_function(next_f)?;
        let (cursor, _) =
            self.specialize_cursor_call(&open_f, source_type, ast::PassingMode::Borrow)?;
        let (result, _) = self.specialize_cursor_call(&next_f, cursor, ast::PassingMode::Mut)?;
        let TypeKind::Named {
            definition,
            arguments,
        } = self.kind(result)
        else {
            return Err(protocol_error("invalid published next result"));
        };
        if *definition.path() != association.option {
            return Err(protocol_error("invalid published Option identity"));
        }
        let [GenericArgument::Type(item)] = arguments.as_slice() else {
            return Err(protocol_error("invalid published Option argument"));
        };
        Ok(Some(*item))
    }
    fn specialize_cursor_call(
        &mut self,
        function: &FunctionType,
        actual: TypeId,
        passing: ast::PassingMode,
    ) -> Result<(TypeId, Vec<GenericArgument>), QueryError> {
        let [parameter] = function.parameters.as_slice() else {
            return Err(protocol_error("cursor operations must be unary"));
        };
        if parameter.passing != passing || parameter.label.is_some() {
            return Err(protocol_error(
                "cursor operation has the wrong passing mode or label",
            ));
        }
        let mut inferred = vec![None; function.generics.len()];
        if !self.infer(parameter.ty, actual, &mut inferred)? || inferred.iter().any(Option::is_none)
        {
            return Err(protocol_error(
                "cursor operation does not specialize to its actual",
            ));
        }
        let arguments = inferred.into_iter().flatten().collect::<Vec<_>>();
        for (generic, actual) in function.generics.iter().zip(&arguments) {
            if let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(ty)) =
                (generic, actual)
            {
                for bound in bounds {
                    if !self.bound(*ty, *bound)? {
                        return Err(protocol_error("cursor generic bound is not satisfied"));
                    }
                }
            }
        }
        let result = self.substitute(function.result, &arguments)?;
        Ok((result, arguments))
    }
    pub(super) fn loop_execution(
        &mut self,
        source: ast::ExprId,
        source_type: TypeId,
        element: TypeId,
        protocol: &ForProtocol,
    ) -> Result<ForExecution, QueryError> {
        if self.invalid(element) {
            return Ok(ForExecution::Recovery);
        }
        Ok(match protocol {
            ForProtocol::ConsumingSequence(_) => {
                return Err(protocol_error(
                    "consuming sequence requires its owning cursor operations",
                ));
            }
            ForProtocol::Range { .. } => ForExecution::Range,
            ForProtocol::Bytes => {
                let definition = ast::DefPath::core("bytes.len");
                let result = self.loop_call(source, &definition, vec![])?;
                if !matches!(self.kind(result), TypeKind::Primitive(Primitive::U64)) {
                    return Err(protocol_error("Bytes length must return u64"));
                }
                ForExecution::Indexed {
                    length: LoopLength::Definition { definition, result },
                    read: LoopRead::Index { result: element },
                }
            }
            ForProtocol::Sequence(kind @ (BuiltinType::Array | BuiltinType::List)) => {
                let module = if *kind == BuiltinType::Array {
                    "array"
                } else {
                    "list"
                };
                let definition = ast::DefPath::core(&format!("{module}.len"));
                let TypeKind::Builtin { arguments, .. } = self.kind(source_type) else {
                    unreachable!()
                };
                let result = self.loop_call(source, &definition, arguments.to_vec())?;
                if !matches!(self.kind(result), TypeKind::Primitive(Primitive::U64)) {
                    return Err(protocol_error("sequence length must return u64"));
                }
                ForExecution::Indexed {
                    length: LoopLength::Definition { definition, result },
                    read: LoopRead::Index { result: element },
                }
            }
            ForProtocol::Sequence(BuiltinType::Inline) => {
                let TypeKind::Builtin { arguments, .. } = self.kind(source_type) else {
                    unreachable!()
                };
                let Some(GenericArgument::Size(size)) = arguments.get(1) else {
                    return Err(protocol_error("Inline has no size argument"));
                };
                ForExecution::Indexed {
                    length: LoopLength::Inline {
                        size: size.clone(),
                        result: self.primitive(Primitive::U64),
                    },
                    read: LoopRead::Index { result: element },
                }
            }
            ForProtocol::Iterator => {
                let definition = ast::DefPath::core("iter.next");
                let result =
                    self.loop_call(source, &definition, vec![GenericArgument::Type(element)])?;
                let step = self.option_layout(result, element)?;
                ForExecution::Advance {
                    operation: definition,
                    step,
                }
            }
            ForProtocol::Set | ForProtocol::BorrowedSource(_) => {
                return Err(protocol_error(
                    "loop source requires its registered cursor operations",
                ));
            }
            ForProtocol::Map(kind) => {
                if matches!(
                    self.input.syntax.tree.expressions[source.0 as usize].kind,
                    ast::ExprKind::Unary {
                        op: ast::UnaryOp::Take,
                        ..
                    }
                ) {
                    return Err(protocol_error(
                        "consuming map requires its owning cursor operations",
                    ));
                }
                let protocol = if *kind == BuiltinType::Map {
                    "MapInsertionOrder"
                } else {
                    "SortedMapOrder"
                };
                let definition = ast::DefPath::core(&format!("$intrinsic.{protocol}_len"));
                let TypeKind::Builtin { arguments, .. } = self.kind(source_type) else {
                    unreachable!()
                };
                let arguments = arguments.to_vec();
                let result = self.loop_call(source, &definition, arguments.clone())?;
                if !matches!(self.kind(result), TypeKind::Primitive(Primitive::U64)) {
                    return Err(protocol_error("map length must return u64"));
                }
                let read = if *kind == BuiltinType::Map {
                    let advance = ast::DefPath::core("$intrinsic.MapInsertionOrder_advance");
                    let result = self.loop_call(source, &advance, arguments)?;
                    if !matches!(self.kind(result), TypeKind::Primitive(Primitive::U64)) {
                        return Err(protocol_error("map advance must return u64"));
                    }
                    LoopRead::MapIndex {
                        result: element,
                        advance,
                    }
                } else {
                    LoopRead::Index { result: element }
                };
                ForExecution::Indexed {
                    length: LoopLength::Definition { definition, result },
                    read,
                }
            }
            _ => {
                return Err(protocol_error(
                    "no checked execution protocol for loop source",
                ));
            }
        })
    }
    fn loop_call(
        &mut self,
        source: ast::ExprId,
        definition: &ast::DefPath,
        arguments: Vec<GenericArgument>,
    ) -> Result<TypeId, QueryError> {
        let signature = self.snapshot(definition)?;
        let SignatureKind::Function(function) = &signature.kind else {
            return Err(protocol_error("loop operation is not a declared function"));
        };
        let function = self.import_function(function)?;
        if function.generics.len() != arguments.len() {
            return Err(protocol_error(
                "loop operation generic arity does not match its protocol",
            ));
        }
        for (generic, actual) in function.generics.iter().zip(&arguments) {
            if let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(ty)) =
                (generic, actual)
            {
                for bound in bounds {
                    if !self.bound(*ty, *bound)? {
                        return Err(protocol_error("cursor generic bound is not satisfied"));
                    }
                }
            }
        }
        let result = self.substitute(function.result, &arguments)?;
        self.body.instantiations.push(Instantiation {
            expression: source,
            callee: definition.clone(),
            arguments,
        });
        Ok(result)
    }
    fn option_layout(&mut self, result: TypeId, item: TypeId) -> Result<OptionLayout, QueryError> {
        let TypeKind::Named {
            definition,
            arguments,
        } = self.kind(result)
        else {
            return Err(protocol_error(
                "loop advance does not return a nominal Option",
            ));
        };
        if *definition.path() != ast::DefPath::core("Option")
            || !matches!(arguments.as_slice(),[GenericArgument::Type(t)] if self.compatible(*t,item))
        {
            return Err(protocol_error(
                "loop advance does not return the registered Option of its item",
            ));
        }
        let (_, representation) = self.representation(result)?;
        let TypeKind::Variant(variants) = self.kind(representation) else {
            return Err(protocol_error("Option layout is not a variant"));
        };
        let none = variants
            .iter()
            .position(|v| v.name.as_str() == "None" && v.fields.is_empty());
        let some = variants.iter().position(|v| {
            v.name.as_str() == "Some" && v.fields.len() == 1 && self.compatible(v.fields[0].1, item)
        });
        let (Some(none), Some(some)) = (none, some) else {
            return Err(protocol_error(
                "Option layout lacks its declared None/Some variants",
            ));
        };
        Ok(OptionLayout {
            result,
            item,
            definition: definition.path().clone(),
            none_variant: VariantId(none as u32),
            some_variant: VariantId(some as u32),
        })
    }
}
fn protocol_error(message: &str) -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "loop execution plan".into(),
        message: message.into(),
    })
}

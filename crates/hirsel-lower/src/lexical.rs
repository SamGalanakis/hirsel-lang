//! Lexical callable values desugar to existing closure construction and calls.
use super::builder::{Builder, Value};
use super::*;
use hirsel_tir::*;
use hirsel_types::{NameSite, ParamId, ResolvedName, TypeKind, UseKind};

impl Builder<'_, '_> {
    /// Closure-convert a lexical instantiation. The captured static function
    /// keeps its existing environment; the concrete call specializes its body.
    pub(super) fn lexical_function_value(
        &mut self,
        id: ExprId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Value> {
        let span = self.span(id)?;
        let row = self.decision(id)?.clone();
        let TypeKind::Function(function) = self.types.get(row.ty).unwrap().kind.clone() else {
            return self.error(span, "lexical function value is not callable");
        };
        let owner = NameSite::Expression(id);
        let result = self.decisions.formal_result(&owner, self.types)?;
        let body_id = BodyId(bodies.len() as u32);
        bodies.push(None);
        let info = self
            .decisions
            .closures
            .get(&id)
            .first()
            .copied()
            .ok_or_else(|| LowerError {
                site: span,
                message: "missing lexical callable capture".into(),
            })?;
        let capture_free = info.captures.is_empty();
        let (source, capture_ty, capture_usage, static_body) = if capture_free {
            let name = self
                .decisions
                .names
                .get(&id)
                .first()
                .map(|(_, name)| name)
                .ok_or_else(|| LowerError {
                    site: span,
                    message: "missing lexical name".into(),
                })?;
            let binding = self
                .input
                .checked
                .bindings
                .iter()
                .find(|binding| {
                    &binding.binding == name && matches!(binding.site, NameSite::Statement(_))
                })
                .ok_or_else(|| LowerError {
                    site: span,
                    message: "missing lexical declaration".into(),
                })?;
            let body = bodies
                .iter()
                .position(|body| {
                    body.as_ref().is_some_and(|body| {
                        body.parameter_scope == self.decisions.lexical.site(binding.site)
                    })
                })
                .ok_or_else(|| LowerError {
                    site: span,
                    message: "missing static lexical body".into(),
                })?;
            {
                let (body, ty) = clone_static(
                    bodies,
                    self.types,
                    BodyId(body as u32),
                    binding.ty,
                    span,
                    self.decisions.lexical,
                    self.input.syntax.tree.expressions.len(),
                    &mut self.decisions.cloned_scopes.borrow_mut(),
                )?;
                (None, ty, UseKind::Borrow, Some(body))
            }
        } else {
            let [capture] = info.captures.as_slice() else {
                return self.error(span, "invalid lexical callable capture");
            };
            (
                Some(self.resolved_place(id)?),
                capture.ty,
                capture.usage,
                None,
            )
        };
        let mut capture = source.map(|place| Value {
            checked_ty: None,
            place,
            usage: capture_usage,
        });
        if let Some(capture) = &mut capture {
            self.loan(capture, span);
        }
        let capture_binding =
            source.and_then(|source| self.body.places[source.0 as usize].binding.clone());
        let mut child = Builder::new(self.input, self.decisions, owner, result)?;
        let mut parameters = vec![];
        for (ordinal, parameter) in function.parameters.iter().enumerate() {
            let ty = hirsel_types::bind_parameter_scopes(
                child.types,
                parameter.ty,
                &[self.decisions.lexical.site(owner)],
            )
            .map_err(|e| LowerError {
                site: span,
                message: format!("lexical callable parameter: {e:?}"),
            })?;
            let usage = match parameter.passing {
                hirsel_syntax::PassingMode::Borrow => UseKind::Borrow,
                hirsel_syntax::PassingMode::Mut => UseKind::Mut,
                hirsel_syntax::PassingMode::Take => UseKind::Take,
            };
            let place = child.slot(ty, Storage::Parameter, span, usage == UseKind::Take);
            child.state.live.insert(child.root(place).0);
            child.body.parameters.push(child.root(place));
            child.bindings.push((
                owner,
                ResolvedName::Parameter(ParamId(ordinal as u32)),
                place,
            ));
            child.body.places[place.0 as usize].binding = Some(hirsel_types::Origin {
                root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::body(
                    self.decisions.lexical.site(owner),
                    hirsel_types::ParamId(ordinal as u32),
                )),
                fields: vec![].into(),
            });
            parameters.push(Value {
                checked_ty: None,
                place,
                usage,
            });
        }
        let slot = if let Some(body) = static_body {
            child
                .rvalue(
                    capture_ty,
                    Rvalue::Closure {
                        body,
                        captures: vec![],
                        destination: child.row_destination(capture_ty),
                    },
                    span,
                )
                .place
        } else {
            let slot = child.slot(
                capture_ty,
                Storage::Capture,
                span,
                matches!(capture_usage, UseKind::Copy | UseKind::Take),
            );
            child.state.live.insert(child.root(slot).0);
            child.body.places[slot.0 as usize].binding = capture_binding;
            slot
        };
        let mut arguments = vec![];
        for (ordinal, mut value) in parameters.into_iter().enumerate() {
            child.loan(&mut value, span);
            arguments.push(CallArgument {
                parameter: ParamId(ordinal as u32),
                value: child.consume(value),
            });
        }
        let mut callee = Value {
            checked_ty: None,
            place: slot,
            usage: UseKind::Borrow,
        };
        child.loan(&mut callee, span);
        let returns = if matches!(
            child.types.get(result).unwrap().kind,
            TypeKind::Primitive(hirsel_types::Primitive::Never)
        ) {
            None
        } else {
            let destination = child.slot(result, Storage::Temporary, span, true);
            child.emit(InstructionKind::StorageLive(child.root(destination)), span);
            Some(CallReturn {
                destination,
                next: child.block(span),
            })
        };
        // Keep invocation permissions that the declared result borrows. The
        // ordinary call lowerer publishes the same metadata for last-use flow.
        if let Some(CallReturn { destination, .. }) = returns {
            let relation = &child.types.get(function.result).unwrap().modes.relation;
            let lifetime = match &child.input.arena {
                Some(arena) => relation.retain(|reach| {
                    !matches!(reach, hirsel_types::Reach::Place(origin)
                    if hirsel_types::arena_parameter(child.types, &function, origin, 0, arena))
                }),
                None => relation.clone(),
            };
            let retained = if lifetime.is_global() {
                Some(vec![])
            } else {
                lifetime.named_parameters()
            };
            let parameters = arguments
                .iter()
                .filter(|argument| {
                    retained.as_ref().is_none_or(|selected| {
                        selected.contains(&hirsel_types::ParameterRef::bound(argument.parameter))
                    })
                })
                .map(|argument| argument.parameter.0)
                .collect::<std::collections::BTreeSet<_>>();
            let environment = retained.as_ref().is_none_or(|selected| {
                selected.iter().any(|parameter| {
                    !matches!(
                        parameter,
                        hirsel_types::ParameterRef::Bound { depth: 0, .. }
                    )
                })
            });
            let mut held = arguments
                .iter()
                .filter(|argument| parameters.contains(&argument.parameter.0))
                .flat_map(|argument| child.operand_loans(&argument.value))
                .collect::<Vec<_>>();
            if environment {
                held.extend(child.content_loans(callee.place));
            }
            child
                .call_retention
                .insert(destination.0, (parameters, environment, false));
            child.set_held_loans(destination, held);
        }
        let failure = child.cleanup();
        child.terminate(
            Terminator::Call {
                callee: Callee::Indirect {
                    forward_caller: false,
                    value: Operand {
                        place: callee.place,
                        usage: callee.usage,
                    },
                    arguments: row.instantiated_arguments,
                },
                arguments,
                returns,
                on_failure: failure,
            },
            span,
        );
        if let Some(CallReturn { destination, next }) = returns {
            child.current = Some(next);
            child.state.live.insert(child.root(destination).0);
            child.return_value(
                Value {
                    checked_ty: None,
                    place: destination,
                    usage: UseKind::Take,
                },
                span,
                bodies,
            )?;
        }
        bodies[body_id.0 as usize] = Some(child.finish()?);
        let failure = self.cleanup();
        let captures = capture
            .into_iter()
            .map(|capture| self.consume(capture))
            .collect();
        Ok(self.rvalue_with_failure(
            row.ty,
            Rvalue::Closure {
                body: body_id,
                captures,
                destination: self.row_destination(row.ty),
            },
            span,
            failure,
        ))
    }
}
/// Copy a static closure tree into a new construction site. TIR gives every
/// executable closure body one producer, so a reference needs fresh body IDs
/// and fresh parameter namespaces for each copied body.
fn clone_static(
    bodies: &mut Vec<Option<Body>>,
    types: &hirsel_types::TypeInterner,
    root: BodyId,
    root_ty: TypeId,
    span: Span,
    lexical: hirsel_types::LexicalOwner,
    syntax_expressions: usize,
    cloned_scopes: &mut std::collections::BTreeMap<hirsel_types::SiteRef, hirsel_types::SiteRef>,
) -> Result<(BodyId, TypeId)> {
    use hirsel_types::{BinderRef, GenericArgument, GenericBinder, OriginRoot, ParameterRef, Size};
    use std::collections::BTreeMap;
    let mut copies = BTreeMap::new();
    let mut scopes = BTreeMap::new();
    let mut pending = vec![root];
    while let Some(old) = pending.pop() {
        if copies.contains_key(&old.0) {
            continue;
        }
        let body = bodies[old.0 as usize].as_ref().ok_or_else(|| LowerError {
            site: span,
            message: "unfinished static lexical body".into(),
        })?;
        let new = BodyId(bodies.len() as u32);
        {
            let expression = syntax_expressions
                .checked_add(new.0 as usize)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| LowerError {
                    site: span,
                    message: "lexical scope arena exhausted".into(),
                })?;
            scopes.insert(
                body.parameter_scope,
                lexical.site(NameSite::Expression(ExprId(expression))),
            );
        }
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if let InstructionKind::Assign { value, .. } = instruction.kind
                && let Rvalue::Closure { body: child, .. } = body.rvalues[value.0 as usize]
            {
                pending.push(child);
            }
        }
        copies.insert(old.0, new);
        bodies.push(None);
    }
    cloned_scopes.extend(scopes.iter().map(|(old, new)| (*new, *old)));
    let parameter = |p| match p {
        ParameterRef::Body { scope, parameter } => {
            ParameterRef::body(scopes.get(&scope).copied().unwrap_or(scope), parameter)
        }
        p => p,
    };
    let size = |p| match p {
        BinderRef::Body { scope, parameter } => {
            BinderRef::body(scopes.get(&scope).copied().unwrap_or(scope), parameter)
        }
        p => p,
    };
    let origin = |origin: &mut hirsel_types::Origin| {
        if let OriginRoot::Parameter(p) | OriginRoot::ArgumentBrand(p) = &mut origin.root {
            *p = parameter(*p);
        }
    };
    let mut cache = std::collections::HashMap::new();
    let mut graph = |ty: &mut TypeId| -> Result<()> {
        if let Some(mapped) = cache.get(ty) {
            *ty = *mapped;
            return Ok(());
        }
        let mapped = hirsel_types::map_parameter_references(types, *ty, |p, _| Ok(parameter(p)))
            .and_then(|ty| {
                hirsel_types::map_generic_binders(types, ty, |binder, _| {
                    Ok(match binder {
                        GenericBinder::Body(scope) => {
                            GenericBinder::Body(scopes.get(&scope).copied().unwrap_or(scope))
                        }
                        binder => binder,
                    })
                })
            })
            .map_err(|error| LowerError {
                site: span,
                message: format!("lexical body namespace: {error:?}"),
            })?;
        cache.insert(*ty, mapped);
        *ty = mapped;
        Ok(())
    };
    {
        let mut argument = |argument: &mut GenericArgument| -> Result<()> {
            match argument {
                GenericArgument::Type(ty) => graph(ty),
                GenericArgument::Size(Size::Parameter(p)) => {
                    *p = size(*p);
                    Ok(())
                }
                _ => Ok(()),
            }
        };
        // Map argument roots through the same graph cache as storage roots.
        // Keep this closure's borrow short before mapping body storage.
        for (&old, &new) in &copies {
            let mut body = bodies[old as usize].as_ref().unwrap().clone();
            body.parameter_scope = scopes
                .get(&body.parameter_scope)
                .copied()
                .unwrap_or(body.parameter_scope);
            for rvalue in &mut body.rvalues {
                match rvalue {
                    Rvalue::FunctionReference { arguments, .. } => {
                        for actual in arguments {
                            argument(actual)?;
                        }
                    }
                    Rvalue::Size(p) => *p = size(*p),
                    Rvalue::Closure { body, .. } => *body = copies[&body.0],
                    _ => (),
                }
                if let Rvalue::Closure { destination, .. } | Rvalue::Construct { destination, .. } =
                    rvalue
                {
                    match destination {
                        hirsel_types::Destination::Arena(place) => origin(place),
                        hirsel_types::Destination::Caller(ps) => {
                            for p in ps.iter_mut() {
                                *p = parameter(*p);
                            }
                        }
                        _ => (),
                    }
                }
            }
            for block in &mut body.blocks {
                if let Terminator::Call {
                    callee:
                        Callee::Definition { arguments, .. } | Callee::Indirect { arguments, .. },
                    ..
                } = &mut block.terminator
                {
                    for actual in arguments {
                        argument(actual)?;
                    }
                }
            }
            for tail in &mut body.tail_sites {
                tail.owner_body = copies[&tail.owner_body.0];
            }
            bodies[new.0 as usize] = Some(body);
        }
    }
    for &new in copies.values() {
        let body = bodies[new.0 as usize].as_mut().unwrap();
        graph(&mut body.result)?;
        for local in &mut body.locals {
            graph(&mut local.ty)?;
        }
        for place in &mut body.places {
            graph(&mut place.ty)?;
            if let Some(binding) = &mut place.binding {
                origin(binding);
            }
        }
        for rvalue in &mut body.rvalues {
            if let Rvalue::Shape(ty) = rvalue {
                graph(ty)?;
            }
        }
    }
    let mut ty = root_ty;
    graph(&mut ty)?;
    Ok((copies[&root.0], ty))
}

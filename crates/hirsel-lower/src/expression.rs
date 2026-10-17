use super::builder::{Builder, Value};
use super::*;
use hirsel_syntax::{BinaryOp, ExprKind, Literal, UnaryOp};
use hirsel_tir::*;
use hirsel_types::{
    CallState, CallTarget, ContextualName, NameSite, ParamId, ResolvedName, SourceProjectionKind,
    TypeKind, UseKind,
};

impl Builder<'_, '_> {
    pub(super) fn source_place(&self, id: ExprId) -> Result<hirsel_types::SourcePlace> {
        self.decisions
            .places
            .get(&id)
            .iter()
            .copied()
            .next()
            .cloned()
            .ok_or_else(|| LowerError {
                site: self.input.tree.expressions[id.0 as usize].origin.span,
                message: "missing checked source place".into(),
            })
    }
    fn physical_projection(
        &self,
        id: ExprId,
        fallback: hirsel_types::TypeId,
    ) -> Result<hirsel_types::TypeId> {
        let rows = self.decisions.physical_projections.get(&id);
        match rows {
            [] => Ok(fallback), // Manufactured inputs may already supply physical expression rows.
            [row] if self.types.get(row.1).is_some() => Ok(row.1),
            _ => self.error(self.span(id)?, "invalid physical projection fact"),
        }
    }
    fn checked_projection_place(
        &mut self,
        id: ExprId,
        base: PlaceId,
        index: Option<hirsel_tir::LocalId>,
        ty: hirsel_types::TypeId,
    ) -> Result<PlaceId> {
        let source = self.source_place(id)?;
        let mut parent = self.body.places[base.0 as usize].clone();
        let mut count = 0;
        for step in source.projections.iter().filter(|p| p.expression == id) {
            let projection = match step.kind {
                SourceProjectionKind::Field(field) => Projection::Field(field.0),
                SourceProjectionKind::VariantField { variant, field } => Projection::VariantField {
                    variant: variant.0,
                    field: field.0,
                },
                SourceProjectionKind::Index(_) => {
                    Projection::Index(index.ok_or_else(|| LowerError {
                        site: self.input.tree.expressions[id.0 as usize].origin.span,
                        message: "checked field has index step".into(),
                    })?)
                }
            };
            parent.projection.push(projection);
            parent.binding = parent.binding.and_then(|mut origin| {
                match step.kind {
                    SourceProjectionKind::Field(field)
                    | SourceProjectionKind::VariantField { field, .. } => origin.fields.push(field),
                    SourceProjectionKind::Index(_) => return None,
                }
                Some(origin)
            });
            count += 1;
        }
        if count == 0 {
            return self.error(self.span(id)?, "missing checked projection");
        }
        let place = self.place(
            parent.root,
            parent.projection,
            self.physical_projection(id, ty)?,
        );
        self.body.places[place.0 as usize].loan = parent.loan;
        self.body.places[place.0 as usize].binding = parent.binding;
        Ok(place)
    }
    /// The value a chain of transparent blocks yields (spec #50): the
    /// innermost tail, which the consumer receives as if written directly.
    pub(super) fn transparent_value(&self, mut id: ExprId) -> ExprId {
        while let ExprKind::Block {
            tail: Some(tail), ..
        } = &self.input.tree.expressions[id.0 as usize].kind
        {
            id = *tail;
        }
        id
    }
    /// The authority pass's rule for a transparent block's identity.
    pub(super) fn names_outer_storage(&self, id: ExprId) -> bool {
        let ExprKind::Block {
            statements,
            tail: Some(tail),
        } = &self.input.tree.expressions[id.0 as usize].kind
        else {
            return false;
        };
        if matches!(
            self.input.tree.expressions[tail.0 as usize].kind,
            ExprKind::Block { .. }
        ) {
            return self.names_outer_storage(*tail);
        }
        let Ok(place) = self.source_place(*tail) else {
            return false;
        };
        let hirsel_types::SourceRoot::Binding { binding, .. } = &place.root else {
            return false;
        };
        let local = statements.iter().any(|s| {
            matches!(self.input.tree.statements[s.0 as usize].kind,
            hirsel_syntax::StmtKind::Let { pattern, .. }
                if self.decisions.bindings.get(&NameSite::Pattern(pattern)).iter().copied().any(|b| {
                    b.site == NameSite::Pattern(pattern) && b.binding == *binding
                }))
        });
        !local
            && !matches!(
                place.access,
                hirsel_types::SourceAccess::Unique | hirsel_types::SourceAccess::Take
            )
            && !place
                .projections
                .iter()
                .any(|p| matches!(p.kind, SourceProjectionKind::Index(_)))
    }
    pub fn expression(
        &mut self,
        id: ExprId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        if self.current.is_none() {
            self.keep_result = false;
            return Ok(None);
        }
        // Only a call's own destination can be the kept result; `mut` names
        // that destination itself.
        if !matches!(
            self.input.tree.expressions[id.0 as usize].kind,
            hirsel_syntax::ExprKind::Call { .. }
                | hirsel_syntax::ExprKind::Unary {
                    op: hirsel_syntax::UnaryOp::Mut,
                    ..
                }
        ) {
            self.keep_result = false;
        }
        let mut result = self.expression_inner(id, bodies);
        if let Ok(Some(value)) = &mut result {
            self.publish_identity_joins(&NameSite::Expression(id), self.span(id)?)?;
            let checked_ty = self.decision(id)?.ty;
            let checked = self.types.get(checked_ty).unwrap();
            let source = self.types.get(self.ty(value.place)).unwrap();
            // Source places already carry concrete projection/owner identities.
            // Use I's changed mode judgment when sharing refines that value.
            if checked.modes.relation != source.modes.relation
                || checked.modes.uniqueness != source.modes.uniqueness
                || checked.modes.sendability != source.modes.sendability
                || checked.sendability_requirement != source.sendability_requirement
            {
                value.checked_ty = Some(checked_ty);
            }
            // A block whose final expression names storage declared outside
            // it (spec issue #50) has that storage's identity, as the authority
            // pass typed it: it introduces no value of its own to name.
            let lent_tail = self.names_outer_storage(id);
            if !lent_tail
                && !matches!(
                    self.input.tree.expressions[id.0 as usize].kind,
                    ExprKind::Path(_)
                        | ExprKind::Field { .. }
                        | ExprKind::Unary {
                            op: UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut,
                            ..
                        }
                )
            {
                value.place = self.bind_origin(
                    value.place,
                    hirsel_types::Origin {
                        root: hirsel_types::OriginRoot::Expression(id),
                        fields: vec![].into(),
                    },
                    OriginBindingKind::Fresh,
                    self.span(id)?,
                );
            }
        }
        result
    }
    fn expression_inner(
        &mut self,
        id: ExprId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let row = self.decision(id)?.clone();
        if matches!(
            self.input.tree.expressions[id.0 as usize].kind,
            ExprKind::Path(_)
        ) {
            if let Some((_, ResolvedName::Definition(definition))) =
                self.decisions.names.get(&id).iter().copied().next()
            {
                if !self.decisions.constants.get(&id).is_empty()
                    || !self.decisions.materialized.get(&id).is_empty()
                {
                    return Ok(Some(self.rvalue(
                        row.ty,
                        Rvalue::ReadConstant {
                            definition: definition.clone(),
                        },
                        span,
                    )));
                }
            }
        }
        if let Some((_, constant)) = self.decisions.constants.get(&id).iter().copied().next() {
            return Ok(Some(self.rvalue(
                row.ty,
                Rvalue::Constant(constant.clone()),
                span,
            )));
        }
        if let Some((_, value)) = self.decisions.materialized.get(&id).iter().copied().next() {
            if !hirsel_types::semantic_equal(&value.types, value.root.ty, &self.types, row.ty)
                .map_err(|e| LowerError {
                    site: span,
                    message: format!("materialized type graph: {e:?}"),
                })?
            {
                return self.error(
                    span,
                    "materialized root disagrees with checked expression type",
                );
            }
            return self
                .materialized_value(&value.types, &value.root, id, span)
                .map(Some);
        }
        let kind = self.input.tree.expressions[id.0 as usize].kind.clone();
        let value = match kind {
            ExprKind::Literal(literal) => {
                let constant = super::literal::constant(
                    &literal,
                    &self.types.get(row.ty).unwrap().kind,
                    span,
                )?;
                self.rvalue(row.ty, Rvalue::Constant(constant), span)
            }
            ExprKind::Path(_) => {
                // A nullary constructor is still a resolved constructor, never a
                // naming convention such as uppercase first letter.
                let constructor = self
                    .decisions
                    .contextual
                    .get(&NameSite::Expression(id))
                    .iter()
                    .copied()
                    .find(|(site, name)| {
                        *site == NameSite::Expression(id)
                            && matches!(
                                name,
                                ContextualName::Constructor(_) | ContextualName::Variant { .. }
                            )
                    })
                    .map(|(_, n)| n.clone());
                if let Some(ContextualName::Variant { owner, variant }) = constructor {
                    self.rvalue(
                        row.ty,
                        Rvalue::Construct {
                            constructor: Constructor::Variant {
                                definition: owner,
                                variant: variant.0,
                            },
                            fields: vec![],
                            destination: self.row_destination(row.ty),
                        },
                        span,
                    )
                } else if let Some(ContextualName::Constructor(path)) = constructor {
                    self.rvalue(
                        row.ty,
                        Rvalue::Construct {
                            constructor: Constructor::Record(path),
                            fields: vec![],
                            destination: self.row_destination(row.ty),
                        },
                        span,
                    )
                } else if let Some((_, ResolvedName::Size(parameter))) =
                    self.decisions.names.get(&id).iter().copied().next()
                {
                    self.rvalue(row.ty, Rvalue::Size(*parameter), span)
                } else if let Some((_, ResolvedName::Definition(path))) =
                    self.decisions.names.get(&id).iter().copied().next()
                {
                    self.function_reference(id, path.clone(), row.ty)?
                } else {
                    let base = self.resolved_place(id)?;
                    let mut path = self.body.places[base.0 as usize].projection.clone();
                    let source = self.source_place(id)?;
                    path.extend(source.projections.iter().map(|p| match p.kind {
                        SourceProjectionKind::Field(field) => Projection::Field(field.0),
                        SourceProjectionKind::VariantField { variant, field } => {
                            Projection::VariantField {
                                variant: variant.0,
                                field: field.0,
                            }
                        }
                        SourceProjectionKind::Index(_) => {
                            unreachable!("dotted checked path has index")
                        }
                    }));
                    // A projected pattern view is already the physical place.
                    // Its receiving expression modes cannot retype that field.
                    let place = if source.projections.is_empty() {
                        base
                    } else {
                        let place = self.place(
                            self.root(base),
                            path,
                            self.physical_projection(id, row.ty)?,
                        );
                        self.body.places[place.0 as usize].loan =
                            self.body.places[base.0 as usize].loan;
                        self.body.places[place.0 as usize].binding =
                            self.body.places[base.0 as usize].binding.clone().and_then(
                                |mut origin| {
                                    for projection in &source.projections {
                                        match projection.kind {
                                            SourceProjectionKind::Field(field)
                                            | SourceProjectionKind::VariantField {
                                                field, ..
                                            } => origin.fields.push(field),
                                            SourceProjectionKind::Index(_) => return None,
                                        }
                                    }
                                    Some(origin)
                                },
                            );
                        place
                    };
                    Value {
                        checked_ty: None,
                        place,
                        usage: row.usage,
                    }
                }
            }
            ExprKind::Tuple(values) | ExprKind::Sequence(values) => {
                let sequence = matches!(
                    self.input.tree.expressions[id.0 as usize].kind,
                    ExprKind::Sequence(_)
                );
                let mut fields = vec![];
                for (field, value) in values.into_iter().enumerate() {
                    let Some(value) = self.expression(value, bodies)? else {
                        return Ok(None);
                    };
                    let value = self.transport(value, Storage::Temporary, span);
                    fields.push(ConstructField {
                        field: field as u32,
                        value: Operand {
                            place: value.place,
                            usage: value.usage,
                        },
                    });
                }
                let rv = Rvalue::Construct {
                    constructor: if sequence {
                        Constructor::Sequence
                    } else {
                        Constructor::Tuple
                    },
                    fields,
                    destination: self.row_destination(row.ty),
                };
                self.construct(row.ty, rv, span)
            }
            ExprKind::Unary { op, value } => {
                // Signed MIN is a checked literal magnitude, not an overflowing
                // positive scalar followed by negation.
                if op == UnaryOp::Neg
                    && let ExprKind::Literal(Literal::Integer(ref spelling)) =
                        self.input.tree.expressions[value.0 as usize].kind
                {
                    if let Some(bits) =
                        super::literal::signed_min(spelling, &self.types.get(row.ty).unwrap().kind)
                    {
                        self.decision(value)?;
                        return Ok(Some(self.rvalue(
                            row.ty,
                            Rvalue::Constant(Constant::Integer(bits)),
                            span,
                        )));
                    }
                }
                let operand = value;
                let Some(mut value) = self.expression(value, bodies)? else {
                    return Ok(None);
                };
                match op {
                    UnaryOp::Take | UnaryOp::Copy => {
                        if op == UnaryOp::Take {
                            self.emit(InstructionKind::EndOrigin(value.place), span);
                        }
                        // A written ownership operation evaluates its own value.
                        // An enclosing marker consumes that prepared value; it
                        // cannot replace this operation's checked extraction.
                        value.usage = row.usage;
                        self.transport_typed(value, row.ty, Storage::Temporary, span)
                    }
                    UnaryOp::Mut => {
                        // Ownership judged a plain part of a borrowed computed
                        // value a copy (spec issue #50): the callee mutates that
                        // copy, never the storage the value was read from.
                        if value.usage == UseKind::Copy
                            && matches!(
                                self.source_place(operand)?.root,
                                hirsel_types::SourceRoot::Value(_)
                            )
                        {
                            value = self.transport(value, Storage::Temporary, span);
                        }
                        value.usage = row.usage;
                        value
                    }
                    UnaryOp::Unique => {
                        let Some(fact) = self.decisions.unique.get(&id).iter().copied().next()
                        else {
                            return self
                                .error(span, "unique needs I's checked deep-copy/detach plan");
                        };
                        value.usage = fact.action.usage();
                        let _ = self.loan(&mut value, span);
                        let failure = self.cleanup();
                        let value = self.consume(value);
                        let operation = if fact.action == hirsel_types::UniqueAction::AlreadyUnique
                        {
                            Rvalue::Use(value)
                        } else {
                            // Preserve the checked operation: an ordinary Copy
                            // shares blocks and cannot establish deep uniqueness.
                            Rvalue::Unary {
                                op: UnaryOp::Unique,
                                value,
                            }
                        };
                        self.rvalue_with_failure(row.ty, operation, span, failure)
                    }
                    UnaryOp::Box => {
                        let value = self.transport(value, Storage::Temporary, span);
                        let rv = Rvalue::Construct {
                            constructor: Constructor::Box,
                            fields: vec![ConstructField {
                                field: 0,
                                value: Operand {
                                    place: value.place,
                                    usage: value.usage,
                                },
                            }],
                            destination: self.row_destination(row.ty),
                        };
                        self.construct(row.ty, rv, span)
                    }
                    _ => {
                        let _ = self.loan(&mut value, span);
                        let failure = self.cleanup();
                        let value = self.consume(value);
                        self.rvalue_with_failure(row.ty, Rvalue::Unary { op, value }, span, failure)
                    }
                }
            }
            ExprKind::Binary {
                op: BinaryOp::And | BinaryOp::Or,
                left,
                right,
            } => return self.short_circuit(id, left, right, bodies),
            ExprKind::Binary {
                op: BinaryOp::Range | BinaryOp::RangeInclusive,
                ..
            } => return self.error(span, "range requires H's iteration plan"),
            ExprKind::Binary { op, left, right } => {
                let loans_before = self.loan_count;
                let Some(left) = self.expression(left, bodies)? else {
                    return Ok(None);
                };
                let left = self.prepare_operand(left, span);
                let pending = self.evaluation_holders.len();
                self.evaluation_holders.push(Operand {
                    place: left.place,
                    usage: left.usage,
                });
                let Some(right) = self.expression(right, bodies)? else {
                    return Ok(None);
                };
                let right = self.prepare_operand(right, span);
                self.evaluation_holders.truncate(pending);
                let failure = self.cleanup();
                let left = self.consume(left);
                let right = self.consume(right);
                let result = self.rvalue_with_failure(
                    row.ty,
                    Rvalue::Binary { op, left, right },
                    span,
                    failure,
                );
                // Eager arithmetic and comparisons produce owned bits. Their
                // read dependencies finish after the calculation, before any
                // argument temporary or source owner can be cleaned/replaced.
                self.end_new_loans(loans_before, span);
                result
            }
            ExprKind::Call { callee, arguments } => {
                return self.call(id, callee, arguments, bodies);
            }
            ExprKind::Field { base, .. } => {
                let Some(base) = self.expression(base, bodies)? else {
                    return Ok(None);
                };
                let place = self.checked_projection_place(id, base.place, None, row.ty)?;
                Value {
                    checked_ty: None,
                    place,
                    usage: row.usage,
                }
            }
            ExprKind::Index { base, index: _ } => {
                let source = self.source_place(id)?;
                let Some(SourceProjectionKind::Index(index)) = source
                    .projections
                    .iter()
                    .find(|p| {
                        p.expression == id && matches!(p.kind, SourceProjectionKind::Index(_))
                    })
                    .map(|p| p.kind.clone())
                else {
                    return self.error(span, "missing checked index");
                };
                let Some(base) = self.expression(base, bodies)? else {
                    return Ok(None);
                };
                let Some(index) = self.expression(index, bodies)? else {
                    return Ok(None);
                };
                let index = self.transport(index, Storage::Temporary, span);
                // Index is a pinned address component; retain until all projected
                // uses/loans finish, rather than consuming it with the element.
                let place = self.checked_projection_place(
                    id,
                    base.place,
                    Some(self.root(index.place)),
                    row.ty,
                )?;
                Value {
                    checked_ty: None,
                    place,
                    usage: row.usage,
                }
            }
            ExprKind::Block { statements, tail } => {
                return self.source_block(id, statements, tail, bodies);
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => return self.if_expression(id, condition, then_branch, else_branch, bodies),
            ExprKind::Match { value, arms } => {
                return self.match_expression(id, value, arms, bodies);
            }
            ExprKind::Try { value, .. } => return self.propagate(id, value, bodies),
            ExprKind::Closure { body, .. } => self.closure(id, body, bodies)?,
            ExprKind::Return(operand) => {
                let value = match operand {
                    Some(e) => {
                        let Some(v) = self.expression(e, bodies)? else {
                            return Ok(None);
                        };
                        v
                    }
                    None => self.unit(span),
                };
                self.return_value(value, span, bodies)?;
                return Ok(None);
            }
            ExprKind::Break | ExprKind::Continue => {
                let is_break = matches!(
                    self.input.tree.expressions[id.0 as usize].kind,
                    ExprKind::Break
                );
                let Some(lp) = self.loops.last().cloned() else {
                    return self.error(span, "loop exit outside checked loop");
                };
                self.exit_scope(lp.scope, span);
                if is_break {
                    self.loops
                        .last_mut()
                        .unwrap()
                        .exits
                        .push(self.state.clone());
                } else {
                    self.loops
                        .last_mut()
                        .unwrap()
                        .feedback
                        .push(self.state.clone());
                }
                self.terminate(
                    Terminator::Goto(if is_break { lp.exit } else { lp.next }),
                    span,
                );
                return Ok(None);
            }
            ExprKind::Shape(ty) => {
                let Some((_, ty)) = self.decisions.shapes.get(&ty).iter().copied().next() else {
                    return self.error(span, "missing shape type plan");
                };
                self.rvalue(row.ty, Rvalue::Shape(*ty), span)
            }
            ExprKind::Todo(_) => {
                let cleanup = self.cleanup();
                self.terminate(
                    Terminator::Fail {
                        rule: const { hirsel_diag::RuleId::of("LANG-6") },
                        cleanup,
                    },
                    span,
                );
                return Ok(None);
            }
            ExprKind::Before(_) => {
                let place = self
                    .snapshots
                    .iter()
                    .find(|(e, _)| *e == id)
                    .map(|(_, p)| *p)
                    .ok_or_else(|| LowerError {
                        site: span,
                        message: "missing checked entry snapshot".into(),
                    })?;
                return Ok(Some(Value {
                    checked_ty: None,
                    place,
                    usage: UseKind::Copy,
                }));
            }
            ExprKind::Result => {
                let place = self.contract_result.ok_or_else(|| LowerError {
                    site: span,
                    message: "result outside exit check".into(),
                })?;
                return Ok(Some(Value {
                    checked_ty: None,
                    place,
                    usage: UseKind::Borrow,
                }));
            }
            ExprKind::Comptime(_) => {
                return self.error(span, "comptime requires O's materialized constant plan");
            }
            ExprKind::Heap => {
                if !self.decisions.calls.get(&id).iter().copied().any(|p| {
                    p.expression == id
                        && matches!(p.target, CallTarget::Definition(_))
                        && p.arguments.is_empty()
                }) {
                    return self.error(span, "heap requires its checked private operation plan");
                }
                return self.call(id, id, vec![], bodies);
            }
            ExprKind::Error => return self.error(span, "recovery expression reached lowering"),
        };
        Ok(Some(value))
    }
    fn materialized_value(
        &mut self,
        types: &hirsel_types::TypeStore,
        node: &ValueNode,
        expression: ExprId,
        span: Span,
    ) -> Result<Value> {
        enum Work<'a> {
            Enter(&'a ValueNode),
            Construct {
                ty: TypeId,
                constructor: &'a Constructor,
                fields: usize,
            },
        }
        let mut work = vec![Work::Enter(node)];
        let mut values = vec![];
        while let Some(next) = work.pop() {
            match next {
                Work::Enter(node) => {
                    let ty = hirsel_types::import_type(types, node.ty, &mut self.types).map_err(
                        |e| LowerError {
                            site: span,
                            message: format!("materialized type import: {e:?}"),
                        },
                    )?;
                    match &node.kind {
                        ValueKind::FunctionReference(path) => values.push(self.rvalue(
                            ty,
                            Rvalue::FunctionReference {
                                definition: path.clone(),
                                arguments: vec![],
                            },
                            self.span(expression)?,
                        )),
                        ValueKind::Constant(constant) => {
                            values.push(self.rvalue(ty, Rvalue::Constant(constant.clone()), span))
                        }
                        ValueKind::Construct {
                            constructor,
                            fields,
                        } => {
                            work.push(Work::Construct {
                                ty,
                                constructor,
                                fields: fields.len(),
                            });
                            work.extend(fields.iter().rev().map(Work::Enter));
                        }
                    }
                }
                Work::Construct {
                    ty,
                    constructor,
                    fields,
                } => {
                    let inputs = values
                        .split_off(values.len() - fields)
                        .into_iter()
                        .enumerate()
                        .map(|(ordinal, value): (usize, Value)| ConstructField {
                            field: ordinal as u32,
                            value: Operand {
                                place: value.place,
                                usage: UseKind::Take,
                            },
                        })
                        .collect();
                    // Already-owned evaluated data retain source field order and
                    // exactly the same ownership transfer/failure cleanup.
                    values.push(self.construct(
                        ty,
                        Rvalue::Construct {
                            constructor: constructor.clone(),
                            fields: inputs,
                            destination: self.row_destination(ty),
                        },
                        span,
                    ));
                }
            }
        }
        Ok(values.pop().expect("materialized tree has a root"))
    }
    fn prepare_operand(&mut self, mut value: Value, span: Span) -> Value {
        if matches!(value.usage, UseKind::Borrow | UseKind::Mut) {
            // Preserve the checked source place. A nonowned alias temporary
            // has no destruction action and cannot be reused on a backedge.
            let _ = self.loan(&mut value, span);
            value
        } else {
            self.transport(value, Storage::Temporary, span)
        }
    }
    pub fn rvalue_with_failure(
        &mut self,
        ty: TypeId,
        value: Rvalue,
        span: Span,
        failure: CleanupId,
    ) -> Value {
        let destination = self.slot(ty, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let id = RvalueId(self.body.rvalues.len() as u32);
        self.retain_rvalue(destination, &value);
        self.body.rvalues.push(value);
        self.emit_with(
            InstructionKind::Assign {
                destination,
                value: id,
            },
            span,
            failure,
        );
        self.state.live.insert(self.root(destination).0);
        Value {
            checked_ty: None,
            place: destination,
            usage: UseKind::Take,
        }
    }
    fn construct(&mut self, ty: TypeId, rv: Rvalue, span: Span) -> Value {
        let failure = self.cleanup();
        if let Rvalue::Construct { ref fields, .. } = rv {
            for f in fields {
                self.consume(Value {
                    checked_ty: None,
                    place: f.value.place,
                    usage: f.value.usage,
                });
            }
        }
        self.rvalue_with_failure(ty, rv, span, failure)
    }
    fn call(
        &mut self,
        id: ExprId,
        callee_id: ExprId,
        arguments: Vec<hirsel_syntax::Argument>,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let keep_result = std::mem::take(&mut self.keep_result);
        let span = self.span(id)?;
        let ty = self.decision(id)?.ty;
        let mut plans = self.decisions.calls.get(&id).iter().copied();
        let Some(plan) = plans.next() else {
            return self.error(span, "missing checked call plan");
        };
        if plans.next().is_some() {
            return self.error(span, "duplicate checked call plan");
        }
        let plan = plan.clone();
        if plan.state != CallState::Complete {
            return self.error(span, "recovery call reached lowering");
        }
        if arguments
            .iter()
            .map(|a| a.value)
            .ne(plan.arguments.iter().map(|a| a.expression))
        {
            return self.error(span, "checked call changed source argument order");
        }
        let ordinals = plan
            .arguments
            .iter()
            .map(|a| a.parameter.as_ref().map(|(i, _)| *i))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| LowerError {
                site: span,
                message: "missing checked call parameter".into(),
            })?;
        let pending = self.evaluation_holders.len();
        let mut retain_callee_access = false;
        let (callee, constructor) = match plan.target {
            CallTarget::Definition(path) => (
                Some(Callee::Definition {
                    path,
                    arguments: plan.generic_arguments,
                }),
                None,
            ),
            CallTarget::Constructor(name) => (
                None,
                Some(
                    super::checked::constructor(&name).ok_or_else(|| LowerError {
                        site: span,
                        message: "invalid checked constructor".into(),
                    })?,
                ),
            ),
            CallTarget::Value(_) => {
                let Some(mut value) = self.expression(callee_id, bodies)? else {
                    return Ok(None);
                };
                // A borrowed result of an owned erased environment keeps its
                // source read loan. Scoped local callable views instead keep
                // their capture dependencies; their own slot may end sooner.
                let can_retain_access = self
                    .types
                    .get(self.ty(value.place))
                    .unwrap()
                    .modes
                    .relation
                    .is_global();
                // Generic callees are static lexical producers, without a
                // runtime value (LANG-10/#42), so cannot be snapshotted.
                // A scoped projected share retains its existing content-proof
                // path; this proof only replaces owned global implicit shares.
                let stable = !plan.generic_arguments.is_empty()
                    || ((value.usage != UseKind::Copy || can_retain_access)
                        && self.callee_is_stable(value.place, &arguments));
                if stable && value.usage == UseKind::Copy {
                    // A projected callable's implicit share is only an
                    // invocation holder. A read loan supplies that holder when
                    // the original place remains stable; explicit copies have
                    // already evaluated into owned temporaries.
                    value.usage = UseKind::Borrow;
                }
                if value.usage == UseKind::Borrow && !stable && !arguments.is_empty() {
                    // LANG-3 evaluates the callable before its arguments. An
                    // owned share keeps that environment even if an argument
                    // replaces the source slot; the invocation borrows only
                    // this snapshot, retaining its capture dependencies.
                    // Unproved calls keep the existing owned snapshot.
                    let mut row = self
                        .types
                        .get(value.checked_ty.unwrap_or_else(|| self.ty(value.place)))
                        .unwrap()
                        .clone();
                    row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
                    let ty = self.types.push(row);
                    value.usage = UseKind::Copy;
                    value =
                        self.transport_typed(value, ty, Storage::Temporary, self.span(callee_id)?);
                    value.usage = UseKind::Borrow;
                }
                retain_callee_access = stable
                    && can_retain_access
                    && plan.generic_arguments.is_empty()
                    && value.usage == UseKind::Borrow;
                let value = self.prepare_operand(value, self.span(callee_id)?);
                if stable
                    && plan.generic_arguments.is_empty()
                    && value.usage == UseKind::Borrow
                    && !self.body.places[value.place.0 as usize]
                        .projection
                        .is_empty()
                {
                    // A bounds/tag failure selecting the callable precedes all
                    // arguments (LANG-3). Read under the source loan now, into
                    // a disposable nonowning view. EndScope retires that view
                    // without a release; invocation still uses the stable place.
                    let parent = self.scope;
                    let scope = ScopeId(self.body.scopes.len() as u32);
                    self.body.scopes.push(Scope {
                        parent: Some(parent),
                        span: self.span(callee_id)?,
                    });
                    self.scope = scope;
                    let observed = self.transport(value, Storage::Temporary, self.span(callee_id)?);
                    self.state.live.remove(&self.root(observed.place).0);
                    self.emit(InstructionKind::EndScope(scope), self.span(callee_id)?);
                    self.scope = parent;
                }
                self.evaluation_holders.push(Operand {
                    place: value.place,
                    usage: value.usage,
                });
                (
                    Some(Callee::Indirect {
                        value: Operand {
                            place: value.place,
                            usage: value.usage,
                        },
                        arguments: plan.generic_arguments,
                    }),
                    None,
                )
            }
        };
        self.decision(callee_id)?;
        if ordinals.len() != arguments.len() {
            return self.error(span, "call plan arity mismatch");
        }
        let mut seen = std::collections::BTreeSet::new();
        if ordinals
            .iter()
            .any(|o| *o as usize >= arguments.len() || !seen.insert(*o))
        {
            return self.error(span, "call plan ordinals are not a permutation");
        }
        let mut prepared = vec![];
        let mut argument_dependencies = Vec::new();
        let mut global_arguments = std::collections::BTreeSet::new();
        for (a, ordinal) in arguments.iter().zip(&ordinals) {
            let actual = self.types.get(self.decision(a.value)?.ty).unwrap();
            // A copied or moved arena value keeps only its arena bound: the
            // read loan on its temporary ends at return (P2).
            let computed = matches!(
                self.input.tree.expressions[a.value.0 as usize].kind,
                ExprKind::Unary {
                    op: UnaryOp::Copy | UnaryOp::Take,
                    ..
                }
            );
            if actual.modes.relation.is_global()
                || (computed && self.lifetime(&actual.modes.relation).is_global())
            {
                global_arguments.insert(hirsel_types::ParameterRef::bound(ParamId(*ordinal)));
            }
        }
        // The parameters whose arguments the declared result keeps; `None`
        // keeps them all. The checked callable uses parameter ordinals,
        // independently of argument evaluation order and of the result's
        // instantiated local access mode.
        let lifetime = self.lifetime(&self.types.get(ty).unwrap().modes.relation);
        let retained: Option<std::sync::Arc<[hirsel_types::ParameterRef]>> = if lifetime.is_global()
        {
            Some(Default::default())
        } else if let hirsel_types::TypeKind::Function(f) =
            &self.types.get(self.decision(callee_id)?.ty).unwrap().kind
        {
            // The callee's own Arena parameters place the result; they
            // are not lifetime sources the result retains (PROV-4).
            let relation = &self.types.get(f.result).unwrap().modes.relation;
            let lifetime = match &self.input.arena {
                Some(arena) => relation.retain(|r| {
                    !matches!(r, hirsel_types::Reach::Place(o)
                            if hirsel_types::arena_parameter(self.types, f, o, 0, arena))
                }),
                None => relation.clone(),
            };
            if lifetime.is_global() {
                Some(Default::default())
            } else {
                lifetime.named_parameters().map(|ps| {
                    ps.into_iter()
                        .filter(|p| !global_arguments.contains(p))
                        .collect::<Vec<_>>()
                        .into()
                })
            }
        } else {
            None
        };
        // A result related only to a field path of a parameter reads only
        // that path of its argument (#77).
        let related = match &self.types.get(self.decision(callee_id)?.ty).unwrap().kind {
            hirsel_types::TypeKind::Function(f) if constructor.is_none() => {
                Some(self.types.get(f.result).unwrap().modes.relation.clone())
            }
            _ => None,
        };
        let mut field_loans = std::collections::BTreeMap::new();
        let keeps = |parameter: u32| {
            retained.as_ref().is_none_or(|selected| {
                selected.contains(&hirsel_types::ParameterRef::bound(ParamId(parameter)))
            })
        };
        let keeping = self.keeping;
        for (a, ordinal) in arguments.into_iter().zip(ordinals) {
            // A constructor stores its fields; a call keeps the arguments its
            // result selects. Others end with this call.
            let kept = keeping && (constructor.is_some() || keeps(ordinal));
            let lent = constructor.is_none()
                && self
                    .decisions
                    .arguments
                    .get(&(id, a.value))
                    .iter()
                    .any(|r| matches!(r.2, UseKind::Borrow | UseKind::Mut));
            self.keeping = kept;
            self.keep_result = kept && lent;
            let value = self.expression(a.value, bodies);
            self.keeping = keeping;
            self.keep_result = false;
            let Some(mut value) = value? else {
                return Ok(None);
            };
            value.usage = self.decision(a.value)?.usage;
            if self
                .decisions
                .tails
                .get(&id)
                .iter()
                .copied()
                .any(|t| t.call == id && t.owned_loop_arguments.iter().any(|(_, e)| *e == a.value))
            {
                // The source expression has already performed its explicit copy.
                // Transport its owned temporary instead of copying that owner again.
                value.usage = UseKind::Take;
            }
            let consumer = if constructor.is_none() {
                let mut uses = self
                    .decisions
                    .arguments
                    .get(&(id, a.value))
                    .iter()
                    .copied()
                    .filter(|r| r.0 == id && r.1 == a.value);
                let Some((_, _, usage)) = uses.next() else {
                    return self.error(span, "call operand requires one ownership consumer use");
                };
                if uses.next().is_some() {
                    return self.error(span, "call operand requires one ownership consumer use");
                }
                Some(*usage)
            } else {
                None
            };
            // Owned evaluation (including an explicit copy or take) happens
            // before borrowing the prepared value. A borrowed source preserves
            // its storage identity for mut and local-result provenance.
            let borrowed = matches!(value.usage, UseKind::Borrow | UseKind::Mut);
            // A prepared argument the callee borrows and a binding keeps is
            // released like a named local declared before that binding.
            let storage = if kept
                && constructor.is_none()
                && matches!(consumer, Some(UseKind::Borrow | UseKind::Mut))
            {
                Storage::Local
            } else {
                Storage::Temporary
            };
            let at = if storage == Storage::Local {
                self.keep_span
            } else {
                self.span(a.value)?
            };
            let mut value = if constructor.is_none() && borrowed {
                value
            } else {
                self.transport(value, storage, at)
            };
            if constructor.is_none() {
                value.usage = consumer.unwrap_or(value.usage);
                // A `mut` argument names a place: its exclusive loan begins
                // when the callee is entered, so later arguments may still
                // read the storage (spec #66).
                if value.usage != UseKind::Mut {
                    let _ = self.loan(&mut value, self.span(a.value)?);
                }
            }
            let path = related.as_ref().and_then(|relation| {
                relation.named_field_path(&hirsel_types::ParameterRef::bound(ParamId(ordinal)))
            });
            let field = match path {
                Some(path) if kept && value.usage == UseKind::Borrow => {
                    self.field_loan(a.value, &value, &path, self.span(a.value)?)
                }
                _ => None,
            };
            argument_dependencies.push((
                ordinal,
                self.operand_loans(&Operand {
                    place: field.unwrap_or(value.place),
                    usage: value.usage,
                }),
            ));
            if let Some(field) = field {
                field_loans.insert(ordinal, field);
            }
            self.evaluation_holders.push(Operand {
                place: value.place,
                usage: value.usage,
            });
            prepared.push((ordinal, value, a.value));
        }
        let evaluation_holders = self.evaluation_holders.split_off(pending);
        let mut entered = Vec::with_capacity(prepared.len());
        for (index, (ordinal, mut value, source)) in prepared.into_iter().enumerate() {
            if constructor.is_none() && value.usage == UseKind::Mut {
                let _ = self.loan(&mut value, self.span(source)?);
                argument_dependencies[index].1 = self.operand_loans(&Operand {
                    place: value.place,
                    usage: value.usage,
                });
            }
            entered.push((ordinal, value));
        }
        let prepared = entered;
        if let Some(constructor) = constructor {
            if let Some(fact) = self
                .decisions
                .facts
                .get(&id)
                .iter()
                .copied()
                .find(|f| f.expression == id)
                .cloned()
            {
                if prepared.len() != 1 {
                    return self.error(span, "facted constructor requires its one checked base");
                }
                self.fact_check(&fact, prepared[0].1, span)?;
            }
            let fields = prepared
                .into_iter()
                .map(|(field, v)| ConstructField {
                    field,
                    value: Operand {
                        place: v.place,
                        usage: v.usage,
                    },
                })
                .collect();
            let rv = Rvalue::Construct {
                constructor,
                fields,
                destination: self.row_destination(ty),
            };
            return Ok(Some(self.construct(ty, rv, span)));
        }
        let arguments: Vec<CallArgument> = prepared
            .into_iter()
            .map(|(o, value)| CallArgument {
                parameter: ParamId(o),
                value: self.consume(value),
            })
            .collect();
        let callee = callee.unwrap();
        let callee_dependencies = match &callee {
            // Global results end the callable loan after invocation.
            // A Local result also keeps the source access permission: its
            // borrowed payload may live in an owned erased environment.
            Callee::Indirect { value, .. } if retain_callee_access => self.operand_loans(value),
            Callee::Indirect { value, .. } => self.content_loans(value.place),
            _ => vec![],
        };
        if let Callee::Indirect { ref value, .. } = callee {
            self.consume(Value {
                checked_ty: None,
                place: value.place,
                usage: value.usage,
            });
        }
        // H published eligibility using source lifetimes and declaration facts.
        if self.owner == self.input.owner {
            if let Some(tail) = self.decisions.tails.get(&id).iter().copied().next() {
                let owned_loop_arguments = tail
                    .owned_loop_arguments
                    .iter()
                    .map(|(ordinal, _)| {
                        arguments
                            .iter()
                            .find(|a| a.parameter == *ordinal)
                            .map(|a| (*ordinal, a.value.place))
                            .ok_or_else(|| LowerError {
                                site: span,
                                message: "missing checked tail argument".into(),
                            })
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.body.tail_sites.push(TailSite {
                    owner_body: super::entry_body(),
                    call_block: self.current.unwrap(),
                    kind: match &tail.kind {
                        hirsel_types::CheckedTailKind::SelfTail => TailKind::SelfTail,
                        hirsel_types::CheckedTailKind::ConstructorHole(path) => {
                            TailKind::ConstructorHole(path.clone())
                        }
                    },
                    owned_loop_arguments,
                });
            }
        }
        // A Never-typed call diverges: no result slot and no normal successor
        // (TIR v3). A diverging self-tail site above becomes a loop.
        if matches!(
            self.types.get(ty).unwrap().kind,
            hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::Never)
        ) {
            let on_failure = self.cleanup();
            for argument in &arguments {
                self.retire_operand_content(&argument.value);
            }
            self.terminate(
                Terminator::Call {
                    callee,
                    arguments,
                    returns: None,
                    on_failure,
                },
                span,
            );
            self.current = None;
            return Ok(None);
        }
        let (storage, at) = if keep_result {
            (Storage::Local, self.keep_span)
        } else {
            (Storage::Temporary, span)
        };
        let destination = self.slot(ty, storage, at, true);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let next = self.block(span);
        let on_failure = self.cleanup();
        for argument in &arguments {
            self.retire_operand_content(&argument.value);
        }
        self.terminate(
            Terminator::Call {
                callee,
                arguments,
                returns: Some(CallReturn {
                    destination: destination,
                    next: next,
                }),
                on_failure,
            },
            span,
        );
        self.current = Some(next);
        self.state.live.insert(self.root(destination).0);
        if !field_loans.is_empty() {
            self.call_field_loans.insert(destination.0, field_loans);
        }
        // End evaluation loans that the declared result does not retain.
        self.call_retention.insert(
            destination.0,
            (
                argument_dependencies
                    .iter()
                    .map(|(parameter, _)| *parameter)
                    .filter(|parameter| {
                        retained.as_ref().is_none_or(|selected| {
                            selected
                                .contains(&hirsel_types::ParameterRef::bound(ParamId(*parameter)))
                        })
                    })
                    .collect(),
                retained.as_ref().is_none_or(|selected| {
                    selected.iter().any(|parameter| {
                        !matches!(
                            parameter,
                            hirsel_types::ParameterRef::Bound { depth: 0, .. }
                        )
                    })
                }),
                retain_callee_access,
            ),
        );
        let held = argument_dependencies
            .into_iter()
            .filter(|(parameter, _)| {
                retained.as_ref().is_none_or(|selected| {
                    selected.contains(&hirsel_types::ParameterRef::bound(ParamId(*parameter)))
                })
            })
            .flat_map(|(_, held)| held)
            .collect();
        let mut held: Vec<_> = held;
        if retained.as_ref().is_none_or(|selected| {
            selected.iter().any(|parameter| {
                !matches!(
                    parameter,
                    hirsel_types::ParameterRef::Bound { depth: 0, .. }
                )
            })
        }) {
            held.extend(callee_dependencies);
        }
        self.set_held_loans(destination, held);
        if retained.is_some() {
            // Evaluated owners last through their statement, including owners
            // named only by a result's brand. End invocation access now, while
            // their installed capture dependencies remain held until cleanup.
            for value in evaluation_holders {
                let root = self.root(value.place);
                if self.owned[root.0 as usize]
                    && self.state.live.contains(&root.0)
                    && self.body.locals[root.0 as usize].storage == Storage::Temporary
                {
                    self.evaluation_holders.push(Operand {
                        place: value.place,
                        usage: UseKind::Take,
                    });
                }
            }
            self.end_unheld_loans(span);
            self.evaluation_holders.truncate(pending);
        }
        Ok(Some(Value {
            checked_ty: None,
            place: destination,
            usage: UseKind::Take,
        }))
    }
    fn function_reference(&mut self, id: ExprId, path: DefPath, ty: TypeId) -> Result<Value> {
        let actuals = self
            .decisions
            .instantiations
            .get(&id)
            .iter()
            .copied()
            .find(|i| i.expression == id && i.callee == path)
            .map(|i| i.arguments.clone())
            .unwrap_or_else(|| self.decision(id).unwrap().instantiated_arguments.clone());
        Ok(self.rvalue(
            ty,
            Rvalue::FunctionReference {
                definition: path,
                arguments: actuals,
            },
            self.span(id)?,
        ))
    }
    fn closure(
        &mut self,
        id: ExprId,
        expression: ExprId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Value> {
        self.closure_for(id, expression, NameSite::Expression(id), bodies)
    }
    pub fn closure_for(
        &mut self,
        id: ExprId,
        expression: ExprId,
        owner: NameSite,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Value> {
        let span = self.span(id)?;
        let Some(info) = self
            .decisions
            .closures
            .get(&id)
            .iter()
            .copied()
            .next()
            .cloned()
        else {
            return self.error(span, "missing checked capture table");
        };
        let TypeKind::Function(_) = &self.types.get(info.ty).unwrap().kind else {
            return self.error(span, "closure does not have function type");
        };
        let result = self
            .decisions
            .formal_result(&owner, &self.input.checked.types)?;
        let body_id = BodyId(bodies.len() as u32);
        bodies.push(None);
        let mut capture_rows = vec![];
        let mut captures = vec![];
        for (ordinal, capture) in info.captures.into_iter().enumerate() {
            let mut rows = self
                .decisions
                .captures
                .get(&(id, ordinal as u32))
                .iter()
                .copied()
                .filter(|row| row.expression == id && row.ordinal as usize == ordinal);
            let Some(row) = rows.next() else {
                return self.error(span, "missing scoped capture owner");
            };
            if rows.next().is_some() {
                return self.error(span, "duplicate scoped capture owner");
            }
            let owner = row.owner;
            let place = self.lookup(&capture.binding, &owner, span)?;
            let mut value = Value {
                checked_ty: None,
                place,
                usage: capture.usage,
            };
            let _ = self.loan(&mut value, span);
            captures.push(Operand {
                place: value.place,
                usage: capture.usage,
            });
            capture_rows.push((owner, capture));
        }
        let mut child = Builder::new(self.input, self.decisions, owner, result, self.types)?;
        for (owner, capture) in capture_rows {
            let owned = !matches!(capture.usage, UseKind::Borrow | UseKind::Mut);
            let slot = child.slot(capture.ty, Storage::Capture, span, owned);
            child.state.live.insert(child.root(slot).0);
            child.body.places[slot.0 as usize].binding =
                Self::binding_origin(owner, &capture.binding);
            child.bindings.push((owner, capture.binding, slot));
        }
        child.entry_checks(bodies)?;
        if let Some(value) = child.expression(expression, bodies)? {
            child.return_value(value, span, bodies)?;
        }
        bodies[body_id.0 as usize] = Some(child.finish()?);
        let failure = self.cleanup();
        for operand in &captures {
            self.consume(Value {
                checked_ty: None,
                place: operand.place,
                usage: operand.usage,
            });
        }
        let destination = self.row_destination(info.ty);
        Ok(self.rvalue_with_failure(
            info.ty,
            Rvalue::Closure {
                body: body_id,
                captures,
                destination,
            },
            span,
            failure,
        ))
    }
}

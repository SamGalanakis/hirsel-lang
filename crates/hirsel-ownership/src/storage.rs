//! Source-owned storage equations, published beside the completed mode result.
//! This reads only this body's checked ownership CFG and declaration snapshots;
//! it never demands a callee's inferred body facts inside a mode query.
use crate::{cfg, properties, source_rows};
use hirsel_syntax::{self as ast, DefId, ExprId, ExprKind, ItemSyntax};
use hirsel_types::storage::graph::*;
use hirsel_types::storage::{StorageCallable, StorageInput, StorageResult, StorageSelector};
use hirsel_types::*;
use std::sync::Arc;

pub(super) fn derive(
    types: &TypeInterner,
    syntax: &ItemSyntax,
    signature: &Signature,
    checked: &mut CheckedBody,
    graphs: &[cfg::Graph],
    rows: &source_rows::SourceRows,
    properties: &properties::PropertyTable,
    signatures: &mut properties::Signatures,
) -> Result<StorageDefinition, QueryError> {
    let mut producer = Producer {
        types,
        syntax,
        signature,
        checked,
        rows,
        properties,
        signatures,
    };
    let bodies = graphs
        .iter()
        .filter(|graph| graph.clause.is_none())
        .map(|graph| producer.body(graph))
        .collect::<Result<_, _>>()?;
    Ok(StorageDefinition { bodies })
}

struct Producer<'a, 'q> {
    types: &'a TypeInterner,
    syntax: &'a ItemSyntax,
    signature: &'a Signature,
    checked: &'a mut CheckedBody,
    rows: &'a source_rows::SourceRows,
    properties: &'a properties::PropertyTable,
    signatures: &'a mut properties::Signatures<'q>,
}
impl Producer<'_, '_> {
    fn callable(&self, owner: NameSite) -> StorageCallable {
        StorageCallable {
            definition: self.signature.definition,
            scope: self.syntax.owner().site(owner),
        }
    }
    fn expression(&self, expression: ExprId) -> StoragePlace {
        StoragePlace::root(expression.0)
    }
    fn binding(&self, row: usize) -> StoragePlace {
        StoragePlace::root(self.syntax.tree.expressions.len() as u32 + row as u32)
    }
    fn ty(&self, expression: ExprId) -> Option<TypeId> {
        self.rows
            .expression(expression)
            .map(|row| self.checked.expressions[row].ty)
    }
    fn detaches(&self, ty: Option<TypeId>) -> bool {
        ty.and_then(|ty| self.properties.get(ty))
            .is_some_and(|property| property.valid && property.copy_detaches)
    }
    fn source_place(&self, expression: ExprId) -> Option<StoragePlace> {
        let place = &self.checked.places[self.rows.source_place(expression)?];
        let mut source = match &place.root {
            SourceRoot::Binding { owner, binding } => {
                self.binding(self.rows.binding(self.checked, Some(*owner), binding)?)
            }
            SourceRoot::Value(value) if *value != expression => self.expression(*value),
            _ => return None,
        };
        source.parts.extend(
            place
                .projections
                .iter()
                .map(|projection| match projection.kind {
                    SourceProjectionKind::Field(field) => OwnedTypePart::Field(field.0),
                    SourceProjectionKind::VariantField { variant, field } => {
                        OwnedTypePart::VariantField {
                            variant: variant.0,
                            field: field.0,
                        }
                    }
                    SourceProjectionKind::Index(_) => OwnedTypePart::Elements,
                }),
        );
        Some(source)
    }
    fn origin(&self, origin: &Origin, owner: NameSite) -> Option<StoragePlace> {
        let (scope, name) = match origin.root {
            OriginRoot::Parameter(ParameterRef::Body { scope, parameter })
                if scope.owner == self.syntax.owner() =>
            {
                (scope.site, ResolvedName::Parameter(parameter))
            }
            OriginRoot::Parameter(ParameterRef::Bound {
                depth: 0,
                parameter,
            }) => (owner, ResolvedName::Parameter(parameter)),
            OriginRoot::Local(lexical, local) if lexical == self.syntax.owner() => {
                (owner, ResolvedName::Local(local))
            }
            _ => return None,
        };
        let mut place = self.binding(self.rows.binding(self.checked, Some(scope), &name)?);
        place.parts.extend(
            origin
                .fields
                .iter()
                .map(|field| OwnedTypePart::Field(field.0)),
        );
        Some(place)
    }
    fn placement(
        &self,
        expression: ExprId,
        owner: NameSite,
        needs_destination: bool,
    ) -> StoragePlacement {
        let Some(row) = self.rows.expression(expression) else {
            return StoragePlacement::Unknown;
        };
        match self.checked.expressions[row]
            .construction_destination
            .as_ref()
        {
            None if !needs_destination => StoragePlacement::Independent,
            Some(Destination::Heap | Destination::Inferred) => StoragePlacement::Independent,
            Some(Destination::Arena(origin)) => self
                .origin(origin, owner)
                .map(|place| StoragePlacement::Sources(vec![place]))
                .unwrap_or(StoragePlacement::Unknown),
            Some(Destination::Caller(parameters)) => parameters
                .iter()
                .map(|parameter| {
                    self.origin(
                        &Origin {
                            root: OriginRoot::Parameter(*parameter),
                            fields: Default::default(),
                        },
                        owner,
                    )
                })
                .collect::<Option<Vec<_>>>()
                .map(StoragePlacement::Sources)
                .unwrap_or(StoragePlacement::Unknown),
            _ => StoragePlacement::Unknown,
        }
    }
    fn closure(&self, expression: ExprId, graph: &cfg::Graph) -> StorageValue {
        if let Some(value) = self.named_callable(expression) {
            return value;
        }
        let Some(row) = self.rows.closure(expression) else {
            return StorageValue::Unknown;
        };
        let closure = &self.checked.closures[row];
        let captures = closure
            .captures
            .iter()
            .enumerate()
            .map(|(ordinal, capture)| {
                // A nested closure may recapture an outer frame's local. The
                // original capture judgment names that binding's lexical owner;
                // the constructor's current frame can have a different owner.
                let owner = graph
                    .capture_modes
                    .get(expression, ordinal as u32)
                    .map_or(graph.owner, |capture| capture.owner);
                self.rows
                    .binding(self.checked, Some(owner), &capture.binding)
                    .map(|row| self.binding(row))
            })
            .collect::<Option<Vec<_>>>();
        match captures {
            Some(captures) => StorageValue::Callable {
                target: self.callable(NameSite::Expression(expression)),
                captures,
            },
            None => StorageValue::Unknown,
        }
    }
    fn named_callable(&self, expression: ExprId) -> Option<StorageValue> {
        if !self
            .ty(expression)
            .and_then(|ty| self.types.get(ty))
            .is_some_and(|ty| matches!(ty.kind, TypeKind::Function(_)))
        {
            return None;
        }
        let source = self.rows.source_place(expression).and_then(|row| {
            match &self.checked.places[row].root {
                SourceRoot::Definition(path) => Some(path),
                _ => None,
            }
        });
        let path = source.or_else(|| {
            self.checked
                .resolution
                .paths
                .iter()
                .find_map(|(site, name)| match name {
                    ResolvedName::Definition(path) if *site == NameSite::Expression(expression) => {
                        Some(path)
                    }
                    _ => None,
                })
        })?;
        Some(StorageValue::Reference {
            definition: DefId::intern(path),
            arguments: self
                .checked
                .instantiations
                .iter()
                .find(|instantiation| {
                    instantiation.expression == expression && &instantiation.callee == path
                })
                .map(|instantiation| instantiation.arguments.clone())
                .unwrap_or_default(),
        })
    }
    fn body(&mut self, graph: &cfg::Graph) -> Result<StorageBody, QueryError> {
        let types: Vec<_> = self
            .checked
            .expressions
            .iter()
            .map(|row| row.ty)
            .chain(self.checked.bindings.iter().map(|row| row.ty))
            .collect();
        let mut structures = std::collections::HashMap::new();
        for ty in types {
            if structures.contains_key(&ty) {
                continue;
            }
            let structure =
                hirsel_types::storage::storage_type_paths(self.types, ty, &mut |definition| {
                    self.signatures
                        .read(
                            definition,
                            Arc::make_mut(&mut self.checked.resolution.reads),
                        )
                        .map(Some)
                })?;
            structures.insert(ty, structure);
        }
        let paths = structures
            .values()
            .flat_map(|structure| structure.paths.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut inputs = Vec::new();
        for (row, binding) in self
            .checked
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.owner == graph.owner)
        {
            if let ResolvedName::Parameter(parameter) = binding.binding {
                inputs.push(StorageInputBinding {
                    input: StorageInput::Parameter(ParameterRef::bound(parameter)),
                    place: self.binding(row),
                    mutable: binding.usage == UseKind::Mut,
                    holds_storage: !self.detaches(Some(binding.ty)),
                    callables: structures
                        .get(&binding.ty)
                        .map(|structure| structure.callables.iter().cloned().collect())
                        .unwrap_or_default(),
                });
            }
        }
        for (owner, name, ordinal) in &graph.captures {
            let Some(row) = self.rows.binding(self.checked, Some(*owner), name) else {
                continue;
            };
            let ty = self.checked.bindings[row].ty;
            // The original lexical binding is the symbolic capture input in
            // this CFG; it is not a fictitious parameter of the nested body.
            inputs.push(StorageInputBinding {
                input: StorageInput::Capture(*ordinal as u32),
                mutable: false,
                place: self.binding(row),
                holds_storage: !self.detaches(Some(ty)),
                callables: structures
                    .get(&ty)
                    .map(|structure| structure.callables.iter().cloned().collect())
                    .unwrap_or_default(),
            });
        }
        let ty = match graph.owner {
            NameSite::Item => match &self.signature.kind {
                SignatureKind::Function(function) => Some(
                    self.types
                        .intern(Type::with_kind(TypeKind::Function(function.clone()))),
                ),
                _ => None,
            },
            NameSite::Expression(expression) => self.ty(expression),
            NameSite::Statement(statement) => self
                .rows
                .at(NameSite::Statement(statement))
                .next()
                .map(|row| self.checked.bindings[row].ty),
            _ => None,
        };
        let source_type =
            ty.and_then(|ty| encoding::closed_storage_source_type(self.types, ty).ok());
        let nodes = graph
            .nodes
            .iter()
            .map(|node| {
                Ok(StorageNode {
                    operation: self.operation(&node.op, graph)?,
                    successors: node.next.clone(),
                    site: node.span,
                })
            })
            .collect::<Result<_, QueryError>>()?;
        Ok(StorageBody {
            callable: self.callable(graph.owner),
            external_entry: graph.owner == NameSite::Item
                && self.signature.metadata.visibility != ast::Visibility::Private,
            source_type,
            paths,
            generics: ty
                .and_then(|ty| match &self.types.row(ty).kind {
                    TypeKind::Function(function) => Some(function.generics.to_vec()),
                    _ => None,
                })
                .unwrap_or_default(),
            inputs,
            nodes,
            entry: graph.entry,
        })
    }
    fn assign(&self, expression: ExprId, value: StorageValue) -> StorageOp {
        StorageOp::Assign(vec![(self.expression(expression), value)])
    }
    fn copy(&self, value: ExprId, ty: Option<TypeId>) -> StorageValue {
        StorageValue::Copy {
            value: self.expression(value),
            detaches: self.detaches(ty),
        }
    }
    fn operation(
        &mut self,
        operation: &cfg::Op,
        graph: &cfg::Graph,
    ) -> Result<StorageOp, QueryError> {
        use cfg::Op;
        let operation = match operation {
            Op::Use { expression, .. } => {
                let value = self
                    .source_place(*expression)
                    .map_or(StorageValue::Unknown, |value| StorageValue::Copy {
                        value,
                        detaches: self.detaches(self.ty(*expression)),
                    });
                self.assign(*expression, value)
            }
            Op::Forward { expression, child } => {
                self.assign(*expression, self.copy(*child, self.ty(*expression)))
            }
            Op::Finish {
                expression,
                children,
                fresh,
                ..
            } => {
                let expression_ty = self.ty(*expression);
                if matches!(
                    self.syntax.tree.expressions[expression.0 as usize].kind,
                    ExprKind::Path(_)
                ) {
                    let path =
                        self.checked
                            .resolution
                            .expressions
                            .iter()
                            .find_map(|(site, name)| match name {
                                ResolvedName::Definition(path) if site == expression => {
                                    Some(path.clone())
                                }
                                _ => None,
                            });
                    if let Some(path) = path {
                        let definition = DefId::intern(&path);
                        let signature = self.signatures.read(
                            definition,
                            Arc::make_mut(&mut self.checked.resolution.reads),
                        )?;
                        if let SignatureKind::Const { generics, .. } = &signature.kind {
                            // Size-specialized constants need their selected value,
                            // never the unspecialized declaration's initializer.
                            let value = if generics.is_empty() {
                                StorageValue::ConstantReference {
                                    definition,
                                    arguments: vec![],
                                }
                            } else {
                                StorageValue::Unknown
                            };
                            return Ok(self.assign(*expression, value));
                        }
                    }
                }
                let value = if self.detaches(expression_ty) {
                    StorageValue::Fresh
                } else {
                    match &self.syntax.tree.expressions[expression.0 as usize].kind {
                        ExprKind::Unary {
                            op: ast::UnaryOp::Unique,
                            value,
                        } => StorageValue::Unique {
                            value: self.expression(*value),
                            allocates: !self.detaches(self.ty(*value)),
                        },
                        ExprKind::Unary {
                            op: ast::UnaryOp::Box,
                            value,
                        } => StorageValue::Construct {
                            fields: vec![(
                                vec![OwnedTypePart::Field(0)].into(),
                                self.expression(*value),
                            )],
                            placement: self.placement(*expression, graph.owner, true),
                        },
                        ExprKind::Unary { value, .. } => self.copy(*value, expression_ty),
                        ExprKind::Tuple(fields) | ExprKind::Sequence(fields) => {
                            let sequence = matches!(
                                self.syntax.tree.expressions[expression.0 as usize].kind,
                                ExprKind::Sequence(_)
                            );
                            StorageValue::Construct {
                                fields: fields
                                    .iter()
                                    .enumerate()
                                    .map(|(ordinal, field)| {
                                        (
                                            vec![if sequence {
                                                OwnedTypePart::Elements
                                            } else {
                                                OwnedTypePart::Field(ordinal as u32)
                                            }]
                                            .into(),
                                            self.expression(*field),
                                        )
                                    })
                                    .collect(),
                                placement: self.placement(*expression, graph.owner, sequence),
                            }
                        }
                        ExprKind::Literal(_) | ExprKind::Heap | ExprKind::Shape(_) => {
                            StorageValue::Fresh
                        }
                        ExprKind::Path(_) => {
                            self.named_callable(*expression).unwrap_or(if *fresh {
                                StorageValue::Fresh
                            } else {
                                StorageValue::Unknown
                            })
                        }
                        _ => match self.source_place(*expression) {
                            Some(value) => StorageValue::Copy {
                                value,
                                detaches: false,
                            },
                            None if !fresh && children.len() == 1 => {
                                self.copy(children[0], expression_ty)
                            }
                            _ => StorageValue::Unknown,
                        },
                    }
                };
                self.assign(*expression, value)
            }
            Op::Capture { expression, .. } => {
                self.assign(*expression, self.closure(*expression, graph))
            }
            Op::LocalFunction(statement) => {
                let function = self
                    .checked
                    .resolution
                    .local_functions
                    .iter()
                    .find(|(site, _)| site == statement)
                    .map(|(_, local)| *local);
                let binding = function.and_then(|local| {
                    self.rows
                        .binding(self.checked, Some(graph.owner), &ResolvedName::Local(local))
                });
                match binding {
                    Some(row) => {
                        StorageOp::Assign(vec![(self.binding(row), StorageValue::Unknown)])
                    }
                    None => StorageOp::Nop,
                }
            }
            Op::Bind {
                pattern,
                value,
                choice,
                ..
            } => {
                let mut assignments = Vec::new();
                let mut pending = vec![(*pattern, self.expression(*value))];
                while let Some((pattern, source)) = pending.pop() {
                    let Some(decision) = self
                        .checked
                        .patterns
                        .iter()
                        .find(|decision| decision.pattern == pattern)
                    else {
                        continue;
                    };
                    if let PatternDecisionKind::Or(alternatives) = &decision.kind {
                        if let Some(selected) = choice.get(&pattern) {
                            pending.push((*selected, source));
                        } else {
                            pending.extend(
                                alternatives
                                    .iter()
                                    .map(|alternative| (*alternative, source.clone())),
                            );
                        }
                        continue;
                    }
                    for row in self.rows.at(NameSite::Pattern(pattern)) {
                        let ty = self.checked.bindings[row].ty;
                        assignments.push((
                            self.binding(row),
                            StorageValue::Copy {
                                value: source.clone(),
                                detaches: self.detaches(Some(ty)),
                            },
                        ));
                    }
                    let variant = match &decision.kind {
                        PatternDecisionKind::Constructor(ContextualName::Variant {
                            variant,
                            ..
                        }) => Some(*variant),
                        _ => None,
                    };
                    for (field, pattern) in &decision.fields {
                        let mut source = source.clone();
                        source.parts.push(variant.map_or(
                            OwnedTypePart::Field(field.0),
                            |variant| OwnedTypePart::VariantField {
                                variant: variant.0,
                                field: field.0,
                            },
                        ));
                        pending.push((*pattern, source));
                    }
                }
                StorageOp::Assign(assignments)
            }
            Op::Write { place, value, .. } => {
                self.source_place(*place)
                    .map_or(StorageOp::Nop, |destination| {
                        StorageOp::Assign(vec![(destination, self.copy(*value, self.ty(*place)))])
                    })
            }
            Op::Call { expression, .. } => return self.call(*expression, graph),
            Op::Return { value, .. } => StorageOp::Return(self.expression(*value)),
            Op::Propagate { expression, .. } | Op::MapError { expression, .. } => {
                self.assign(*expression, StorageValue::Unknown)
            }
            Op::Kill(locals) => {
                StorageOp::Kill(locals.iter().map(|row| self.binding(*row).local).collect())
            }
            _ => StorageOp::Nop,
        };
        Ok(operation)
    }
    fn map_arguments(&mut self, ty: TypeId) -> Result<Option<Vec<TypeId>>, QueryError> {
        let mut ty = ty;
        if let TypeKind::Named {
            definition,
            arguments,
        } = &self.types.row(ty).kind
        {
            let signature = self.signatures.read(
                *definition,
                Arc::make_mut(&mut self.checked.resolution.reads),
            )?;
            let SignatureKind::Type(declaration) = &signature.kind else {
                return Ok(None);
            };
            let Some(representation) = declaration.representation else {
                return Ok(None);
            };
            let Ok(expanded) = substitute(
                self.types,
                &TypeMemo::new(),
                representation,
                &Substitution {
                    binder: Some(GenericBinder::Bound(0)),
                    arguments,
                    authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                    roles: &signature.metadata.roles,
                },
            ) else {
                return Ok(None);
            };
            ty = expanded;
        }
        Ok(match &self.types.row(ty).kind {
            TypeKind::Builtin {
                kind: BuiltinType::Map,
                arguments,
            } if arguments.len() == 2 => arguments
                .iter()
                .map(|argument| match argument {
                    GenericArgument::Type(ty) => Some(*ty),
                    _ => None,
                })
                .collect(),
            _ => None,
        })
    }
    fn map_callee(
        &mut self,
        path: &ast::DefPath,
        function: &FunctionType,
        result: TypeId,
    ) -> Result<Option<StorageCallee>, QueryError> {
        if function.generics.len() != 2
            || function.incomplete
            || function.foreign.is_some()
            || function.error.is_some()
        {
            return Ok(None);
        }
        let Some(first) = function.parameters.first() else {
            return Ok(None);
        };
        let Some(arguments) = self.map_arguments(first.ty)? else {
            return Ok(None);
        };
        if !matches!(&function.generics[0], GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Key))
            || !matches!(&function.generics[1], GenericParameter::Type { .. })
        {
            return Ok(None);
        }
        if path.is_core("map.insert") {
            let reviewed = function.parameters.len() == 3
                && first.passing == ast::PassingMode::Mut
                && function.parameters[1].passing == ast::PassingMode::Take
                && function.parameters[2].passing == ast::PassingMode::Take
                && self
                    .types
                    .semantic_eq(function.parameters[1].ty, arguments[0])
                && self
                    .types
                    .semantic_eq(function.parameters[2].ty, arguments[1])
                && matches!(&self.types.row(function.result).kind, TypeKind::Tuple(fields) if fields.is_empty())
                && function.allocation == Allocation::MayAllocate;
            return Ok(reviewed.then_some(StorageCallee::MapInsert));
        }
        if function.parameters.len() != 2
            || first.passing != ast::PassingMode::Borrow
            || function.parameters[1].passing != ast::PassingMode::Borrow
            || self.types.row(function.parameters[1].ty).kind != self.types.row(arguments[0]).kind
            || function.allocation != Allocation::NoAlloc
        {
            return Ok(None);
        }
        let TypeKind::Named {
            definition,
            arguments: option_arguments,
        } = &self.types.row(function.result).kind
        else {
            return Ok(None);
        };
        if !definition.path().is_core("Option")
            || option_arguments.as_slice() != [GenericArgument::Type(arguments[1])]
        {
            return Ok(None);
        }
        let option = self.signatures.read(
            *definition,
            Arc::make_mut(&mut self.checked.resolution.reads),
        )?;
        let SignatureKind::Type(declaration) = &option.kind else {
            return Ok(None);
        };
        let Some(representation) = declaration.representation else {
            return Ok(None);
        };
        let TypeKind::Variant(variants) = &self.types.row(representation).kind else {
            return Ok(None);
        };
        if variants.len() != 2
            || variants
                .iter()
                .filter(|variant| variant.name.as_str() == "None" && variant.fields.is_empty())
                .count()
                != 1
        {
            return Ok(None);
        }
        let Some(variant) = variants
            .iter()
            .position(|variant| variant.name.as_str() == "Some" && variant.fields.len() == 1)
        else {
            return Ok(None);
        };
        let reaches = self.types.row(function.result).modes.relation.reaches();
        if reaches.len() != 1
            || !matches!(&reaches[0], Reach::Place(Origin { root: OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: ParamId(0) }), fields }) if fields.is_empty())
        {
            return Ok(None);
        }
        Ok(Some(StorageCallee::MapGet { result_path: vec![OwnedTypePart::VariantField { variant: variant as u32, field: 0 }].into(), requires_code: match &self.types.row(result).kind { TypeKind::Named { arguments, .. } => arguments.iter().any(|argument| matches!(argument, GenericArgument::Type(ty) if matches!(self.types.row(*ty).kind, TypeKind::Function(_)))), _ => false } }))
    }
    fn call(&mut self, expression: ExprId, graph: &cfg::Graph) -> Result<StorageOp, QueryError> {
        let Some(row) = self.rows.call(expression) else {
            return Ok(self.assign(expression, StorageValue::Unknown));
        };
        let call = self.checked.calls[row].clone();
        let arguments = call
            .arguments
            .iter()
            .map(|argument| {
                argument.parameter.as_ref().map(|(ordinal, parameter)| {
                    (
                        ParamId(*ordinal),
                        if parameter.passing == ast::PassingMode::Mut {
                            self.source_place(argument.expression)
                                .unwrap_or_else(|| self.expression(argument.expression))
                        } else {
                            self.expression(argument.expression)
                        },
                    )
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(arguments) = arguments.filter(|_| call.state == CallState::Complete) else {
            return Ok(self.assign(expression, StorageValue::Unknown));
        };
        let callee = match &call.target {
            CallTarget::Value(value) => StorageCallee::Value(self.expression(*value)),
            CallTarget::Constructor(constructor) => {
                let variant = match constructor {
                    ContextualName::Variant { variant, .. } => Some(variant.0),
                    _ => None,
                };
                let fields = arguments
                    .iter()
                    .map(|(field, place)| {
                        (
                            vec![variant.map_or(OwnedTypePart::Field(field.0), |variant| {
                                OwnedTypePart::VariantField {
                                    variant,
                                    field: field.0,
                                }
                            })]
                            .into(),
                            place.clone(),
                        )
                    })
                    .collect();
                return Ok(self.assign(
                    expression,
                    StorageValue::Construct {
                        fields,
                        placement: self.placement(expression, graph.owner, false),
                    },
                ));
            }
            CallTarget::Definition(path) if path.is_core("region.with") => {
                StorageCallee::ReadRegion
            }
            CallTarget::Definition(path) if path.is_core("arena.seal") => StorageCallee::SealArena,
            CallTarget::Definition(path) if path.is_core("arena.with") => {
                StorageCallee::ScopeCallback {
                    callback: ParamId(0),
                    result_path: Default::default(),
                }
            }
            // CONC-2 scope entry invokes this original body with a fresh Scope;
            // it does not infer effects or code from the callback's result type.
            CallTarget::Definition(path) if path.is_core("task.scope") => {
                let signature = self.signatures.read(
                    DefId::intern(path),
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let reviewed = signature.definition.path() == path
                    && match &signature.kind {
                        SignatureKind::Function(function) => {
                            function.generics.len() == 1
                                && matches!(&function.generics[0], GenericParameter::Type { bounds, .. } if bounds.is_empty())
                                && function.parameters.len() == 2
                                && !function.incomplete
                                && function.foreign.is_none()
                                && function.error.is_none()
                                && function.allocation == Allocation::MayAllocate
                                && function.parameters[0].passing == PassingMode::Borrow
                                && matches!(&self.types.row(function.parameters[0].ty).kind, TypeKind::Named { definition, arguments } if definition.path().is_core("Tasks") && arguments.is_empty())
                                && function.parameters[1].passing == PassingMode::Borrow
                                && self.types.row(function.parameters[1].ty).modes.multiplicity
                                    == Multiplicity::Once
                                && matches!(
                                    &self.types.row(function.result).kind,
                                    TypeKind::Parameter(TypeParameterRef::Bound {
                                        depth: 0,
                                        parameter: TypeParamId(0)
                                    })
                                )
                                && matches!(&self.types.row(function.parameters[1].ty).kind,
                                    TypeKind::Function(callback) if callback.generics.is_empty()
                                        && callback.parameters.len() == 1
                                        && callback.parameters[0].passing == PassingMode::Borrow
                                        && matches!(&self.types.row(callback.parameters[0].ty).kind, TypeKind::Named { definition, arguments } if definition.path().is_core("Scope") && arguments.is_empty())
                                        && matches!(&self.types.row(callback.result).kind, TypeKind::Parameter(TypeParameterRef::Bound { depth: 1, parameter: TypeParamId(0) })))
                        }
                        _ => false,
                    };
                if reviewed {
                    StorageCallee::ScopeCallback {
                        callback: ParamId(1),
                        result_path: Default::default(),
                    }
                } else {
                    StorageCallee::Declared {
                        allocations: vec![],
                        results: vec![],
                        complete: false,
                    }
                }
            }
            CallTarget::Definition(path)
                if path.is_core("task.async") || path.is_core("task.await") =>
            {
                let signature = self.signatures.read(
                    DefId::intern(path),
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let SignatureKind::Function(function) = &signature.kind else {
                    return Ok(self.assign(expression, StorageValue::Unknown));
                };
                let reviewed = if path.is_core("task.async") {
                    signature.definition.path() == path
                        && function.generics.len() == 1
                        && function.parameters.len() == 2
                        && !function.incomplete
                        && function.foreign.is_none()
                        && function.error.is_none()
                        && function.allocation == Allocation::MayAllocate
                        && function.parameters[0].passing == PassingMode::Borrow
                        && matches!(&self.types.row(function.parameters[0].ty).kind, TypeKind::Named {definition,arguments} if definition.path().is_core("Scope") && arguments.is_empty())
                        && function.parameters[1].passing == PassingMode::Take
                        && matches!(&function.generics[0],GenericParameter::Type {bounds,..} if bounds.as_slice() == [Bound::Sendable])
                        && self.types.row(function.parameters[1].ty).modes.multiplicity
                            == Multiplicity::Once
                        && self
                            .types
                            .row(function.parameters[1].ty)
                            .modes
                            .relation
                            .is_global()
                        && matches!(&self.types.row(function.parameters[1].ty).kind,TypeKind::Function(callback) if callback.generics.is_empty() && callback.parameters.is_empty()
                && matches!(self.types.row(callback.result).kind,TypeKind::Parameter(TypeParameterRef::Bound {depth:1,parameter:TypeParamId(0)})))
                        && matches!(&self.types.row(function.result).kind, TypeKind::Builtin {kind:BuiltinType::Task,arguments} if matches!(arguments.as_slice(),[GenericArgument::Type(result)] if matches!(self.types.row(*result).kind,TypeKind::Parameter(TypeParameterRef::Bound {depth:0,parameter:TypeParamId(0)}))))
                        && matches!(self.types.row(function.result).brand.alternatives.as_slice(),[Origin {root:OriginRoot::Parameter(ParameterRef::Bound {depth:0,parameter:ParamId(0)}),fields}] if fields.is_empty())
                } else {
                    signature.definition.path() == path
                        && function.generics.len() == 1
                        && function.parameters.len() == 2
                        && !function.incomplete
                        && function.foreign.is_none()
                        && function.error.is_none()
                        && function.allocation == Allocation::MayAllocate
                        && function.parameters[0].passing == PassingMode::Borrow
                        && matches!(&self.types.row(function.parameters[0].ty).kind, TypeKind::Named {definition,arguments} if definition.path().is_core("Scope") && arguments.is_empty())
                        && function.parameters[1].passing == PassingMode::Take
                        && matches!(&function.generics[0],GenericParameter::Type {bounds,..} if bounds.is_empty())
                        && matches!(
                            self.types.row(function.result).kind,
                            TypeKind::Parameter(TypeParameterRef::Bound {
                                depth: 0,
                                parameter: TypeParamId(0)
                            })
                        )
                        && matches!(&self.types.row(function.parameters[1].ty).kind,TypeKind::Builtin {kind:BuiltinType::Task,arguments} if arguments.as_slice() == [GenericArgument::Type(function.result)])
                        && matches!(self.types.row(function.parameters[1].ty).brand.alternatives.as_slice(),[Origin {root:OriginRoot::Parameter(ParameterRef::Bound {depth:0,parameter:ParamId(0)}),fields}] if fields.is_empty())
                };
                if !reviewed {
                    StorageCallee::Declared {
                        allocations: vec![],
                        results: vec![],
                        complete: false,
                    }
                } else if path.is_core("task.async") {
                    StorageCallee::ScopeCallback {
                        callback: ParamId(1),
                        result_path: vec![OwnedTypePart::Elements].into(),
                    }
                } else {
                    StorageCallee::TaskAwait
                }
            }
            CallTarget::Definition(path) if path.is_core("copy_to") => {
                let signature = self.signatures.read(
                    DefId::intern(path),
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let reviewed = signature.definition.path() == path && match &signature.kind {
                    SignatureKind::Function(function) if function.parameters.len() == 2 && function.generics.len() == 1 => {
                        function.parameters.iter().all(|parameter| parameter.passing == ast::PassingMode::Borrow)
                            && signature.metadata.roles.heap.as_ref().is_some_and(|heap| is_arena_type(self.types, function.parameters[0].ty, &heap.arena))
                            && matches!(&function.generics[0], GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Copyable))
                            && matches!((&self.types.row(function.parameters[1].ty).kind, &self.types.row(function.result).kind), (TypeKind::Parameter(left), TypeKind::Parameter(right)) if left == right)
                            && self.types.row(function.result).modes.relation.reaches().iter().any(|reach| matches!(reach,
                                Reach::Place(Origin { root: OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: ParamId(0) }), fields }) if fields.is_empty()))
                    }
                    _ => false,
                };
                if reviewed {
                    StorageCallee::CopyTo {
                        holds_storage: !self.detaches(Some(call.result)),
                        requires_code: matches!(
                            self.types.row(call.result).kind,
                            TypeKind::Function(_)
                        ),
                    }
                } else {
                    StorageCallee::Declared {
                        allocations: vec![],
                        results: vec![],
                        complete: false,
                    }
                }
            }
            CallTarget::Definition(path) if path.is_core("shared.with") => {
                let signature = self.signatures.read(
                    DefId::intern(path),
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let reviewed = signature.definition.path() == path
                    && match &signature.kind {
                        SignatureKind::Function(function) if function.parameters.len() == 2 => {
                            function.parameters[0].passing == ast::PassingMode::Borrow
                                && matches!(
                                    self.types.row(function.parameters[0].ty).kind,
                                    TypeKind::Builtin {
                                        kind: BuiltinType::Shared,
                                        ..
                                    }
                                )
                                && function.parameters[1].passing == ast::PassingMode::Borrow
                                && matches!(&self.types.row(function.parameters[1].ty).kind, TypeKind::Function(callback)
                                if callback.parameters.len() == 1 && callback.parameters[0].passing == ast::PassingMode::Borrow)
                        }
                        _ => false,
                    };
                if !reviewed {
                    return Ok(self.assign(expression, StorageValue::Unknown));
                }
                StorageCallee::ScopeCallback {
                    callback: ParamId(1),
                    result_path: Default::default(),
                }
            }
            CallTarget::Definition(path) if path.is_core("arena.bounded") => {
                let signature = self.signatures.read(
                    DefId::intern(path),
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let result = match &signature.kind {
                    SignatureKind::Function(function) => function.result,
                    _ => return Ok(self.assign(expression, StorageValue::Unknown)),
                };
                let definition = match self.types.row(result).kind {
                    TypeKind::Named { definition, .. } => definition,
                    _ => return Ok(self.assign(expression, StorageValue::Unknown)),
                };
                let result_signature = self.signatures.read(
                    definition,
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let representation = match &result_signature.kind {
                    SignatureKind::Type(declaration) => declaration.representation,
                    _ => None,
                };
                let variant = representation.and_then(|ty| match &self.types.row(ty).kind {
                    TypeKind::Variant(variants) => variants
                        .iter()
                        .enumerate()
                        .find(|(_, variant)| {
                            variant.name.as_str() == "Ok" && variant.fields.len() == 1
                        })
                        .map(|(ordinal, _)| ordinal as u32),
                    _ => None,
                });
                let Some(variant) = variant else {
                    return Ok(self.assign(expression, StorageValue::Unknown));
                };
                StorageCallee::ScopeCallback {
                    callback: ParamId(1),
                    result_path: vec![OwnedTypePart::VariantField { variant, field: 0 }].into(),
                }
            }
            CallTarget::Definition(path) if path.0.package.name == "core" => {
                let signature = self.signatures.read(
                    DefId::intern(path),
                    Arc::make_mut(&mut self.checked.resolution.reads),
                )?;
                let SignatureKind::Function(function) = &signature.kind else {
                    return Ok(self.assign(expression, StorageValue::Unknown));
                };
                if path.is_core("map.insert") || path.is_core("map.get") {
                    let recipe = self.map_callee(path, function, call.result)?;
                    return Ok(StorageOp::Call {
                        mutations: call
                            .arguments
                            .iter()
                            .filter_map(|argument| {
                                argument
                                    .parameter
                                    .as_ref()
                                    .filter(|(_, parameter)| {
                                        parameter.passing == ast::PassingMode::Mut
                                    })
                                    .map(|(ordinal, _)| ParamId(*ordinal))
                            })
                            .collect(),
                        callee: recipe.unwrap_or(StorageCallee::Declared {
                            allocations: vec![],
                            results: vec![],
                            complete: false,
                        }),
                        arguments: call
                            .arguments
                            .iter()
                            .map(|argument| {
                                argument.parameter.as_ref().map(|(ordinal, parameter)| {
                                    (
                                        ParamId(*ordinal),
                                        if parameter.passing == ast::PassingMode::Mut {
                                            self.source_place(argument.expression).unwrap_or_else(
                                                || self.expression(argument.expression),
                                            )
                                        } else {
                                            self.expression(argument.expression)
                                        },
                                    )
                                })
                            })
                            .collect::<Option<Vec<_>>>()
                            .unwrap_or_default(),
                        destination: Some(self.expression(expression)),
                    });
                }
                let mut results = Vec::new();
                // A bodyless result signature does not prove callback effects.
                let mut complete = !function.parameters.iter().any(|parameter| {
                    matches!(self.types.row(parameter.ty).kind, TypeKind::Function(_))
                }) && ![
                    "iter.next",
                    "sorted_map.insert",
                    "sorted_map.get",
                    "sorted_map.remove",
                    "runner.compute",
                    "runner.call",
                    "runner.spawn",
                ]
                .iter()
                .any(|name| path.is_core(name));
                if !self.detaches(Some(call.result)) && !path.is_core("arena.seal") {
                    for reach in self.types.row(function.result).modes.relation.reaches() {
                        match reach {
                            Reach::Place(Origin {
                                root:
                                    OriginRoot::Parameter(
                                        parameter @ ParameterRef::Bound { depth: 0, .. },
                                    ),
                                fields,
                            }) => results.push(StorageResult {
                                parts: Default::default(),
                                source: StorageSelector {
                                    input: StorageInput::Parameter(*parameter),
                                    parts: fields
                                        .iter()
                                        .map(|field| OwnedTypePart::Field(field.0))
                                        .collect(),
                                },
                            }),
                            Reach::Environment | Reach::Region(_) | Reach::Place(_) => {
                                complete = false
                            }
                        }
                    }
                }
                let mut allocations = Vec::new();
                if function.allocation == Allocation::MayAllocate {
                    allocations.extend(results.iter().map(|result| result.source.clone()));
                    allocations.extend(
                        function
                            .parameters
                            .iter()
                            .enumerate()
                            .filter(|(_, parameter)| parameter.passing == ast::PassingMode::Mut)
                            .map(|(ordinal, _)| StorageSelector {
                                input: StorageInput::Parameter(ParameterRef::bound(ParamId(
                                    ordinal as u32,
                                ))),
                                parts: Default::default(),
                            }),
                    );
                }
                StorageCallee::Declared {
                    allocations,
                    results,
                    complete,
                }
            }
            CallTarget::Definition(path) => StorageCallee::Apply {
                definition: DefId::intern(path),
                arguments: call.generic_arguments.clone(),
            },
        };
        Ok(StorageOp::Call {
            mutations: call
                .arguments
                .iter()
                .filter_map(|argument| {
                    argument
                        .parameter
                        .as_ref()
                        .filter(|(_, parameter)| parameter.passing == ast::PassingMode::Mut)
                        .map(|(ordinal, _)| ParamId(*ordinal))
                })
                .collect(),
            callee,
            arguments,
            destination: Some(self.expression(expression)),
        })
    }
}

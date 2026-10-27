//! Source allocation and representation obligations, before TIR lowering.
//! Inputs are producer decisions; this pass does not infer modes or call effects.
use hirsel_diag::{Diagnostic, RuleId};
use hirsel_intern::work::{self, Counter};
use hirsel_syntax::{self as ast, ExprId};
use hirsel_types::*;
use std::collections::{BTreeMap, BTreeSet};

mod allocation;
mod composition;
mod cpu;
mod layout;
mod packed;

pub struct Input<'a> {
    /// The session interner holding every row this audit reads.
    pub types: &'a TypeInterner,
    pub memo: &'a TypeMemo,
    pub syntax: &'a ast::ItemSyntax,
    pub signature: &'a Signature,
    pub call_allocations: &'a [(ExprId, Allocation)],
    pub allocating_copies: &'a [ExprId],
    pub allocating_mutations: &'a [ExprId],
    pub checked: &'a CheckedBody,
    pub loops: &'a [hirsel_check_protocol::body::ForPlan],
}

pub fn check(
    input: Input<'_>,
    provider: &mut dyn SignatureProvider,
) -> Result<QueryOutput<()>, QueryError> {
    let mut audit = Audit {
        source_rows: source_rows::SourceRows::new(input.checked, &input.syntax.tree),
        types: input.types,
        expression_types: input
            .checked
            .expressions
            .iter()
            .map(|row| (row.expression, row.ty))
            .collect(),
        call_parameters: input
            .checked
            .calls
            .iter()
            .flat_map(|call| {
                call.arguments.iter().filter_map(move |argument| {
                    argument
                        .parameter
                        .clone()
                        .map(|parameter| ((call.expression, argument.expression), parameter))
                })
            })
            .collect(),
        input,
        provider,
        snapshots: BTreeMap::new(),
        diagnostics: vec![],
    };
    audit.declaration();
    audit.inline_layout()?;
    audit.layout_composition()?;
    audit.inline_soa()?;
    audit.allocations()?;
    audit.cpu_callbacks()?;
    audit.variant_places()?;
    for row in &audit.input.checked.expressions {
        let expression = &audit.input.syntax.tree.expressions[row.expression.0 as usize];
        if let ast::ExprKind::Index { base, .. } = expression.kind {
            let Some(ty) = audit.expression_type(base) else {
                continue;
            };
            let kind = audit.types.get(ty).unwrap().kind.clone();
            if let TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments,
            } = kind
                && let Some(GenericArgument::Type(element)) = arguments.first()
                && audit.in_flat_projection(base)?
                && !audit.fixed_size(*element, &mut BTreeSet::new())?
            {
                audit.report(
                    const { RuleId::of("MEM-12") },
                    expression.origin.span,
                    "a flat list of variable-size elements is sequential and cannot be indexed",
                );
            }
        }
    }
    // Checked expression membership excludes statements in unrelated definitions.
    for statement in &audit.input.syntax.tree.statements {
        if let ast::StmtKind::Assign { place, .. } = statement.kind
            && let Some(ty) = audit.expression_type(place)
            && audit.in_flat_projection(place)?
            && !audit.fixed_size(ty, &mut BTreeSet::new())?
        {
            audit.report(
                const { RuleId::of("MEM-12") },
                statement.origin.span,
                "variable-size flat fields are fixed at construction",
            );
        }
    }
    Ok(QueryOutput {
        value: (),
        diagnostics: audit.diagnostics,
        dependencies: audit
            .snapshots
            .into_values()
            .map(|s| SignatureDependency {
                definition: s.definition,
                fingerprint: s.fingerprint,
            })
            .collect(),
    })
}

struct Audit<'a, 'p> {
    source_rows: source_rows::SourceRows,
    input: Input<'a>,
    types: &'a TypeInterner,
    expression_types: BTreeMap<ExprId, TypeId>,
    call_parameters: BTreeMap<(ExprId, ExprId), (u32, Parameter)>,
    provider: &'p mut dyn SignatureProvider,
    snapshots: BTreeMap<ast::DefPath, SignatureRef>,
    diagnostics: Vec<Diagnostic>,
}
impl Audit<'_, '_> {
    fn report(&mut self, rule: RuleId, span: hirsel_span::Span, message: &str) {
        self.diagnostics
            .push(Diagnostic::error(rule, span, message));
    }
    fn snapshot(&mut self, definition: &hirsel_syntax::DefId) -> Result<SignatureRef, QueryError> {
        if *definition == self.input.signature.definition {
            return Ok(std::sync::Arc::new(self.input.signature.clone()));
        }
        let path = definition.path();
        if let Some(s) = self.snapshots.get(path) {
            return Ok(s.clone());
        }
        let s = self.provider.signature(*definition)?;
        self.snapshots.insert(path.clone(), s.clone());
        Ok(s)
    }
    fn declaration(&mut self) {
        let item = &self.input.syntax.item;
        let ast::ItemKind::Type {
            flavor,
            body: ast::TypeBody::Record(fields),
            ..
        } = &item.kind
        else {
            return;
        };
        let SignatureKind::Type(def) = &self.input.signature.kind else {
            return;
        };
        let Some(repr) = def.representation else {
            return;
        };
        let Some(Type {
            kind: TypeKind::Record(semantic),
            ..
        }) = self.types.get(repr)
        else {
            return;
        };
        for (ordinal, field) in fields.iter().enumerate() {
            if !field.growable {
                continue;
            }
            let list = semantic
                .get(ordinal)
                .and_then(|f| self.types.get(f.ty))
                .is_some_and(|t| {
                    matches!(
                        t.kind,
                        TypeKind::Builtin {
                            kind: BuiltinType::List,
                            ..
                        }
                    )
                });
            // Flat-compatible elements are already proved by signature::flat_storage.
            if *flavor != ast::TypeFlavor::Flat || !list || ordinal + 1 != fields.len() {
                self.report(
                    const { RuleId::of("MEM-12") },
                    field.origin.span,
                    "a growable field must be the final List field of a flat record",
                );
            }
        }
    }
    /// Whether an annotation's root is written `in heap`.
    fn written_heap(&self, mut syntax: ast::TypeExprId) -> bool {
        while let ast::TypeExprKind::Qualified { base, relation, .. } =
            &self.input.syntax.tree.types[syntax.0 as usize].kind
        {
            if relation
                .as_ref()
                .is_some_and(|r| r.places == [ast::RelationPlace::Heap])
            {
                return true;
            }
            syntax = *base;
        }
        false
    }
    /// Whether a body relation place names an Arena-typed binding: such a
    /// place sets placement rather than bounding a lifetime (PROV-4).
    fn arena_place(&self, origin: &Origin) -> bool {
        let Some(heap) = &self.input.signature.metadata.roles.heap else {
            return false;
        };
        let lexical = self.input.syntax.owner();
        origin.fields.is_empty()
            && self.input.checked.bindings.iter().any(|b| {
                let root = match (&origin.root, &b.binding) {
                    (OriginRoot::Local(owner, l), ResolvedName::Local(m)) => {
                        *owner == lexical && l == m
                    }
                    (
                        OriginRoot::Parameter(ParameterRef::Body { scope, parameter }),
                        ResolvedName::Parameter(p),
                    ) => *scope == lexical.site(b.owner) && parameter == p,
                    _ => false,
                };
                root && is_arena_type(self.types, b.ty, &heap.arena)
            })
    }
    /// Whether a body row's relation places its blocks in an arena or region.
    fn placed(&self, relation: &Relation) -> bool {
        relation.scoped(|o| self.arena_place(o))
    }
    fn expression_type(&self, expression: ExprId) -> Option<TypeId> {
        self.expression_types.get(&expression).copied()
    }
    fn representation(&mut self, id: TypeId) -> Result<(TypeId, bool), QueryError> {
        if let TypeKind::Named {
            definition,
            arguments,
        } = self.types.get(id).unwrap().kind.clone()
        {
            let s = self.snapshot(&definition)?;
            if let SignatureKind::Type(d) = &s.kind
                && let Some(repr) = d.representation
            {
                let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
                let imported = substitute(
                    self.types,
                    self.input.memo,
                    repr,
                    &Substitution {
                        binder: Some(hirsel_types::GenericBinder::Bound(0)),
                        roles: &s.metadata.roles,
                        arguments: &arguments,
                        authorities: &authorities,
                    },
                )
                .map_err(|e| {
                    QueryError::Operational(OperationalFailure {
                        operation: "representation audit".into(),
                        message: format!("{e:?}"),
                    })
                })?;
                return Ok((imported, d.layout.flat));
            }
        }
        Ok((id, false))
    }
    fn place_root_type(&self, place: &SourcePlace) -> Option<TypeId> {
        match &place.root {
            SourceRoot::Binding { owner, binding } => {
                self.source_binding(binding, Some(*owner)).map(|b| b.ty)
            }
            SourceRoot::Value(root) => self.expression_type(*root),
            SourceRoot::Definition(_) => None,
        }
    }
    fn projected_type(
        &mut self,
        current: TypeId,
        projection: &SourceProjectionKind,
    ) -> Result<Option<TypeId>, QueryError> {
        let (repr, _) = self.representation(current)?;
        Ok(match projection {
            SourceProjectionKind::Field(field) => match &self.types.get(repr).unwrap().kind {
                TypeKind::Record(fields) => fields.get(field.0 as usize).map(|f| f.ty),
                TypeKind::Tuple(types) => types.get(field.0 as usize).copied(),
                _ => None,
            },
            SourceProjectionKind::VariantField { variant, field } => {
                let payload = match &self.types.get(repr).unwrap().kind {
                    TypeKind::Builtin {
                        kind: BuiltinType::Packed,
                        arguments,
                    } => match arguments.first() {
                        Some(GenericArgument::Type(t)) => Some(*t),
                        _ => None,
                    },
                    _ => Some(repr),
                };
                if let Some(payload) = payload {
                    let (repr, _) = self.representation(payload)?;
                    match &self.types.get(repr).unwrap().kind {
                        TypeKind::Variant(variants) => variants
                            .get(variant.0 as usize)
                            .and_then(|v| v.fields.get(field.0 as usize))
                            .map(|(_, t)| *t),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            SourceProjectionKind::Index(_) => match &self.types.get(repr).unwrap().kind {
                TypeKind::Builtin { arguments, .. } => arguments.first().and_then(|a| {
                    if let GenericArgument::Type(t) = a {
                        Some(*t)
                    } else {
                        None
                    }
                }),
                _ => None,
            },
        })
    }
    fn in_flat_projection(&mut self, expression: ExprId) -> Result<bool, QueryError> {
        let place = self
            .source_place(expression)
            .cloned()
            .expect("H source place");
        let mut ty = self.place_root_type(&place);
        for step in place.projections {
            let Some(current) = ty else { return Ok(false) };
            let (repr, flat) = self.representation(current)?;
            if flat {
                return Ok(true);
            }
            ty = self.projected_type(repr, &step.kind)?;
        }
        Ok(false)
    }
    fn fixed_size(
        &mut self,
        id: TypeId,
        active: &mut BTreeSet<ast::DefPath>,
    ) -> Result<bool, QueryError> {
        Ok(match self.types.get(id).unwrap().kind.clone() {
            TypeKind::Primitive(p) => !matches!(p, Primitive::String | Primitive::Bytes),
            TypeKind::Tuple(ts) => {
                for t in ts {
                    if !self.fixed_size(t, active)? {
                        return Ok(false);
                    }
                }
                true
            }
            TypeKind::Record(fs) => {
                for f in fs {
                    if !self.fixed_size(f.ty, active)? {
                        return Ok(false);
                    }
                }
                true
            }
            TypeKind::Variant(vs) => {
                for v in vs {
                    for (_, t) in v.fields {
                        if !self.fixed_size(t, active)? {
                            return Ok(false);
                        }
                    }
                }
                true
            }
            TypeKind::Named { definition, .. } => {
                if !active.insert(definition.path().clone()) {
                    return Ok(false);
                }
                let s = self.snapshot(&definition)?;
                let (repr, _) = self.representation(id)?;
                let fixed = if repr == id {
                    matches!(&s.kind, SignatureKind::Type(d) if d.bounds.contains(&Bound::Plain))
                } else {
                    self.fixed_size(repr, active)?
                };
                active.remove(definition.path());
                fixed
            }
            TypeKind::Builtin {
                kind: BuiltinType::Inline | BuiltinType::Simd,
                arguments,
            } => {
                for a in arguments {
                    if let GenericArgument::Type(t) = a
                        && !self.fixed_size(t, active)?
                    {
                        return Ok(false);
                    }
                }
                true
            }
            _ => false,
        })
    }
}

mod source_rows;

use hirsel_modes::reader::Reader;
use hirsel_syntax::DefId;
use hirsel_syntax::DefPath;
use hirsel_type_analysis::representation::{
    RepresentationError, RepresentationFailure, expand_representation,
};
use hirsel_types::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
type HashMap<K, V> = std::collections::HashMap<K, V, hirsel_intern::IdHasher>;

/// The signature reads of the ownership query and its contextual memo. Every
/// read goes through the query's `Reader`; first reads are recorded into the
/// body's dependency reads by the caller's `reads` table.
pub(super) struct Signatures<'q> {
    pub m: &'q TypeMemo,
    reader: &'q RefCell<Reader<'q>>,
    memo: ContextMemo<'q>,
    /// Flow can reach nested declarations that classification did not need.
    /// Its memo reads through the same recording Reader as classification.
    flow_memo: RefCell<ContextMemo<'q>>,
    diagnostic_lookups: RefCell<Vec<DependencyRead>>,
}
impl<'q> Signatures<'q> {
    pub(super) fn type_memo(&self) -> &TypeMemo {
        self.m
    }
    pub fn new(
        m: &'q TypeMemo,
        reader: &'q RefCell<Reader<'q>>,
        memo: ContextMemo<'q>,
        flow_memo: ContextMemo<'q>,
    ) -> Self {
        Self {
            m,
            reader,
            memo,
            flow_memo: RefCell::new(flow_memo),
            diagnostic_lookups: RefCell::new(Vec::new()),
        }
    }
    /// A flow proof, including any newly needed declaration reads. The caller
    /// records the Reader's pending reads after each flow graph, even on error.
    pub fn prove_flow<T>(&self, proof: impl FnOnce(&mut ContextMemo<'q>) -> T) -> T {
        let memo = &mut self.flow_memo.borrow_mut();
        let hits = memo.hits();
        let result = proof(memo);
        hirsel_modes::proofs::count_memo(hits, memo.hits());
        result
    }
    pub fn read(
        &mut self,
        definition: DefId,
        reads: &mut SharedPath<DependencyRead>,
    ) -> Result<SignatureRef, QueryError> {
        let result = self.reader.borrow_mut().read(definition);
        self.record(reads);
        result
    }
    /// Record the first reads made since the last call, the memo's included.
    pub fn record(&mut self, reads: &mut SharedPath<DependencyRead>) {
        reads.extend(self.diagnostic_lookups.borrow_mut().drain(..));
        for (definition, result) in self.reader.borrow_mut().take_pending() {
            reads.push(DependencyRead::Signature {
                definition,
                fingerprint: result.as_ref().ok().map(|s| s.fingerprint),
            });
        }
    }
    /// A proof or comparison through the query's memo, its reads recorded.
    pub fn prove<T>(
        &mut self,
        reads: &mut SharedPath<DependencyRead>,
        proof: impl FnOnce(&mut ContextMemo<'q>) -> Result<T, QueryError>,
    ) -> Result<T, QueryError> {
        let hits = self.memo.hits();
        let result = proof(&mut self.memo);
        hirsel_modes::proofs::count_memo(hits, self.memo.hits());
        self.record(reads);
        result
    }
    /// Diagnostic-only freshness check. Probe the exact declaration module,
    /// and retain misses so a newly added
    /// module value invalidates a cached recipe. No signature/body is needed.
    pub fn diagnostic_name_available(&self, owner: &DefPath, name: &str) -> bool {
        let context = hirsel_syntax::FileContext {
            package: owner.0.package.clone(),
            modules: owner.0.modules.clone(),
        };
        let segments: Vec<String> = owner
            .0
            .modules
            .iter()
            .cloned()
            .chain(std::iter::once(name.to_owned()))
            .collect();
        let path = hirsel_syntax::Path {
            package: Some(owner.0.package.clone()),
            segments: segments
                .iter()
                .map(|s| hirsel_intern::Symbol::intern(s))
                .collect(),
        };
        let output = self.reader.borrow_mut().provider().lookup(&context, &path);
        let available = matches!(output.result, Err(QueryError::NotImplemented));
        self.diagnostic_lookups
            .borrow_mut()
            .push(DependencyRead::ProviderLookup {
                context,
                package: path.package,
                segments,
                output,
            });
        available
    }
    pub fn unique_failure(&self, path: &DefPath, claim: UniqueClaim) -> Option<UniqueFailure> {
        self.reader
            .borrow_mut()
            .provider()
            .cite_unique_failure(path, claim)
    }
    /// A signature an earlier read retained; flow makes no new reads.
    pub fn cached(&self, definition: DefId) -> Option<SignatureRef> {
        self.reader.borrow().cached(definition)
    }
    pub fn dependencies(&self) -> Vec<SignatureDependency> {
        self.reader
            .borrow()
            .signatures()
            .into_iter()
            .map(|s| SignatureDependency {
                definition: s.definition,
                fingerprint: s.fingerprint,
            })
            .collect()
    }
}

/// SEM-3: naming a function in a nominal receiver's association promises
/// uniqueness of its builder, including for ordinary direct calls.
pub(super) fn format_association(
    types: &TypeInterner,
    function: &FunctionType,
    path: &DefPath,
    signatures: &Signatures,
) -> Option<DefPath> {
    let receiver = function.parameters.first()?;
    let TypeKind::Named { definition, .. } = &types.get(receiver.ty)?.kind else {
        return None;
    };
    let signature = signatures.cached(*definition)?;
    let SignatureKind::Type(nominal) = &signature.kind else {
        return None;
    };
    (nominal.format.as_ref() == Some(path)).then(|| definition.path().clone())
}

/// The properties of each row the ownership pass reads: every row of the
/// body's tables when it starts, and each row it derives, which keeps the
/// structural proof of the row it was derived from.
#[derive(Clone, Debug, Default)]
pub(super) struct PropertyTable(HashMap<TypeId, Properties>);
impl PropertyTable {
    pub fn get(&self, id: TypeId) -> Option<&Properties> {
        self.0.get(&id)
    }
    /// `id`'s properties, unless the row already has its own.
    pub fn insert(&mut self, id: TypeId, properties: Properties) {
        self.0.entry(id).or_insert(properties);
    }
    /// A row derived from `from` keeps `from`'s proof.
    pub fn inherit(&mut self, id: TypeId, from: TypeId) {
        let properties = self[from];
        self.insert(id, properties);
    }
}
impl std::ops::Index<TypeId> for PropertyTable {
    type Output = Properties;
    fn index(&self, id: TypeId) -> &Properties {
        self.0.get(&id).expect("a classified row")
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Properties {
    pub plain: bool,
    pub flat: bool,
    /// GEN-1 `data`: no callable, iterator, resource or capability part, so
    /// a value of this type cannot reach a binding's own storage.
    pub data: bool,
    pub sendable_aliased: bool,
    pub copyable: bool,
    pub valid: bool,
    pub function: bool,
    pub shared: bool,
    /// A `Shared`, `Mutex` or `Atomic` root: the handle owns one count on a
    /// run-owned cell, so it supplies a unique result whether or not it is
    /// unique (MODE-2), as V's `handle_root`.
    pub handle_root: bool,
    /// Plain bits and `Shared`, `Mutex` or `Atomic` handles only: a copy is an
    /// independent global value (MEM-11, CONC-2).
    pub copy_detaches: bool,
    pub scoped_capability: bool,
}

/// Producer classification uses the shared declaration-based property proof,
/// for every row the body's tables and `roots` reach.
pub(super) fn classify(
    t: &TypeInterner,
    body: &mut CheckedBody,
    roots: impl IntoIterator<Item = TypeId>,
    signatures: &mut Signatures,
) -> Result<PropertyTable, QueryError> {
    let mut rows = hirsel_modes::rows::BodyRows::new(t, body);
    for root in roots {
        rows.note(t, root);
    }
    let mut table = PropertyTable::default();
    for &root in rows.ids() {
        let properties = classify_row(t, root, body, signatures)?;
        table.insert(root, properties);
    }
    Ok(table)
}
/// The properties of one row; derived rows inherit theirs (`PropertyTable`).
pub(super) fn classify_row(
    t: &TypeInterner,
    root: TypeId,
    body: &mut CheckedBody,
    signatures: &mut Signatures,
) -> Result<Properties, QueryError> {
    use hirsel_modes::proofs;
    let row = t.row(root);
    let reads = Arc::make_mut(&mut body.resolution.reads);
    let plain = signatures.prove(reads, |memo| {
        proofs::property(memo, root, StructuralProperty::Plain)
    })?;
    let copyable = signatures.prove(reads, |memo| {
        proofs::property(memo, root, StructuralProperty::Copyable)
    })?;
    let scoped_capability = if let TypeKind::Named { definition, .. } = &row.kind {
        let signature = signatures.read(*definition, reads)?;
        matches!(&signature.kind, SignatureKind::Type(t) if t.capability == Some(CapabilityKind::Scoped))
    } else {
        false
    };
    let flat = signatures.prove(reads, |memo| proofs::flat(memo, root))?;
    let copy_detaches = signatures.prove(reads, |memo| proofs::copy_detaches(memo, root))?;
    let data = signatures.prove(reads, |memo| memo.data(root))?;
    // Flow can invalidate the root's uniqueness after J has proved a
    // generic bound. Cache the same structural proof without that root
    // evidence; locality is checked separately at the call.
    let mut aliased = row.clone();
    aliased.modes.uniqueness = Uniqueness::Aliased;
    if !matches!(
        aliased.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    ) {
        aliased.modes.sendability = Sendability::NotSendable;
    }
    aliased.modes.relation = Relation::global();
    let aliased = t.intern(aliased);
    let sendable_aliased = signatures.prove(reads, |memo| {
        proofs::sendability_after_sharing(memo, aliased)
    })? == Sendability::Sendable;
    Ok(Properties {
        plain,
        flat,
        data,
        sendable_aliased,
        copyable,
        valid: t.valid(root),
        function: matches!(row.kind, TypeKind::Function(_)),
        shared: matches!(
            row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        ),
        handle_root: matches!(
            row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared | BuiltinType::Mutex | BuiltinType::Atomic,
                ..
            }
        ),
        copy_detaches,
        scoped_capability,
    })
}

/// The implicit result-read permission is separate from written body bounds.
pub(super) fn result_sharing(
    t: &TypeInterner,
    body: &mut CheckedBody,
    graphs: &[super::cfg::Graph],
    signatures: &mut Signatures,
    rows: &super::source_rows::SourceRows,
) -> Result<std::collections::BTreeSet<hirsel_syntax::ExprId>, QueryError> {
    let declarations = body.resolution.generic_declarations.clone();
    let mut reads = std::collections::BTreeSet::new();
    for graph in graphs {
        let expressions: Vec<_> = graph
            .expressions
            .iter()
            .filter_map(|&expression| {
                super::cfg::expression_type(body, expression, rows).map(|ty| (expression, ty))
            })
            .collect();
        let declarations = declarations.with_copyable(&graph.result_shareable);
        let mut lookup = |d| {
            signatures
                .read(d, Arc::make_mut(&mut body.resolution.reads))
                .map(Some)
        };
        let mut memo = ContextMemo::new(t, &declarations, &mut lookup);
        for (expression, ty) in expressions {
            if hirsel_modes::proofs::property(&mut memo, ty, StructuralProperty::Copyable)? {
                reads.insert(expression);
            }
        }
    }
    Ok(reads)
}

/// Flow refines locality and uniqueness. Recompute their derived summaries once
/// all graphs finish, keeping immutable type rows and every provider read.
pub(super) fn normalize(
    t: &TypeInterner,
    body: &mut CheckedBody,
    results: &mut [(hirsel_types::NameSite, TypeId)],
    physical: &mut [(hirsel_syntax::ExprId, TypeId)],
    signatures: &mut Signatures,
) -> Result<(), QueryError> {
    fn visit_roots(
        body: &mut CheckedBody,
        results: &mut [(hirsel_types::NameSite, TypeId)],
        physical: &mut [(hirsel_syntax::ExprId, TypeId)],
        mut normalize: impl FnMut(&mut TypeId),
    ) {
        let mut arguments = |args: &mut Vec<GenericArgument>| {
            for argument in args {
                if let GenericArgument::Type(ty) = argument {
                    normalize(ty);
                }
            }
        };
        for instantiation in &mut body.instantiations {
            arguments(&mut instantiation.arguments);
        }
        for expression in &mut body.expressions {
            arguments(&mut expression.instantiated_arguments);
        }
        for expression in &mut body.expressions {
            normalize(&mut expression.ty);
        }
        for binding in &mut body.bindings {
            normalize(&mut binding.ty);
        }
        for pattern in &mut body.patterns {
            normalize(&mut pattern.ty);
        }
        for closure in &mut body.closures {
            normalize(&mut closure.ty);
            if let Some(result) = &mut closure.result_evidence {
                normalize(result);
            }
            for capture in &mut closure.captures {
                normalize(&mut capture.ty);
            }
        }
        for (_, ty) in physical {
            normalize(ty);
        }
        // A frame's formal result is the row its callable type publishes.
        for (_, result) in results {
            normalize(result);
        }
    }
    let mut roots = vec![];
    visit_roots(body, results, physical, |root| roots.push(*root));
    let result = canonical_current_types(
        t,
        &mut roots,
        &body.resolution.generic_declarations,
        &mut |d| {
            signatures
                .read(d, Arc::make_mut(&mut body.resolution.reads))
                .map(Some)
        },
    );
    let mut roots = roots.into_iter();
    visit_roots(body, results, physical, |root| {
        *root = roots.next().unwrap()
    });
    assert!(roots.next().is_none());
    result
}

/// J supplies field identities before ownership refines the receiving binding.
/// Bound each physical part by that final owner, retaining J's sibling brands
/// and nested callable contracts. K must not publish the earlier Global row
/// through a binding whose lifetime widened at a loop or branch join.
pub(super) fn identity_projections(
    t: &TypeInterner,
    body: &mut CheckedBody,
    fields: &mut [(NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)],
    signatures: &mut Signatures,
) -> Result<(), QueryError> {
    if fields.is_empty() {
        return Ok(());
    }
    let declarations = body.resolution.generic_declarations.clone();
    let bindings: BTreeMap<_, _> =
        body.bindings
            .iter()
            .filter_map(|row| {
                let root = match row.binding {
                    ResolvedName::Local(local) => OriginRoot::Local(body.lexical, local),
                    ResolvedName::Parameter(parameter) => OriginRoot::Parameter(
                        ParameterRef::body(body.lexical.site(row.owner), parameter),
                    ),
                    _ => return None,
                };
                Some(((row.owner, root), row.ty))
            })
            .collect();
    for (_, owner, origin, projections) in fields {
        let Some(&root) = bindings.get(&(*owner, origin.root.clone())) else {
            continue;
        };
        // Each part is its declared row: place and project it under the
        // binding's final row, which flow may have refined since J recorded it.
        let mut parent = root;
        for (_, part) in projections {
            let mut lookup = |d| {
                signatures
                    .read(d, Arc::make_mut(&mut body.resolution.reads))
                    .map(Some)
            };
            let placed = placed_part_type(t, parent, *part, &declarations, &mut lookup)?;
            let m = signatures.m;
            *part = signatures.prove(Arc::make_mut(&mut body.resolution.reads), |memo| {
                memo.projected(m, parent, placed)
            })?;
            parent = *part;
        }
    }
    Ok(())
}

/// The declaration-owned field lifetime precedes J's borrowed read refinement.
/// I applies this plan to the current temporary root instead of retaining a
/// projection made from an earlier, weaker call-result row.
#[derive(Clone, Copy)]
pub(super) enum ProjectionRoot {
    Value(hirsel_syntax::ExprId),
    Binding(usize),
    Constant(TypeId),
}
pub(super) struct ProjectionLifetime {
    pub root: ProjectionRoot,
    pub parts: Vec<(Relation, bool)>,
    pub physical: TypeId,
    /// Replacement traverses a counted owner, rather than inline storage alone.
    pub counted_path: bool,
    pub declared: Modes,
}
/// Physical layouts expanded so far into one append-only store. Expanding a
/// nominal type again would only push an equal copy of its representation,
/// once per projection of every field, so a wide record grew the body's type
/// rows with the square of its fields (GAP 5).
type Layouts = HashMap<TypeId, TypeKind>;
fn physical_layout(
    layouts: &mut Layouts,
    t: &TypeInterner,
    ty: TypeId,
    reads: &mut SharedPath<DependencyRead>,
    signatures: &mut Signatures,
) -> Result<TypeKind, QueryError> {
    if let Some(kind) = layouts.get(&ty) {
        return Ok(kind.clone());
    }
    let m = signatures.m;
    let root = match expand_representation(
        t,
        m,
        ty,
        &mut |_, definition, arguments| {
            let signature = signatures.read(definition, reads)?;
            Ok(Some((
                signature,
                vec![AuthoritySummary::Unknown; arguments.len()],
            )))
        },
        None,
    ) {
        Ok(root) => root,
        Err(RepresentationError {
            failure: RepresentationFailure::Query(error),
            ..
        }) => {
            return Err(error);
        }
        Err(error) => error.root,
    };
    let kind = t.get(root).unwrap().kind.clone();
    layouts.insert(ty, kind.clone());
    Ok(kind)
}

/// Declared part rows before owner locality is applied. A projected pattern
/// already carries inherited locality and cannot identify an inferred Local part.
pub(super) fn pattern_declarations(
    t: &TypeInterner,
    body: &mut CheckedBody,
    signatures: &mut Signatures,
) -> Result<BTreeMap<hirsel_syntax::PatternId, TypeId>, QueryError> {
    let rows: BTreeMap<_, _> = body
        .patterns
        .iter()
        .map(|p| (p.pattern, p.clone()))
        .collect();
    let children: std::collections::BTreeSet<_> =
        body.patterns.iter().flat_map(|p| p.subpatterns()).collect();
    let mut pending: Vec<_> = body
        .patterns
        .iter()
        .filter(|p| !children.contains(&p.pattern))
        .map(|p| (p.pattern, p.ty))
        .collect();
    let mut result = BTreeMap::new();
    let mut layouts = Layouts::default();
    while let Some((pattern, ty)) = pending.pop() {
        let row = &rows[&pattern];
        if let PatternDecisionKind::Or(alternatives) = &row.kind {
            // Alternatives declare the same place as their or-pattern.
            for alternative in alternatives {
                result.insert(*alternative, ty);
                pending.push((*alternative, ty));
            }
            continue;
        }
        if row.fields.is_empty() {
            continue;
        }
        let mut inspected = ty;
        while let TypeKind::Box(part) = t.get(inspected).unwrap().kind {
            inspected = part;
        }
        let kind = physical_layout(
            &mut layouts,
            t,
            inspected,
            Arc::make_mut(&mut body.resolution.reads),
            signatures,
        )?;
        for (field, child) in &row.fields {
            let part = match &kind {
                TypeKind::Record(fields) => fields.get(field.0 as usize).map(|f| f.ty),
                TypeKind::Tuple(fields) => fields.get(field.0 as usize).copied(),
                TypeKind::Variant(variants) => match &row.kind {
                    PatternDecisionKind::Constructor(ContextualName::Variant {
                        variant, ..
                    }) => variants
                        .get(variant.0 as usize)
                        .and_then(|v| v.fields.get(field.0 as usize))
                        .map(|f| f.1),
                    _ => None,
                },
                _ => None,
            };
            if let Some(part) = part {
                result.insert(*child, part);
                pending.push((*child, part));
            }
        }
    }
    Ok(result)
}

pub(super) fn projection_lifetimes(
    t: &TypeInterner,
    body: &mut CheckedBody,
    own: &Signature,
    signatures: &mut Signatures,
) -> Result<BTreeMap<hirsel_syntax::ExprId, ProjectionLifetime>, QueryError> {
    let arenas = crate::arenas::Arenas::new(t, body, own);
    let mut result = BTreeMap::new();
    let expression_types: BTreeMap<_, _> = body
        .expressions
        .iter()
        .map(|e| (e.expression, e.ty))
        .collect();
    let mut layouts = Layouts::default();
    let outcome = (|| {
        for place in &body.places {
            if place.projections.is_empty() {
                continue;
            }
            let (root, mut ty) = match &place.root {
                SourceRoot::Value(root) => {
                    let Some(ty) = expression_types.get(root) else {
                        continue;
                    };
                    (ProjectionRoot::Value(*root), *ty)
                }
                SourceRoot::Binding { owner, binding } => {
                    let Some(index) = body
                        .bindings
                        .iter()
                        .position(|row| row.owner == *owner && row.binding == *binding)
                    else {
                        continue;
                    };
                    (ProjectionRoot::Binding(index), body.bindings[index].ty)
                }
                SourceRoot::Definition(path) => {
                    let signature = signatures.read(
                        DefId::intern(path),
                        Arc::make_mut(&mut body.resolution.reads),
                    )?;
                    let SignatureKind::Const { ty: root, .. } = signature.kind else {
                        continue;
                    };
                    (ProjectionRoot::Constant(root), root)
                }
            };
            let mut parts = vec![];
            let mut counted_path = false;
            for step in &place.projections {
                let kind = physical_layout(
                    &mut layouts,
                    t,
                    ty,
                    Arc::make_mut(&mut body.resolution.reads),
                    signatures,
                )?;
                counted_path |= matches!(
                    &kind,
                    TypeKind::Box(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Array
                                | BuiltinType::List
                                | BuiltinType::Map
                                | BuiltinType::SortedMap,
                            ..
                        }
                );
                let next = match (&step.kind, kind) {
                    (SourceProjectionKind::Field(FieldId(0)), TypeKind::Box(t)) => Some(t),
                    (SourceProjectionKind::Field(field), TypeKind::Record(fields)) => {
                        fields.get(field.0 as usize).map(|f| f.ty)
                    }
                    (SourceProjectionKind::Field(field), TypeKind::Tuple(fields)) => {
                        fields.get(field.0 as usize).copied()
                    }
                    (
                        SourceProjectionKind::VariantField { variant, field },
                        TypeKind::Variant(variants),
                    ) => variants
                        .get(variant.0 as usize)
                        .and_then(|v| v.fields.get(field.0 as usize))
                        .map(|f| f.1),
                    (SourceProjectionKind::Index(_), TypeKind::Builtin { kind, arguments })
                        if matches!(
                            kind,
                            BuiltinType::Array
                                | BuiltinType::List
                                | BuiltinType::Inline
                                | BuiltinType::Simd
                        ) =>
                    {
                        arguments.first().and_then(|a| match a {
                            GenericArgument::Type(t) => Some(*t),
                            _ => None,
                        })
                    }
                    // A byte is a plain u8, as V projects it; without this
                    // step `xs[i].payload[j]` published no physical rows and
                    // lowering typed the Bytes part at its receiving row.
                    (SourceProjectionKind::Index(_), TypeKind::Primitive(Primitive::Bytes)) => {
                        let mut byte = Type::boolean();
                        byte.kind = TypeKind::Primitive(Primitive::U8);
                        Some(t.intern(byte))
                    }
                    _ => None,
                };
                let Some(next) = next else {
                    break;
                };
                let plain = structural_property(
                    t,
                    next,
                    StructuralProperty::Plain,
                    &body.resolution.generic_declarations,
                    &mut |path| {
                        signatures
                            .read(path, Arc::make_mut(&mut body.resolution.reads))
                            .map(Some)
                    },
                )?;
                parts.push((t.get(next).unwrap().modes.relation.clone(), plain));
                ty = next;
            }
            if parts.len() == place.projections.len() {
                // Physical storage retains the declaration's complete header and
                // nested contract. J has already specialized the occurrence's
                // owner-relative provenance and placement; keep that transport,
                // independently of its receiving modes and callable promises.
                let occurrence = t.get(expression_types[&place.expression]).unwrap();
                let mut physical = t.get(ty).unwrap().clone();
                physical.brand = occurrence.brand.clone();
                physical.modes.relation =
                    arenas.relocate(&occurrence.modes.relation, &physical.modes.relation);
                physical.allocation_target = occurrence.allocation_target.clone();
                let physical = t.intern(physical);
                result.insert(
                    place.expression,
                    ProjectionLifetime {
                        root,
                        parts,
                        physical,
                        counted_path,
                        declared: t.get(ty).unwrap().modes.clone(),
                    },
                );
            }
        }
        Ok(result)
    })();
    outcome
}

/// A field place a call result reads within (#77): the path within the
/// argument, the declared row at its end and each step's declared relation
/// and plainness, which the flow folds into the argument's locality.
pub(super) struct FieldPlace {
    pub path: Vec<u32>,
    pub declared: TypeId,
    pub parts: Vec<(Relation, bool)>,
}
/// For each borrowed call argument whose callee's result relation names only
/// a field path of its parameter, that field place, read from the declared
/// layouts (#77).
pub(super) fn field_loan_places(
    t: &TypeInterner,
    body: &mut CheckedBody,
    graphs: &[super::cfg::Graph],
    signatures: &mut Signatures,
    rows: &super::source_rows::SourceRows,
) -> Result<BTreeMap<hirsel_syntax::ExprId, FieldPlace>, QueryError> {
    let mut requests = vec![];
    for node in graphs.iter().flat_map(|graph| &graph.nodes) {
        let super::cfg::Op::Call {
            arguments,
            requirements,
            result,
            constructor: false,
            ..
        } = &node.op
        else {
            continue;
        };
        for (argument, requirement) in arguments.iter().zip(requirements) {
            let Some(requirement) = requirement
                .as_ref()
                .filter(|r| r.passing == hirsel_syntax::PassingMode::Borrow)
            else {
                continue;
            };
            let parameter = ParameterRef::bound(ParamId(requirement.parameter as u32));
            let (Some(path), Some(ty)) = (
                result
                    .relation
                    .named_field_path(&parameter)
                    .map(|path| path.iter().map(|f| f.0).collect::<Vec<u32>>()),
                super::cfg::expression_type(body, argument.value, rows),
            ) else {
                continue;
            };
            requests.push((argument.value, path, ty));
        }
    }
    let mut result = BTreeMap::new();
    let mut layouts = Layouts::default();
    let outcome = (|| {
        for (argument, path, mut ty) in requests {
            let mut found = true;
            let mut parts = vec![];
            for field in &path {
                let kind = physical_layout(
                    &mut layouts,
                    t,
                    ty,
                    Arc::make_mut(&mut body.resolution.reads),
                    signatures,
                )?;
                ty = match kind {
                    TypeKind::Box(payload) if *field == 0 => payload,
                    TypeKind::Record(fields) if (*field as usize) < fields.len() => {
                        fields[*field as usize].ty
                    }
                    TypeKind::Tuple(fields) if (*field as usize) < fields.len() => {
                        fields[*field as usize]
                    }
                    _ => {
                        found = false;
                        break;
                    }
                };
                let plain = structural_property(
                    t,
                    ty,
                    StructuralProperty::Plain,
                    &body.resolution.generic_declarations,
                    &mut |path| {
                        signatures
                            .read(path, Arc::make_mut(&mut body.resolution.reads))
                            .map(Some)
                    },
                )?;
                parts.push((t.get(ty).unwrap().modes.relation.clone(), plain));
            }
            if found {
                result.insert(
                    argument,
                    FieldPlace {
                        path,
                        declared: ty,
                        parts,
                    },
                );
            }
        }
        Ok(result)
    })();
    outcome
}

#[cfg(test)]
mod limit_tests {
    use super::*;
    use hirsel_authority::test_support::Fixture;

    #[test]
    fn physical_layout_resolves_finite_nominal_chains_past_128() {
        for depth in [128, 129, 200] {
            let mut f = Fixture::new();
            let mut root = f.ty(TypeKind::Primitive(Primitive::U8));
            for n in 0..depth {
                root = f.nominal(
                    &format!("D{n}"),
                    None,
                    vec![],
                    AuthoritySummary::inert(),
                    Some(root),
                    false,
                );
            }
            let mut reads = hirsel_types::SharedPath::default();
            let generics = f.body.resolution.generic_declarations.clone();
            let reader = Reader::new(&mut f.provider, &f.signature);
            let mut read = |d: DefId| reader.borrow_mut().read(d).map(Some);
            let mut cached = |d: DefId| Ok(reader.borrow().cached(d));
            let memo = ContextMemo::new(f.types, &generics, &mut read);
            let flow_memo = ContextMemo::new(f.types, &generics, &mut cached);
            let mut signatures = Signatures::new(&f.memo, &reader, memo, flow_memo);
            assert_eq!(
                physical_layout(
                    &mut Layouts::default(),
                    f.types,
                    root,
                    &mut reads,
                    &mut signatures,
                )
                .unwrap(),
                TypeKind::Primitive(Primitive::U8)
            );
        }
    }
}

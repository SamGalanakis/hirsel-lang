//! Per-definition products in the ordinary ResultStore. Records reference
//! canonical type components and relocate spans through their lexical owners.
//! Reads are verified before payload decoding; malformed records are misses.
use super::*;
use crate::store::{Entry as StoredEntry, EntryKey, Variant, VariantRef};
use hirsel_span::Anchor;
use hirsel_types::{OwnershipFacts, TypeId};
use serde::{Deserialize, Serialize};
mod expected;
pub(crate) use expected::{Expected, Spool};
pub(crate) use hirsel_product_codec::TypeRecords;
#[cfg(test)]
use hirsel_product_codec::{Record as Envelope, TypeRef};

#[cfg(test)]
mod expected_tests {
    use super::*;

    #[test]
    fn verify_preflight_releases_old_payloads_before_workers_start() {
        let mut engine = QueryEngine::new(crate::default_environment(), PhaseAdapters::default());
        let mut observed = Vec::new();
        for i in 0..64 {
            let path = DefPath(hirsel_syntax::ItemPath {
                package: crate::default_context().package,
                modules: vec![],
                name: format!("p{i}"),
            });
            let payload: Arc<[u8]> = vec![i as u8; 128 * 1024].into();
            observed.push(Arc::downgrade(&payload));
            let expected = Expected::capture(
                Variant {
                    inputs: ContentHash::default(),
                    reads: Reads::empty(),
                    payload: payload.into(),
                },
                &mut engine.store_check_spool,
            );
            engine
                .pending_store_checks
                .insert((DefId::intern(&path), QueryKind::Modes), expected);
        }
        assert_eq!(engine.pending_store_checks.len(), 64);
        assert!(
            observed.iter().all(|weak| weak.upgrade().is_none()),
            "preflight retained all old canonical payloads until the frontier drained"
        );
    }
}

// Match complete_reads' observation semantics: manifest graph revisions are
// unused provenance for every lookup, including failed probes of local names.
// Negative lookups observe accessible repair candidates through their canonical
// Reads, not through this stamp. Keeping it here makes equal products differ
// when the same package moves between root and dependency positions.
fn canonical_reads(reads: &mut [hirsel_types::DependencyRead]) {
    use hirsel_types::DependencyRead;
    for read in reads {
        let manifest = match read {
            DependencyRead::Signature { .. } => continue,
            DependencyRead::Lookup { manifest, .. }
            | DependencyRead::ModuleHead { manifest, .. } => manifest,
            DependencyRead::ProviderLookup { output, .. } => &mut output.manifest,
        };
        *manifest = ContentHash::default();
    }
}
// Canonicalization owns a serialization copy; live phase snapshots keep their
// shared prefixes and their original provenance fields unchanged.
fn canonical_read_log(reads: &mut Arc<hirsel_types::SharedPath<hirsel_types::DependencyRead>>) {
    let mut canonical = reads.iter().cloned().collect::<Vec<_>>();
    canonical_reads(&mut canonical);
    *reads = Arc::new(canonical.into());
}
fn canonical_checked(checked: &CheckedBody) -> CheckedBody {
    let mut checked = checked.clone();
    canonical_read_log(&mut checked.resolution.reads);
    checked
}
fn canonical_detail(mut detail: DependencyReads) -> DependencyReads {
    canonical_reads(&mut detail.manifest_lookups);
    canonical_read_log(&mut detail.resolver);
    detail
}

/// Syntax registration is an ordinary product, separate from generator text
/// and independent V authority. Lossless actuals use canonical type records.
#[derive(Serialize, Deserialize)]
struct GeneratedRegistration {
    declaration: DefId,
    arguments: Vec<hirsel_types::GenericArgument>,
    instance: DefId,
}

#[derive(Serialize, Deserialize)]
struct SignatureRecord {
    output: Result<QueryOutput<hirsel_types::SignatureRef>, QueryError>,
    detail: DependencyReads,
    covered: Vec<SignatureDependency>,
    resolution: Option<(u32, QueryOutput<Resolution>)>,
    owner: Option<(
        ContentHash,
        hirsel_types::TypeDefinition,
        Vec<SignatureDependency>,
    )>,
}
impl SignatureRecord {
    fn capture(entry: &Entry<hirsel_types::SignatureRef>) -> Self {
        Self {
            output: entry.output(),
            detail: canonical_detail(entry.detail.clone()),
            covered: entry.covered.clone(),
            resolution: entry.products.resolution.as_ref().map(|r| {
                let mut output = r.output.clone();
                canonical_read_log(&mut output.value.reads);
                (r.syntax.occurrence, output)
            }),
            owner: entry.products.representation.as_ref().map(|owner| {
                (
                    owner.text_hash,
                    owner.representation.declaration.clone(),
                    owner.dependencies.clone(),
                )
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Body {
    checked: CheckedBody,
    plans: hirsel_check_protocol::body::Plans,
    unit: TypeId,
    boolean: TypeId,
    ownership: OwnershipFacts,
    // HashSets are encoded as content-sorted sequences, never bucket order.
    plain: Vec<TypeId>,
    silent: Vec<TypeId>,
    results: Vec<(hirsel_types::NameSite, TypeId)>,
    calls: Vec<(hirsel_syntax::ExprId, hirsel_types::Allocation)>,
}
impl Body {
    fn capture(types: &Arc<hirsel_types::TypeInterner>, body: &AnalyzedBody) -> Self {
        fn ordered(
            types: &hirsel_types::TypeInterner,
            set: &std::collections::HashSet<TypeId>,
        ) -> Vec<TypeId> {
            let mut entries: Vec<_> = set
                .iter()
                .map(|id| {
                    let (table, _) = hirsel_types::encode_table(types, &[*id]);
                    (bincode::serialize(&table).expect("type table"), *id)
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries.into_iter().map(|(_, id)| id).collect()
        }
        let mut ownership = body.ownership.clone();
        let plain = ordered(types, &ownership.plain_types);
        let silent = ordered(types, &ownership.silent_drop_types);
        ownership.plain_types.clear();
        ownership.silent_drop_types.clear();
        Self {
            checked: canonical_checked(&body.typed.checked),
            plans: body.typed.plans.clone(),
            unit: body.typed.unit,
            boolean: body.typed.boolean,
            ownership,
            plain,
            silent,
            results: body.results.clone(),
            calls: body.call_allocations.clone(),
        }
    }
    fn restore(mut self, syntax: Arc<ItemSyntax>) -> AnalyzedBody {
        self.ownership.plain_types.extend(self.plain);
        self.ownership.silent_drop_types.extend(self.silent);
        AnalyzedBody {
            typed: TypedBody {
                syntax,
                checked: self.checked,
                plans: self.plans,
                unit: self.unit,
                boolean: self.boolean,
            }
            .into(),
            ownership: self.ownership,
            results: self.results,
            call_allocations: self.calls,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct CheckedRecord {
    checked: CheckedBody,
    plans: hirsel_check_protocol::body::Plans,
    unit: TypeId,
    boolean: TypeId,
    diagnostics: Vec<Diagnostic>,
    dependencies: Vec<SignatureDependency>,
    detail: DependencyReads,
}
#[derive(Serialize, Deserialize)]
struct ModeRecord {
    checked: Option<CheckedRecord>,
    phases: Vec<(Diagnostic, String)>,
    output: Result<QueryOutput<Body>, QueryError>,
    detail: DependencyReads,
}
impl ModeRecord {
    fn valid(&self, syntax: &ItemSyntax) -> bool {
        let valid = |checked: &CheckedBody, plans: &hirsel_check_protocol::body::Plans| {
            valid_body_indices(syntax, checked, plans)
        };
        if self
            .checked
            .as_ref()
            .is_some_and(|c| !valid(&c.checked, &c.plans))
        {
            return false;
        }
        match &self.output {
            Ok(output) => {
                let body = &output.value;
                self.checked.is_some()
                    && valid(&body.checked, &body.plans)
                    && body
                        .results
                        .iter()
                        .all(|(site, _)| valid_site(syntax, *site))
                    && body
                        .calls
                        .iter()
                        .all(|(id, _)| valid_expression(syntax, *id))
                    && valid_ownership(syntax, &body.ownership)
            }
            Err(error) => error.cacheable(),
        }
    }
}
fn valid_expression(syntax: &ItemSyntax, id: hirsel_syntax::ExprId) -> bool {
    (id.0 as usize) < syntax.tree.expressions.len()
}
fn valid_site(syntax: &ItemSyntax, site: hirsel_types::NameSite) -> bool {
    use hirsel_types::NameSite;
    match site {
        NameSite::Expression(id) => valid_expression(syntax, id),
        NameSite::Statement(id) => (id.0 as usize) < syntax.tree.statements.len(),
        NameSite::Pattern(id) => (id.0 as usize) < syntax.tree.patterns.len(),
        NameSite::Type(id) => (id.0 as usize) < syntax.tree.types.len(),
        NameSite::Item => true,
    }
}
fn valid_resolution(syntax: &ItemSyntax, resolution: &Resolution) -> bool {
    use hirsel_types::NameSite;
    let expression = |id| valid_expression(syntax, id);
    let site = |id| valid_site(syntax, id);
    let pattern = |id| site(NameSite::Pattern(id));
    resolution.contextual.iter().all(|(id, _)| site(*id))
        && resolution.paths.iter().all(|(id, _)| site(*id))
        && resolution
            .local_functions
            .iter()
            .all(|(id, _)| site(NameSite::Statement(*id)))
        && resolution.deferred.iter().all(|row| site(row.site))
        && resolution.generics.iter().all(|row| site(row.owner))
        && resolution
            .reference_scopes
            .iter()
            .all(|(a, b)| site(*a) && site(*b))
        && resolution.expressions.iter().all(|(id, _)| expression(*id))
        && resolution
            .types
            .iter()
            .all(|(id, _)| site(NameSite::Type(*id)))
        && resolution.bindings.iter().all(|(id, _, _)| pattern(*id))
}

/// These are source arena positions, never process IDs. Check the lexical
/// owner first: even an in-range row from another item has no meaning here.
fn valid_body_indices(
    syntax: &ItemSyntax,
    checked: &CheckedBody,
    plans: &hirsel_check_protocol::body::Plans,
) -> bool {
    use hirsel_types::{CallTarget, NameSite, SourceProjectionKind, SourceRoot};
    let expression = |id| valid_expression(syntax, id);
    let site = |id| valid_site(syntax, id);
    let pattern = |id| site(NameSite::Pattern(id));
    let target = |target: &CallTarget| match target {
        CallTarget::Value(id) => expression(*id),
        _ => true,
    };
    let resolution = &checked.resolution;
    checked.lexical == syntax.owner()
        && checked
            .expressions
            .iter()
            .all(|row| expression(row.expression))
        && checked.calls.iter().all(|row| {
            expression(row.expression)
                && target(&row.target)
                && row.arguments.iter().all(|arg| expression(arg.expression))
        })
        && checked.tails.iter().all(|row| {
            expression(row.call)
                && row
                    .owned_loop_arguments
                    .iter()
                    .all(|(_, id)| expression(*id))
        })
        && checked
            .instantiations
            .iter()
            .all(|row| expression(row.expression))
        && checked
            .closures
            .iter()
            .all(|row| expression(row.expression))
        && checked
            .bindings
            .iter()
            .all(|row| site(row.owner) && site(row.site))
        && checked
            .patterns
            .iter()
            .all(|row| pattern(row.pattern) && row.subpatterns().all(pattern))
        && checked.places.iter().all(|row| {
            expression(row.expression)
                && match row.root {
                    SourceRoot::Value(id) => expression(id),
                    SourceRoot::Binding { owner, .. } => site(owner),
                    _ => true,
                }
                && row.projections.iter().all(|projection| {
                    expression(projection.expression)
                        && match projection.kind {
                            SourceProjectionKind::Index(id) => expression(id),
                            _ => true,
                        }
                })
        })
        && valid_resolution(syntax, resolution)
        && plans.resource_patterns.iter().all(|(id, _)| pattern(*id))
        && plans.pattern_literals.iter().all(|(id, _)| pattern(*id))
        && plans.unreachable_arms.iter().all(|id| pattern(*id))
        && plans
            .tries
            .iter()
            .all(|row| expression(row.expression) && row.mapper.as_ref().is_none_or(target))
        && plans.loops.iter().all(|row| {
            site(NameSite::Statement(row.statement()))
                && row.cursor().is_none_or(|cursor| expression(cursor.source))
        })
        && plans.functions.iter().all(|row| site(row.owner))
        && plans
            .shapes
            .iter()
            .all(|(a, b, _)| expression(*a) && site(NameSite::Type(*b)))
        && plans.contracts.iter().all(|row| {
            site(row.owner)
                && expression(row.expression)
                && row.snapshots.iter().copied().all(expression)
                && row.parameters.iter().all(|p| match p {
                    hirsel_check_protocol::body::ContractParameter::Snapshot(id) => expression(*id),
                    _ => true,
                })
        })
        && plans.facts.iter().all(|row| expression(row.expression))
}
fn valid_ownership(syntax: &ItemSyntax, facts: &OwnershipFacts) -> bool {
    let expression = |id| valid_expression(syntax, id);
    let site = |id| valid_site(syntax, id);
    facts.field_loans.iter().all(|(id, _)| expression(*id))
        && facts.composed_arguments.iter().copied().all(expression)
        && facts
            .captures
            .iter()
            .all(|row| expression(row.expression) && site(row.owner))
        && facts
            .call_arguments
            .iter()
            .all(|(a, b, _)| expression(*a) && expression(*b))
        && facts
            .identity_joins
            .iter()
            .all(|(a, b, _)| site(*a) && site(*b))
        && facts
            .identity_fields
            .iter()
            .all(|(a, b, _, _)| site(*a) && site(*b))
        && facts
            .physical_projections
            .iter()
            .all(|(id, _)| expression(*id))
        && facts.unique.iter().all(|row| expression(row.expression))
        && facts
            .guarded_patterns
            .iter()
            .all(|row| site(hirsel_types::NameSite::Pattern(row.pattern)))
        && facts.cursor_rebindings.iter().copied().all(expression)
        && facts
            .borrow_cursors
            .iter()
            .all(|cursor| valid_site(syntax, cursor.binding))
        && facts.allocating_mutations.iter().copied().all(expression)
        && facts.replacements.iter().all(|(id, _, _)| expression(*id))
}

#[derive(Serialize, Deserialize)]
struct Registrations {
    owner: DefId,
    rows: Vec<crate::comptime::StagedRegistration>,
}
impl Registrations {
    fn valid(
        &self,
        owner: hirsel_syntax::LexicalOwner,
        shape: super::catalog::LexicalShape,
        comptime: &[(u32, u32, u32)],
        types: &hirsel_types::TypeInterner,
    ) -> bool {
        self.owner == owner.def
            && !self.rows.is_empty()
            && self.rows.windows(2).all(|w| w[0].path < w[1].path)
            && self.rows.iter().all(|row| {
                let constant = &row.constant;
                let valid_signature = matches!(&constant.signature.kind,
                    hirsel_types::SignatureKind::Const { generics, .. }
                        if generics.len() == constant.inputs.len()
                            && generics.iter().all(|p| matches!(p, hirsel_types::GenericParameter::Size { .. })));
                constant.owner == *owner.def.path()
                    && row.path.0.package == constant.owner.0.package
                    && row.path.0.modules == constant.owner.0.modules
                    && row.path.0.name.starts_with(&format!("$comptime:{}:", constant.owner.0.name))
                    && constant.signature.definition == DefId::intern(&row.path)
                    && valid_signature
                    && hirsel_types::encoding::fingerprint(types, &constant.signature, &[])
                        .is_ok_and(|hash| hash == constant.signature.fingerprint)
                    && !constant.inputs.is_empty()
                    && constant.inputs.windows(2).all(|w| w[0] < w[1])
                    && constant.inputs.iter().all(|p| match p.binder() {
                        hirsel_types::GenericBinder::Body(scope) => scope.owner == owner && shape.valid_site(scope.site),
                        hirsel_types::GenericBinder::Bound(_) => false,
                    })
                    && comptime.binary_search_by_key(&constant.expression.0, |site| site.0).is_ok()
            })
    }
}

#[derive(Serialize, Deserialize)]
struct LowerRecord {
    selections: Vec<crate::comptime::Selection>,
    depth: crate::memo::DepthEvidence<Span>,
    output: QueryOutput<hirsel_tir::DefinitionRef>,
    detail: DependencyReads,
}
/// Unassociated canonical bytes from a worker. Only serial prefix admission
/// may publish the ordinary query association and its local V completion.
pub(super) struct CapturedLower {
    payload: crate::store::Reference,
    reads: Reads,
    acceptance: Option<hirsel_validate::ExecutionReceipt>,
}
#[allow(clippy::too_many_arguments)]
pub(super) fn capture_lower(
    records: &TypeRecords,
    owner: impl Fn(Anchor) -> Option<(DefPath, u32)>,
    store: &dyn crate::store::ResultStore,
    output: &QueryOutput<hirsel_tir::DefinitionRef>,
    detail: &DependencyReads,
    reads: &Reads,
    depth: &crate::memo::DepthEvidence<Span>,
    selections: &[crate::comptime::Selection],
    acceptance: Option<hirsel_validate::ExecutionReceipt>,
) -> Option<CapturedLower> {
    let payload = records.encode_record(
        &LowerRecord {
            output: output.clone(),
            detail: canonical_detail(detail.clone()),
            depth: depth.clone(),
            selections: selections.to_vec(),
        },
        owner,
    )?;
    let payload = store.put_content(
        crate::store::Domain {
            kind: 0x4c4f_5752,
            schema: 1,
        },
        &payload,
    )?;
    Some(CapturedLower {
        payload,
        reads: reads.clone(),
        acceptance,
    })
}

#[derive(Serialize, Deserialize)]
struct LowerIdentityMetadata {
    depth: crate::memo::DepthEvidence<Span>,
    selections: Vec<crate::comptime::Selection>,
}
#[derive(Serialize, Deserialize)]
struct SemanticBodyRecord {
    owner: DefId,
    content: Vec<Vec<u8>>,
    dependencies: BTreeSet<DefPath>,
    selections: Vec<crate::comptime::Selection>,
    depth: crate::memo::DepthEvidence<Span>,
}
#[derive(Serialize, Deserialize)]
struct AuditRecord {
    products: crate::native_audit::SourceAuditProducts,
    selections: Vec<crate::comptime::Selection>,
    depth: crate::memo::DepthEvidence<Span>,
}
#[derive(Serialize, Deserialize)]
struct EvaluationRecord {
    owner: DefId,
    constant: crate::comptime::EvaluatedConstant,
}
#[derive(Serialize, Deserialize)]
pub(super) struct GenerationRecord {
    pub owner: DefId,
    pub body: hirsel_syntax::ExprId,
    pub text: String,
    pub detail: DependencyReads,
    pub depth: crate::memo::DepthEvidence<Span>,
    pub selections: Vec<crate::comptime::Selection>,
    #[serde(skip)]
    pub acceptance: Option<hirsel_validate::ExecutionReceipt>,
}
#[derive(Serialize, Deserialize)]
struct StagedEvaluationRecord {
    declaration: DefId,
    arguments: Vec<u64>,
    constant: crate::comptime::EvaluatedConstant,
}
/// Frozen workers keep V-universe guards while products cross registration
/// epochs. Durable values use the independent local execution association for
/// those same guards; ordinary producer records never carry V authority.
/// Remove only guards covered exactly by the opaque verifier completion.
fn persisted_evaluation(
    value: &crate::comptime::EvaluatedConstant,
) -> Option<crate::comptime::EvaluatedConstant> {
    let mut canonical = value.clone();
    canonical.executed = persisted_execution_reads(&value.executed, value.acceptance.as_ref())?;
    Some(canonical)
}
fn persisted_execution_reads(
    executed: &Reads,
    acceptance: Option<&hirsel_validate::ExecutionReceipt>,
) -> Option<Reads> {
    let mut reads = Vec::new();
    for (read, digest) in executed.iter() {
        if let ReadKey::ValidationUniverse(package) = read {
            if !acceptance?
                .contexts()
                .iter()
                .any(|(id, observed)| *id == package.id() && observed == digest)
            {
                return None;
            }
        } else {
            reads.push((read.clone(), *digest));
        }
    }
    Reads::from_vec(reads).ok()
}
pub(super) fn evaluation_subject(key: &EntryKey, payload: &[u8]) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.evaluated-output.v1",
        [key.0.0.as_slice(), key.1.0.as_slice(), payload],
    )
}
/// Completion facts consumed by checking and source growth, independently of
/// the checked/analyzed body. Its read set is validated before these bytes are
/// decoded; a hit does not demand the separate Modes product.
#[derive(Serialize, Deserialize)]
struct PublicationRecord {
    summary: Arc<PublishedSummary>,
    diagnostics: Vec<Diagnostic>,
    phases: Vec<(Diagnostic, String)>,
    detail: DependencyReads,
    dead_arms: Option<crate::tools::DeadArmUse>,
    lowering: Option<lowering::LoweringUse>,
}
impl PublicationRecord {
    fn valid(
        &self,
        owner: hirsel_syntax::LexicalOwner,
        shape: super::catalog::LexicalShape,
    ) -> bool {
        fn summary(
            value: &DefinitionSummary,
            owner: hirsel_syntax::LexicalOwner,
            shape: super::catalog::LexicalShape,
        ) -> bool {
            value.owner == owner.def
                && value.frames.iter().enumerate().all(|(index, frame)| {
                    shape.valid_site(frame.site)
                        && frame.parent.is_none_or(|parent| (parent as usize) < index)
                })
                && value.calls.iter().all(|call| {
                    (call.owning_frame as usize) < value.frames.len()
                        && call
                            .target_frame
                            .is_none_or(|frame| (frame as usize) < value.frames.len())
                        && (call.binder.owner.def != value.owner
                            || (call.binder.owner == owner && shape.valid_site(call.binder.site)))
                })
                && value.staged.iter().all(|(site, _)| shape.valid_site(*site))
        }
        summary(&self.summary.summary, owner, shape)
            && self
                .summary
                .checked
                .as_ref()
                .is_some_and(|checked| summary(&checked.summary, owner, shape))
    }
}
pub(super) fn canonical_bytes<T: Serialize>(
    types: Arc<hirsel_types::TypeInterner>,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    record: &T,
) -> Option<Vec<u8>> {
    hirsel_product_codec::canonical_bytes(types, record, |anchor| anchor_owner(items, anchor))
}
pub(crate) fn anchor_owner(
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    anchor: Anchor,
) -> Option<(DefPath, u32)> {
    items.iter().find_map(|(path, items)| {
        items
            .iter()
            .find(|item| item.anchor == anchor)
            .map(|item| (path.clone(), item.occurrence))
    })
}
/// V identities include every span, including generated literal bodies. The
/// source map supplies the placed base; the local generation table supplies
/// its owner and exact text. This does not admit producer-supplied anchors.
pub(crate) fn artifact_anchor_owner(
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    sources: &SourceMap,
    generated: &BTreeMap<DefPath, generation::GeneratedSource>,
    anchor: Anchor,
) -> Option<(DefPath, u32)> {
    anchor_owner(items, anchor).or_else(|| {
        let (file, base) = sources.anchor(anchor)?;
        let (owner, source) = generated.iter().find(|(_, source)| source.file == file)?;
        Some((generated_codec_owner(owner, &source.text), base))
    })
}
pub(super) fn generated_codec_owner(owner: &DefPath, text: &str) -> DefPath {
    let hash = ContentHash::of_bytes(text.as_bytes());
    let mut identity = owner.clone();
    identity.0.name = format!(
        "$generated-source:{}:{}",
        owner.0.name,
        hash.0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    identity
}
pub(super) struct Contents(pub(super) Arc<dyn crate::store::ResultStore>);
impl hirsel_artifact_store::Contents for Contents {
    fn get_content(&self, reference: crate::store::Reference) -> Option<crate::store::Bytes> {
        self.0.get_content(reference)
    }
    fn put_content(
        &self,
        domain: crate::store::Domain,
        bytes: &[u8],
    ) -> Option<crate::store::Reference> {
        self.0.put_content(domain, bytes)
    }
}

/// Capture through the exact scalar mode codec using immutable inputs only.
/// Callers must check the selected provider before invoking this: an off store
/// never clones body data or enters canonical type/anchor encoding.
pub(super) fn capture_mode(
    records: &TypeRecords,
    types: Arc<hirsel_types::TypeInterner>,
    owner: impl Fn(Anchor) -> Option<(DefPath, u32)>,
    result: &Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    checked: Option<(&QueryOutput<TypedBody>, &DependencyReads)>,
    phases: Vec<(Diagnostic, String)>,
    detail: DependencyReads,
) -> Option<Vec<u8>> {
    let output = match result {
        Ok(output) => Ok(QueryOutput {
            value: Body::capture(&types, &output.value),
            diagnostics: output.diagnostics.clone(),
            dependencies: output.dependencies.clone(),
        }),
        Err(error) if error.cacheable() => Err(error.clone()),
        Err(_) => return None,
    };
    let checked = checked.map(|(output, detail)| CheckedRecord {
        checked: canonical_checked(&output.value.checked),
        plans: output.value.plans.clone(),
        unit: output.value.unit,
        boolean: output.value.boolean,
        diagnostics: output.diagnostics.clone(),
        dependencies: output.dependencies.clone(),
        detail: canonical_detail(detail.clone()),
    });
    records.encode_record(
        &ModeRecord {
            checked,
            phases,
            output,
            detail: canonical_detail(detail),
        },
        owner,
    )
}

pub(super) struct FrozenMode {
    pub(super) outcome: ModeResult,
    pub(super) checked: Option<(Arc<CheckedSummary>, Arc<Reads>)>,
    pub(super) table_keys: Option<Arc<crate::table_keys::TableKeyUse>>,
    /// The package lint's facts of the checked body, under `signature`.
    pub(super) dead_arms: Option<crate::tools::DeadArmUse>,
}
/// Decode only after the caller has validated the ordinary S0 variant reads.
/// This uses the existing Modes codec and type/lexical validation; no engine
/// query or registration controller is reachable from this helper.
pub(super) fn restore_frozen_mode(
    records: &Arc<TypeRecords>,
    types: &hirsel_types::TypeInterner,
    syntax: Arc<ItemSyntax>,
    citations: &crate::snapshot::OwnedSourceContext,
    text: ContentHash,
    bytes: &[u8],
    reads: Arc<Reads>,
    signature: Option<&hirsel_types::Signature>,
) -> Option<FrozenMode> {
    let record: ModeRecord = records.decode_record(bytes, |path, occurrence| {
        citations
            .items
            .get(path)
            .and_then(|items| items.iter().find(|item| item.occurrence == occurrence))
            .map(|item| item.anchor)
            .or_else(|| citations.catalog.anchor(path, occurrence))
            .or_else(|| citations.owners.record_source_anchor(path, occurrence))
    })?;
    let (encoded, roots) =
        hirsel_types::with_table_collection(|| bincode::serialize_into(std::io::sink(), &record));
    encoded.ok()?;
    if !roots.iter().all(|id| types.get(*id).is_some()) || !record.valid(&syntax) {
        return None;
    }
    let mut table_keys = None;
    let mut dead_arms = None;
    let checked = record.checked.map(|checked| {
        let typed = TypedBody {
            syntax: syntax.clone(),
            checked: checked.checked,
            plans: checked.plans,
            unit: checked.unit,
            boolean: checked.boolean,
        };
        table_keys = Some(Arc::new(crate::table_keys::TableKeyUse::capture(&typed)));
        dead_arms = Some(super::dead_arms::body_use(types, signature, &typed));
        let (summary, growth) = summarize(types, &typed);
        (Arc::new(CheckedSummary { summary, growth }), reads.clone())
    });
    let result = record.output.map(|output| {
        Arc::new(QueryOutput {
            value: output.value.restore(syntax.clone()),
            diagnostics: output.diagnostics,
            dependencies: output.dependencies,
        })
    });
    let summary = result.as_ref().ok().map(|output| {
        let (summary, growth) = summarize(types, &output.value.typed);
        Arc::new(PublishedSummary {
            definition: DefId::intern(&syntax.item.definition),
            summary,
            growth,
            storage: None,
            checked: checked.as_ref().map(|(checked, _)| checked.clone()),
            reads: reads.clone(),
            text,
        })
    });
    Some(FrozenMode {
        outcome: ModeResult {
            definition: syntax.item.definition.clone(),
            phase: result.is_err().then_some(QueryKind::Modes),
            result,
            summary,
            reads,
            diagnostic_phases: record.phases,
        },
        checked,
        table_keys,
        dead_arms,
    })
}

/// Immutable writer shared by serial and worker publication. Keys and expected
/// verification bytes are selected by the engine before freezing; provider I/O
/// owns no query state and retains no encoded payload after write returns.
pub(super) struct EncodedWriter {
    pub(super) types: Arc<TypeRecords>,
    store: Arc<dyn crate::store::ResultStore>,
    key: EntryKey,
    path: DefPath,
    kind: QueryKind,
    expected: Option<Expected>,
}
impl EncodedWriter {
    /// Retain only the archive this completed computation actually published,
    /// after any required byte-for-byte oracle comparison succeeded.
    pub(super) fn write_mode_archive(
        &self,
        reads: Arc<Reads>,
        payload: Vec<u8>,
    ) -> Result<Option<crate::semantic::ModeArchive>, QueryError> {
        let record = self.write_archive(&reads, payload)?;
        Ok(record
            .filter(|_| self.kind == QueryKind::Modes)
            .map(|record| crate::semantic::ModeArchive {
                key: self.key,
                reads,
                record,
            }))
    }
    pub(super) fn write(&self, reads: &Reads, payload: Vec<u8>) -> Result<(), QueryError> {
        self.write_archive(reads, payload).map(|_| ())
    }
    fn write_archive(
        &self,
        reads: &Reads,
        payload: Vec<u8>,
    ) -> Result<Option<crate::store::Reference>, QueryError> {
        let _batch = self.store.local_product_store().map(|store| store.batch());
        if let Some(expected) = &self.expected {
            let matches = expected.matches(reads, &payload);
            if !matches.as_ref().is_ok_and(|same| *same) {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "verify-cache".into(),
                    message: match matches {
                        Ok(_) => format!(
                            "{}: {:?} differs from its cached result",
                            self.path, self.kind
                        ),
                        Err(error) => {
                            format!("{}: {:?} expected bytes: {error}", self.path, self.kind)
                        }
                    },
                }));
            }
        }
        let mut entry = StoredEntry::load(&*self.store, &self.key);
        let Some(variant) = VariantRef::publish(&*self.store, &self.key, reads.clone(), &payload)
        else {
            return Ok(None);
        };
        let record = variant.output;
        entry.insert(variant);
        self.store.put_query(
            self.kind,
            &self.key,
            entry.action(&self.key),
            &entry.encode_for(&self.key),
        );
        Ok(Some(record))
    }
}

impl QueryEngine {
    /// Observe only declared structure before private inference settles. The
    /// producer's own body proof changes inferred_unique, which the declared
    /// projection removes. Every other read and the complete record binding
    /// still validate; this never adopts a full Signature or a private proof.
    pub(super) fn load_declared_signature_value(&mut self, path: &DefPath) -> Option<ContentHash> {
        if !self.s1.is_empty() {
            return None;
        }
        let marker = (path.clone(), QueryKind::PrivateInference);
        if !self.active.insert(marker.clone()) {
            return None;
        }
        let result = (|| {
            let key = self.store_key(path, QueryKind::Signature)?;
            let store = self.result_store.clone()?;
            let entry = StoredEntry::load(&*store, &key);
            let id = DefId::intern(path);
            for variant in entry.variants {
                if variant.inputs != key.1 || !variant.valid(&key) {
                    continue;
                }
                if variant.reads.iter().any(|(read, value)| {
                    !matches!(read, ReadKey::PrivateSummary(owner) if *owner == id)
                        && self.observe(path, read) != Some(*value)
                }) {
                    continue;
                }
                let variant = variant.resolve(&*store, &key)?;
                let record: SignatureRecord = self.decode_record(&variant.payload)?;
                let output = record.output.ok()?;
                if output.value.definition == id {
                    return memos::declared_signature_value(&self.types, &output.value);
                }
            }
            None
        })();
        self.active.remove(&marker);
        result
    }

    pub(super) fn load_signature(
        &mut self,
        path: &DefPath,
    ) -> Option<Arc<Entry<hirsel_types::SignatureRef>>> {
        if !self.s1.is_empty() {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::Signature)?;
        let record: SignatureRecord = self.decode_record(&variant.payload)?;
        if record
            .output
            .as_ref()
            .is_ok_and(|o| o.value.definition != DefId::intern(path))
        {
            return None;
        }
        if !matches!(record.output, Ok(_) | Err(QueryError::Diagnostics(_))) {
            return None;
        }
        let resolution = if let Some((occurrence, output)) = record.resolution
            && self.items.contains_key(path)
        {
            let syntax = self
                .items
                .get(path)?
                .iter()
                .find(|s| s.occurrence == occurrence)?
                .clone();
            if !valid_resolution(&syntax, &output.value) {
                return None;
            }
            Some(ResolverSnapshot { syntax, output })
        } else {
            None
        };
        let representation = if let Some((text_hash, declaration, dependencies)) = record.owner {
            if self.implementation_hash(path)? != text_hash || declaration.representation.is_none()
            {
                return None;
            }
            Some(crate::representation::OwnerRecord {
                text_hash,
                representation: hirsel_check_protocol::header::OwnerRepresentation { declaration },
                dependencies,
                view: Default::default(),
            })
        } else {
            None
        };
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Signature),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        let mut entry = self.adopted(
            path,
            QueryKind::Signature,
            variant,
            record.output,
            record.detail,
        );
        let mutable = Arc::get_mut(&mut entry).expect("unpublished signature");
        mutable.covered = record.covered;
        mutable.products = Arc::new(PhaseProducts {
            signature_persisted: true,
            mode_archive: None,
            resolution,
            representation,
            mode_diagnostics: vec![],
        });
        timings::count(Count::StoreHits);
        timings::count(Count::StoreSignatureHits);
        self.store_adoptions += 1;
        Some(entry)
    }
    pub(super) fn store_signature(
        &mut self,
        path: &DefPath,
        entry: &Entry<hirsel_types::SignatureRef>,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none()
            || entry.products.signature_persisted
            || !entry.consistent
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
        {
            return Ok(());
        }
        if !matches!(entry.memo.value, Ok(_) | Err(QueryError::Diagnostics(_))) {
            return Ok(());
        }
        self.write_record(
            path,
            QueryKind::Signature,
            entry.reads(),
            &SignatureRecord::capture(entry),
        )
    }

    pub(super) fn load_publication(&mut self, path: &DefPath) -> Option<PublishedResult> {
        if !self.s1.is_empty() {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::Summary)?;
        let record: PublicationRecord = self.decode_record(&variant.payload)?;
        let id = DefId::intern(path);
        let (owner, shape) = self.lexical_shape(path)?;
        if record.summary.definition != id
            || record.summary.summary.owner != id
            || record.summary.text != self.key_text(path, QueryKind::Summary).ok()?
            || *record.summary.reads != variant.reads
            || record.summary.checked.is_none()
            || !record.valid(owner, shape)
        {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (id, QueryKind::Summary),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        let checked = record.summary.checked.as_ref()?.clone();
        self.record_checked_summary(
            id,
            checked,
            record.summary.text,
            record.summary.reads.clone(),
            record.detail.clone(),
            true,
        );
        self.record_summary_products(
            record.summary.clone(),
            record.detail,
            true,
            Arc::new(PhaseProducts {
                mode_diagnostics: record.phases.clone(),
                ..Default::default()
            }),
        );
        if let Some(facts) = record.dead_arms {
            self.front_products.dead_arms.insert(id, facts);
        }
        if let Some(facts) = record.lowering {
            self.front_products.lowering_inputs.insert(id, facts);
        }
        timings::count(Count::StoreHits);
        timings::count(Count::StoreSummaryHits);
        self.store_adoptions += 1;
        Some(PublishedResult {
            definition: path.clone(),
            result: Ok(PublishedDefinition {
                summary: record.summary,
                diagnostics: record.diagnostics,
            }),
            phase: Some(QueryKind::Modes),
            diagnostic_phases: record.phases,
            reads: Arc::new(variant.reads),
        })
    }

    pub(super) fn store_publication(
        &mut self,
        outcome: &PublishedResult,
    ) -> Result<(), QueryError> {
        let Ok(published) = &outcome.result else {
            return Ok(());
        };
        let path = &outcome.definition;
        let id = DefId::intern(path);
        if self.result_store.is_none()
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
            || published.summary.checked.is_none()
            || *published.summary.reads != *outcome.reads
        {
            return Ok(());
        }
        let Some(entry) = self.summaries.get(&id).cloned() else {
            return Ok(());
        };
        if !entry.consistent || self.graph.is_dirty(&entry.memo.key) {
            return Ok(());
        }
        self.write_record(
            path,
            QueryKind::Summary,
            &outcome.reads,
            &PublicationRecord {
                summary: published.summary.clone(),
                diagnostics: published.diagnostics.clone(),
                phases: outcome.diagnostic_phases.clone(),
                detail: canonical_detail(entry.detail.clone()),
                dead_arms: self.front_products.dead_arms.get(&id).cloned(),
                lowering: self.front_products.lowering_inputs.get(&id).cloned(),
            },
        )
    }

    pub fn store_adoptions(&self) -> u64 {
        self.store_adoptions
    }
    /// Select a persistent result store. Library engines default to no store;
    /// CLI engines use dir@1. Injected stores keep integration tests isolated.
    pub fn with_result_store(mut self, store: Arc<dyn crate::store::ResultStore>) -> Self {
        self.result_store = store.enabled().then_some(store);
        self
    }
    /// Recompute store hits and compare their canonical bytes (cache oracle).
    pub fn with_verify_store(mut self, verify: crate::store::VerifyStore) -> Self {
        self.verify_store = verify;
        self
    }
    /// The Signature and Modes rows a front request publishes for
    /// `definitions`, as its store demand.
    pub fn front_store_keys(&mut self, definitions: &[DefPath]) -> Vec<EntryKey> {
        let mut keys = Vec::new();
        for path in definitions {
            for kind in [QueryKind::Signature, QueryKind::Modes] {
                keys.extend(self.store_key(path, kind));
            }
        }
        keys
    }
    pub(super) fn store_key(&mut self, path: &DefPath, kind: QueryKind) -> Option<EntryKey> {
        // Source-owned generated Lower products bind the accepted text below.
        // Synthetic instance registrations still replay through S1.
        if !self.items.contains_key(path) && self.catalog.body(path).is_some() {
            if !self.catalog.storeable(path, kind) {
                return None;
            }
        } else {
            let item = self.items.get(path)?.first()?;
            if kind == QueryKind::Generate {
                if generation::generator(item).is_none() || generation::instance_bodied(item) {
                    return None;
                }
            } else if kind != QueryKind::GeneratedRegistration
                && !(kind == QueryKind::Lower && self.source_owned_generator(path))
                && (generation::generator(item).is_some() || self.generated.contains_key(path))
            {
                return None;
            }
            if self
                .sources
                .file(self.item_file(item))?
                .path
                .to_string_lossy()
                .starts_with('<')
            {
                return None;
            }
        }
        let mut text = self.key_text(path, kind).ok()?;
        if kind == QueryKind::Lower && self.source_owned_generator(path) {
            let generated = self.generated.get(path)?;
            text = ContentHash::of_parts(
                "hirsel.generated-lower.inputs.v1",
                [text.0.as_slice(), generated.text.as_bytes()],
            );
        }
        Some(self.product_store_key(path, kind, text))
    }
    /// A canonical product address is independent of its source-admission gate.
    /// Callers must establish the ordinary gate or an opaque accepted view.
    fn product_store_key(&self, path: &DefPath, kind: QueryKind, text: ContentHash) -> EntryKey {
        let env = ContentHash::of_parts(
            "hirsel.persisted-products.v1",
            [
                self.environment_value().0.as_slice(),
                self.store_identity.0.as_slice(),
                self.package_environment(&path.0.package)
                    .package_manifest_hash
                    .0
                    .as_slice(),
            ],
        );
        EntryKey::new(env, path, text, kind, LoweringPolicy::Reachable)
    }
    /// The bodyless header of a generic generated declaration is an ordinary
    /// front product. This gate cannot admit a mono generator or an instance.
    pub(super) fn front_declaration_store_key(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
    ) -> Result<EntryKey, QueryError> {
        let syntax = self.snapshot(path)?;
        if !generation::instance_bodied(&syntax)
            || !matches!(
                kind,
                QueryKind::Modes | QueryKind::Lower | QueryKind::Evaluate
            )
        {
            return Err(QueryError::NotImplemented);
        }
        let text = self.key_text(path, kind)?;
        Ok(self.product_store_key(path, kind, text))
    }
    pub(super) fn accepted_front_store_key(
        &mut self,
        accepted: &AcceptedFrontRegistration,
        kind: QueryKind,
    ) -> Result<EntryKey, QueryError> {
        accepted.validate(self)?;
        if !matches!(
            kind,
            QueryKind::Modes | QueryKind::Lower | QueryKind::Evaluate
        ) {
            return Err(QueryError::NotImplemented);
        }
        let path = accepted.definition();
        let mut text = self.key_text(path, kind)?;
        if kind == QueryKind::Lower {
            text = ContentHash::of_parts(
                "hirsel.generated-lower.inputs.v1",
                [text.0.as_slice(), accepted.text().as_bytes()],
            );
        }
        Ok(self.product_store_key(path, kind, text))
    }
    fn source_owned_generator(&self, path: &DefPath) -> bool {
        self.items
            .get(path)
            .and_then(|items| items.first())
            .is_some_and(|item| {
                generation::generator(item).is_some()
                    && !generation::instance_bodied(item)
                    && self
                        .sources
                        .file(self.item_file(item))
                        .is_some_and(|file| !file.path.to_string_lossy().starts_with('<'))
            })
    }
    fn prepare_lower_generation(&mut self, path: &DefPath) -> Option<()> {
        if self.source_owned_generator(path) {
            // Reconstruct and accept the current generated source before an old
            // payload can resolve its content-bound anchors. This is ordinary
            // generation replay, never authority supplied by the Lower bytes.
            // Anchor preparation is not another execution demand. The checked
            // generation retains its own transitive selection log; a Lower hit
            // replays its stored log, and a miss records the ordinary request.
            // Keep this preparation out of every enclosing selection frame.
            let selections = std::mem::take(&mut self.evaluation_selections);
            let prepared = self.check_query(path);
            self.evaluation_selections = selections;
            prepared.ok()?;
        }
        Some(())
    }
    /// Current owned source identity, including literal generated bodies. The
    /// generated owner includes its exact text hash; arbitrary persisted ids
    /// cannot name an anchor in another source generation.
    pub(crate) fn record_anchor_owner(&self, anchor: Anchor) -> Option<(DefPath, u32)> {
        artifact_anchor_owner(&self.items, &self.sources, &self.generated, anchor)
            .or_else(|| self.catalog.owner(anchor))
    }
    pub(crate) fn record_anchor(&self, path: &DefPath, occurrence: u32) -> Option<Anchor> {
        self.items
            .get(path)
            .and_then(|items| items.iter().find(|item| item.occurrence == occurrence))
            .map(|item| item.anchor)
            .or_else(|| self.catalog.anchor(path, occurrence))
            .or_else(|| {
                self.sources.anchors().find_map(|(anchor, _, _)| {
                    (self.record_anchor_owner(anchor) == Some((path.clone(), occurrence)))
                        .then_some(anchor)
                })
            })
    }
    pub(super) fn encode_record<T: Serialize>(&self, record: &T) -> Option<Vec<u8>> {
        let records = self.type_records()?;
        records.encode_record(record, |anchor| self.record_anchor_owner(anchor))
    }
    pub(super) fn type_records(&self) -> Option<Arc<TypeRecords>> {
        let store = self.result_store.clone()?;
        let mut current = self
            .portable_type_records
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((previous, records)) = &*current
            && Arc::ptr_eq(&records.types, &self.types)
            && Arc::ptr_eq(previous, &store)
        {
            return Some(records.clone());
        }
        let records = TypeRecords::new(self.types.clone(), Arc::new(Contents(store.clone())));
        *current = Some((store, records.clone()));
        Some(records)
    }
    /// A staged declaration template was prepared in this exact base context.
    /// Revalidating its old source body cannot redo private preparation of the
    /// registration. A changed universe must demand that preparation again.
    fn load_staged_lower_receipt(
        &mut self,
        subject: ContentHash,
    ) -> Option<hirsel_validate::ExecutionReceipt> {
        let cache = self.result_store.as_ref()?.verifier()?;
        let snapshot = self.signature_snapshot();
        cache.execution(subject, |package| snapshot.validation_input_digest(package))
    }
    fn load_execution_receipt(
        &mut self,
        subject: ContentHash,
    ) -> Option<hirsel_validate::ExecutionReceipt> {
        let cache = self.result_store.as_ref()?.verifier()?;
        let snapshot = self.signature_snapshot();
        cache.execution_revalidated(
            subject,
            self.types.clone(),
            |path, occurrence| {
                self.items
                    .get(path)
                    .and_then(|items| items.iter().find(|item| item.occurrence == occurrence))
                    .map(|item| item.anchor)
                    .or_else(|| self.catalog.anchor(path, occurrence))
                    .or_else(|| {
                        self.sources.anchors().find_map(|(anchor, _, _)| {
                            (artifact_anchor_owner(
                                &self.items,
                                &self.sources,
                                &self.generated,
                                anchor,
                            ) == Some((path.clone(), occurrence)))
                            .then_some(anchor)
                        })
                    })
            },
            &*snapshot,
        )
    }
    pub(super) fn decode_record<T: serde::de::DeserializeOwned + Serialize>(
        &self,
        bytes: &[u8],
    ) -> Option<T> {
        let records = self.type_records()?;
        let record: T = records.decode_record(bytes, |path, occurrence| {
            self.record_anchor(path, occurrence)
        })?;
        // Check every nested payload TypeId before any semantic consumer indexes it.
        let (encoded, roots) = hirsel_types::with_table_collection(|| {
            bincode::serialize_into(std::io::sink(), &record)
        });
        encoded.ok()?;
        roots
            .iter()
            .all(|id| self.types.get(*id).is_some())
            .then_some(record)
    }
    fn stored_variant(&mut self, path: &DefPath, kind: QueryKind) -> Option<Variant> {
        let key = self.store_key(path, kind)?;
        self.stored_variant_at(path, &key)
    }
    fn stored_variant_at(&mut self, owner: &DefPath, key: &EntryKey) -> Option<Variant> {
        let _time = timings::phase(Phase::StoreLookup);
        let store = self.result_store.clone()?;
        let entry = StoredEntry::load(&*store, key);
        let hit = entry.resolve_current(&*store, key, |read| {
            if matches!(
                key.2,
                QueryKind::Evaluate
                    | QueryKind::Lower
                    | QueryKind::Generate
                    | QueryKind::SourceAudit
                    | QueryKind::SemanticBody
            ) && let ReadKey::Signature(id) | ReadKey::ExecutedSignature(id) = read
                && id.path().0.name.starts_with("$comptime:")
                && !self.staged_constants.contains_key(id.path())
            {
                let declaration_owner = self.staged_declaration_owner(id.path())?;
                self.load_registrations(&declaration_owner)?;
                self.staged_constants.get(id.path())?;
            }
            if matches!(
                key.2,
                QueryKind::Evaluate
                    | QueryKind::Lower
                    | QueryKind::Generate
                    | QueryKind::SourceAudit
                    | QueryKind::SemanticBody
            ) && let Some(id) = read.definition()
                && id.path().0.name.contains("$instance$")
                && !self.items.contains_key(id.path())
            {
                self.load_generated_registration(id.path())?;
            }
            self.observe(owner, read)
        });
        if hit.is_none() {
            timings::count(Count::StoreMisses);
            match key.2 {
                QueryKind::Signature => timings::count(Count::StoreSignatureMisses),
                QueryKind::Summary => timings::count(Count::StoreSummaryMisses),
                QueryKind::Modes => timings::count(Count::StoreModeMisses),
                _ => (),
            }
        }
        hit
    }
    fn write_record<T: Serialize>(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        reads: &Reads,
        record: &T,
    ) -> Result<(), QueryError> {
        self.write_encoded(path, kind, reads, |engine| engine.encode_record(record))
    }
    fn write_encoded(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        reads: &Reads,
        encode: impl FnOnce(&Self) -> Option<Vec<u8>>,
    ) -> Result<(), QueryError> {
        let Some(store) = self.result_store.clone() else {
            return Ok(());
        };
        let Some(key) = self.store_key(path, kind) else {
            return Ok(());
        };
        let Some(payload) = encode(self) else {
            return Ok(());
        };
        let expected = self
            .pending_store_checks
            .remove(&(DefId::intern(path), kind));
        EncodedWriter {
            types: self.type_records().expect("enabled result store"),
            store,
            key,
            path: path.clone(),
            kind,
            expected,
        }
        .write(reads, payload)
    }

    fn adopted<T>(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        variant: Variant,
        value: Result<QueryOutput<T>, QueryError>,
        detail: DependencyReads,
    ) -> Arc<Entry<T>> {
        let key = MemoKey::new(kind, DefId::intern(path));
        if kind == QueryKind::Lower {
            self.front_products.forget(&key);
        }
        let reads = Arc::new(variant.reads);
        let durability = reads.durability(|read| self.durability_of(read));
        let text = self.key_text(path, kind).expect("indexed");
        let memo = self
            .graph
            .adopt_value(key, text, value.map(Arc::new), reads, durability);
        Arc::new(Entry {
            memo,
            detail,
            covered: vec![],
            depth: None,
            acceptance: None,
            verification: Default::default(),
            selections: vec![],
            consistent: true,
            products: Default::default(),
        })
    }
    /// Restore only an existing ordinary body before execution owns an S1 frame.
    /// A miss must keep the original cycle, failure and fuel evaluation path.
    pub(crate) fn restore_execution_mode(&mut self, path: &DefPath) {
        if !self.modes.contains_key(&DefId::intern(path))
            && let Some(entry) = self.load_mode(path)
        {
            self.modes.insert(DefId::intern(path), entry);
        }
    }

    fn restore_execution_dependencies(&mut self, reads: &Reads) {
        for (read, _) in reads.iter() {
            if let ReadKey::ExecutedBody(id) = read {
                self.restore_execution_mode(id.path());
            }
        }
    }

    pub(super) fn load_mode(&mut self, path: &DefPath) -> Option<Arc<Entry<AnalyzedBody>>> {
        if !self.s1.is_empty() {
            return None;
        }
        let key = self.store_key(path, QueryKind::Modes)?;
        let candidate = self
            .summaries
            .get(&DefId::intern(path))
            .filter(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
            .and_then(|entry| entry.products.mode_archive.clone());
        let own = candidate
            .filter(|archive| archive.key == key)
            .filter(|archive| {
                archive
                    .reads
                    .iter()
                    .all(|(read, value)| self.observe(path, read) == Some(*value))
            })
            .and_then(|archive| {
                let payload = self.result_store.as_ref()?.get_content(archive.record)?;
                Some((
                    Variant {
                        inputs: key.1,
                        reads: (*archive.reads).clone(),
                        payload,
                    },
                    archive,
                ))
            });
        let (variant, mode_archive) = match own {
            Some((variant, archive)) => (variant, Some(archive)),
            None => (self.stored_variant(path, QueryKind::Modes)?, None),
        };
        let record: ModeRecord = self.decode_record(&variant.payload)?;
        // An ordinary product hit can start from declaration heads alone.
        // Materialize only this demanded body's syntax after validating its reads.
        self.materialize(path).ok()?;
        let syntax = self.snapshot(path).ok()?;
        if !record.valid(&syntax) {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute && mode_archive.is_none() {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Modes),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        let output = record.output.map(|o| QueryOutput {
            value: o.value.restore(syntax),
            diagnostics: o.diagnostics,
            dependencies: o.dependencies,
        });
        if let Some(c) = record.checked {
            let checked = QueryOutput {
                value: TypedBody {
                    syntax: self.snapshot(path).ok()?,
                    checked: c.checked,
                    plans: c.plans,
                    unit: c.unit,
                    boolean: c.boolean,
                },
                diagnostics: c.diagnostics,
                dependencies: c.dependencies,
            };
            let entry = self.adopted(
                path,
                QueryKind::Check,
                variant.clone(),
                Ok(checked),
                c.detail,
            );
            self.checked.insert(DefId::intern(path), entry);
        }
        if mode_archive.is_none() {
            timings::count(Count::StoreHits);
            timings::count(Count::StoreModeHits);
            self.store_adoptions += 1;
        }
        let mut entry = self.adopted(path, QueryKind::Modes, variant, output, record.detail);
        Arc::get_mut(&mut entry)
            .expect("restored mode entry is not yet shared")
            .products = Arc::new(PhaseProducts {
            mode_diagnostics: record.phases,
            mode_archive,
            ..Default::default()
        });
        Some(entry)
    }
    pub(super) fn store_mode(
        &mut self,
        path: &DefPath,
        entry: &Entry<AnalyzedBody>,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none()
            || !entry.consistent
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
        {
            return Ok(());
        }
        let output = entry.result();
        let diagnostics = match &output {
            Ok(o) => &o.diagnostics,
            Err(QueryError::Diagnostics(ds)) => ds,
            _ => return Ok(()),
        };
        let phases = diagnostics
            .iter()
            .filter_map(|d| {
                entry
                    .products
                    .mode_diagnostics
                    .iter()
                    .find(|(diagnostic, _)| {
                        diagnostic.rule == d.rule && diagnostic.primary == d.primary
                    })
                    .map(|(_, phase)| phase.clone())
                    .or_else(|| self.diagnostic_phase(d))
                    .map(|phase| (d.clone(), phase))
            })
            .collect();
        self.write_encoded(path, QueryKind::Modes, entry.reads(), |engine| {
            let records = engine.type_records()?;
            let checked = engine
                .checked
                .get(&DefId::intern(path))
                .and_then(|entry| entry.result().ok().map(|output| (output, &entry.detail)));
            capture_mode(
                &records,
                engine.types.clone(),
                |anchor| engine.record_anchor_owner(anchor),
                &output,
                checked
                    .as_ref()
                    .map(|(output, detail)| (&**output, *detail)),
                phases,
                entry.detail.clone(),
            )
        })
    }
    /// Snapshot this task's store policy without consuming its pending
    /// verification obligation; canonical merge retires it only after encoding.
    pub(super) fn mode_writer(&mut self, path: &DefPath) -> Option<EncodedWriter> {
        if !self.s1.is_empty() || !self.s1.failure_is_own() {
            return None;
        }
        Some(EncodedWriter {
            types: self.type_records()?,
            store: self.result_store.clone()?,
            key: self.store_key(path, QueryKind::Modes)?,
            path: path.clone(),
            kind: QueryKind::Modes,
            expected: self
                .pending_store_checks
                .get(&(DefId::intern(path), QueryKind::Modes))
                .cloned(),
        })
    }
    pub(super) fn finish_mode_write(&mut self, path: &DefPath) {
        self.pending_store_checks
            .remove(&(DefId::intern(path), QueryKind::Modes));
    }
    pub(crate) fn load_registrations(&mut self, path: &DefPath) -> Option<()> {
        if !self.s1.is_empty() {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::Registration)?;
        let record: Registrations = self.decode_record(&variant.payload)?;
        let (owner, shape) = self.lexical_shape(path)?;
        let facts = self.source_facts(path)?;
        if !record.valid(owner, shape, &facts.comptime, &self.types) {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Registration),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        for row in record.rows {
            self.install_staged(row);
        }
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some(())
    }
    pub(crate) fn store_registrations(&mut self, path: &DefPath) -> Result<(), QueryError> {
        if self.result_store.is_none() || !self.s1.is_empty() || !self.s1.failure_is_own() {
            return Ok(());
        }
        let reads = self
            .modes
            .get(&DefId::intern(path))
            .filter(|e| e.consistent)
            .map(|entry| entry.reads().clone())
            .or_else(|| {
                self.summaries
                    .get(&DefId::intern(path))
                    .filter(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
                    .map(|entry| entry.reads().clone())
            });
        let Some(reads) = reads else {
            return Ok(());
        };
        let rows: Vec<_> = self
            .staged_constants
            .iter()
            .filter(|(_, c)| c.owner == *path)
            .map(|(p, c)| crate::comptime::StagedRegistration {
                path: p.clone(),
                constant: c.clone(),
            })
            .collect();
        if rows.is_empty() {
            return Ok(());
        }
        let record = Registrations {
            owner: DefId::intern(path),
            rows,
        };
        self.write_record(path, QueryKind::Registration, &reads, &record)
    }

    fn generated_registration_key(&mut self, instance: &DefPath) -> Option<(DefPath, EntryKey)> {
        let (name, suffix) = instance.0.name.rsplit_once("$instance$")?;
        if suffix.len() != 16
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return None;
        }
        let mut declaration = instance.clone();
        declaration.0.name = name.into();
        let item = self.items.get(&declaration)?.first()?;
        if !generation::instance_bodied(item) {
            return None;
        }
        let mut key = self.store_key(&declaration, QueryKind::GeneratedRegistration)?;
        key.0 = ContentHash::of_parts(
            "hirsel.result.generated-registration-address.v1",
            [key.0.0.as_slice(), instance.fingerprint().0.as_slice()],
        );
        key.2 = QueryKind::GeneratedRegistration;
        Some((declaration, key))
    }
    fn valid_generated_registration(
        &self,
        instance: &DefPath,
        record: &GeneratedRegistration,
    ) -> bool {
        let Some(syntax) = self
            .items
            .get(record.declaration.path())
            .and_then(|items| items.first())
        else {
            return false;
        };
        let hirsel_syntax::ItemKind::Function { signature, .. } = &syntax.item.kind else {
            return false;
        };
        if !generation::instance_bodied(syntax)
            || signature.generics.len() != record.arguments.len()
            || !signature
                .generics
                .iter()
                .zip(&record.arguments)
                .all(|(parameter, argument)| {
                    matches!(
                        (&parameter.kind, argument),
                        (
                            hirsel_syntax::GenericKind::Type(_),
                            hirsel_types::GenericArgument::Type(_)
                        ) | (
                            hirsel_syntax::GenericKind::Size,
                            hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(_))
                        )
                    )
                })
        {
            return false;
        }
        let Some(key) = hirsel_tir::InstanceBodies::key(&self.types, &record.arguments) else {
            return false;
        };
        record.instance.path() == instance
            && generation::instance_path(record.declaration.path(), key) == *instance
    }
    pub(super) fn store_generated_registration(
        &mut self,
        declaration: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
        instance: &DefPath,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none() || !self.s1.failure_is_own() {
            return Ok(());
        }
        let Some((owner, key)) = self.generated_registration_key(instance) else {
            return Ok(());
        };
        if owner != *declaration {
            return Ok(());
        }
        let record = GeneratedRegistration {
            declaration: DefId::intern(declaration),
            arguments: arguments.to_vec(),
            instance: DefId::intern(instance),
        };
        if !self.valid_generated_registration(instance, &record) {
            return Ok(());
        }
        let Some(payload) = self.encode_record(&record) else {
            return Ok(());
        };
        let mut reads = ReadsBuilder::new();
        reads.record(
            ReadKey::ExecutedBody(record.declaration),
            memos::optional(self.implementation_hash(declaration)),
        );
        let reads = reads.finish().map_err(|_| QueryError::NotImplemented)?;
        EncodedWriter {
            types: self.type_records().expect("enabled registration types"),
            store: self
                .result_store
                .as_ref()
                .expect("enabled registration store")
                .clone(),
            key,
            path: instance.clone(),
            kind: QueryKind::GeneratedRegistration,
            expected: self
                .pending_store_checks
                .remove(&(record.instance, QueryKind::GeneratedRegistration)),
        }
        .write(&reads, payload)
    }
    pub(super) fn load_generated_registration(&mut self, instance: &DefPath) -> Option<()> {
        let (declaration, key) = self.generated_registration_key(instance)?;
        let variant = self.stored_variant_at(&declaration, &key)?;
        let record: GeneratedRegistration = self.decode_record(&variant.payload)?;
        if record.declaration.path() != &declaration
            || !self.valid_generated_registration(instance, &record)
        {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (record.instance, QueryKind::GeneratedRegistration),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        let site = self.snapshot(&declaration).ok()?.item.name_span;
        let (registered, _) = self
            .register_generated_instance(
                &declaration,
                &hirsel_types::Actuals {
                    arguments: record.arguments,
                },
                site,
            )
            .ok()?;
        if registered != *instance {
            return None;
        }
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some(())
    }

    pub(crate) fn lower_requires_execution(&self, path: &DefPath, reads: &Reads) -> bool {
        reads.iter().any(|(read, _)| {
            matches!(
                read,
                ReadKey::ExecutedBody(_) | ReadKey::ValidationUniverse(_)
            )
        }) || self.source_facts(path).is_some_and(|facts| facts.execution)
    }
    fn staged_declaration_owner(&self, path: &DefPath) -> Option<DefPath> {
        if let Some(constant) = self.staged_constants.get(path) {
            return Some(constant.owner.clone());
        }
        let (name, ordinal) = path.0.name.strip_prefix("$comptime:")?.rsplit_once(':')?;
        if ordinal.parse::<u32>().ok()?.to_string() != ordinal {
            return None;
        }
        let mut owner = path.clone();
        owner.0.name = name.into();
        self.declared(&owner).then_some(owner)
    }
    fn portable_evaluation_reads(&self, value: &crate::comptime::EvaluatedConstant) -> bool {
        self.portable_selection_reads(None, &value.executed, &value.selections)
    }
    fn portable_selection_reads(
        &self,
        owner: Option<&DefPath>,
        reads: &Reads,
        selections: &[crate::comptime::Selection],
    ) -> bool {
        use crate::comptime::Selection;
        reads.iter().all(|(read, _)| {
            let Some(id) = read.definition() else { return true };
            let path = id.path();
            if let Some(constant) = self.staged_constants.get(path) {
                return matches!(read, ReadKey::Signature(_) | ReadKey::ExecutedSignature(_))
                    && (owner == Some(&constant.owner) || selections.iter().any(|selection| matches!(selection, Selection::Staged(selected, _) if *selected == id)));
            }
            // Merely calling a declared generated function reads its public
            // signature, not its generated implementation or registration.
            if matches!(read, ReadKey::Signature(_) | ReadKey::ExecutedSignature(_))
                && self.source_owned_generator(path)
            {
                return true;
            }
            if self.generated.contains_key(path) || path.0.name.contains("$instance$")
                || self.items.get(path).and_then(|items| items.first()).is_some_and(|syntax| generation::generator(syntax).is_some() && !generation::instance_bodied(syntax))
            {
                return owner == Some(path) || selections.contains(&Selection::Generated(id));
            }
            !path.0.name.starts_with('$')
        }) && selections.iter().all(|selection| match selection {
            Selection::Staged(id, arguments) => {
                self.staged_constants.get(id.path()).is_some_and(|constant| constant.inputs.len() == arguments.len())
                    && reads.iter().any(|(read, _)| matches!(read, ReadKey::Signature(observed) | ReadKey::ExecutedSignature(observed) if observed == id))
            }
            Selection::Generated(id) => {
                self.items.get(id.path()).and_then(|items| items.first()).is_some_and(|syntax| generation::generator(syntax).is_some() && !generation::instance_bodied(syntax))
                    && reads.iter().any(|(read, _)| matches!(read, ReadKey::ExecutedBody(observed) if observed == id))
                    && reads.iter().any(|(read, _)| matches!(read, ReadKey::Signature(observed) | ReadKey::ExecutedSignature(observed) if observed == id))
            }
        })
    }
    fn replay_evaluation_selections(
        &mut self,
        value: &crate::comptime::EvaluatedConstant,
    ) -> Option<()> {
        if !self.portable_evaluation_reads(value) {
            return None;
        }
        self.replay_selections(&value.selections)
    }
    pub(super) fn replay_selections(
        &mut self,
        selections: &[crate::comptime::Selection],
    ) -> Option<()> {
        use crate::comptime::Selection;
        for selection in selections {
            match selection {
                Selection::Staged(id, arguments) => {
                    let key = (id.path().clone(), arguments.clone());
                    let memory = self.staged_values.get(&key).cloned().filter(|old| {
                        self.evaluation_validation_holds(old)
                            && self.execution_valid(id.path(), &old.executed)
                    });
                    let selected =
                        memory.or_else(|| self.load_staged_evaluation(id.path(), arguments))?;
                    self.staged_values.insert(key, selected);
                }
                Selection::Generated(id) => {
                    let old = self.checked.get(id).cloned();
                    let valid = old.is_some_and(|entry| {
                        self.key_text(id.path(), QueryKind::Check)
                            .ok()
                            .is_some_and(|text| self.memo_valid(id.path(), &entry, text))
                            && self.lower_acceptance_holds(entry.acceptance.as_ref())
                            && entry.result().is_ok()
                    });
                    if !valid {
                        self.check_query(id.path()).ok()?;
                    }
                }
            }
        }
        Some(())
    }
    fn store_evaluation_selections(
        &mut self,
        value: &crate::comptime::EvaluatedConstant,
    ) -> Result<bool, QueryError> {
        if !self.portable_evaluation_reads(value) {
            return Ok(false);
        }
        self.store_selections(&value.selections)
    }
    fn store_selections(
        &mut self,
        selections: &[crate::comptime::Selection],
    ) -> Result<bool, QueryError> {
        use crate::comptime::Selection;
        for selection in selections {
            match selection {
                Selection::Staged(id, arguments) => {
                    let Some(selected) = self
                        .staged_values
                        .get(&(id.path().clone(), arguments.clone()))
                        .cloned()
                    else {
                        return Ok(false);
                    };
                    let Some(owner) = self.staged_declaration_owner(id.path()) else {
                        return Ok(false);
                    };
                    self.store_registrations(&owner)?;
                    self.store_staged_evaluation(id.path(), arguments, &selected)?;
                }
                Selection::Generated(id) => {
                    if !self.generated.contains_key(id.path()) {
                        return Ok(false);
                    }
                    if let Some((declaration, instance)) =
                        self.instance_bodies.instances.iter().find_map(
                            |(declaration, instances)| {
                                instances
                                    .values()
                                    .find(|instance| instance.definition == *id)
                                    .map(|instance| (*declaration, instance.clone()))
                            },
                        )
                    {
                        self.store_generated_registration(
                            declaration.path(),
                            &instance.arguments,
                            id.path(),
                        )?;
                    }
                }
            }
        }
        Ok(true)
    }
    fn portable_lower_reads(
        &self,
        owner: &DefPath,
        reads: &Reads,
        selections: &[crate::comptime::Selection],
    ) -> bool {
        self.portable_selection_reads(Some(owner), reads, selections)
    }
    fn valid_staged_lowering(&self, owner: &DefPath, definition: &hirsel_tir::Definition) -> bool {
        definition
            .bodies
            .iter()
            .flat_map(|body| &body.rvalues)
            .all(|value| match value {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => {
                    self.staged_constants
                        .get(definition)
                        .is_some_and(|constant| {
                            constant.owner == *owner && constant.inputs.len() == arguments.len()
                        })
                }
                _ => true,
            })
    }
    pub(super) fn load_generation(
        &mut self,
        path: &DefPath,
        body: hirsel_syntax::ExprId,
    ) -> Option<(GenerationRecord, Reads)> {
        let key = self.store_key(path, QueryKind::Generate)?;
        let variant = self.stored_variant(path, QueryKind::Generate)?;
        let mut record: GenerationRecord = self.decode_record(&variant.payload)?;
        if record.owner != DefId::intern(path)
            || record.body != body
            || !self.portable_lower_reads(path, &variant.reads, &record.selections)
            || record.depth.bounded
            || record.depth.first.len() != self.depth_limit() as usize + 1
            || record
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
        {
            return None;
        }
        if self.verify == Verify::On {
            record.acceptance =
                Some(self.load_execution_receipt(evaluation_subject(&key, &variant.payload))?);
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Generate),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        self.replay_selections(&record.selections)?;
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some((record, variant.reads))
    }
    pub(super) fn store_generation(
        &mut self,
        path: &DefPath,
        reads: &Reads,
        mut record: GenerationRecord,
    ) -> Result<(), QueryError> {
        let Some(reads) = persisted_execution_reads(reads, record.acceptance.as_ref()) else {
            return Ok(());
        };
        if self.result_store.is_none()
            || !self.s1.failure_is_own()
            || !self.portable_lower_reads(path, &reads, &record.selections)
            || !self.store_selections(&record.selections)?
        {
            return Ok(());
        }
        let Some(key) = self.store_key(path, QueryKind::Generate) else {
            return Ok(());
        };
        record.detail = canonical_detail(record.detail);
        let Some(payload) = self.encode_record(&record) else {
            return Ok(());
        };
        let store = self
            .result_store
            .as_ref()
            .expect("enabled generation store")
            .clone();
        if let Some(receipt) = &record.acceptance
            && let Some(cache) = store.verifier()
        {
            cache.publish_execution(evaluation_subject(&key, &payload), receipt);
        }
        EncodedWriter {
            types: self.type_records().expect("enabled generation types"),
            store,
            key,
            path: path.clone(),
            kind: QueryKind::Generate,
            expected: self
                .pending_store_checks
                .remove(&(DefId::intern(path), QueryKind::Generate)),
        }
        .write(&reads, payload)
    }
    pub(super) fn load_semantic_body(
        &mut self,
        path: &DefPath,
    ) -> Option<(Vec<Vec<u8>>, BTreeSet<DefPath>)> {
        if !self.s1.is_empty() {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::SemanticBody)?;
        let record: SemanticBodyRecord = self.decode_record(&variant.payload)?;
        if record.owner != DefId::intern(path)
            || record.content.is_empty()
            || !self.portable_lower_reads(path, &variant.reads, &record.selections)
            || record.depth.first.len() != self.depth_limit() as usize + 1
            || record
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
        {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (record.owner, QueryKind::SemanticBody),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        if self.lower_requires_execution(path, &variant.reads) {
            let key = self.store_key(path, QueryKind::SemanticBody)?;
            let receipt =
                self.load_execution_receipt(evaluation_subject(&key, &variant.payload))?;
            self.merge_lower_acceptance(Some(&receipt), true);
        }
        self.replay_selections(&record.selections)?;
        self.depth_replay_requests(&record.depth).ok()?;
        self.merge_execution(&variant.reads);
        self.record_selections(&record.selections);
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some((record.content, record.dependencies))
    }
    pub(super) fn store_semantic_body(
        &mut self,
        path: &DefPath,
        node: &(Vec<Vec<u8>>, BTreeSet<DefPath>),
        reads: &Reads,
        depth: crate::memo::DepthEvidence<Span>,
        selections: Vec<crate::comptime::Selection>,
        acceptance: Option<&hirsel_validate::ExecutionReceipt>,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none()
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
            || !self.portable_lower_reads(path, reads, &selections)
            || !self.store_selections(&selections)?
        {
            return Ok(());
        }
        // The identity remains a producer product. Only the local verifier
        // association can cover a removed V-universe guard, and all other
        // semantic reads remain in the ordinary dependency record unchanged.
        let Some(reads) = persisted_execution_reads(reads, acceptance) else {
            return Ok(());
        };
        let Some(key) = self.store_key(path, QueryKind::SemanticBody) else {
            return Ok(());
        };
        let Some(payload) = self.encode_record(&SemanticBodyRecord {
            owner: DefId::intern(path),
            content: node.0.clone(),
            dependencies: node.1.clone(),
            depth,
            selections,
        }) else {
            return Ok(());
        };
        let store = self
            .result_store
            .as_ref()
            .expect("enabled semantic store")
            .clone();
        if let Some(receipt) = acceptance
            && let Some(cache) = store.verifier()
        {
            cache.publish_execution(evaluation_subject(&key, &payload), receipt);
        }
        EncodedWriter {
            types: self.type_records().expect("enabled semantic types"),
            store,
            key,
            path: path.clone(),
            kind: QueryKind::SemanticBody,
            expected: self
                .pending_store_checks
                .remove(&(DefId::intern(path), QueryKind::SemanticBody)),
        }
        .write(&reads, payload)
    }
    pub(crate) fn load_source_audit(
        &mut self,
        path: &DefPath,
    ) -> Option<crate::native_audit::SourceAuditProducts> {
        if !self.s1.is_empty() {
            return None;
        }
        let key = self.store_key(path, QueryKind::SourceAudit)?;
        let variant = self.stored_variant(path, QueryKind::SourceAudit)?;
        let record: AuditRecord = self.decode_record(&variant.payload)?;
        let (_, shape) = self.lexical_shape(path)?;
        let audit = &record.products.audit;
        if audit.definition != *path
            || Some(audit.implementation_revision) != self.implementation_hash(path)
            || !self.portable_lower_reads(path, &variant.reads, &record.selections)
            || record.depth.first.len() != self.depth_limit() as usize + 1
            || record
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
            || audit
                .closures
                .iter()
                .chain(&audit.invocations)
                .map(|(id, _)| id)
                .chain(record.products.materialized.iter().map(|(id, _)| id))
                .any(|id| !shape.valid_site(hirsel_types::NameSite::Expression(*id)))
            || self.signature_output(path).ok()?.value.fingerprint != audit.signature
        {
            return None;
        }
        let required = self.lower_requires_execution(path, &variant.reads);
        let acceptance = if required && self.verify == Verify::On {
            Some(self.load_execution_receipt(evaluation_subject(&key, &variant.payload))?)
        } else {
            None
        };
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::SourceAudit),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        self.replay_selections(&record.selections)?;
        self.depth_replay_requests(&record.depth).ok()?;
        self.merge_execution(&variant.reads);
        self.merge_lower_acceptance(acceptance.as_ref(), required);
        self.record_selections(&record.selections);
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some(record.products)
    }
    pub(crate) fn store_source_audit(
        &mut self,
        products: &crate::native_audit::SourceAuditProducts,
        reads: &Reads,
        depth: crate::memo::DepthEvidence<Span>,
        selections: Vec<crate::comptime::Selection>,
        acceptance: Option<hirsel_validate::ExecutionReceipt>,
    ) -> Result<(), QueryError> {
        let path = &products.audit.definition;
        if self.result_store.is_none() || !self.s1.is_empty() || !self.s1.failure_is_own() {
            return Ok(());
        }
        let Some(reads) = persisted_execution_reads(reads, acceptance.as_ref()) else {
            return Ok(());
        };
        if !self.portable_lower_reads(path, &reads, &selections)
            || !self.store_selections(&selections)?
        {
            return Ok(());
        }
        let Some(key) = self.store_key(path, QueryKind::SourceAudit) else {
            return Ok(());
        };
        let Some(payload) = self.encode_record(&AuditRecord {
            products: products.clone(),
            depth,
            selections,
        }) else {
            return Ok(());
        };
        let store = self
            .result_store
            .as_ref()
            .expect("enabled audit store")
            .clone();
        if let Some(receipt) = acceptance.as_ref()
            && let Some(cache) = store.verifier()
        {
            cache.publish_execution(evaluation_subject(&key, &payload), receipt);
        }
        EncodedWriter {
            types: self.type_records().expect("enabled audit types"),
            store,
            key,
            path: path.clone(),
            kind: QueryKind::SourceAudit,
            expected: self
                .pending_store_checks
                .remove(&(DefId::intern(path), QueryKind::SourceAudit)),
        }
        .write(&reads, payload)
    }
    pub(super) fn load_lower(
        &mut self,
        path: &DefPath,
    ) -> Option<Arc<Entry<hirsel_tir::DefinitionRef>>> {
        self.load_lower_with_replay(path, &mut false)
    }
    /// A current producer body may lack this consumer's private execution
    /// receipt. Its admitted root must independently replay through the normal
    /// Lower publication path before a check-only worker drops the body.
    pub(super) fn load_lower_with_replay(
        &mut self,
        path: &DefPath,
        replay: &mut bool,
    ) -> Option<Arc<Entry<hirsel_tir::DefinitionRef>>> {
        if !self.s1.is_empty() {
            return None;
        }
        // A durable lookup must not bypass this session's rejected private
        // context. Fresh readers may independently revalidate retained bytes;
        // an invalidated resident completion follows ordinary recomputation.
        let id = DefId::intern(path);
        let retained = self
            .lowered
            .get(&id)
            .and_then(|entry| entry.acceptance.clone())
            .or_else(|| {
                self.lower_receipts
                    .get(&id)
                    .and_then(|entry| entry.acceptance.clone())
            });
        if retained
            .as_ref()
            .is_some_and(|receipt| !self.lower_acceptance_holds(Some(receipt)))
        {
            return None;
        }
        self.result_store.as_ref()?;
        self.prepare_lower_generation(path)?;
        let variant = self.stored_variant(path, QueryKind::Lower)?;
        let record: LowerRecord = self.decode_record(&variant.payload)?;
        if record.output.value.path != DefId::intern(path)
            || !self.portable_lower_reads(path, &variant.reads, &record.selections)
            || record.depth.first.len() != self.depth_limit() as usize + 1
            || record
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
        {
            return None;
        }
        let staged = crate::comptime::has_staged_reads(&record.output.value);
        if staged {
            self.load_registrations(path)?;
            if !self.valid_staged_lowering(path, &record.output.value) {
                return None;
            }
        }
        let acceptance =
            if self.verify == Verify::On && self.lower_requires_execution(path, &variant.reads) {
                let key = self.store_key(path, QueryKind::Lower)?;
                let subject = evaluation_subject(&key, &variant.payload);
                let receipt = if staged {
                    self.load_staged_lower_receipt(subject)
                } else {
                    self.load_execution_receipt(subject)
                };
                let Some(receipt) = receipt else {
                    // A body product does not supply private execution authority.
                    // Retain only existing mode inputs for the required replay.
                    self.restore_execution_dependencies(&variant.reads);
                    *replay = true;
                    return None;
                };
                Some(receipt)
            } else {
                None
            };
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Lower),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        let verification = self.ensure_lowered_verdict(&record.output.value).ok()?;
        self.replay_selections(&record.selections)?;
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        let mut entry = self.adopted(
            path,
            QueryKind::Lower,
            variant,
            Ok(record.output),
            record.detail,
        );
        let unpublished = Arc::get_mut(&mut entry).expect("unpublished lower record");
        unpublished.depth = Some(record.depth);
        unpublished.verification = std::sync::Mutex::new(verification);
        unpublished.acceptance = acceptance;
        unpublished.selections = record.selections;
        Some(entry)
    }
    pub(crate) fn load_evaluation(
        &mut self,
        path: &DefPath,
    ) -> Option<crate::comptime::EvaluatedConstant> {
        if !self.s1.is_empty() {
            return None;
        }
        let key = self.store_key(path, QueryKind::Evaluate)?;
        let variant = self.stored_variant(path, QueryKind::Evaluate)?;
        let record: EvaluationRecord = self.decode_record(&variant.payload)?;
        let mut value = record.constant;
        let (owner, shape) = self.lexical_shape(path)?;
        if record.owner != owner.def
            || !self.source_facts(path)?.constant
            || value.executed != variant.reads
            || !self.portable_evaluation_reads(&value)
            || value.depth.first.len() != self.depth_limit() as usize + 1
            || value
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
            || value
                .substitutions
                .iter()
                .any(|(id, _)| !shape.valid_site(hirsel_types::NameSite::Expression(*id)))
        {
            return None;
        }
        if self.verify == Verify::On {
            let Some(receipt) =
                self.load_execution_receipt(evaluation_subject(&key, &variant.payload))
            else {
                // The old action and its complete observed reads have held, but
                // this reader must independently execute and verify the value.
                // Restore only existing ordinary mode bodies of those executed
                // dependencies before S1 starts. Missing products stay misses;
                // no helper is evaluated or granted execution authority here.
                self.restore_execution_dependencies(&value.executed);
                return None;
            };
            value.validation = Some(receipt.contexts().to_vec().into());
            value.acceptance = Some(receipt);
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Evaluate),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        self.replay_evaluation_selections(&value)?;
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some(value)
    }
    /// S1 may first compute a signature while persistence is withheld. Once
    /// execution settles, publish only its already checked, current signature
    /// closure so the value's prior reads can be validated in a fresh process.
    fn store_observed_signatures(&mut self, reads: &Reads) -> Result<(), QueryError> {
        let mut pending: BTreeSet<_> = reads
            .iter()
            .filter_map(|(read, _)| match read {
                ReadKey::Signature(id) | ReadKey::ExecutedSignature(id) => Some(*id),
                _ => None,
            })
            .collect();
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop_first() {
            if !seen.insert(id) {
                continue;
            }
            let Some(entry) = self
                .signatures
                .get(&id)
                .cloned()
                .filter(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
            else {
                continue;
            };
            for (read, _) in entry.reads().iter() {
                if let ReadKey::Signature(dependency) | ReadKey::ExecutedSignature(dependency) =
                    read
                {
                    pending.insert(*dependency);
                }
            }
            self.store_signature(id.path(), &entry)?;
        }
        Ok(())
    }
    pub(crate) fn store_evaluation(
        &mut self,
        path: &DefPath,
        value: &crate::comptime::EvaluatedConstant,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none() || !self.s1.is_empty() || !self.s1.failure_is_own() {
            return Ok(());
        }
        self.store_observed_signatures(&value.executed)?;
        let Some(value) = persisted_evaluation(value) else {
            return Ok(());
        };
        if !self.store_evaluation_selections(&value)? {
            return Ok(());
        }
        let Some(key) = self.store_key(path, QueryKind::Evaluate) else {
            return Ok(());
        };
        let Some(payload) = self.encode_record(&EvaluationRecord {
            owner: DefId::intern(path),
            constant: value.clone(),
        }) else {
            return Ok(());
        };
        let store = self
            .result_store
            .as_ref()
            .expect("enabled evaluation store")
            .clone();
        if let Some(receipt) = &value.acceptance
            && let Some(cache) = store.verifier()
        {
            cache.publish_execution(evaluation_subject(&key, &payload), receipt);
        }
        EncodedWriter {
            types: self.type_records().expect("enabled evaluation types"),
            store,
            key,
            path: path.clone(),
            kind: QueryKind::Evaluate,
            expected: self
                .pending_store_checks
                .remove(&(DefId::intern(path), QueryKind::Evaluate)),
        }
        .write(&value.executed, payload)
    }

    /// A selected staged value has the ordinary Evaluate kind, but its logical
    /// address names the declaration and complete canonical size arguments.
    /// The source-owned declaration supplies the input and environment key.
    fn staged_evaluation_key(&mut self, path: &DefPath, arguments: &[u64]) -> Option<EntryKey> {
        let declaration = self.staged_constants.get(path)?.clone();
        if declaration.inputs.len() != arguments.len() {
            return None;
        }
        let owner = self.store_key(&declaration.owner, QueryKind::Evaluate)?;
        let instance = hirsel_tir::constant_instance_path(path, arguments);
        Some(EntryKey(
            ContentHash::of_parts(
                "hirsel.result.staged-evaluation-address.v1",
                [owner.0.0.as_slice(), instance.fingerprint().0.as_slice()],
            ),
            ContentHash::of_parts(
                "hirsel.result.staged-evaluation-input.v1",
                [
                    owner.1.0.as_slice(),
                    declaration.signature.fingerprint.0.as_slice(),
                ],
            ),
            QueryKind::Evaluate,
        ))
    }
    pub(crate) fn load_staged_evaluation(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Option<crate::comptime::EvaluatedConstant> {
        let instance = hirsel_tir::constant_instance_path(path, arguments);
        let active = (instance, QueryKind::Evaluate);
        if !self.active.insert(active.clone()) {
            return None;
        }
        let result = self.load_staged_evaluation_inner(path, arguments);
        self.active.remove(&active);
        result
    }
    fn load_staged_evaluation_inner(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Option<crate::comptime::EvaluatedConstant> {
        if !self.s1.is_empty() {
            return None;
        }
        let declaration = self.staged_constants.get(path)?.clone();
        let key = self.staged_evaluation_key(path, arguments)?;
        let variant = self.stored_variant_at(&declaration.owner, &key)?;
        let record: StagedEvaluationRecord = self.decode_record(&variant.payload)?;
        let (_, shape) = self.lexical_shape(&declaration.owner)?;
        let mut value = record.constant;
        if record.declaration != DefId::intern(path)
            || record.arguments != arguments
            || value.executed != variant.reads
            || !self.portable_evaluation_reads(&value)
            || value.depth.first.len() != self.depth_limit() as usize + 1
            || value
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
            || value
                .substitutions
                .iter()
                .any(|(id, _)| !shape.valid_site(hirsel_types::NameSite::Expression(*id)))
        {
            return None;
        }
        if self.verify == Verify::On {
            let receipt =
                self.load_execution_receipt(evaluation_subject(&key, &variant.payload))?;
            value.validation = Some(receipt.contexts().to_vec().into());
            value.acceptance = Some(receipt);
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            let instance = hirsel_tir::constant_instance_path(path, arguments);
            self.pending_store_checks.insert(
                (DefId::intern(&instance), QueryKind::Evaluate),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
        self.replay_evaluation_selections(&value)?;
        timings::count(Count::StoreHits);
        self.store_adoptions += 1;
        Some(value)
    }
    pub(crate) fn store_staged_evaluation(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
        value: &crate::comptime::EvaluatedConstant,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none() || !self.s1.is_empty() || !self.s1.failure_is_own() {
            return Ok(());
        }
        let Some(value) = persisted_evaluation(value) else {
            return Ok(());
        };
        if !self.store_evaluation_selections(&value)? {
            return Ok(());
        }
        let Some(key) = self.staged_evaluation_key(path, arguments) else {
            return Ok(());
        };
        let Some(payload) = self.encode_record(&StagedEvaluationRecord {
            declaration: DefId::intern(path),
            arguments: arguments.to_vec(),
            constant: value.clone(),
        }) else {
            return Ok(());
        };
        let store = self
            .result_store
            .as_ref()
            .expect("enabled staged evaluation store")
            .clone();
        if let Some(receipt) = &value.acceptance
            && let Some(cache) = store.verifier()
        {
            cache.publish_execution(evaluation_subject(&key, &payload), receipt);
        }
        let instance = hirsel_tir::constant_instance_path(path, arguments);
        EncodedWriter {
            types: self
                .type_records()
                .expect("enabled staged evaluation types"),
            store,
            key,
            path: instance.clone(),
            kind: QueryKind::Evaluate,
            expected: self
                .pending_store_checks
                .remove(&(DefId::intern(&instance), QueryKind::Evaluate)),
        }
        .write(&value.executed, payload)
    }

    pub(crate) fn source_verification_policy(&self) -> ContentHash {
        let private = ContentHash::of_parts(
            "hirsel.validate.private-evaluation.v1",
            [
                self.environment_value().0.as_slice(),
                self.store_identity.0.as_slice(),
            ],
        );
        ContentHash::of_parts("hirsel.validate.frozen-source.v2", [private.0.as_slice()])
    }
    pub(crate) fn ensure_lowered_verdict(
        &mut self,
        definition: &hirsel_tir::DefinitionRef,
    ) -> Result<Option<Arc<hirsel_validate::VerifiedDefinition>>, QueryError> {
        if self.verify == Verify::Off {
            return Ok(None);
        }
        let entry = self.lowered.get(&definition.path).cloned();
        let snapshot = self.signature_snapshot();
        let policy = self.source_verification_policy();
        if let Some(evidence) = self.lowered_verification(definition.path)
            && evidence.holds(
                definition,
                &self.types,
                policy,
                snapshot.validation(&definition.path.path().0.package),
            )
        {
            return Ok(Some(evidence.clone()));
        }
        let (report, evidence) = self.validate_lowered_artifact(definition);
        if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
            return Err(error);
        }
        if !report.violations.is_empty() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "TIR validation".into(),
                message: format!("{:?}", report.violations),
            }));
        }
        if let Some(entry) = entry
            && entry
                .result()
                .is_ok_and(|o| Arc::ptr_eq(&o.value, definition))
        {
            *entry.verification.lock().unwrap() = evidence.clone();
        }
        Ok(evidence)
    }
    /// Only V can mint the evidence attached to this ordinary query product.
    pub(crate) fn validate_lowered_artifact(
        &mut self,
        definition: &hirsel_tir::DefinitionRef,
    ) -> (
        hirsel_validate::Validation,
        Option<Arc<hirsel_validate::VerifiedDefinition>>,
    ) {
        if self.verify == Verify::Off {
            return (hirsel_validate::Validation::default(), None);
        }
        let cache = self
            .result_store
            .as_ref()
            .and_then(|store| store.verifier());
        let policy = self.source_verification_policy();
        let snapshot = self.signature_snapshot();
        if !self.evaluation_validation.is_empty() {
            let package = definition.path.path().0.package.clone();
            self.record_evaluation_validation(
                package.clone(),
                snapshot.validation_input_digest(&package),
            );
        }
        let _time = timings::phase(Phase::Verify);
        let (report, executed, acceptance, evidence) = hirsel_validate::validate_definition(
            definition,
            self.types.clone(),
            |anchor| {
                artifact_anchor_owner(&self.items, &self.sources, &self.generated, anchor)
                    .or_else(|| self.catalog.owner(anchor))
            },
            snapshot.validation(&definition.path.path().0.package),
            policy,
            cache.as_deref(),
        );
        if executed {
            timings::count(Count::VRuns);
        }
        let evidence = evidence.map(Arc::new);
        self.record_evaluation_acceptance(acceptance);
        if executed {
            *self
                .scalar_validation_runs
                .entry(definition.path)
                .or_default() += 1;
        }
        (report, evidence)
    }
    /// A body-free native cutoff over the same ordinary Lower query. Metadata
    /// and private execution are checked before V's own body envelope is used.
    /// A miss follows the ordinary lowering/validation path; no alternate proof.
    pub fn verified_lower_output(
        &mut self,
        path: &DefPath,
    ) -> Result<Option<hirsel_validate::VerifiedDefinitionOutput>, QueryError> {
        let Some((output, depth)) = self.verified_lower_output_inner(path) else {
            return Ok(None);
        };
        self.depth_replay_requests(&depth)?;
        Ok(Some(output))
    }
    /// Complete the ordinary Lower demand and return its actual V-owned output
    /// when locally available. This also works after a forced recomputation;
    /// unlike a cutoff lookup it never replays the completed demand's depth.
    pub fn lower_to_tir_with_verified_output(
        &mut self,
        path: &DefPath,
    ) -> Result<
        (
            QueryOutput<hirsel_tir::DefinitionRef>,
            Option<hirsel_validate::VerifiedDefinitionOutput>,
        ),
        QueryError,
    > {
        let lowered = self.lower_to_tir(path)?;
        let output = if self.verify == Verify::On
            && let Some(cache) = self
                .result_store
                .as_ref()
                .and_then(|store| store.verifier())
            && let Some(evidence) = self.ensure_lowered_verdict(&lowered.value)?
        {
            let snapshot = self.signature_snapshot();
            cache.definition_output_from_evidence(
                &lowered.value,
                self.types.clone(),
                self.source_verification_policy(),
                snapshot.validation(&path.0.package),
                &evidence,
            )
        } else {
            None
        };
        Ok((lowered, output))
    }
    /// Materialize this verifier-owned source envelope only after a native
    /// consumer has decided it needs the body. Native validators stay separate.
    pub fn materialize_verified_lower_output(
        &mut self,
        output: &hirsel_validate::VerifiedDefinitionOutput,
    ) -> Option<hirsel_tir::DefinitionRef> {
        if self.verify != Verify::On {
            return None;
        }
        let cache = self.result_store.as_ref()?.verifier()?;
        let snapshot = self.signature_snapshot();
        cache.read_definition(
            output,
            self.types.clone(),
            |path, occurrence| self.record_anchor(path, occurrence),
            self.source_verification_policy(),
            snapshot.validation(&output.owner().path().0.package),
        )
    }
    fn verified_lower_output_inner(
        &mut self,
        path: &DefPath,
    ) -> Option<(
        hirsel_validate::VerifiedDefinitionOutput,
        crate::memo::DepthEvidence<Span>,
    )> {
        if self.verify != Verify::On
            || !self.s1.is_empty()
            || self.verify_store == crate::store::VerifyStore::Recompute
        {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::Lower)?;
        let key = self.store_key(path, QueryKind::Lower)?;
        let cache = self.result_store.as_ref()?.verifier()?;
        let snapshot = self.signature_snapshot();
        let policy = self.source_verification_policy();
        let subject = evaluation_subject(&key, &variant.payload);
        let output = cache.definition_output(
            subject,
            DefId::intern(path),
            &self.types,
            policy,
            snapshot.validation(&path.0.package),
        )?;
        let metadata: LowerIdentityMetadata = self.decode_record(output.metadata())?;
        if metadata.depth.first.len() != self.depth_limit() as usize + 1
            || metadata
                .depth
                .first
                .iter()
                .flatten()
                .any(|site| self.sources.slice(*site).is_none())
            || !self.portable_lower_reads(path, &variant.reads, &metadata.selections)
        {
            return None;
        }
        if self.lower_requires_execution(path, &variant.reads) {
            self.load_execution_receipt(subject)?;
        }
        self.replay_selections(&metadata.selections)?;
        Some((output, metadata.depth))
    }
    pub(super) fn publish_captured_lower(
        &mut self,
        path: &DefPath,
        captured: CapturedLower,
    ) -> Result<(), QueryError> {
        if !self.execution_valid(path, &captured.reads) {
            return Ok(());
        }
        let Some(bytes) = self
            .result_store
            .as_ref()
            .and_then(|store| store.get_content(captured.payload))
        else {
            return Ok(());
        };
        let Some(record): Option<LowerRecord> = self.decode_record(&bytes) else {
            return Ok(());
        };
        if record.output.value.path.path() != path
            || !self.portable_lower_reads(path, &captured.reads, &record.selections)
            || !self.lower_acceptance_holds(captured.acceptance.as_ref())
        {
            return Ok(());
        }
        let verification = self.ensure_lowered_verdict(&record.output.value)?;
        let mut entry = self.adopted(
            path,
            QueryKind::Lower,
            Variant {
                inputs: ContentHash::default(),
                reads: captured.reads,
                payload: bytes,
            },
            Ok(record.output),
            record.detail,
        );
        let unpublished = Arc::get_mut(&mut entry).expect("unpublished worker Lower");
        unpublished.depth = Some(record.depth);
        unpublished.selections = record.selections;
        unpublished.acceptance = captured.acceptance;
        unpublished.verification = std::sync::Mutex::new(verification);
        self.store_lower(path, &entry)?;
        self.lowered.insert(DefId::intern(path), entry);
        self.retain_lowering(DefId::intern(path));
        Ok(())
    }

    pub(super) fn store_lower(
        &mut self,
        path: &DefPath,
        entry: &Entry<hirsel_tir::DefinitionRef>,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none()
            || !entry.consistent
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
        {
            return Ok(());
        }
        if !self.portable_lower_reads(path, entry.reads(), &entry.selections)
            || !self.store_selections(&entry.selections)?
        {
            return Ok(());
        }
        let Ok(output) = entry.output() else {
            return Ok(());
        };
        let Some(depth) = entry.depth.clone() else {
            return Ok(());
        };
        if crate::comptime::has_staged_reads(&output.value) {
            if !self.valid_staged_lowering(path, &output.value) {
                return Ok(());
            }
            self.store_registrations(path)?;
        }
        let Some(key) = self.store_key(path, QueryKind::Lower) else {
            return Ok(());
        };
        let definition = output.value.clone();
        let identity_metadata = self.encode_record(&LowerIdentityMetadata {
            depth: depth.clone(),
            selections: entry.selections.clone(),
        });
        let Some(payload) = self.encode_record(&LowerRecord {
            selections: entry.selections.clone(),
            depth,
            output,
            detail: canonical_detail(entry.detail.clone()),
        }) else {
            return Ok(());
        };
        let store = self
            .result_store
            .as_ref()
            .expect("enabled lower store")
            .clone();
        if let Some(receipt) = &entry.acceptance
            && let Some(cache) = store.verifier()
        {
            cache.publish_execution(evaluation_subject(&key, &payload), receipt);
        }
        let Some(reads) = persisted_execution_reads(entry.reads(), entry.acceptance.as_ref())
        else {
            return Ok(());
        };
        if let (Some(cache), Some(metadata), Some(evidence)) = (
            store.verifier(),
            identity_metadata,
            entry.verification.lock().unwrap().clone(),
        ) {
            let snapshot = self.signature_snapshot();
            cache.publish_definition(
                evaluation_subject(&key, &payload),
                &definition,
                self.types.clone(),
                self.source_verification_policy(),
                snapshot.validation(&path.0.package),
                &evidence,
                &metadata,
            );
        }
        EncodedWriter {
            types: self.type_records().expect("enabled lower types"),
            store,
            key,
            path: path.clone(),
            kind: QueryKind::Lower,
            expected: self
                .pending_store_checks
                .remove(&(DefId::intern(path), QueryKind::Lower)),
        }
        .write(&reads, payload)
    }
}

#[cfg(test)]
mod malformed_tests {
    use super::*;

    #[test]
    fn source_generated_signature_reads_do_not_grant_execution_selection() {
        let (engine, syntax) = fixture_source("fn main()->u32 uses {}=comptime ignored()");
        let id = DefId::intern(&syntax.item.definition);
        let caller = DefPath::core("caller");
        let digest = syntax.text_hash;
        let signature = vec![
            (ReadKey::Signature(id), digest),
            (ReadKey::ExecutedSignature(id), digest),
        ];
        assert!(engine.portable_lower_reads(
            &caller,
            &Reads::from_vec(signature.clone()).unwrap(),
            &[],
        ));
        let mut executed = signature;
        executed.push((ReadKey::ExecutedBody(id), digest));
        let executed = Reads::from_vec(executed).unwrap();
        assert!(!engine.portable_lower_reads(&caller, &executed, &[]));
        assert!(engine.portable_lower_reads(
            &caller,
            &executed,
            &[crate::comptime::Selection::Generated(id)],
        ));
        assert!(!engine.portable_lower_reads(
            &caller,
            &Reads::from_vec(vec![(ReadKey::ExecutedBody(id), digest)]).unwrap(),
            &[crate::comptime::Selection::Generated(id)],
        ));
    }

    #[test]
    fn layout_reads_never_discharge_callable_execution_authority() {
        let (engine, syntax) = fixture();
        let path = &syntax.item.definition;
        let id = DefId::intern(path);
        let digest = syntax.text_hash;
        let layout = Reads::from_vec(vec![
            (ReadKey::LayoutBody(id), digest),
            (ReadKey::Representation(id), digest),
        ])
        .unwrap();
        assert!(!engine.lower_requires_execution(path, &layout));
        let mut executed: Vec<_> = layout.iter().cloned().collect();
        executed.push((ReadKey::ExecutedBody(id), digest));
        assert!(engine.lower_requires_execution(path, &Reads::from_vec(executed).unwrap()));
        assert!(
            engine.lower_requires_execution(
                path,
                &Reads::from_vec(vec![
                    (ReadKey::ExecutedBody(id), digest),
                    (ReadKey::Representation(id), digest)
                ])
                .unwrap()
            )
        );
        assert!(
            engine.lower_requires_execution(
                path,
                &Reads::from_vec(vec![(
                    ReadKey::ValidationUniverse(crate::memo::PackageKey::of(&path.0.package)),
                    digest
                )])
                .unwrap()
            )
        );
    }

    #[test]
    fn malformed_semantic_bodies_miss_before_adoption_or_comparison() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..4 {
                let (mut engine, syntax) = fixture();
                let path = &syntax.item.definition;
                let mut record = SemanticBodyRecord {
                    owner: DefId::intern(path),
                    content: vec![vec![1]],
                    dependencies: BTreeSet::new(),
                    selections: vec![],
                    depth: crate::memo::DepthEvidence::leaf(engine.depth_limit()),
                };
                match fault {
                    0 => record.owner = DefId::intern(&DefPath::core("foreign")),
                    1 => record.content.clear(),
                    2 => record.depth.first = Box::new([]),
                    3 => record
                        .selections
                        .push(crate::comptime::Selection::Generated(DefId::intern(
                            &DefPath::core("absent-generator"),
                        ))),
                    _ => unreachable!(),
                }
                engine
                    .write_record(path, QueryKind::SemanticBody, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                assert!(
                    engine.load_semantic_body(path).is_none(),
                    "fault={fault} verify={verify:?}"
                );
                assert_eq!(engine.store_adoptions(), 0);
                assert!(engine.pending_store_checks.is_empty());
            }
        }
    }

    #[test]
    fn malformed_source_audits_miss_before_signature_demand_or_comparison() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) = fixture();
                let path = &syntax.item.definition;
                let mut record = AuditRecord {
                    products: crate::native_audit::SourceAuditProducts {
                        audit: crate::native_audit::SourceAudit {
                            definition: path.clone(),
                            implementation_revision: syntax.text_hash,
                            signature: ContentHash::default(),
                            noalloc: false,
                            cleanup: vec![],
                            closures: vec![],
                            invocations: vec![],
                        },
                        materialized: vec![],
                    },
                    depth: crate::memo::DepthEvidence::leaf(engine.depth_limit()),
                    selections: vec![],
                };
                match fault {
                    0 => record.products.audit.definition = DefPath::core("foreign"),
                    1 => record.products.audit.implementation_revision = ContentHash::default(),
                    2 => record.depth.first = Box::new([]),
                    3 => record.products.audit.closures.push((
                        hirsel_syntax::ExprId(u32::MAX),
                        hirsel_types::Allocation::NoAlloc,
                    )),
                    4 => record.products.audit.invocations.push((
                        hirsel_syntax::ExprId(u32::MAX),
                        hirsel_types::Allocation::NoAlloc,
                    )),
                    5 => record
                        .selections
                        .push(crate::comptime::Selection::Generated(DefId::intern(
                            &DefPath::core("absent-generator"),
                        ))),
                    6 => record.selections.push(crate::comptime::Selection::Staged(
                        DefId::intern(&DefPath::core("absent-stage")),
                        vec![2],
                    )),
                    _ => unreachable!(),
                }
                engine
                    .write_record(path, QueryKind::SourceAudit, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                assert!(
                    engine.load_source_audit(path).is_none(),
                    "fault={fault} verify={verify:?}"
                );
                assert_eq!(engine.recompute_count(path, QueryKind::Signature), 0);
                assert_eq!(engine.store_adoptions(), 0);
                assert!(engine.pending_store_checks.is_empty());
            }
        }
    }

    #[test]
    fn generated_registrations_validate_before_installation_and_comparison() {
        use hirsel_types::{GenericArgument, Size};
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) =
                    fixture_source("fn main[T,n:size]()->u32 uses {}=comptime ignored()");
                engine.adapters.lexer = Some(Arc::new(crate::adapters::LexerAdapter));
                engine.adapters.parser = Some(Arc::new(crate::adapters::ParserAdapter));
                let declaration = syntax.item.definition.clone();
                let arguments = vec![
                    GenericArgument::Type(engine.types.intern(hirsel_types::Type::unit())),
                    GenericArgument::Size(Size::Constant(2)),
                ];
                let canonical = hirsel_tir::InstanceBodies::key(&engine.types, &arguments).unwrap();
                let instance = generation::instance_path(&declaration, canonical);
                let mut record = GeneratedRegistration {
                    declaration: DefId::intern(&declaration),
                    instance: DefId::intern(&instance),
                    arguments,
                };
                match fault {
                    0 => (),
                    1 => record.declaration = DefId::intern(&DefPath::core("foreign")),
                    2 => record.instance = DefId::intern(&DefPath::core("foreign")),
                    3 => {
                        record.arguments.pop();
                    }
                    4 => record.arguments[0] = GenericArgument::Size(Size::Constant(2)),
                    5 => {
                        record.arguments[1] = GenericArgument::Size(Size::Parameter(
                            hirsel_types::SizeParameterRef::bound(hirsel_types::SizeParamId(0)),
                        ))
                    }
                    6 => record.arguments[1] = GenericArgument::Size(Size::Constant(3)),
                    _ => unreachable!(),
                }
                let (_, key) = engine.generated_registration_key(&instance).unwrap();
                let payload = engine.encode_record(&record).unwrap();
                let store = engine.result_store.as_ref().unwrap().clone();
                let mut entry = StoredEntry::default();
                entry.insert(VariantRef::publish(&*store, &key, Reads::empty(), &payload).unwrap());
                store.put(&key, &entry.encode_for(&key));
                engine.verify_store = verify;
                let hit = engine.load_generated_registration(&instance);
                let accepted = fault == 0 && verify == crate::store::VerifyStore::Off;
                assert_eq!(hit.is_some(), accepted, "fault={fault} verify={verify:?}");
                assert_eq!(engine.items.contains_key(&instance), accepted);
                assert_eq!(
                    engine.instance_bodies.instances.len(),
                    usize::from(accepted)
                );
                assert!(
                    engine.generated_source(&instance).is_none(),
                    "registration never executes or publishes generator text"
                );
                if fault != 0 {
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }

    #[test]
    fn malformed_generation_records_miss_before_source_publication() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) =
                    fixture_source("fn main()->u32 uses {}=comptime ignored()");
                engine.verify = Verify::Off;
                let path = &syntax.item.definition;
                let body = generation::generator(&syntax).unwrap();
                let mut depth = crate::memo::DepthEvidence::leaf(engine.depth_limit());
                depth.bounded = false;
                let mut record = GenerationRecord {
                    owner: DefId::intern(path),
                    body,
                    depth,
                    text: "fn()->u32 uses {}{7u32}".into(),
                    detail: Default::default(),
                    selections: vec![],
                    acceptance: None,
                };
                match fault {
                    0 => (),
                    1 => record.owner = DefId::intern(&DefPath::core("foreign")),
                    2 => record.body = hirsel_syntax::ExprId(u32::MAX),
                    3 => record.depth.bounded = true,
                    4 => record.depth.first = Box::new([]),
                    5 => record.selections.push(crate::comptime::Selection::Staged(
                        DefId::intern(&DefPath::core("absent-stage")),
                        vec![2],
                    )),
                    6 => record
                        .selections
                        .push(crate::comptime::Selection::Generated(DefId::intern(
                            &DefPath::core("absent-generator"),
                        ))),
                    _ => unreachable!(),
                }
                engine
                    .write_record(path, QueryKind::Generate, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_generation(path, body);
                assert_eq!(
                    hit.is_some(),
                    fault == 0 && verify == crate::store::VerifyStore::Off,
                    "fault={fault} verify={verify:?}"
                );
                assert!(
                    engine.generated_source(path).is_none(),
                    "a text query does not publish generated syntax"
                );
                if fault != 0 {
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }

    #[test]
    fn durable_evaluation_requires_exact_coverage_for_every_removed_v_guard() {
        use hirsel_tir::*;
        let (engine, syntax) = fixture();
        let ty = engine.types.intern(hirsel_types::Type::unit());
        // The validator's unit fixture (hirsel-validate/tests/it/support.rs)
        // supplies a real successful terminal capability, never a fake token.
        let definition = Definition {
            path: syntax.owner().def,
            types: vec![ty].into(),
            inferred_unique: Default::default(),
            signature_hash: ContentHash::default(),
            entry: BodyId(0),
            caller_bodies: vec![],
            protocol_associations: vec![],
            contracts: vec![],
            bodies: vec![hirsel_tir::Body {
                arena_scope_grant: None,
                scoped_parameters: vec![],
                parameter_scope: syntax.owner().site(hirsel_types::NameSite::Item),
                locals: vec![Local {
                    ty,
                    storage: Storage::ReturnSlot,
                    scope: ScopeId(0),
                    span: Span::default(),
                }],
                places: vec![Place {
                    loan: None,
                    binding: None,
                    root: LocalId(0),
                    projection: vec![].into(),
                    ty,
                }],
                scopes: vec![Scope {
                    parent: None,
                    span: Span::default(),
                }],
                blocks: vec![Block {
                    instructions: vec![Instruction {
                        kind: InstructionKind::Assign {
                            destination: PlaceId(0),
                            value: RvalueId(0),
                        },
                        span: Span::default(),
                        on_failure: CleanupId(0),
                    }],
                    terminator: Terminator::Return {
                        value: Operand {
                            place: PlaceId(0),
                            usage: hirsel_types::UseKind::Take,
                        },
                        cleanup: CleanupId(0),
                    },
                    span: Span::default(),
                }],
                rvalues: vec![Rvalue::Constant(Constant::Unit)],
                cleanups: vec![Cleanup::default()],
                cleanup_nodes: vec![],
                entry: BlockId(0),
                parameters: vec![],
                result: ty,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
            }],
        };
        let artifact =
            hirsel_validate::Artifact::new(&definition, engine.types.clone(), |anchor| {
                anchor_owner(&engine.items, anchor)
            });
        let (report, _, accepted) = hirsel_validate::validate_with_receipt(
            &artifact,
            &hirsel_validate::SignatureMap::new(engine.types.clone()),
            ContentHash::default(),
            Some(&hirsel_validate::VerifierCache::default()),
            &mut vec![],
        );
        assert!(report.violations.is_empty(), "{:?}", report.violations);
        let package = syntax.item.definition.0.package.clone();
        let digest = ContentHash::of_bytes(b"complete private universe");
        let guard = ReadKey::ValidationUniverse(crate::memo::PackageKey::of(&package));
        let semantic = vec![
            (
                ReadKey::ExecutedBody(syntax.owner().def),
                ContentHash::of_bytes(b"executed source"),
            ),
            (
                ReadKey::Environment,
                ContentHash::of_bytes(b"compiler and policy"),
            ),
        ];
        let mut rows = semantic.clone();
        rows.push((guard, digest));
        let original = crate::comptime::EvaluatedConstant {
            value: Materialized {
                root: ValueNode {
                    ty,
                    kind: ValueKind::Constant(Constant::Unit),
                },
            },
            substitutions: vec![],
            executed: Reads::from_vec(rows).unwrap(),
            depth: crate::memo::DepthEvidence::leaf(engine.depth_limit()),
            selections: vec![],
            validation: None,
            acceptance: Some(
                hirsel_validate::ExecutionReceipt::compose(
                    &[(package, digest)],
                    &[accepted.unwrap()],
                )
                .unwrap(),
            ),
        };
        let expected = Reads::from_vec(semantic.clone()).unwrap();
        assert_eq!(persisted_evaluation(&original).unwrap().executed, expected);
        assert_eq!(
            original.executed.get(&guard),
            Some(digest),
            "worker memory keeps its epoch guard"
        );
        for fault in 0..4 {
            let mut value = original.clone();
            let mut rows = semantic.clone();
            match fault {
                0 => {
                    value.acceptance = None;
                    rows.push((guard, digest));
                }
                1 => rows.push((guard, ContentHash::of_bytes(b"wrong universe"))),
                2 => rows.push((
                    ReadKey::ValidationUniverse(crate::memo::PackageKey::of(
                        &DefPath::core("other").0.package,
                    )),
                    digest,
                )),
                3 => {
                    rows.push((guard, digest));
                    rows.push((
                        ReadKey::ValidationUniverse(crate::memo::PackageKey::of(
                            &DefPath::core("other").0.package,
                        )),
                        digest,
                    ));
                }
                _ => unreachable!(),
            }
            value.executed = Reads::from_vec(rows).unwrap();
            assert!(
                persisted_evaluation(&value).is_none(),
                "uncovered guard fault={fault}"
            );
        }
        let mut no_guard = original;
        no_guard.executed = expected.clone();
        no_guard.acceptance = None;
        assert_eq!(
            persisted_evaluation(&no_guard).unwrap().executed,
            expected,
            "ordinary semantic reads are never dropped"
        );
    }

    #[test]
    fn malformed_staged_values_miss_before_adoption_or_comparison() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) = fixture_source("fn main[n:size]()->u64{comptime n}");
                engine.verify = Verify::Off;
                let owner = &syntax.item.definition;
                let mut path = owner.clone();
                path.0.name = "$comptime:main:0".into();
                let ty = engine.types.intern(hirsel_types::Type::unit());
                engine.install_staged(crate::comptime::StagedRegistration {
                    path: path.clone(),
                    constant: crate::comptime::StagedConstant {
                        owner: owner.clone(),
                        expression: hirsel_syntax::ExprId(0),
                        inputs: vec![hirsel_types::SizeParameterRef::bound(
                            hirsel_types::SizeParamId(0),
                        )],
                        signature: Arc::new(Signature {
                            definition: DefId::intern(&path),
                            metadata: Default::default(),
                            kind: hirsel_types::SignatureKind::Const {
                                ty,
                                generics: Default::default(),
                            },
                            fingerprint: ContentHash::default(),
                        }),
                    },
                });
                let mut record = StagedEvaluationRecord {
                    declaration: DefId::intern(&path),
                    arguments: vec![2],
                    constant: crate::comptime::EvaluatedConstant {
                        value: hirsel_tir::Materialized {
                            root: hirsel_tir::ValueNode {
                                ty,
                                kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                            },
                        },
                        substitutions: vec![],
                        executed: Reads::empty(),
                        depth: crate::memo::DepthEvidence::leaf(engine.depth_limit()),
                        selections: vec![],
                        validation: None,
                        acceptance: None,
                    },
                };
                match fault {
                    0 => (),
                    1 => record.declaration = DefId::intern(&DefPath::core("foreign")),
                    2 => record.arguments = vec![3],
                    3 => record.arguments.push(3),
                    4 => record.constant.depth.first = Box::new([]),
                    5 => record.constant.substitutions.push((
                        hirsel_syntax::ExprId(u32::MAX),
                        record.constant.value.clone(),
                    )),
                    6 => {
                        record.constant.executed = Reads::from_vec(vec![(
                            ReadKey::Signature(DefId::intern(&path)),
                            ContentHash::default(),
                        )])
                        .unwrap()
                    }
                    _ => unreachable!(),
                }
                let key = engine.staged_evaluation_key(&path, &[2]).unwrap();
                let payload = engine.encode_record(&record).unwrap();
                EncodedWriter {
                    types: engine.type_records().unwrap(),
                    store: engine.result_store.clone().unwrap(),
                    key,
                    path: hirsel_tir::constant_instance_path(&path, &[2]),
                    kind: QueryKind::Evaluate,
                    expected: None,
                }
                .write(&Reads::empty(), payload)
                .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_staged_evaluation(&path, &[2]);
                assert_eq!(
                    hit.is_some(),
                    fault == 0 && verify == crate::store::VerifyStore::Off,
                    "fault={fault} verify={verify:?}"
                );
                assert!(engine.load_staged_evaluation(&path, &[3]).is_none());
                if fault != 0 {
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }

    #[test]
    fn producer_serialization_cannot_transfer_scalar_private_admission() {
        let (mut engine, _) = fixture();
        let mut value = crate::comptime::EvaluatedConstant {
            value: hirsel_tir::Materialized {
                root: hirsel_tir::ValueNode {
                    ty: engine.types.intern(hirsel_types::Type::unit()),
                    kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                },
            },
            substitutions: vec![],
            executed: Reads::empty(),
            depth: crate::memo::DepthEvidence::leaf(engine.depth_limit()),
            acceptance: None,
            selections: vec![],
            validation: Some(
                vec![(crate::default_context().package, ContentHash::default())].into(),
            ),
        };
        let encoded = engine.encode_record(&value).unwrap();
        value.validation = None;
        assert_eq!(encoded, engine.encode_record(&value).unwrap());
        let decoded: crate::comptime::EvaluatedConstant = engine.decode_record(&encoded).unwrap();
        assert!(decoded.validation.is_none());
        assert!(!engine.evaluation_validation_holds(&decoded));
    }

    #[test]
    fn malformed_evaluation_owner_reads_depth_and_substitutions_miss_before_adoption() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) = fixture_source("const VALUE:u8=7u8");
                engine.verify = Verify::Off;
                let path = &syntax.item.definition;
                let mut record = EvaluationRecord {
                    owner: DefId::intern(path),
                    constant: crate::comptime::EvaluatedConstant {
                        value: hirsel_tir::Materialized {
                            root: hirsel_tir::ValueNode {
                                ty: engine.types.intern(hirsel_types::Type::unit()),
                                kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                            },
                        },
                        substitutions: vec![],
                        executed: Reads::empty(),
                        depth: crate::memo::DepthEvidence::leaf(engine.depth_limit()),
                        selections: vec![],
                        validation: None,
                        acceptance: None,
                    },
                };
                match fault {
                    0 => (),
                    1 => record.owner = DefId::intern(&DefPath::core("foreign")),
                    2 => record.constant.depth.first = Box::new([]),
                    3 => record.constant.substitutions.push((
                        hirsel_syntax::ExprId(u32::MAX),
                        record.constant.value.clone(),
                    )),
                    4 => {
                        record.constant.executed = Reads::from_vec(vec![(
                            ReadKey::ExecutedBody(record.owner),
                            ContentHash::default(),
                        )])
                        .unwrap()
                    }
                    5 => record
                        .constant
                        .selections
                        .push(crate::comptime::Selection::Staged(
                            DefId::intern(&DefPath::core("absent-stage")),
                            vec![2],
                        )),
                    6 => record
                        .constant
                        .selections
                        .push(crate::comptime::Selection::Generated(DefId::intern(
                            &DefPath::core("absent-generator"),
                        ))),
                    _ => unreachable!(),
                }
                engine
                    .write_record(path, QueryKind::Evaluate, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_evaluation(path);
                assert_eq!(
                    hit.is_some(),
                    fault == 0 && verify == crate::store::VerifyStore::Off,
                    "fault {fault}"
                );
                if fault != 0 {
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }

    // Equivalent sequence serialization for the bulk byte payload vectors.
    #[derive(Serialize, Deserialize)]
    struct SequenceEnvelope {
        anchors: Vec<(DefPath, u32)>,
        types: Vec<TypeRef>,
        payload: Vec<u8>,
    }

    #[test]
    fn bulk_payload_matches_sequence_encoding_and_bounded_decoding() {
        use bincode::Options;
        let (engine, syntax) = fixture();
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let bytes = engine
            .encode_record(&(unit, syntax.item.origin.span))
            .unwrap();
        let mut envelope: Envelope = bincode::deserialize(&bytes).unwrap();
        assert!(!envelope.anchors.is_empty());
        assert!(!envelope.types.is_empty());
        // Include bincode 1.3.3 tests/test.rs:565-572's byte equality vector,
        // every byte value, and payloads spanning multiple copy chunks.
        for payload in [
            vec![],
            vec![0],
            b"abc\0123".to_vec(),
            (0..=255).collect(),
            vec![0xa5; 65_537],
            vec![0xff; 1 << 20],
        ] {
            envelope.payload = payload;
            let legacy = SequenceEnvelope {
                anchors: envelope.anchors.clone(),
                types: envelope.types.clone(),
                payload: envelope.payload.clone(),
            };
            let old = bincode::serialize(&legacy).unwrap();
            let new = bincode::serialize(&envelope).unwrap();
            assert_eq!(old, new);
            let options = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(old.len() as u64)
                .reject_trailing_bytes();
            let decoded: Envelope = options.deserialize(&old).unwrap();
            assert_eq!(decoded.payload, legacy.payload);
            let decoded: SequenceEnvelope = options.deserialize(&new).unwrap();
            assert_eq!(decoded.payload, envelope.payload);
            assert!(
                options
                    .deserialize::<Envelope>(&old[..old.len() - 1])
                    .is_err()
            );
            let mut trailing = old;
            trailing.push(0);
            assert!(options.deserialize::<Envelope>(&trailing).is_err());
        }
    }

    fn fixture() -> (QueryEngine, Arc<ItemSyntax>) {
        fixture_source("fn main() { true }")
    }
    fn fixture_source(source: &str) -> (QueryEngine, Arc<ItemSyntax>) {
        let mut engine = QueryEngine::new(crate::default_environment(), PhaseAdapters::default())
            .with_result_store(Arc::new(crate::store::MemStore::default()));
        let file = engine
            .sources
            .add("cache-record.hir", source.into())
            .unwrap();
        let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
        let context = crate::default_context();
        let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
        assert!(parsed.diagnostics.is_empty());
        engine.index(&mut parsed.syntax, &context).unwrap();
        (engine, parsed.syntax.items[0].clone())
    }
    fn record(engine: &QueryEngine, syntax: &ItemSyntax) -> ModeRecord {
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let boolean = engine.types.intern(hirsel_types::Type::boolean());
        let checked = CheckedBody {
            lexical: syntax.owner(),
            places: vec![],
            calls: vec![],
            tails: vec![],
            bindings: vec![],
            patterns: vec![],
            resolution: Default::default(),
            expressions: vec![hirsel_types::Elaboration {
                construction_destination: None,
                expression: hirsel_syntax::ExprId(0),
                ty: boolean,
                usage: hirsel_types::UseKind::Copy,
                instantiated_arguments: vec![],
            }],
            instantiations: vec![],
            closures: vec![],
        };
        ModeRecord {
            checked: Some(CheckedRecord {
                checked: checked.clone(),
                plans: Default::default(),
                unit,
                boolean,
                diagnostics: vec![],
                dependencies: vec![],
                detail: Default::default(),
            }),
            phases: vec![],
            detail: Default::default(),
            output: Ok(QueryOutput {
                value: Body {
                    checked,
                    plans: Default::default(),
                    unit,
                    boolean,
                    ownership: Default::default(),
                    plain: vec![],
                    silent: vec![],
                    results: vec![],
                    calls: vec![],
                },
                diagnostics: vec![],
                dependencies: vec![],
            }),
        }
    }
    #[test]
    fn structurally_invalid_records_miss_before_adoption_or_verification() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) = fixture();
                let mut record = record(&engine, &syntax);
                match fault {
                    0 => (),
                    1 => {
                        record.output.as_mut().unwrap().value.checked.expressions[0]
                            .expression
                            .0 = u32::MAX
                    }
                    2 => record.checked.as_mut().unwrap().checked.lexical.occurrence += 1,
                    3 => record.checked = None,
                    4 => record.output.as_mut().unwrap().value.unit = TypeId::DANGLING,
                    5 => record.output.as_mut().unwrap().value.results.push((
                        hirsel_types::NameSite::Pattern(hirsel_syntax::PatternId(u32::MAX)),
                        record.checked.as_ref().unwrap().unit,
                    )),
                    6 => record
                        .output
                        .as_mut()
                        .unwrap()
                        .value
                        .checked
                        .resolution
                        .expressions
                        .push((
                            hirsel_syntax::ExprId(u32::MAX),
                            hirsel_types::ResolvedName::Primitive(hirsel_types::Primitive::Bool),
                        )),
                    _ => unreachable!(),
                }
                let path = &syntax.item.definition;
                // Encode through the genuine writer, with valid key, checksum
                // and reads. These exercise semantic guards, not just framing.
                engine
                    .write_record(path, QueryKind::Modes, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_mode(path);
                if fault == 0 {
                    assert_eq!(hit.is_some(), verify == crate::store::VerifyStore::Off);
                    assert_eq!(
                        engine.pending_store_checks.len(),
                        usize::from(verify == crate::store::VerifyStore::Recompute)
                    );
                } else {
                    assert!(hit.is_none(), "fault {fault}");
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.checked.is_empty());
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }
    #[test]
    fn malformed_lower_depth_is_rejected_before_adoption_or_replay() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..4 {
                let (mut engine, syntax) = fixture();
                engine.verify = Verify::Off;
                let path = &syntax.item.definition;
                let mut depth = crate::memo::DepthEvidence::leaf(engine.depth_limit());
                match fault {
                    0 => depth.first = Box::new([]),
                    1 => depth.first = vec![None; 2].into_boxed_slice(),
                    2 => {
                        depth.first[0] = Some(Span {
                            anchor: hirsel_span::Anchor::fresh(),
                            start: 0,
                            end: 1,
                        })
                    }
                    3 => (),
                    _ => unreachable!(),
                }
                if fault == 2 {
                    // A foreign anchor cannot even enter the portable record.
                    assert!(engine.encode_record(&depth).is_none());
                    continue;
                }
                let record = LowerRecord {
                    selections: if fault == 3 {
                        vec![crate::comptime::Selection::Staged(
                            DefId::intern(&DefPath::core("absent-stage")),
                            vec![2],
                        )]
                    } else {
                        vec![]
                    },
                    depth,
                    output: output(Arc::new(hirsel_tir::Definition {
                        path: DefId::intern(path),
                        inferred_unique: Default::default(),
                        signature_hash: ContentHash::default(),
                        caller_bodies: vec![],
                        protocol_associations: vec![],
                        types: Arc::from([]),
                        bodies: vec![],
                        entry: hirsel_tir::BodyId(0),
                        contracts: vec![],
                    })),
                    detail: Default::default(),
                };
                engine
                    .write_record(path, QueryKind::Lower, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                assert!(engine.load_lower(path).is_none());
                assert_eq!(engine.store_adoptions(), 0);
                assert!(engine.pending_store_checks.is_empty());
            }
        }
    }

    #[test]
    fn invalid_registration_records_do_not_publish_a_partial_declaration_set() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..10 {
                let (mut engine, syntax) = fixture_source("fn main[n:size]()->u64{comptime n}");
                let path = &syntax.item.definition;
                let expression = hirsel_syntax::ExprId(
                    syntax
                        .tree
                        .expressions
                        .iter()
                        .position(|e| matches!(e.kind, hirsel_syntax::ExprKind::Comptime(_)))
                        .unwrap() as u32,
                );
                let mut hidden = path.clone();
                hidden.0.name = "$comptime:main:0".into();
                let mut signature = hirsel_types::Signature {
                    metadata: Default::default(),
                    definition: DefId::intern(&hidden),
                    kind: hirsel_types::SignatureKind::Const {
                        ty: engine.types.intern(hirsel_types::Type::unit()),
                        generics: vec![hirsel_types::GenericParameter::Size {
                            name: "size0".into(),
                        }]
                        .into(),
                    },
                    fingerprint: ContentHash::default(),
                };
                signature.fingerprint =
                    hirsel_types::encoding::fingerprint(&engine.types, &signature, &[]).unwrap();
                let mut record = Registrations {
                    owner: syntax.owner().def,
                    rows: vec![crate::comptime::StagedRegistration {
                        path: hidden,
                        constant: crate::comptime::StagedConstant {
                            owner: path.clone(),
                            expression,
                            signature: Arc::new(signature),
                            inputs: vec![hirsel_types::SizeParameterRef::body(
                                hirsel_types::SiteRef {
                                    owner: syntax.owner(),
                                    site: hirsel_types::NameSite::Item,
                                },
                                hirsel_types::SizeParamId(0),
                            )],
                        },
                    }],
                };
                match fault {
                    0 => (),
                    1 => record.rows[0].constant.owner = DefPath::core("foreign"),
                    2 => record.rows[0].path.0.name = "foreign".into(),
                    3 => record.rows[0].constant.inputs.clear(),
                    4 => record.rows[0]
                        .constant
                        .inputs
                        .push(hirsel_types::SizeParameterRef::body(
                            hirsel_types::SiteRef {
                                owner: syntax.owner(),
                                site: hirsel_types::NameSite::Item,
                            },
                            hirsel_types::SizeParamId(1),
                        )),
                    5 => {
                        record.rows[0].constant.inputs[0] =
                            hirsel_types::SizeParameterRef::bound(hirsel_types::SizeParamId(0))
                    }
                    6 => record.rows[0].constant.expression = hirsel_syntax::ExprId(u32::MAX),
                    7 => record.rows.push(record.rows[0].clone()),
                    8 => record.owner = DefId::intern(&DefPath::core("foreign")),
                    9 => {
                        Arc::make_mut(&mut record.rows[0].constant.signature).fingerprint =
                            ContentHash::default()
                    }
                    _ => unreachable!(),
                }
                engine
                    .write_record(path, QueryKind::Registration, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_registrations(path);
                let accepted = fault == 0 && verify == crate::store::VerifyStore::Off;
                assert_eq!(hit.is_some(), accepted, "fault {fault}, verify {verify:?}");
                assert_eq!(engine.staged_constants.len(), usize::from(accepted));
                assert_eq!(engine.store_adoptions(), u64::from(accepted));
                assert_eq!(
                    engine.pending_store_checks.len(),
                    usize::from(fault == 0 && verify == crate::store::VerifyStore::Recompute)
                );
            }
        }
    }

    #[test]
    fn invalid_signature_records_miss_before_adoption_or_verification() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..5 {
                let (mut engine, syntax) = fixture();
                let path = &syntax.item.definition;
                let signature = Arc::new(hirsel_types::Signature {
                    metadata: Default::default(),
                    definition: DefId::intern(path),
                    kind: hirsel_types::SignatureKind::Const {
                        ty: engine.types.intern(hirsel_types::Type::unit()),
                        generics: Default::default(),
                    },
                    fingerprint: ContentHash::default(),
                });
                let mut record = SignatureRecord {
                    output: Ok(output(signature)),
                    detail: Default::default(),
                    covered: vec![],
                    resolution: Some((syntax.occurrence, output(Resolution::default()))),
                    owner: None,
                };
                match fault {
                    0 => (),
                    1 => {
                        record.output.as_mut().unwrap().value = Arc::new(hirsel_types::Signature {
                            definition: DefId::intern(&DefPath::core("foreign")),
                            ..(*record.output.as_ref().unwrap().value).clone()
                        })
                    }
                    2 => record.resolution.as_mut().unwrap().0 += 1,
                    3 => record
                        .resolution
                        .as_mut()
                        .unwrap()
                        .1
                        .value
                        .expressions
                        .push((
                            hirsel_syntax::ExprId(u32::MAX),
                            hirsel_types::ResolvedName::Primitive(hirsel_types::Primitive::Bool),
                        )),
                    4 => record.output = Err(QueryError::NotImplemented),
                    _ => unreachable!(),
                }
                engine
                    .write_record(path, QueryKind::Signature, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_signature(path);
                if fault == 0 {
                    assert_eq!(hit.is_some(), verify == crate::store::VerifyStore::Off);
                    assert_eq!(
                        engine.pending_store_checks.len(),
                        usize::from(verify == crate::store::VerifyStore::Recompute)
                    );
                } else {
                    assert!(hit.is_none(), "fault {fault}");
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }

    #[test]
    fn invalid_compact_publications_miss_before_adoption_or_verification() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..6 {
                let (mut engine, syntax) = fixture();
                let path = &syntax.item.definition;
                let summary = DefinitionSummary {
                    owner: syntax.owner().def,
                    frames: vec![crate::summary::Frame {
                        site: hirsel_types::NameSite::Item,
                        parent: None,
                        kind: crate::summary::FrameKind::Item,
                        generic: false,
                        size_bindings: Box::new([]),
                    }]
                    .into(),
                    calls: Box::new([]),
                    staged: Box::new([]),
                };
                let mut published = PublishedSummary {
                    definition: summary.owner,
                    storage: None,
                    checked: Some(Arc::new(CheckedSummary {
                        summary: summary.clone(),
                        growth: Default::default(),
                    })),
                    summary,
                    growth: Default::default(),
                    reads: Arc::new(Reads::empty()),
                    text: engine.key_text(path, QueryKind::Summary).unwrap(),
                };
                match fault {
                    0 => (),
                    1 => published.checked = None,
                    2 => published.summary.frames[0].parent = Some(0),
                    3 => {
                        published.summary.frames[0].site =
                            hirsel_types::NameSite::Expression(hirsel_syntax::ExprId(u32::MAX))
                    }
                    4 => published.text = ContentHash::default(),
                    5 => {
                        Arc::make_mut(published.checked.as_mut().unwrap())
                            .summary
                            .frames[0]
                            .parent = Some(0)
                    }
                    _ => unreachable!(),
                }
                let record = PublicationRecord {
                    summary: Arc::new(published),
                    diagnostics: vec![],
                    phases: vec![],
                    detail: Default::default(),
                    dead_arms: None,
                    lowering: None,
                };
                engine
                    .write_record(path, QueryKind::Summary, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_publication(path);
                if fault == 0 {
                    assert_eq!(hit.is_some(), verify == crate::store::VerifyStore::Off);
                    assert_eq!(
                        engine.pending_store_checks.len(),
                        usize::from(verify == crate::store::VerifyStore::Recompute)
                    );
                } else {
                    assert!(hit.is_none(), "fault {fault}");
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.summaries.is_empty());
                    assert!(engine.checked_summaries.is_empty());
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }
    #[test]
    fn canonical_type_and_anchor_domains_and_exact_payload_consumption_are_checked() {
        let (engine, syntax) = fixture();
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let bytes = engine.encode_record(&unit).unwrap();
        assert_eq!(engine.decode_record::<TypeId>(&bytes), Some(unit));
        let mut envelope: Envelope = bincode::deserialize(&bytes).unwrap();
        envelope.payload = u32::MAX.to_le_bytes().to_vec();
        assert!(
            engine
                .decode_record::<TypeId>(&bincode::serialize(&envelope).unwrap())
                .is_none()
        );
        let bad = engine
            .types
            .intern(hirsel_types::Type::with_kind(hirsel_types::TypeKind::Box(
                TypeId::DANGLING,
            )));
        assert!(engine.encode_record(&bad).is_none());
        let bytes = engine.encode_record(&syntax.item.origin.span).unwrap();
        let mut envelope: Envelope = bincode::deserialize(&bytes).unwrap();
        envelope.payload[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            engine
                .decode_record::<Span>(&bincode::serialize(&envelope).unwrap())
                .is_none()
        );
        let mut envelope: Envelope =
            bincode::deserialize(&engine.encode_record(&unit).unwrap()).unwrap();
        envelope.payload.push(0);
        assert!(
            engine
                .decode_record::<TypeId>(&bincode::serialize(&envelope).unwrap())
                .is_none()
        );
        let mut bytes = engine.encode_record(&unit).unwrap();
        bytes.push(0);
        assert!(engine.decode_record::<TypeId>(&bytes).is_none());
    }
}

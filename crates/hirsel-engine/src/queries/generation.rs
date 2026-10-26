//! CT-5 typed generation. A function whose body is `= comptime e` gets the body
//! its generator returns. `e` is checked and evaluated as the private constant
//! `name$generator: core@1/Result[core@1/Body[F], core@1/String]`, where `F` is
//! the header's own function type, so ordinary checking rejects a mismatched
//! header before evaluation. The generated text, one function literal, becomes
//! its own source file: the literal is normalized like a header and compared
//! with the declared one, and its body follows the written header in one token
//! tape that is parsed, resolved and checked like a written body.
use super::*;
use crate::memo::Entered;
use hirsel_syntax::{
    ExprId, ExprKind, FunctionSyntax, Generated, Keyword, PackageId, Path, TokenKind, TypeArgument,
    TypeExpr, TypeExprId, TypeExprKind, TypeModifier, Visibility,
};
use hirsel_types::SignatureKind;
mod accepted;
mod bootstrap;
mod instance_bootstrap;
mod instances;
pub use bootstrap::{
    FrontRegistrationBootstrap, FrontRegistrationBootstrapAdmission,
    FrontRegistrationBootstrapError,
};
pub use instance_bootstrap::{
    FrontInstanceBootstrap, FrontInstanceBootstrapAdmission, FrontInstanceBootstrapError,
};
pub(crate) mod task;
pub use accepted::{AcceptedFrontRegistration, FrontRegistrationFailure};
pub use instances::{FrontInstanceDiscovery, FrontInstanceRequest, FrontInstanceSyntax};

/// A declaration's generated source, for diagnostics and `hirsel expand`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedSource {
    pub file: FileId,
    pub text: String,
    /// The `comptime e` body that generated it.
    pub site: Span,
}

/// The `comptime e` body of a generated declaration.
pub(crate) fn generator(syntax: &ItemSyntax) -> Option<ExprId> {
    match syntax.item.kind {
        ItemKind::Function {
            body: Some(body), ..
        } if matches!(
            syntax.tree.expressions[body.0 as usize].kind,
            ExprKind::Comptime(_)
        ) =>
        {
            Some(body)
        }
        _ => None,
    }
}

/// Whether a declaration's front work runs serially: a generator, or a body
/// whose `with` clause names a specialization (GEN-6).
pub(crate) fn serial(syntax: &ItemSyntax) -> bool {
    generator(syntax).is_some() || crate::families::closes(syntax)
}

/// Whether a declaration is generic and generated: it has no body of its own,
/// and each selected instance is a monomorphic generated declaration.
pub(crate) fn instance_bodied(syntax: &ItemSyntax) -> bool {
    generator(syntax).is_some()
        && matches!(
            &syntax.item.kind,
            ItemKind::Function { signature, .. } if !signature.generics.is_empty()
        )
}

/// Whether an item has a body to lower: a function with one, a constant or a
/// finalizer. A generic generated declaration has none; its instances do. A
/// declaration with implementation binders has none either; its
/// specializations do (GEN-6).
pub(crate) fn executable(syntax: &ItemSyntax) -> bool {
    matches!(
        &syntax.item.kind,
        ItemKind::Function { body: Some(_), .. }
            | ItemKind::Const { .. }
            | ItemKind::Type { drop: Some(_), .. }
    ) && !instance_bodied(syntax)
        && !abstract_family(syntax)
}

/// Whether an item declares implementation binders it does not bind (GEN-6).
pub(crate) fn abstract_family(syntax: &ItemSyntax) -> bool {
    matches!(
        &syntax.item.kind,
        ItemKind::Function { signature, .. } if signature.generics.iter().any(|g|
            matches!(g.kind, hirsel_syntax::GenericKind::Implementation { .. }))
    ) && !matches!(
        syntax.item.generated,
        Some(hirsel_syntax::Generated::Specialization(_))
    )
}

/// The most instances one generic generated declaration may select (CT-5).
pub const MAX_GENERATED_INSTANCES: usize = 256;

fn renamed(definition: &DefPath, suffix: &str) -> DefPath {
    let mut path = definition.clone();
    path.0.name = format!("{}${suffix}", path.0.name);
    path
}

pub(super) fn instance_path(
    declaration: &DefPath,
    key: hirsel_types::GeneratedActualKey,
) -> DefPath {
    let digest = ContentHash::of_parts(
        "hirsel.generated-instance.v1",
        key.spellings().iter().map(|spelling| spelling.as_bytes()),
    );
    renamed(
        declaration,
        &format!(
            "instance${}",
            digest.0[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
    )
}

fn diagnostic(rule: &'static str, span: Span, message: impl Into<String>) -> QueryError {
    QueryError::Diagnostics(vec![Diagnostic::error(
        hirsel_diag::RuleId::new(rule).expect("rule id"),
        span,
        message,
    )])
}

/// Every primary site in generated text also marks the declaration that generated it.
fn label(mut error: QueryError, engine: &QueryEngine, generated: FileId, site: Span) -> QueryError {
    if let QueryError::Diagnostics(diagnostics) = &mut error {
        for d in diagnostics {
            if engine.sources.resolve(d.primary).file == generated {
                engine.schema_hook_context(d, generated);
                d.secondary.push(hirsel_diag::Label {
                    span: site,
                    message: "generated by this `= comptime` body".into(),
                });
            }
        }
    }
    error
}

/// The phase reads of one generated check, accumulated across its steps.
struct Recording {
    reads: DependencyReads,
    observed: Observed,
}

impl QueryEngine {
    /// Late mode failures use the same generated-source explanations as checking.
    /// Only diagnostic-bearing results are copied; successful bodies stay shared.
    pub(super) fn label_generated_result<T: Clone>(
        &self,
        result: Result<Arc<QueryOutput<T>>, QueryError>,
    ) -> Result<Arc<QueryOutput<T>>, QueryError> {
        let label = |diagnostics: &mut [Diagnostic]| {
            for diagnostic in diagnostics {
                let file = self.sources.resolve(diagnostic.primary).file;
                let Some(generated) = self.generated.values().find(|g| g.file == file) else {
                    continue;
                };
                let origin = hirsel_diag::Label {
                    span: generated.site,
                    message: "generated by this `= comptime` body".into(),
                };
                if !diagnostic.secondary.contains(&origin) {
                    let labels = diagnostic.secondary.len();
                    self.schema_hook_context(diagnostic, file);
                    if diagnostic.secondary.len() != labels {
                        diagnostic.secondary.push(origin);
                    }
                }
            }
        };
        match result {
            Ok(mut output) => {
                if !output.diagnostics.is_empty() {
                    label(&mut Arc::make_mut(&mut output).diagnostics);
                }
                Ok(output)
            }
            Err(QueryError::Diagnostics(mut diagnostics)) => {
                label(&mut diagnostics);
                Err(QueryError::Diagnostics(diagnostics))
            }
            Err(error) => Err(error),
        }
    }

    /// The std generator emits one breadcrumb and contract before a single-line
    /// converter. They explain diagnostics only: no name resolution, verdict,
    /// primary span or repair is inferred from this generated text.
    fn schema_hook_context(&self, diagnostic: &mut Diagnostic, generated: FileId) {
        let Some(source) = self.sources.file(generated) else {
            return;
        };
        let at = self.sources.resolve(diagnostic.primary);
        let Some((parent, member, contract)) = crate::diagnostics::schema_hook_context(
            source.text(),
            at.start as usize,
            at.end as usize,
        ) else {
            return;
        };
        let Some(span) = self.schema_member_span(parent, member) else {
            return;
        };
        diagnostic.secondary.push(hirsel_diag::Label {
            span,
            message: format!("schema field `{parent}.{member}` requires `{contract}`"),
        });
    }

    /// Resolve the breadcrumb to existing application syntax, retaining item
    /// anchors so relocated and UTF-8 sources use the ordinary source map.
    fn schema_member_span(&self, parent: &str, member: &str) -> Option<Span> {
        let (package, path) = parent.split_once('/')?;
        let (name, version) = package.split_once('@')?;
        let path = path.split('[').next()?;
        let (modules, name_in_module) = path.rsplit_once('.').map_or_else(
            || (vec![], path),
            |(modules, name)| (modules.split('/').map(str::to_owned).collect(), name),
        );
        let definition = DefPath(ItemPath {
            package: PackageId {
                name: name.into(),
                version: version.into(),
            },
            modules,
            name: name_in_module.into(),
        });
        let syntax = self.snapshot(&definition).ok()?;
        let ItemKind::Type { body, .. } = &syntax.item.kind else {
            return None;
        };
        match body {
            hirsel_syntax::TypeBody::Record(fields) => fields
                .iter()
                .find(|field| field.name.as_str() == member)
                .map(|field| syntax.tree.types[field.ty.0 as usize].origin.span),
            hirsel_syntax::TypeBody::Variants(variants) => {
                let (variant, member) = member.split_once('.')?;
                let variant = variants.iter().find(|v| v.name.as_str() == variant)?;
                let (_, ty) = variant
                    .fields
                    .iter()
                    .enumerate()
                    .find(|(index, (label, _))| {
                        label.map_or_else(
                            || index.to_string() == member,
                            |label| label.as_str() == member,
                        )
                    })?
                    .1;
                Some(syntax.tree.types[ty.0 as usize].origin.span)
            }
            _ => None,
        }
    }

    /// The source generated for `definition`, once its check has run.
    pub fn generated_source(&self, definition: &DefPath) -> Option<&GeneratedSource> {
        self.generated.get(definition)
    }

    /// Every generated body produced so far, by declaration.
    pub fn generated_sources(&self) -> impl Iterator<Item = (&DefPath, &GeneratedSource)> {
        self.generated.iter()
    }

    /// Run `step` with a recorder for `owner`, keeping its reads.
    fn recorded<T>(
        &mut self,
        owner: &DefPath,
        reads: &mut Recording,
        step: impl FnOnce(
            &hirsel_types::TypeInterner,
            &hirsel_types::TypeMemo,
            &mut Recorder<'_>,
        ) -> Result<T, QueryError>,
    ) -> Result<T, QueryError> {
        let (types, memo) = (self.types.clone(), self.memo.clone());
        let mut recorder = Recorder {
            owner: owner.clone(),
            private_shapes: true,
            observed: std::mem::take(&mut reads.observed),
            engine: self,
            reads: std::mem::take(&mut reads.reads),
            groups: BTreeMap::new(),
        };
        let result = step(&types, &memo, &mut recorder);
        reads.observed = recorder.observed;
        reads.reads = recorder.reads;
        result
    }

    /// check_query for a generated declaration: the generated body's checked
    /// artifact, cached under the implementations its generator executed.
    pub(super) fn generated_check_query(
        &mut self,
        definition: &DefPath,
        snapshot: Arc<ItemSyntax>,
        body: ExprId,
    ) -> Result<Arc<QueryOutput<TypedBody>>, QueryError> {
        if instance_bodied(&snapshot) {
            return self.declaration_check_query(definition, snapshot);
        }
        self.record_generated_selection(DefId::intern(definition));
        let site = snapshot.tree.expressions[body.0 as usize].origin.span;
        // CT-4: a generator that needs its own declaration's body.
        if self.s1.on_stack(definition).is_some() {
            self.s1.fail_cycle();
            return Err(diagnostic(
                "CT-4",
                site,
                "compile-time dependency cycle: this generator runs its own declaration",
            ));
        }
        let signature = self.signature_output(definition)?;
        let _timing = timings::phase(Phase::Check);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Check)?;
        if let Some(entry) = self.checked.get(&id).cloned()
            && self.memo_valid(definition, &entry, text)
            && self.lower_acceptance_holds(entry.acceptance.as_ref())
        {
            if let Some(depth) = &entry.depth {
                self.depth_admit(depth, site)?;
                self.depth_requested(depth, site);
            }
            self.merge_execution(entry.reads());
            self.merge_lower_acceptance(entry.acceptance.as_ref(), true);
            self.record_selections(&entry.selections);
            return entry.result();
        }
        timings::count(Count::Definitions);
        let mut reads = Recording {
            reads: DependencyReads::default(),
            observed: Observed::default(),
        };
        reads
            .reads
            .signatures
            .extend(signature.dependencies.clone());
        reads.reads.signatures.push(SignatureDependency {
            definition: DefId::intern(definition),
            fingerprint: signature.value.fingerprint,
        });
        if self.s1.enter(definition.clone(), site) == Entered::Settled {
            // Another worker finished it: its entry is in the memo, or this
            // stack generates it again.
            return self.generated_check_query(definition, snapshot, body);
        }
        if let Err(error) = self.start(definition, QueryKind::Check) {
            self.s1.leave();
            return Err(error);
        }
        self.execution_stack.push(ReadsBuilder::new());
        self.read_implementation(definition);
        self.begin_evaluation_validation();
        let timing = timings::phase(Phase::Comptime);
        let result = self.generate(definition, &snapshot, body, &signature.value, &mut reads);
        drop(timing);
        let depth = self.depth_composed(false);
        let executed = self
            .execution_stack
            .pop()
            .expect("generation execution frame");
        reads.observed.merged.extend(executed);
        let (_, acceptance, selections) = self.end_evaluation_validation();
        let key = MemoKey::new(QueryKind::Check, id);
        let mut entry = self.finish(key, text, reads.reads, result, reads.observed);
        entry.depth = Some(depth.clone());
        entry.acceptance = acceptance;
        entry.selections = selections;
        let entry = Arc::new(entry);
        // Its own frame is on the stack still: stored before leaving, so a
        // waiting worker finds it.
        let own = self.s1.failure_is_own();
        keep(&mut self.checked, &mut self.graph, entry.clone(), own);
        self.s1.leave();
        self.merge_execution(entry.reads());
        self.depth_requested(&depth, site);
        entry.result()
    }

    fn generate(
        &mut self,
        definition: &DefPath,
        snapshot: &Arc<ItemSyntax>,
        body: ExprId,
        signature: &Signature,
        reads: &mut Recording,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        let SignatureKind::Function(_) = &signature.kind else {
            return Err(QueryError::NotImplemented);
        };
        let text = if let Some((record, observed)) = self.load_generation(definition, body) {
            self.depth_replay_requests(&record.depth)?;
            self.merge_execution(&observed);
            reads.observed.merged.merge(&observed);
            reads.reads = record.detail;
            self.merge_lower_acceptance(record.acceptance.as_ref(), true);
            self.record_selections(&record.selections);
            record.text
        } else {
            self.begin_evaluation_validation();
            let generated = self.generated_text(definition, snapshot, body, reads);
            let (_, acceptance, selections) = self.end_evaluation_validation();
            let generated = generated?;
            if let Some(executed) = self.execution_stack.last() {
                reads.observed.merged.extend(executed.clone());
            }
            if let Ok(observed) = self.typed_reads(definition, &reads.reads, &reads.observed) {
                reads.observed.merged.merge(&observed);
                if self.result_store.is_some() {
                    self.store_generation(
                        definition,
                        &observed,
                        persistent::GenerationRecord {
                            owner: DefId::intern(definition),
                            body,
                            text: generated.clone(),
                            detail: reads.reads.clone(),
                            depth: self.depth_composed(false),
                            selections,
                            acceptance,
                        },
                    )?;
                }
            }
            generated
        };
        self.publish_generated_body(definition, snapshot, body, signature, text, reads)
    }

    fn publish_generated_body(
        &mut self,
        definition: &DefPath,
        snapshot: &Arc<ItemSyntax>,
        body: ExprId,
        signature: &Signature,
        text: String,
        reads: &mut Recording,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        let site = snapshot.tree.expressions[body.0 as usize].origin.span;
        let file = match self.generated.get(definition) {
            Some(g) if g.text == text => g.file,
            _ => {
                // Generated text belongs to the definition, not the physical
                // location of its declaration. Moving that item must give a
                // fresh session and a retained source the same synthetic name.
                // `GeneratedSource.site` keeps the precise declaring origin.
                let path = format!("<generated:{definition}>");
                let logical = path.clone();
                let file = self
                    .sources
                    .add_logical(path, logical, text.clone())
                    .ok_or_else(|| {
                        QueryError::Operational(hirsel_types::OperationalFailure {
                            operation: "typed generation".into(),
                            message: "generated source exceeds the source map".into(),
                        })
                    })?;
                self.generated.insert(
                    definition.clone(),
                    GeneratedSource {
                        file,
                        text: text.clone(),
                        site,
                    },
                );
                file
            }
        };
        self.generated_body(definition, snapshot, body, signature, file, reads)
            .map_err(|e| label(e, self, file, site))
            .map(|mut output| {
                for d in &mut output.diagnostics {
                    if self.sources.resolve(d.primary).file == file {
                        self.schema_hook_context(d, file);
                        d.secondary.push(hirsel_diag::Label {
                            span: site,
                            message: "generated by this `= comptime` body".into(),
                        });
                    }
                }
                output
            })
    }

    /// Publish already evaluated generator text, with no interpreter execution.
    /// Keep the generator frame while parsing/checking so reentry still uses
    /// the same CT-4 and logical-depth protocol as source generation.
    pub(crate) fn merge_generated_text(
        &mut self,
        product: task::GeneratedText,
    ) -> Result<(), QueryError> {
        let definition = &product.definition;
        let signature = self.signature_output(definition)?;
        let text = self.key_text(definition, QueryKind::Check)?;
        let site = product.syntax.tree.expressions[product.body.0 as usize]
            .origin
            .span;
        let mut generation_detail = product.detail.clone();
        let mut generation_signatures = signature.dependencies.clone();
        generation_signatures.push(SignatureDependency {
            definition: DefId::intern(definition),
            fingerprint: signature.value.fingerprint,
        });
        generation_signatures.append(&mut generation_detail.signatures);
        generation_detail.signatures = generation_signatures;
        if let Ok(text) = &product.text {
            let observed = Observed::inheriting(Some(&product.reads));
            if let Ok(reads) = self.typed_reads(definition, &generation_detail, &observed) {
                self.store_generation(
                    definition,
                    &reads,
                    persistent::GenerationRecord {
                        owner: DefId::intern(definition),
                        body: product.body,
                        text: text.clone(),
                        detail: generation_detail.clone(),
                        depth: product.depth.clone(),
                        acceptance: product.acceptance.clone(),
                        selections: product.selections.clone(),
                    },
                )?;
            }
        }
        if self.s1.enter(definition.clone(), site) == Entered::Settled {
            return Err(QueryError::NotImplemented);
        }
        let _timing = timings::phase(Phase::Check);
        timings::count(Count::Definitions);
        if let Err(error) = self.start(definition, QueryKind::Check) {
            self.s1.leave();
            return Err(error);
        }
        let mut reads = Recording {
            reads: generation_detail,
            observed: Observed::inheriting(Some(&product.reads)),
        };
        reads.observed.products = product.products;
        self.execution_stack.push(ReadsBuilder::new());
        let result = self
            .s1
            .replay_requests(&product.depth)
            .map_err(|site| {
                self.comptime_limit(hirsel_tir::ToolingLimit {
                    kind: hirsel_tir::ToolingLimitKind::CallDepth,
                    limit: product.depth.limit() as u64,
                    site: Some(site),
                })
            })
            .and_then(|_| product.text)
            .and_then(|generated| {
                self.publish_generated_body(
                    definition,
                    &product.syntax,
                    product.body,
                    &signature.value,
                    generated,
                    &mut reads,
                )
            });
        let depth = self.depth_composed(false);
        reads.observed.merged.extend(
            self.execution_stack
                .pop()
                .expect("generator publication frame"),
        );
        let mut entry = self.finish(
            MemoKey::new(QueryKind::Check, DefId::intern(definition)),
            text,
            reads.reads,
            result,
            reads.observed,
        );
        entry.depth = Some(depth);
        entry.acceptance = product.acceptance;
        entry.selections = product.selections;
        let retry = entry.result().err().filter(|error| !error.cacheable());
        let own = self.s1.failure_is_own();
        keep(&mut self.checked, &mut self.graph, Arc::new(entry), own);
        self.s1.leave();
        if let Some(error) = retry {
            return Err(error);
        }
        if !self.checked.contains_key(&DefId::intern(definition)) {
            // A contextual rejection or inconsistent receipt was discarded.
            // It cannot settle another round; retry the original root.
            return Err(QueryError::NotImplemented);
        }
        // Rejections remain in the checked memo; the suspended root observes
        // them in its original request context during the next worker round.
        Ok(())
    }

    /// Check and evaluate the generator as a private constant of type
    /// `Result[Body[F], String]`; its text, or CT-1 for its `Error`.
    fn generated_text(
        &mut self,
        definition: &DefPath,
        snapshot: &Arc<ItemSyntax>,
        body: ExprId,
        reads: &mut Recording,
    ) -> Result<String, QueryError> {
        let input = task::GeneratorInput::new(definition, snapshot, body)?;
        let value = input.value;
        let adapters = (
            self.adapters.signature.clone(),
            self.adapters.resolver.clone(),
            self.adapters.checker.clone(),
            self.adapters.modes.clone(),
        );
        let (Some(signer), Some(resolver), Some(checker), Some(modes)) = adapters else {
            return Err(QueryError::NotImplemented);
        };
        let prepared = self.recorded(definition, reads, |t, m, recorder| {
            let declarations = recorder.engine.frozen_declarations();
            let mut products = (*recorder.observed.products).clone();
            let result = input.check(
                t,
                m,
                &declarations,
                &task::GeneratorPhases {
                    signature: &*signer,
                    resolver: &*resolver,
                    checker: &*checker,
                    modes: &*modes,
                },
                &mut products,
                recorder,
            );
            recorder.observed.products = Arc::new(products);
            result
        })?;
        let task::GeneratorBody {
            analyzed,
            signature: constant,
            ty,
        } = prepared;
        if let Some(resolution) = &reads.observed.products.resolution {
            reads.reads.resolver = resolution.reads.clone();
        }
        self.prepare_materialization_environment();
        let constants = self.prepare_values(&analyzed, &[], &constant)?;
        let (result, _) =
            self.run_evaluation(&analyzed, &[], &constant, value, value, ty, &constants, &[])?;
        input.decode(result)
    }

    /// Parse and normalize accepted text without checking a body or entering
    /// any source execution controller. The raw declaration stays unchanged.
    fn generated_syntax(
        &mut self,
        definition: &DefPath,
        snapshot: &Arc<ItemSyntax>,
        body: ExprId,
        generated: FileId,
        reads: &mut Recording,
    ) -> Result<Arc<ItemSyntax>, QueryError> {
        let source = &snapshot.item;
        let ItemKind::Function {
            signature: source_signature,
            ..
        } = &source.kind
        else {
            return Err(QueryError::NotImplemented);
        };
        let context = FileContext {
            package: definition.0.package.clone(),
            modules: definition.0.modules.clone(),
        };
        let (Some(lexer), Some(parser), Some(signer)) = (
            self.adapters.lexer.clone(),
            self.adapters.parser.clone(),
            self.adapters.signature.clone(),
        ) else {
            return Err(QueryError::NotImplemented);
        };
        let lexed = lexer.lex(generated, &self.sources)?;
        if has_errors(&lexed.diagnostics) {
            return Err(QueryError::Diagnostics(lexed.diagnostics));
        }
        let tokens = lexed.value;
        let significant = |t: &hirsel_syntax::Token| !matches!(t.kind, TokenKind::Trivia(_));
        let first = tokens.iter().position(significant);
        let Some(fn_index) = first.filter(|&i| tokens[i].kind == TokenKind::Keyword(Keyword::Fn))
        else {
            let span = first.map_or(
                Span::new(self.sources.file_anchor(generated), 0, 0).unwrap(),
                |i| tokens[i].span,
            );
            return Err(diagnostic(
                "CT-5",
                span,
                "generated text is not one function literal `fn(..) .. { .. }`",
            ));
        };
        // The literal as a declaration: `fn <declared name>(..) .. { .. }`.
        let name = snapshot
            .tree
            .tokens
            .iter()
            .find(|t| t.span == source.name_span);
        let Some(name) = name.cloned() else {
            return Err(QueryError::NotImplemented);
        };
        let mut tape = tokens[..=fn_index].to_vec();
        tape.push(name);
        tape.extend_from_slice(&tokens[fn_index + 1..]);
        let parsed = parser.parse(generated, tape, &context, &mut self.sources)?;
        if has_errors(&parsed.diagnostics) {
            return Err(QueryError::Diagnostics(parsed.diagnostics));
        }
        let not_literal = || {
            diagnostic(
                "CT-5",
                tokens[fn_index].span,
                "generated text is not one function literal `fn(..) .. { .. }`",
            )
        };
        let mut literal = match &parsed.value.items[..] {
            [literal] => (**literal).clone(),
            _ => return Err(not_literal()),
        };
        let (literal_signature, literal_body) = match &literal.item.kind {
            ItemKind::Function {
                signature,
                body: Some(body),
                abi: None,
            } if literal.item.attributes.is_empty() => (signature.clone(), *body),
            _ => return Err(not_literal()),
        };
        let literal_origin = literal.item.origin;
        // The literal's tokens are its tape's: the first item owns the leading trivia.
        let body_start = literal.tree.expressions[literal_body.0 as usize]
            .origin
            .tokens
            .start as usize;
        let header_span = Span::new(
            tokens[fn_index].span.anchor,
            tokens[fn_index].span.start,
            self.sources
                .resolve(literal.tree.tokens[body_start].span)
                .start,
        )
        .unwrap();
        literal.item.definition = renamed(definition, "literal");
        // The literal and the header are compared without uniqueness: the body
        // is checked against the declared header, which alone decides it.
        let mut header = (**snapshot).clone();
        header.item.definition = renamed(definition, "header");
        header.occurrence = 0;
        for (tree, signature) in [
            (&mut header.tree, source_signature),
            (&mut literal.tree, &literal_signature),
        ] {
            for ty in signature
                .parameters
                .iter()
                .map(|p| p.ty)
                .chain(signature.result)
            {
                without_uniqueness(tree, ty);
            }
        }
        let header = Arc::new(header);
        let literal = Arc::new(literal);
        let normalized = self.recorded(definition, reads, |t, m, r| {
            let declarations = r.engine.frozen_declarations();
            let mut products = PhaseProducts::default();
            let output = signer.signature(t, m, literal.clone(), &declarations, &mut products, r);
            r.observed.products = Arc::new(products);
            output
        })?;
        if has_errors(&normalized.diagnostics) {
            return Err(QueryError::Diagnostics(normalized.diagnostics));
        }
        let declared = self.recorded(definition, reads, |t, m, r| {
            let declarations = r.engine.frozen_declarations();
            let mut products = PhaseProducts::default();
            let output = signer.signature(t, m, header.clone(), &declarations, &mut products, r);
            r.observed.products = Arc::new(products);
            output
        })?;
        if has_errors(&declared.diagnostics) {
            return Err(QueryError::Diagnostics(declared.diagnostics));
        }
        if !same_function(&self.types, &declared.value, &normalized.value) {
            return Err(diagnostic(
                "LANG-7",
                header_span,
                "the generated function literal's type differs from the declared header",
            ));
        }
        let parameters: Vec<_> = literal_signature
            .parameters
            .iter()
            .map(|p| {
                let span = literal
                    .tree
                    .parameter_name_span(literal_origin, p)
                    .unwrap_or(header_span);
                (p.name, span)
            })
            .collect();
        // The written header up to its `=`, then the literal's body: one item.
        let written = &snapshot.tree.tokens;
        let start = source.origin.tokens.start as usize;
        // The body's `comptime` token, after the `=` that ends the header.
        let mut end = snapshot.tree.expressions[body.0 as usize]
            .origin
            .tokens
            .start as usize;
        let mut equals = false;
        while end > start && !(equals && significant(&written[end - 1])) {
            equals |= significant(&written[end - 1]);
            end -= 1;
        }
        let mut tape: Vec<_> = written[start..end].to_vec();
        // literal token i >= fn_index + 2 is generated token i - 1.
        tape.extend_from_slice(&tokens[body_start - 1..]);
        // Neither part is relative to the declaring file's own anchor, so the
        // parse keeps every span where it stands: the header in the
        // declaration, the body in the generated source.
        let file = self.item_file(snapshot);
        let parsed = parser.parse(file, tape, &context, &mut self.sources)?;
        if has_errors(&parsed.diagnostics) {
            return Err(QueryError::Diagnostics(parsed.diagnostics));
        }
        let mut syntax = match &parsed.value.items[..] {
            [syntax] => (**syntax).clone(),
            _ => return Err(QueryError::NotImplemented),
        };
        syntax.item.origin.span = source.origin.span;
        syntax.item.generated = Some(Generated::Body(parameters));
        syntax.anchor = snapshot.anchor;
        syntax.occurrence = snapshot.occurrence;
        syntax.text_hash = ContentHash::of_parts(
            "hirsel.generated-item.v1",
            [
                snapshot.text_hash.0.as_slice(),
                self.generated[definition].text.as_bytes(),
            ],
        );
        Ok(Arc::new(syntax))
    }

    /// Check the normalized literal with the ordinary body phases.
    fn generated_body(
        &mut self,
        definition: &DefPath,
        snapshot: &Arc<ItemSyntax>,
        body: ExprId,
        signature: &Signature,
        generated: FileId,
        reads: &mut Recording,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        let (Some(resolver), Some(checker)) = (
            self.adapters.resolver.clone(),
            self.adapters.checker.clone(),
        ) else {
            return Err(QueryError::NotImplemented);
        };
        let syntax = self.generated_syntax(definition, snapshot, body, generated, reads)?;
        let resolution = self.recorded(definition, reads, |t, m, r| {
            let declarations = r.engine.frozen_declarations();
            let mut products = PhaseProducts::default();
            let output = resolver.resolve(t, m, syntax.clone(), &declarations, &mut products, r);
            r.observed.products = Arc::new(products);
            output
        })?;
        if has_errors(&resolution.diagnostics) {
            return Err(QueryError::Diagnostics(resolution.diagnostics));
        }
        self.recorded(definition, reads, |t, m, r| {
            checker.check(t, m, syntax.clone(), resolution.value, signature, r)
        })
    }
}

/// Whether a literal's normalized header has the declared function type:
/// passing modes, parameter types, result and `uses`. Names, labels,
/// attributes and contracts belong to the declaration.
fn same_function(
    t: &hirsel_types::TypeInterner,
    declared: &Signature,
    literal: &Signature,
) -> bool {
    let (SignatureKind::Function(d), SignatureKind::Function(l)) = (&declared.kind, &literal.kind)
    else {
        return false;
    };
    let equal = |a, b| t.semantic_eq(a, b);
    d.parameters.len() == l.parameters.len()
        && d.parameters
            .iter()
            .zip(l.parameters.iter())
            .all(|(a, b)| a.passing == b.passing && equal(a.ty, b.ty))
        && equal(d.result, l.result)
        && d.uses == l.uses
}

/// Drop every `unique` modifier from the type `ty` of `tree`, in place: a
/// generator's body type and a literal's header carry no uniqueness of their
/// own, since the generated body is checked against the declared header.
fn without_uniqueness(tree: &mut hirsel_syntax::SyntaxTree, ty: TypeExprId) {
    let mut pending = vec![ty];
    while let Some(id) = pending.pop() {
        let node = &mut tree.types[id.0 as usize];
        match &mut node.kind {
            TypeExprKind::Named { arguments, .. } => {
                pending.extend(arguments.iter().filter_map(|argument| match argument {
                    TypeArgument::Type(ty) => Some(*ty),
                    TypeArgument::SizeLiteral(_) => None,
                }))
            }
            TypeExprKind::Tuple(items) => pending.extend(items.iter().copied()),
            TypeExprKind::Function(function) => pending.extend(
                function
                    .parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain(function.result),
            ),
            TypeExprKind::Box(inner) => pending.push(*inner),
            TypeExprKind::Qualified {
                base,
                modifiers,
                brand,
                relation,
            } => {
                modifiers.retain(|m| *m != TypeModifier::Unique);
                let base = *base;
                if modifiers.is_empty() && brand.is_none() && relation.is_none() {
                    tree.types[id.0 as usize].kind = tree.types[base.0 as usize].kind.clone();
                    pending.push(id);
                } else {
                    pending.push(base);
                }
            }
            TypeExprKind::Error => {}
        }
    }
}

impl QueryEngine {
    /// Whether `path` is a generic generated declaration (CT-5).
    pub(crate) fn instance_bodied(&self, path: &DefPath) -> bool {
        match self.items.get(path) {
            Some(items) => items.first().is_some_and(|s| instance_bodied(s)),
            None => self
                .catalog
                .facts(path)
                .is_some_and(|facts| facts.instance_bodied),
        }
    }

    /// check_query for a generic generated declaration: its header alone, as a
    /// bodyless declaration. Each selected instance generates and checks its
    /// own body (`generated_instance`).
    fn declaration_check_query(
        &mut self,
        definition: &DefPath,
        snapshot: Arc<ItemSyntax>,
    ) -> Result<Arc<QueryOutput<TypedBody>>, QueryError> {
        let signature = self.signature_output(definition)?;
        let _timing = timings::phase(Phase::Check);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Check)?;
        if let Some(entry) = self.checked.get(&id).cloned()
            && self.memo_valid(definition, &entry, text)
        {
            return entry.result();
        }
        timings::count(Count::Definitions);
        let (Some(resolver), Some(checker)) = (
            self.adapters.resolver.clone(),
            self.adapters.checker.clone(),
        ) else {
            return Err(QueryError::NotImplemented);
        };
        let mut file = (*snapshot).clone();
        if let ItemKind::Function { body, .. } = &mut file.item.kind {
            *body = None;
        }
        let file = Arc::new(file);
        let mut reads = Recording {
            reads: DependencyReads::default(),
            observed: Observed::default(),
        };
        reads
            .reads
            .signatures
            .extend(signature.dependencies.clone());
        reads.reads.signatures.push(SignatureDependency {
            definition: DefId::intern(definition),
            fingerprint: signature.value.fingerprint,
        });
        self.start(definition, QueryKind::Check)?;
        let result = self
            .recorded(definition, &mut reads, |t, m, r| {
                let declarations = r.engine.frozen_declarations();
                let mut products = PhaseProducts::default();
                let output = resolver.resolve(t, m, file.clone(), &declarations, &mut products, r);
                r.observed.products = Arc::new(products);
                output
            })
            .and_then(|resolution| {
                if has_errors(&resolution.diagnostics) {
                    return Err(QueryError::Diagnostics(resolution.diagnostics));
                }
                self.recorded(definition, &mut reads, |t, m, r| {
                    checker.check(t, m, file.clone(), resolution.value, &signature.value, r)
                })
            });
        let key = MemoKey::new(QueryKind::Check, id);
        let entry = Arc::new(self.finish(key, text, reads.reads, result, reads.observed));
        keep(
            &mut self.checked,
            &mut self.graph,
            entry.clone(),
            self.s1.failure_is_own(),
        );
        entry.result()
    }

    /// The monomorphic definition of `declaration` at closed `arguments`,
    /// created on first selection: the written declaration with its generics
    /// dropped and each parameter spelled as its actual (CT-3's canonical
    /// spelling), indexed as a private item of the declaring module and checked
    /// like any generated declaration. `site` selected it.
    pub(crate) fn generated_instance(
        &mut self,
        declaration: &DefPath,
        actuals: &hirsel_types::Actuals,
        site: Span,
    ) -> Result<DefPath, QueryError> {
        let restored = hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments)
            .filter(|key| {
                !self
                    .instance_bodies
                    .instances
                    .get(&DefId::intern(declaration))
                    .is_some_and(|instances| instances.contains_key(key))
            })
            .is_some_and(|key| {
                self.load_generated_registration(&instance_path(declaration, key))
                    .is_some()
            });
        let (instance, fresh) = self.register_generated_instance(declaration, actuals, site)?;
        if !fresh && !restored {
            self.record_generated_selection(DefId::intern(&instance));
            return Ok(instance);
        }
        let key = hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments)
            .expect("registered instance has closed arguments");
        let instance_name = key.instance_name(declaration);
        // The instance's diagnostics also cite the call that selected it.
        let checked = self.mode_query(&instance).and_then(|analyzed| {
            if has_errors(&analyzed.diagnostics) {
                Err(QueryError::Diagnostics(analyzed.diagnostics.clone()))
            } else {
                Ok(())
            }
        });
        if let Err(QueryError::Diagnostics(mut diagnostics)) = checked {
            for d in &mut diagnostics {
                d.secondary.push(hirsel_diag::Label {
                    span: site,
                    message: format!("selects instance {instance_name}"),
                });
            }
            return Err(QueryError::Diagnostics(diagnostics));
        }
        checked?;
        Ok(instance)
    }

    /// Install only the instance syntax/identity. Generator execution and
    /// checking are separate work, so a frozen worker can resume next epoch.
    pub(crate) fn register_generated_instance(
        &mut self,
        declaration: &DefPath,
        actuals: &hirsel_types::Actuals,
        site: Span,
    ) -> Result<(DefPath, bool), QueryError> {
        let key =
            hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments).ok_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "generated instance".into(),
                    message: format!("{declaration} selected at open arguments"),
                })
            })?;
        let declaration_id = DefId::intern(declaration);
        if let Some(instance) = self
            .instance_bodies
            .instances
            .get(&declaration_id)
            .and_then(|instances| instances.get(&key))
        {
            return Ok((instance.definition.path().clone(), false));
        }
        self.materialize(declaration)?;
        let snapshot = self.snapshot(declaration)?;
        let source = &snapshot.item;
        let ItemKind::Function {
            signature,
            body: Some(body),
            ..
        } = &source.kind
        else {
            return Err(QueryError::NotImplemented);
        };
        let generator_site = snapshot.tree.expressions[body.0 as usize].origin.span;
        let selected = self
            .instance_bodies
            .instances
            .get(&declaration_id)
            .map_or(0, |instances| instances.len());
        let spellings = key.spellings();
        let instance_name = format!("{}[{}]", declaration, spellings.join(", "));
        if selected >= MAX_GENERATED_INSTANCES {
            let mut d = Diagnostic::error(
                const { hirsel_diag::RuleId::of("CT-1") },
                generator_site,
                format!(
                    "a generic generated declaration selects at most {MAX_GENERATED_INSTANCES} instances"
                ),
            );
            d.secondary.push(hirsel_diag::Label {
                span: site,
                message: format!("selects instance {instance_name}"),
            });
            return Err(QueryError::Diagnostics(vec![d]));
        }
        // The instance's source: the declaration's tokens with the generic list
        // after its name left out and each generic parameter replaced by its
        // actual's spelling, on the declaration's own lines.
        let names: Vec<_> = signature.generics.iter().map(|g| g.name).collect();
        let tokens = &snapshot.tree.tokens
            [source.origin.tokens.start as usize..source.origin.tokens.end as usize];
        let name_index = tokens
            .iter()
            .position(|t| t.span == source.name_span)
            .ok_or(QueryError::NotImplemented)?;
        let significant = |t: &hirsel_syntax::Token| !matches!(t.kind, TokenKind::Trivia(_));
        let declaring = self.item_file(&snapshot);
        let before = tokens
            .first()
            .map_or(0, |t| self.sources.resolve(t.span).start as usize);
        let lines = self
            .sources
            .file(declaring)
            .and_then(|f| f.text().get(..before))
            .map_or(0, |prefix| prefix.matches('\n').count());
        let mut text = "\n".repeat(lines);
        let mut skip_to = None;
        if let Some(open) = tokens[name_index + 1..]
            .iter()
            .position(significant)
            .map(|i| i + name_index + 1)
            .filter(|&i| self.sources.slice(tokens[i].span) == Some("["))
        {
            let mut depth = 0usize;
            for (i, token) in tokens.iter().enumerate().skip(open) {
                match self.sources.slice(token.span) {
                    Some("[") => depth += 1,
                    Some("]") => {
                        depth -= 1;
                        if depth == 0 {
                            skip_to = Some((open, i));
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        for (i, token) in tokens.iter().enumerate() {
            if skip_to.is_some_and(|(open, close)| i >= open && i <= close) {
                continue;
            }
            let spelling = self
                .sources
                .slice(token.span)
                .ok_or(QueryError::NotImplemented)?;
            let parameter = match token.kind {
                TokenKind::Ident(symbol) => names.iter().position(|n| *n == symbol),
                _ => None,
            };
            match parameter {
                Some(p) => text.push_str(spellings[p]),
                None => text.push_str(spelling),
            }
        }
        let instance = instance_path(declaration, key);
        let origin = self.sources.file(declaring).unwrap().path.display();
        let file = self
            .sources
            .add(format!("{origin}#{instance_name}"), text)
            .ok_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "generated instance".into(),
                    message: "instance source exceeds the source map".into(),
                })
            })?;
        let (Some(lexer), Some(parser)) =
            (self.adapters.lexer.clone(), self.adapters.parser.clone())
        else {
            return Err(QueryError::NotImplemented);
        };
        let context = FileContext {
            package: declaration.0.package.clone(),
            modules: declaration.0.modules.clone(),
        };
        let lexed = lexer.lex(file, &self.sources)?;
        if has_errors(&lexed.diagnostics) {
            return Err(QueryError::Diagnostics(lexed.diagnostics));
        }
        let parsed = parser.parse(file, lexed.value, &context, &mut self.sources)?;
        if has_errors(&parsed.diagnostics) {
            return Err(QueryError::Diagnostics(parsed.diagnostics));
        }
        let [syntax] = &parsed.value.items[..] else {
            return Err(QueryError::NotImplemented);
        };
        let mut syntax = (**syntax).clone();
        syntax.item.definition = instance.clone();
        syntax.item.visibility = Visibility::Private;
        self.items
            .entry(instance.clone())
            .or_default()
            .push(Arc::new(syntax));
        // The instance is a new definition: index its inputs and mark
        // readers; the signature snapshot now includes it.
        self.note_inputs([instance.clone()]);
        self.retire_snapshot();
        self.instance_bodies.declarations.insert(declaration_id);
        self.instance_bodies
            .instances
            .entry(declaration_id)
            .or_default()
            .insert(
                key,
                hirsel_tir::InstanceBody {
                    arguments: actuals.arguments.clone(),
                    definition: DefId::intern(&instance),
                },
            );
        self.store_generated_registration(declaration, &actuals.arguments, &instance)?;
        Ok((instance, true))
    }

    /// Select the instances of generic generated declarations that `roots`
    /// reach, walking each call's closed generic arguments through generic
    /// callees (CT-5). Returns every selected instance definition.
    pub(crate) fn select_generated_instances(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<BTreeSet<DefPath>, QueryError> {
        let any = self.items.iter().any(|(_, items)| {
            self.work.selection_probe_visits += 1;
            items.first().is_some_and(|s| instance_bodied(s))
        }) || self
            .catalog
            .has_instance_bodied(|path| self.items.contains_key(path));
        let mut selected = BTreeSet::new();
        if !any {
            return Ok(selected);
        }
        let mut pending: Vec<(DefPath, hirsel_types::ActualBindings)> = vec![];
        for root in roots {
            self.work.selection_root_visits += 1;
            if self.declared(&root)
                && !self.front_summary(DefId::intern(&root))?.summary.frames[0].generic
            {
                pending.push((root, BTreeMap::new()));
            }
        }
        let mut seen = BTreeSet::new();
        while let Some((path, bindings)) = pending.pop() {
            self.work.selection_walk_visits += 1;
            let binding_key: Vec<_> = bindings
                .iter()
                .map(|(site, actuals)| {
                    (
                        *site,
                        hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments),
                    )
                })
                .collect();
            if !seen.insert((path.clone(), binding_key)) {
                continue;
            }
            if seen.len() > 65536 {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "generated instance".into(),
                    message: "instance selection expansion limit".into(),
                }));
            }
            if !self.declared(&path) || self.instance_bodied(&path) {
                continue;
            }
            let published = self.front_summary(DefId::intern(&path))?;
            self.merge_execution(&published.reads);
            let summary = &published.summary;
            // Calls that name a definition: written ones, then planned instances.
            let calls: Vec<_> = summary
                .calls
                .iter()
                .filter(|call| call.target_frame.is_none())
                .map(|call| {
                    (
                        call.callee.path().clone(),
                        call.binder,
                        call.arguments.clone(),
                        call.site,
                    )
                })
                .collect();
            for (callee, binder, arguments, site) in calls {
                if !self.declared(&callee) {
                    continue;
                }
                let actuals =
                    hirsel_types::close_arguments(&self.types, &self.memo, &arguments, &bindings)
                        .map_err(|e| {
                        QueryError::Operational(hirsel_types::OperationalFailure {
                            operation: "generated instance".into(),
                            message: format!("closing {callee} arguments: {e:?}"),
                        })
                    })?;
                let closed =
                    hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments).is_some();
                if self.instance_bodied(&callee) {
                    if closed {
                        let instance = self.generated_instance(&callee, &actuals, site)?;
                        selected.insert(instance.clone());
                        pending.push((instance, BTreeMap::new()));
                    }
                } else if actuals.arguments.is_empty() {
                    pending.push((callee, BTreeMap::new()));
                } else if closed {
                    pending.push((callee, BTreeMap::from([(binder, actuals)])));
                }
            }
        }
        Ok(selected)
    }

    /// The instance of a generic generated declaration that runs a call at
    /// closed `arguments`, selecting it when evaluation reaches one that
    /// checking did not (a compile-time call); None for any other callee.
    pub(crate) fn resolve_instance(
        &mut self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        if !self.instance_bodied(path) {
            return Ok(None);
        }
        let actuals = hirsel_types::Actuals {
            arguments: arguments.to_vec(),
        };
        self.materialize(path)?;
        let site = self.snapshot(path).map(|s| s.item.name_span)?;
        self.generated_instance(path, &actuals, site).map(Some)
    }

    /// The instance table of the selected program: every generic generated
    /// declaration in `paths`, and the instances `paths` holds.
    pub(crate) fn instance_bodies(&self, paths: &BTreeSet<DefPath>) -> hirsel_tir::InstanceBodies {
        let mut table = hirsel_tir::InstanceBodies::default();
        for path in paths.iter().filter(|p| self.instance_bodied(p)) {
            let id = DefId::intern(path);
            table.declarations.insert(id);
            if let Some(instances) = self.instance_bodies.instances.get(&id) {
                let kept: std::collections::HashMap<_, _> = instances
                    .iter()
                    .filter(|(_, i)| paths.contains(i.definition.path()))
                    .map(|(k, i)| (*k, i.clone()))
                    .collect();
                if !kept.is_empty() {
                    table.instances.insert(id, kept);
                }
            }
        }
        table
    }
}

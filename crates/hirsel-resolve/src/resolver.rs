use crate::{LookupError, ManifestIndex, Namespace};
use hirsel_diag::{Diagnostic, Label, RuleId};
use hirsel_intern::Symbol;
use hirsel_span::Span;
use hirsel_syntax::*;
use hirsel_types::{
    self as ty, LocalId, ParamId, Resolution, ResolvedName, SignatureProvider, SizeParamId,
    SizeParameterRef, StaticArgument, TypeParamId, TypeParameterRef,
};
use std::collections::{BTreeMap, BTreeSet};

pub use hirsel_types::{
    ContextualName, DeferredName, DependencyRead, GenericBinding, GenericParameterName, NameSite,
};

/// Expected nominal/aggregate context supplied by syntax or later checking.
/// No body inference is performed here; external hints can finish deferred uses.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ExpectedType {
    /// A nominal type and its generic arguments in generic-list order; an
    /// unwritten, unknown or size argument is `Unknown`.
    Named {
        definition: DefPath,
        arguments: Vec<ExpectedType>,
    },
    Tuple(Vec<ExpectedType>),
    Function {
        parameters: Vec<ExpectedType>,
        result: Box<ExpectedType>,
    },
    #[default]
    Unknown,
}
impl ExpectedType {
    fn nominal(definition: DefPath) -> Self {
        Self::Named {
            definition,
            arguments: vec![],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExpectedTypes {
    /// Names available from an implicit library, only for deferral. This never
    /// selects a nominal owner or grants a type without expected context.
    pub available_variants: BTreeSet<String>,
    pub expressions: BTreeMap<ExprId, ExpectedType>,
    pub patterns: BTreeMap<PatternId, ExpectedType>,
    pub error_mappers: BTreeMap<ExprId, ExpectedType>,
}

#[derive(Clone, Debug)]
pub struct ResolveOutput {
    pub operational_failure: Option<ty::OperationalFailure>,
    pub query: ty::QueryOutput<Resolution>,
}

impl std::ops::Deref for ResolveOutput {
    type Target = Resolution;
    fn deref(&self) -> &Resolution {
        &self.query.value
    }
}
impl std::ops::DerefMut for ResolveOutput {
    fn deref_mut(&mut self) -> &mut Resolution {
        &mut self.query.value
    }
}

#[derive(Clone)]
struct Binding {
    name: ResolvedName,
    expected: ExpectedType,
    span: Span,
    owner: Option<NameSite>,
}
#[derive(Clone, Default)]
struct Scope {
    values: BTreeMap<Symbol, Binding>,
    generics: BTreeMap<Symbol, GenericName>,
    implementations: BTreeMap<Symbol, ImplementationBinding>,
    future: BTreeSet<Symbol>,
    owner: Option<NameSite>,
}
/// An implementation binder in scope (GEN-6): abstract in its declaration,
/// where each type or size member is a generic parameter and each operation
/// its family's header, or bound to `implementation` in a specialization,
/// where every member names the implementation's exact actual.
#[derive(Clone)]
struct ImplementationBinding {
    family: DefPath,
    implementation: Option<DefPath>,
    members: BTreeMap<String, BoundMember>,
    /// An abstract binder's member parameter ordinals, for sharing.
    ordinals: BTreeMap<String, u32>,
}
#[derive(Clone, PartialEq)]
enum BoundMember {
    Type(ResolvedName),
    /// A concrete type member's representation, for sharing and static actuals.
    TypeActual(ResolvedName, ty::TypeId),
    SizeParameter(SizeParameterRef),
    SizeConstant(u64),
    Operation(DefPath),
}
impl BoundMember {
    fn static_argument(&self) -> Option<StaticArgument> {
        Some(match self {
            Self::Type(ResolvedName::TypeParameter(p)) => {
                StaticArgument::Parameter(GenericParameterName::Type(*p))
            }
            Self::TypeActual(_, ty) => StaticArgument::Actual(ty::GenericArgument::Type(*ty)),
            Self::SizeParameter(p) => StaticArgument::Parameter(GenericParameterName::Size(*p)),
            Self::SizeConstant(n) => {
                StaticArgument::Actual(ty::GenericArgument::Size(ty::Size::Constant(*n)))
            }
            _ => return None,
        })
    }
}
#[derive(Clone)]
enum GenericName {
    Type(TypeParameterRef),
    Size(SizeParameterRef),
}
#[derive(Clone, Default)]
struct Shape {
    fields: Vec<(String, ExpectedType)>,
    variants: Vec<(String, Vec<(Option<String>, ExpectedType)>)>,
}

/// Resolves one definition, so local numbering cannot depend on declaration order.
/// The other items of `file` supply header fixtures, but resolution never walks
/// their bodies.
pub fn resolve_item(
    file: &SyntaxFile,
    syntax: &ItemSyntax,
    index: &ManifestIndex,
    types: &ty::TypeInterner,
    provider: &mut (impl SignatureProvider + ?Sized),
) -> ResolveOutput {
    resolve_item_with_expected(
        file,
        syntax,
        index,
        types,
        provider,
        &ExpectedTypes::default(),
    )
}

/// Syntax read domain. File permits local header fixtures from the file's
/// items; Item and Header require every other declaration to come through the
/// signature provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveView {
    File,
    Item,
    Header,
}

pub fn resolve_item_with_expected(
    file: &SyntaxFile,
    syntax: &ItemSyntax,
    index: &ManifestIndex,
    types: &ty::TypeInterner,
    provider: &mut (impl SignatureProvider + ?Sized),
    expected: &ExpectedTypes,
) -> ResolveOutput {
    resolve_view(
        syntax,
        &file.items,
        ResolveView::File,
        index,
        types,
        provider,
        expected,
    )
}

/// Resolve the selected domain without modifying syntax or its arena identities.
/// `siblings` are the items of the resolved item's file: their written variant
/// names and signatures serve as same-file fallbacks.
pub fn resolve_view(
    syntax: &ItemSyntax,
    siblings: &[std::sync::Arc<ItemSyntax>],
    view: ResolveView,
    index: &ManifestIndex,
    types: &ty::TypeInterner,
    provider: &mut (impl SignatureProvider + ?Sized),
    expected: &ExpectedTypes,
) -> ResolveOutput {
    let definition = &syntax.item.definition;
    let mut resolver = Resolver {
        syntax,
        siblings,
        lexical: syntax.owner(),
        view,
        index,
        types,
        provider,
        expected,
        context: FileContext {
            package: definition.0.package.clone(),
            modules: definition.0.modules.clone(),
        },
        output: ResolveOutput {
            operational_failure: None,
            query: ty::QueryOutput {
                value: Resolution::default(),
                diagnostics: vec![],
                dependencies: vec![],
            },
        },
        scopes: vec![Scope::default()],
        signatures: DefMap::default(),
        next_local: 0,
        alternative_locals: Vec::new(),
        generator_parameters: Vec::new(),
        callees: BTreeSet::new(),
        local_function_depth: 0,
        return_type: ExpectedType::Unknown,
        block_labels: Vec::new(),
        contract_kind: None,
        exported: false,
        shaping: BTreeSet::new(),
        type_arguments: vec![],
        sibling_fields: BTreeMap::new(),
        declaring: vec![],
        deferred: vec![],
    };
    resolver.item();
    let value = &mut resolver.output.query.value;
    value.expressions.sort_by_key(|(id, _)| *id);
    value.expressions.dedup();
    value.types.sort_by_key(|(id, _)| *id);
    value.types.dedup();
    value.bindings.sort_by_key(|(id, _, _)| *id);
    value.dependencies.sort();
    value.dependencies.dedup();
    resolver
        .output
        .query
        .dependencies
        .sort_by_key(|a| a.definition);
    resolver.output
}

struct Resolver<'a, P: ?Sized> {
    /// The item being read: the resolved one, or a same-file header fixture.
    syntax: &'a ItemSyntax,
    /// The items of the resolved item's file.
    siblings: &'a [std::sync::Arc<ItemSyntax>],
    /// The owner of the resolved item's lexical sites.
    lexical: LexicalOwner,
    view: ResolveView,
    index: &'a ManifestIndex,
    /// The session interner every signature's rows live in.
    types: &'a ty::TypeInterner,
    provider: &'a mut P,
    expected: &'a ExpectedTypes,
    context: FileContext,
    output: ResolveOutput,
    scopes: Vec<Scope>,
    signatures: DefMap<Option<ty::SignatureRef>>,
    next_local: u32,
    callees: BTreeSet<ExprId>,
    local_function_depth: usize,
    return_type: ExpectedType,
    /// Each enclosing labelled block's expected type, for its break operands
    /// (LANG-11); a closure or local function starts empty.
    block_labels: Vec<(Symbol, ExpectedType)>,
    contract_kind: Option<ContractKind>,
    exported: bool,
    shaping: BTreeSet<DefPath>,
    /// While shaping a same-file generic type: its parameters' arguments.
    type_arguments: Vec<(Symbol, ExpectedType)>,
    sibling_fields: BTreeMap<Symbol, (DefPath, ty::FieldId, TypeExprId)>,
    /// Names a let pattern or parameter is declaring: a relation naming one
    /// stands for that binding's own scope (MODE-1, LANG-15).
    declaring: Vec<Symbol>,
    /// Relation places that name a binding being declared, resolved once it is.
    deferred: Vec<(Path, Span, NameSite)>,
    /// While resolving a later alternative of an or-pattern: the locals the
    /// first alternative bound, and whether this alternative has bound each.
    alternative_locals: Vec<BTreeMap<Symbol, (LocalId, bool)>>,
    /// While resolving a CT-5 generator: the generated function's declared
    /// parameters, which name run-time values outside its closed frame.
    generator_parameters: Vec<Symbol>,
}

impl<'a, P: SignatureProvider + ?Sized> Resolver<'a, P> {
    fn spelling(&self, symbol: Symbol) -> String {
        symbol.as_str().to_owned()
    }
    fn diagnostic(&mut self, rule: RuleId, span: Span, message: impl Into<String>) {
        self.output
            .query
            .diagnostics
            .push(Diagnostic::error(rule, span, message));
    }
    fn lookup(&mut self, path: &Path, _span: Span) -> Result<DefPath, LookupError> {
        self.lookup_using(path)
    }
    fn lookup_using(&mut self, path: &Path) -> Result<DefPath, LookupError> {
        let segments = path
            .segments
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect::<Vec<_>>();
        let mut result = self.index.lookup(&self.context, path);
        // The provider can supply builtin/driver-owned names absent from this
        // manifest snapshot. It is never allowed to override ambiguity/privacy.
        if matches!(
            result,
            Err(LookupError::Missing | LookupError::MissingMember(_))
        ) {
            let lookup = self.provider.lookup(&self.context, path);
            std::sync::Arc::make_mut(&mut self.output.reads).push(
                DependencyRead::ProviderLookup {
                    context: (self.context.clone()).into(),
                    package: path.package.clone(),
                    segments: (path
                        .segments
                        .iter()
                        .map(|s| s.as_str().to_owned())
                        .collect::<Vec<_>>())
                    .into(),
                    output: (lookup.clone()).into(),
                }
                .shared(),
            );
            match lookup.result {
                Ok(definition) => {
                    // Provider-only declarations must belong to the selected
                    // module targets; an alias cannot fall through to core.
                    if !matches!(&result, Err(LookupError::MissingMember(candidates)) if !candidates.contains(&definition))
                    {
                        result = self.index.access(&self.context, &definition);
                    }
                }
                Err(ty::QueryError::Diagnostics(diagnostics)) => {
                    if !diagnostics.is_empty() {
                        result = Err(LookupError::Provider(diagnostics));
                    }
                }
                Err(ty::QueryError::NotImplemented) => {}
                Err(ty::QueryError::Operational(e)) => self.output.operational_failure = Some(e),
            }
        }
        std::sync::Arc::make_mut(&mut self.output.reads).push(
            DependencyRead::Lookup {
                context: (self.context.clone()).into(),
                package: path.package.clone(),
                segments: (segments).into(),
                manifest: self.index.revision,
                result: (result.clone()).map(Into::into),
            }
            .shared(),
        );
        if let Ok(definition) = &result {
            self.output
                .query
                .value
                .dependencies
                .push(definition.clone());
        }
        result
    }
    // Token tape, rather than the enclosing call/type span, proves the edit
    // only replaces the misspelled identifier. Absent tape means help only.
    fn name_span(&self, span: Span, name: Symbol, last: bool) -> Option<Span> {
        let mut tokens = self.syntax.tree.tokens.iter().filter(|token| {
            token.span.anchor == span.anchor
                && token.span.start >= span.start
                && token.span.end <= span.end
                && token.kind == TokenKind::Ident(name)
        });
        if last {
            tokens.next_back()
        } else {
            tokens.next()
        }
        .map(|t| t.span)
    }
    fn suggest_field(&mut self, span: Span, name: Symbol, candidates: Vec<String>, prefix: &str) {
        let name_span = self.name_span(span, name, true);
        let spelling = self.spelling(name);
        if let Some(diagnostic) = self.output.query.diagnostics.last_mut() {
            hirsel_diag::suggest_names(diagnostic, &spelling, candidates, prefix, name_span);
        }
    }
    fn path_name_span(&self, path: &Path, span: Span, segment: usize) -> Option<Span> {
        let tokens: Vec<_> = self
            .syntax
            .tree
            .tokens
            .iter()
            .filter(|token| {
                token.span.anchor == span.anchor
                    && token.span.start >= span.start
                    && token.span.end <= span.end
                    && !matches!(token.kind, TokenKind::Trivia(_))
            })
            .collect();
        let start = if path.package.is_some() {
            tokens
                .iter()
                .position(|t| t.kind == TokenKind::Punct(Punct::Slash))?
                + 1
        } else {
            let head = *path.segments.first()?;
            tokens
                .iter()
                .position(|t| t.kind == TokenKind::Ident(head))?
        };
        for (i, name) in path.segments.iter().enumerate() {
            if tokens.get(start + i * 2)?.kind != TokenKind::Ident(*name) {
                return None;
            }
            if i > 0
                && !matches!(
                    tokens.get(start + i * 2 - 1)?.kind,
                    TokenKind::Punct(Punct::Dot | Punct::Slash)
                )
            {
                return None;
            }
        }
        Some(tokens.get(start + segment * 2)?.span)
    }
    fn suggest_path(&mut self, path: &Path, span: Span) {
        let segments: Vec<_> = path.segments.iter().map(|s| self.spelling(*s)).collect();
        // Prefer a member typo; then try a misspelled module head.
        for segment in (0..segments.len()).rev() {
            let mut candidates = self.index.candidate_names(
                &self.context,
                path.package.as_ref(),
                &segments,
                segment,
            );
            if path.package.is_none() && segments.len() == 1 {
                for scope in &self.scopes {
                    candidates.extend(
                        scope
                            .values
                            .keys()
                            .chain(scope.generics.keys())
                            .map(|s| self.spelling(*s)),
                    );
                }
            }
            let prefix = if let Some(package) = &path.package {
                format!(
                    "{}@{}/{}",
                    package.name,
                    package.version,
                    segments[..segment]
                        .iter()
                        .map(|s| format!("{s}/"))
                        .collect::<String>()
                )
            } else {
                segments[..segment]
                    .iter()
                    .map(|s| format!("{s}."))
                    .collect()
            };
            let name_span = self.path_name_span(path, span, segment);
            if let Some(diagnostic) = self.output.query.diagnostics.last_mut() {
                hirsel_diag::suggest_names(
                    diagnostic,
                    &segments[segment],
                    candidates,
                    &prefix,
                    name_span,
                );
                if diagnostic
                    .notes
                    .iter()
                    .any(|n| n.starts_with("help: did you mean"))
                {
                    break;
                }
            }
        }
    }
    fn lookup_diagnostic(&mut self, result: &LookupError, path: &Path, span: Span) {
        let segments = path
            .segments
            .iter()
            .map(|s| self.spelling(*s))
            .collect::<Vec<_>>();
        let name = match &path.package {
            Some(package) => format!(
                "{}@{}/{}",
                package.name,
                package.version,
                segments.join("/")
            ),
            None => segments.join("."),
        };
        match result {
            LookupError::Provider(diagnostics) => {
                self.output
                    .query
                    .diagnostics
                    .extend(diagnostics.iter().cloned().map(|mut d| {
                        d.primary = span;
                        d
                    }))
            }
            LookupError::Missing
                if path.package.is_none()
                    && matches!(&path.segments[..], [segment] if self.generator_parameters.contains(segment)) =>
            {
                self.diagnostic(
                    const { RuleId::of("CT-2") },
                    span,
                    format!(
                        "a generator reads no parameter: `{name}` is a run-time value of the \
                         generated function"
                    ),
                )
            }
            LookupError::Missing => self.diagnostic(
                const { RuleId::of("LANG-4") },
                span,
                format!("unresolved name `{name}`"),
            ),
            LookupError::MissingMember(candidates) => self.diagnostic(
                const { RuleId::of("LANG-4") },
                span,
                format!(
                    "unresolved module member `{name}`; looked for {}",
                    candidates
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            LookupError::Ambiguous(candidates) => self.diagnostic(
                const { RuleId::of("LANG-4") },
                span,
                format!(
                    "ambiguous module head in `{name}`: {}; write a full package/version path",
                    candidates
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            LookupError::Invisible(_) => self.diagnostic(
                const { RuleId::of("PROG-2") },
                span,
                "definition is not visible from this package/module",
            ),
        }
        if matches!(result, LookupError::Missing | LookupError::MissingMember(_)) {
            self.suggest_path(path, span);
        }
    }
    /// Explain only a variant in the expected type's already-read header.
    /// This diagnostic must not discover signatures, infer from payloads, or
    /// change the lookup/dependency reads of the rejected expression.
    fn qualified_variant_help(
        &self,
        error: &LookupError,
        path: &Path,
        expected: &ExpectedType,
    ) -> Option<String> {
        let ExpectedType::Named { definition, .. } = expected else {
            return None;
        };
        let (name, modules) = path.segments.split_last()?;
        let owner = &definition.0;
        let same_module = match error {
            LookupError::MissingMember(candidates) => candidates
                .iter()
                .any(|d| d.0.package == owner.package && d.0.modules == owner.modules),
            LookupError::Missing => {
                let modules: Vec<_> = modules.iter().map(|s| s.as_str().to_owned()).collect();
                if let Some(package) = &path.package {
                    package == &owner.package && modules == owner.modules
                } else {
                    !modules.is_empty()
                        && owner.package == self.context.package
                        && (modules == owner.modules
                            || self
                                .context
                                .modules
                                .iter()
                                .chain(&modules)
                                .eq(owner.modules.iter()))
                }
            }
            _ => false,
        };
        if !same_module || self.index.access(&self.context, definition).is_err() {
            return None;
        }
        let signature = self.signatures.get(&DefId::intern(definition))?.as_ref()?;
        if owner.package != self.context.package
            && signature.metadata.visibility != Visibility::Public
        {
            return None;
        }
        let ty::SignatureKind::Type(declaration) = &signature.kind else {
            return None;
        };
        let ty::TypeKind::Variant(variants) = &self.types.get(declaration.representation?)?.kind
        else {
            return None;
        };
        if !variants.iter().any(|v| v.name == *name) {
            return None;
        }
        Some(format!(
            "help: variants are written bare and selected by the expected type; write `{name}` \
             without the qualifier for `{definition}` (keep any payload arguments). \
             When several types declare `{name}`, supply an expected type, for example \
             with a binding annotation or function return type"
        ))
    }
    fn signature(&mut self, definition: &DefPath) -> Option<ty::SignatureRef> {
        // A header's own declaration/group is being normalized, so no published
        // signature exists yet. Syntax supplies recursive nominal shape;
        // probing the provider would turn reentrancy into a false absence read.
        if matches!(self.view, ResolveView::Header)
            && (definition == &self.syntax.item.definition
                || self
                    .provider
                    .in_recursive_group(&self.syntax.item.definition, definition))
        {
            return None;
        }
        let id = DefId::intern(definition);
        if let Some(signature) = self.signatures.get(&id) {
            return signature.clone();
        }
        let result = self.provider.signature(id);
        let signature = match result {
            Ok(signature) => {
                self.output
                    .query
                    .dependencies
                    .push(ty::SignatureDependency {
                        definition: DefId::intern(definition),
                        fingerprint: signature.fingerprint,
                    });
                Some(signature)
            }
            Err(ty::QueryError::Diagnostics(d)) => {
                self.output.query.diagnostics.extend(d);
                None
            }
            Err(ty::QueryError::NotImplemented) => None,
            Err(ty::QueryError::Operational(e)) => {
                self.output.operational_failure = Some(e);
                None
            }
        };
        std::sync::Arc::make_mut(&mut self.output.reads).push(
            DependencyRead::Signature {
                definition: id,
                fingerprint: signature.as_ref().map(|s| s.fingerprint),
            }
            .shared(),
        );
        self.output
            .query
            .value
            .dependencies
            .push(definition.clone());
        self.signatures.insert(id, signature.clone());
        signature
    }
    fn namespace(&mut self, definition: &DefPath) -> Option<Namespace> {
        if let Some(entry) = self.index.entry(definition) {
            return Some(entry.namespace);
        }
        self.signature(definition).map(|s| {
            if matches!(s.kind, ty::SignatureKind::Type(_)) {
                Namespace::Type
            } else {
                Namespace::Value
            }
        })
    }
    fn value(&self, name: Symbol) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.values.get(&name))
            .cloned()
    }
    fn generic(&self, name: Symbol) -> Option<GenericName> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.generics.get(&name))
            .cloned()
    }
    fn bind(&mut self, name: Symbol, binding: Binding) {
        if let Some(old) = self.value(name) {
            let mut d = Diagnostic::error(
                const { RuleId::of("LANG-4") },
                binding.span,
                "a local cannot shadow another local",
            );
            d.secondary.push(Label {
                span: old.span,
                message: "earlier local".into(),
            });
            self.output.query.diagnostics.push(d);
            return;
        }
        // Probe exactly the declaration module. Ordinary value lookup may fall
        // back to the package root; a local is allowed to shadow that fallback.
        // The normal lookup read records both absence and namespace for caching.
        let mut segments: Vec<_> = self
            .context
            .modules
            .iter()
            .map(|m| Symbol::intern(m))
            .collect();
        segments.push(Symbol::intern(&self.spelling(name)));
        let path = Path {
            package: Some(self.context.package.clone()),
            segments,
        };
        let read = self.lookup_using(&path);
        if let Ok(definition) = read
            && definition.0.package == self.context.package
            && definition.0.modules == self.context.modules
            && self.namespace(&definition) == Some(Namespace::Value)
        {
            let mut diagnostic = Diagnostic::error(
                const { RuleId::of("LANG-4") },
                binding.span,
                "a local cannot shadow a top-level value in the same module",
            );
            let module = std::iter::once(format!(
                "{}@{}",
                definition.0.package.name, definition.0.package.version
            ))
            .chain(definition.0.modules.iter().cloned())
            .collect::<Vec<_>>()
            .join("/");
            // Declaration metadata already includes other files and later items.
            // Do not read another body or signature just to obtain a location.
            // Use the existing citation read when available so edits invalidate
            // the diagnostic. The index also locates declarations in other files.
            let declaration_span = self
                .provider
                .cite_name(&definition)
                .or_else(|| self.index.entry(&definition).map(|entry| entry.span));
            if let Some(span) = declaration_span
                && span.anchor != hirsel_span::Anchor(0)
            {
                diagnostic.secondary.push(Label {
                    span,
                    message: format!("conflicting declaration in module `{module}`"),
                });
            } else {
                diagnostic
                    .notes
                    .push("the conflicting declaration's source is unavailable".into());
            }
            // Keep the identity even if a renderer lacks the indexed source map.
            diagnostic
                .notes
                .push(format!("conflicting top-level declaration: `{definition}`"));
            diagnostic.notes.push(format!(
                "top-level declarations share module `{module}`'s namespace regardless of source order, including later test functions"
            ));
            // A binder-only fix would leave its uses unresolved or captured.
            diagnostic.notes.push(
                "help: choose a distinct, non-reserved local name unused in this scope and module, and update every reference to this binding".into(),
            );
            self.output.query.diagnostics.push(diagnostic);
            return;
        }
        self.scopes.last_mut().unwrap().values.insert(name, binding);
    }
    fn local(&mut self, name: Symbol, span: Span, expected: ExpectedType) -> LocalId {
        let id = LocalId(self.next_local);
        self.next_local += 1;
        self.bind(
            name,
            Binding {
                name: ResolvedName::Local(id),
                span,
                expected,
                owner: None,
            },
        );
        id
    }
    fn origin(&self, site: NameSite) -> Origin {
        match site {
            NameSite::Item => self.syntax.item.origin,
            NameSite::Expression(id) => self.syntax.tree.expressions[id.0 as usize].origin,
            NameSite::Statement(id) => self.syntax.tree.statements[id.0 as usize].origin,
            NameSite::Type(id) => self.syntax.tree.types[id.0 as usize].origin,
            NameSite::Pattern(id) => self.syntax.tree.patterns[id.0 as usize].origin,
        }
    }
    fn generics(&mut self, generics: &[GenericParam], span: Span) {
        let owner = self
            .scopes
            .last()
            .unwrap()
            .owner
            .expect("generic declaration has an owner");
        let name_spans = self.syntax.tree.generic_name_spans(self.origin(owner));
        let scope = self.lexical.site(owner);
        // GEN-6: ordinary parameters keep their written order; each abstract
        // implementation binder's unshared type and size members follow, so a
        // specialization (whose binders add none) keeps the same ordinals.
        let mut declarations = vec![];
        for (ordinal, parameter) in generics.iter().enumerate() {
            if self.generic(parameter.name).is_some()
                || self.value(parameter.name).is_some()
                || self.implementation(parameter.name).is_some()
                || generics[..ordinal].iter().any(|p| p.name == parameter.name)
            {
                self.diagnostic(
                    const { RuleId::of("GEN-1") },
                    name_spans.get(ordinal).copied().unwrap_or(span),
                    "duplicate generic parameter",
                );
                continue;
            }
            let position = declarations.len() as u32;
            let name = match &parameter.kind {
                GenericKind::Type(bounds) => {
                    declarations.push(ty::GenericParameter::Type {
                        name: parameter.name.as_str().to_owned(),
                        bounds: bounds.clone().into(),
                    });
                    GenericName::Type(TypeParameterRef::body(scope, TypeParamId(position)))
                }
                GenericKind::Size => {
                    declarations.push(ty::GenericParameter::Size {
                        name: parameter.name.as_str().to_owned(),
                    });
                    GenericName::Size(SizeParameterRef::body(scope, SizeParamId(position)))
                }
                GenericKind::Implementation { .. } => continue,
            };
            let resolved = match name {
                GenericName::Type(id) => GenericParameterName::Type(id),
                GenericName::Size(id) => GenericParameterName::Size(id),
            };
            self.output.generics.push(GenericBinding {
                owner,
                name: parameter.name,
                parameter: resolved,
            });
            self.scopes
                .last_mut()
                .unwrap()
                .generics
                .insert(parameter.name, name);
        }
        for (ordinal, parameter) in generics.iter().enumerate() {
            let GenericKind::Implementation { signature, sharing } = &parameter.kind else {
                continue;
            };
            let at = name_spans.get(ordinal).copied().unwrap_or(span);
            if owner != NameSite::Item
                || !matches!(self.syntax.item.kind, ItemKind::Function { .. })
            {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    at,
                    "only a top-level function declares implementation binders",
                );
                continue;
            }
            if self
                .scopes
                .last()
                .unwrap()
                .implementations
                .contains_key(&parameter.name)
            {
                continue;
            }
            if let Some(binding) = self.implementation_binder(
                parameter,
                signature,
                sharing,
                scope,
                at,
                &mut declarations,
            ) {
                self.scopes
                    .last_mut()
                    .unwrap()
                    .implementations
                    .insert(parameter.name, binding);
            }
        }
        if !declarations.is_empty() {
            self.output.generic_declarations.insert(scope, declarations);
        }
    }
    /// The binder's family and member denotations (GEN-6). A specialization
    /// item binds it to the implementation its `Generated::Specialization`
    /// names; otherwise each unshared type or size member is a new parameter.
    fn implementation_binder(
        &mut self,
        parameter: &GenericParam,
        signature: &Path,
        sharing: &[Sharing],
        scope: ty::SiteRef,
        at: Span,
        declarations: &mut Vec<ty::GenericParameter>,
    ) -> Option<ImplementationBinding> {
        let family_path = match self.lookup(signature, at) {
            Ok(path) => path,
            Err(error) => {
                self.lookup_diagnostic(&error, signature, at);
                return None;
            }
        };
        let family = match self.signature(&family_path).map(|s| s.kind.clone()) {
            Some(ty::SignatureKind::Family(family)) => family,
            Some(_) => {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    at,
                    format!("`{family_path}` is not a signature"),
                );
                return None;
            }
            None => return None,
        };
        let implementation = match &self.syntax.item.generated {
            Some(Generated::Specialization(bindings)) => bindings
                .iter()
                .find(|(name, _)| *name == parameter.name)
                .map(|(_, implementation)| implementation.clone()),
            _ => None,
        };
        let actuals = match &implementation {
            Some(path) => match self.signature(path).map(|s| s.kind.clone()) {
                Some(ty::SignatureKind::Implementation(actuals))
                    if actuals.family.path() == &family_path =>
                {
                    Some(actuals)
                }
                _ => {
                    self.diagnostic(
                        const { RuleId::of("GEN-6") },
                        at,
                        format!("`{path}` does not implement `{family_path}`"),
                    );
                    return None;
                }
            },
            None => None,
        };
        for equation in sharing {
            if family
                .member(equation.member.as_str())
                .is_none_or(|m| matches!(m.kind, ty::FamilyMemberKind::Operation))
            {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    equation.origin.span,
                    format!(
                        "`{family_path}` has no type or size member `{}` to share",
                        equation.member
                    ),
                );
                return None;
            }
        }
        let mut binding = ImplementationBinding {
            family: family_path.clone(),
            implementation: implementation.clone(),
            members: BTreeMap::new(),
            ordinals: BTreeMap::new(),
        };
        for member in &family.members {
            let name = member.name.clone();
            let bound = if let Some(equation) =
                sharing.iter().find(|e| e.member.as_str() == member.name)
            {
                let other = self.implementation(equation.binder);
                let Some(shared) = other
                    .as_ref()
                    .and_then(|other| other.members.get(&member.name).cloned())
                else {
                    self.diagnostic(
                        const { RuleId::of("GEN-6") },
                        equation.origin.span,
                        format!(
                            "`{}` is not an earlier implementation binder with member `{}`",
                            equation.binder, member.name
                        ),
                    );
                    return None;
                };
                let kind_matches = matches!(
                    (&member.kind, &shared),
                    (
                        ty::FamilyMemberKind::Type { .. },
                        BoundMember::Type(_) | BoundMember::TypeActual(..)
                    ) | (
                        ty::FamilyMemberKind::Size,
                        BoundMember::SizeParameter(_) | BoundMember::SizeConstant(_)
                    )
                );
                if !kind_matches {
                    self.diagnostic(
                        const { RuleId::of("GEN-6") },
                        equation.origin.span,
                        "a sharing equation relates two type members or two size members",
                    );
                    return None;
                }
                if let Some(ordinal) = other.and_then(|o| o.ordinals.get(&member.name).copied()) {
                    binding.ordinals.insert(name.clone(), ordinal);
                }
                shared
            } else {
                match (&member.kind, &actuals) {
                    (ty::FamilyMemberKind::Operation, Some(actuals)) => {
                        match actuals.member(&member.name) {
                            Some(ty::MemberActual::Operation(target)) => {
                                self.output.query.value.dependencies.push(target.clone());
                                BoundMember::Operation(target.clone())
                            }
                            _ => return None,
                        }
                    }
                    (ty::FamilyMemberKind::Operation, None) => {
                        let path = ty::family_operation_path(&family_path, &member.name);
                        self.output.query.value.dependencies.push(path.clone());
                        BoundMember::Operation(path)
                    }
                    (ty::FamilyMemberKind::Type { .. }, Some(actuals)) => {
                        let alias = ty::implementation_type_path(
                            implementation.as_ref().unwrap(),
                            &member.name,
                        );
                        self.output.query.value.dependencies.push(alias.clone());
                        match actuals.member(&member.name) {
                            Some(ty::MemberActual::Type(actual)) => {
                                BoundMember::TypeActual(ResolvedName::Definition(alias), *actual)
                            }
                            _ => return None,
                        }
                    }
                    (ty::FamilyMemberKind::Size, Some(actuals)) => {
                        match actuals.member(&member.name) {
                            Some(ty::MemberActual::Size(n)) => BoundMember::SizeConstant(*n),
                            _ => return None,
                        }
                    }
                    (ty::FamilyMemberKind::Type { bounds }, None) => {
                        let position = declarations.len() as u32;
                        declarations.push(ty::GenericParameter::Type {
                            name: format!("{}.{}", parameter.name, member.name),
                            bounds: bounds.clone(),
                        });
                        binding.ordinals.insert(name.clone(), position);
                        BoundMember::Type(ResolvedName::TypeParameter(TypeParameterRef::body(
                            scope,
                            TypeParamId(position),
                        )))
                    }
                    (ty::FamilyMemberKind::Size, None) => {
                        let position = declarations.len() as u32;
                        declarations.push(ty::GenericParameter::Size {
                            name: format!("{}.{}", parameter.name, member.name),
                        });
                        binding.ordinals.insert(name.clone(), position);
                        BoundMember::SizeParameter(SizeParameterRef::body(
                            scope,
                            SizeParamId(position),
                        ))
                    }
                }
            };
            binding.members.insert(name, bound);
        }
        if implementation.is_none() {
            let members = family
                .members
                .iter()
                .filter_map(|m| Some((m.name.clone(), *binding.ordinals.get(&m.name)?)))
                .collect();
            self.output
                .implementation_binders
                .push(ty::ImplementationBinder {
                    name: parameter.name.as_str().to_owned(),
                    family: DefId::intern(&family_path),
                    members,
                });
        }
        Some(binding)
    }
    fn implementation(&self, name: Symbol) -> Option<ImplementationBinding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.implementations.get(&name))
            .cloned()
    }
    /// `H.member` where `H` is an implementation binder in scope (GEN-6):
    /// None for any other path, `Some(None)` for a missing member (reported).
    #[allow(clippy::option_option)]
    fn binder_member(
        &mut self,
        path: &Path,
        span: Span,
    ) -> Option<Option<(ImplementationBinding, BoundMember)>> {
        let [binder, member] = path.segments.as_slice() else {
            return None;
        };
        if path.package.is_some() || self.value(*binder).is_some() {
            return None;
        }
        let binding = self.implementation(*binder)?;
        let bound = binding.members.get(member.as_str()).cloned();
        if bound.is_none() {
            self.diagnostic(
                const { RuleId::of("GEN-6") },
                span,
                format!("signature `{}` has no member `{member}`", binding.family),
            );
        }
        Some(bound.map(|bound| (binding, bound)))
    }
    fn binder_member_quiet(&self, path: &Path) -> Option<BoundMember> {
        let [binder, member] = path.segments.as_slice() else {
            return None;
        };
        self.implementation(*binder)?
            .members
            .get(member.as_str())
            .cloned()
    }
    /// `f with (H = impl, ..)` (GEN-6). Every binder of `f` is named once, by an
    /// implementation of its family or an enclosing binder, and shared members
    /// agree. With every actual closed, the clause names the specialization,
    /// an ordinary definition; otherwise `f` with each member's static actual.
    fn with_clause(&mut self, id: ExprId, callee: ExprId, bindings: &[StaticBinding], span: Span) {
        let Some((_, ResolvedName::Definition(base))) = self
            .output
            .expressions
            .iter()
            .rev()
            .find(|(e, _)| *e == callee)
            .cloned()
        else {
            self.diagnostic(
                const { RuleId::of("GEN-6") },
                span,
                "a `with` clause follows a function declaring implementation binders",
            );
            return;
        };
        let binders = match self.signature(&base) {
            Some(signature) if matches!(signature.kind, ty::SignatureKind::Function(_)) => {
                signature.metadata.implementations.clone()
            }
            _ => vec![],
        };
        if binders.is_empty() {
            self.diagnostic(
                const { RuleId::of("GEN-6") },
                span,
                format!("`{base}` declares no implementation binders"),
            );
            return;
        }
        for (i, binding) in bindings.iter().enumerate() {
            if bindings[..i].iter().any(|b| b.name == binding.name) {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    binding.origin.span,
                    format!("implementation binder `{}` is bound twice", binding.name),
                );
                return;
            }
            if !binders.iter().any(|b| b.name == binding.name.as_str()) {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    binding.origin.span,
                    format!("`{base}` has no implementation binder `{}`", binding.name),
                );
                return;
            }
        }
        let mut actuals = vec![];
        for binder in &binders {
            let Some(binding) = bindings.iter().find(|b| b.name.as_str() == binder.name) else {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    span,
                    format!(
                        "implementation actual for `{}` is required: write `with ({} = ..)`",
                        binder.name, binder.name
                    ),
                );
                return;
            };
            let at = binding.origin.span;
            let path = &binding.implementation;
            let actual = if path.package.is_none()
                && let [name] = path.segments.as_slice()
                && let Some(enclosing) = self.implementation(*name)
            {
                enclosing
            } else {
                let implementation = match self.lookup(path, at) {
                    Ok(definition) => definition,
                    Err(error) => {
                        self.lookup_diagnostic(&error, path, at);
                        return;
                    }
                };
                let Some(ty::SignatureKind::Implementation(signature)) =
                    self.signature(&implementation).map(|s| s.kind.clone())
                else {
                    self.diagnostic(
                        const { RuleId::of("GEN-6") },
                        at,
                        format!("`{implementation}` is not an implementation"),
                    );
                    return;
                };
                let members = signature
                    .members
                    .iter()
                    .map(|(name, actual)| {
                        let bound = match actual {
                            ty::MemberActual::Type(ty) => BoundMember::TypeActual(
                                ResolvedName::Definition(ty::implementation_type_path(
                                    &implementation,
                                    name,
                                )),
                                *ty,
                            ),
                            ty::MemberActual::Size(n) => BoundMember::SizeConstant(*n),
                            ty::MemberActual::Operation(target) => {
                                BoundMember::Operation(target.clone())
                            }
                        };
                        (name.clone(), bound)
                    })
                    .collect();
                ImplementationBinding {
                    family: signature.family.path().clone(),
                    implementation: Some(implementation),
                    members,
                    ordinals: BTreeMap::new(),
                }
            };
            if actual.family != *binder.family.path() {
                self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    at,
                    format!(
                        "binder `{}` requires an implementation of `{}`, not of `{}`",
                        binder.name,
                        binder.family.path(),
                        actual.family
                    ),
                );
                return;
            }
            actuals.push((binder, actual, at));
        }
        // Sharing: binders whose members denote one ordinal take equal actuals.
        let mut statics: BTreeMap<u32, (BoundMember, Span)> = BTreeMap::new();
        for (binder, actual, at) in &actuals {
            for (member, ordinal) in &binder.members {
                let Some(bound) = actual.members.get(member) else {
                    continue;
                };
                match statics.get(ordinal) {
                    Some((earlier, _)) if !self.same_member(earlier, bound) => {
                        self.diagnostic(
                            const { RuleId::of("GEN-6") },
                            *at,
                            format!(
                                "binder `{}` shares member `{member}`, but its actual's `{member}` differs",
                                binder.name
                            ),
                        );
                        return;
                    }
                    Some(_) => {}
                    None => {
                        statics.insert(*ordinal, (bound.clone(), *at));
                    }
                }
            }
        }
        // The call checks against the declaration with every member supplied;
        // a closed clause then targets the specialization (GEN-6).
        let arguments = statics
            .into_iter()
            .filter_map(|(ordinal, (bound, _))| Some((ordinal, bound.static_argument()?)))
            .collect();
        self.output.static_arguments.push((callee, arguments));
        let closed = actuals.iter().all(|(_, a, _)| a.implementation.is_some());
        if closed {
            let named = actuals
                .iter()
                .map(|(b, a, _)| (b.name.clone(), a.implementation.clone().unwrap()))
                .collect::<Vec<_>>();
            let specialization = ty::specialization_path(&base, &named);
            self.output
                .query
                .value
                .dependencies
                .push(specialization.clone());
            self.output
                .expressions
                .push((id, ResolvedName::Definition(specialization)));
        } else {
            self.output
                .expressions
                .push((id, ResolvedName::Definition(base)));
        }
    }
    fn same_member(&self, a: &BoundMember, b: &BoundMember) -> bool {
        match (a, b) {
            (BoundMember::TypeActual(_, x), BoundMember::TypeActual(_, y)) => {
                self.types.semantic_eq(*x, *y)
            }
            _ => a == b,
        }
    }
    /// The static actuals an abstract binder supplies to its family's
    /// operation headers: each type and size member, in family order.
    fn operation_arguments(
        &mut self,
        binding: &ImplementationBinding,
    ) -> Vec<(u32, StaticArgument)> {
        let Some(ty::SignatureKind::Family(family)) =
            self.signature(&binding.family).map(|s| s.kind.clone())
        else {
            return vec![];
        };
        family
            .members
            .iter()
            .filter(|m| !matches!(m.kind, ty::FamilyMemberKind::Operation))
            .enumerate()
            .filter_map(|(ordinal, m)| {
                Some((
                    ordinal as u32,
                    binding.members.get(&m.name)?.static_argument()?,
                ))
            })
            .collect()
    }
    fn item(&mut self) {
        let item = self.syntax.item.clone();
        self.scopes.last_mut().unwrap().owner = Some(NameSite::Item);
        self.exported = item.visibility != Visibility::Private;
        match item.kind {
            ItemKind::Signature { members } => {
                // Operation headers are their own declarations (GEN-6).
                let mut names = BTreeSet::new();
                for member in &members {
                    if !names.insert(member.name()) {
                        self.diagnostic(
                            const { RuleId::of("GEN-6") },
                            member.origin().span,
                            format!("duplicate signature member `{}`", member.name()),
                        );
                    }
                    if let MemberSpec::Type {
                        authority: Some(paths),
                        origin,
                        ..
                    } = member
                        && !paths.is_empty()
                    {
                        self.diagnostic(
                            const { RuleId::of("GEN-6") },
                            origin.span,
                            "a type member's authority is empty: write `uses {}` or a bound",
                        );
                    }
                }
            }
            ItemKind::Implementation { signature, members } => {
                // Paths: the family, then each operation target in member order.
                let span = item.name_span;
                match self.lookup(&signature, span) {
                    Ok(family) => self
                        .output
                        .paths
                        .push((NameSite::Item, ResolvedName::Definition(family))),
                    Err(error) => self.lookup_diagnostic(&error, &signature, span),
                }
                let mut names = BTreeSet::new();
                for member in &members {
                    if !names.insert(member.name()) {
                        self.diagnostic(
                            const { RuleId::of("GEN-6") },
                            member.origin().span,
                            format!("duplicate implementation member `{}`", member.name()),
                        );
                    }
                    match member {
                        MemberDefinition::Type { ty, .. } => {
                            self.type_expr(*ty);
                        }
                        MemberDefinition::Size { .. } => {}
                        MemberDefinition::Operation { target, origin, .. } => {
                            match self.lookup(target, origin.span) {
                                Ok(definition) => {
                                    self.signature(&definition);
                                    self.output.paths.push((
                                        NameSite::Item,
                                        ResolvedName::Definition(definition),
                                    ))
                                }
                                Err(error) => self.lookup_diagnostic(&error, target, origin.span),
                            }
                        }
                    }
                }
            }
            ItemKind::Function {
                signature, body, ..
            } => {
                let result = self.function_header(&signature, item.origin.span, NameSite::Item);
                self.exported = false;
                self.return_type = result.clone();
                // CT-5: a generated body binds the declared parameters by its
                // literal's names; the header and its contracts keep the declared ones.
                if let Some(Generated::Body(names)) = &item.generated {
                    let scope = self.scopes.last_mut().unwrap();
                    let bindings: Vec<_> = signature
                        .parameters
                        .iter()
                        .map(|p| p.name.and_then(|name| scope.values.remove(&name)))
                        .collect();
                    for (&(name, span), binding) in names.iter().zip(bindings) {
                        if let (Some(name), Some(binding)) = (name, binding) {
                            self.bind(name, Binding { span, ..binding });
                        }
                    }
                }
                if let Some(body) = body
                    && !matches!(self.view, ResolveView::Header)
                {
                    self.expr(body, result);
                }
            }
            ItemKind::Const { ty, value } => {
                if let Some(Generated::Generator(names)) = &item.generated {
                    self.generator_parameters = names.clone();
                }
                let expected = self.type_expr(ty);
                self.exported = false;
                if !matches!(self.view, ResolveView::Header) {
                    self.expr(value, expected);
                } else if let ExprKind::Path(path) =
                    &self.syntax.tree.expressions[value.0 as usize].kind
                {
                    // A constant alias target is declaration metadata. Resolve
                    // its manifest identity without reading the target's
                    // signature: alias cycles are checked by GEN-6 later.
                    let path = path.clone();
                    let span = self.syntax.tree.expressions[value.0 as usize].origin.span;
                    match self.lookup(&path, span) {
                        Ok(definition) => self
                            .output
                            .expressions
                            .push((value, ResolvedName::Definition(definition))),
                        Err(error) => self.lookup_diagnostic(&error, &path, span),
                    }
                }
            }
            ItemKind::Type {
                generics,
                authority,
                body,
                drop,
                ..
            } => {
                // Core library type names are defaults a user type shadows
                // (LANG-4). Primitives and the constructors with no core@1
                // declaration have no full path left to reach them by.
                let spelling = self.spelling(item.name);
                if !item.definition.package_is("core", "1")
                    && (matches!(spelling.as_str(), "FnPtr" | "TypeShape")
                        || primitive(&spelling).is_some())
                {
                    self.diagnostic(const { RuleId::of("LANG-7") }, item.name_span,
                        &format!("type `{spelling}` conflicts with a builtin type name; choose a domain name for the type"));
                }
                self.generics(&generics, item.origin.span);
                for argument in item.attributes.iter().flat_map(|a| &a.arguments) {
                    // Stage one records operation identities only. Reading their
                    // signatures here cycles through the receiver's header; stage
                    // two validates them after publication.
                    let (paths, span) = match argument {
                        AttributeArgument::LoopSource(s) => (vec![&s.open, &s.next], s.origin.span),
                        AttributeArgument::Format(s) => (vec![&s.function], s.origin.span),
                        AttributeArgument::SerializableCheck(s) => {
                            (vec![&s.function], s.origin.span)
                        }
                        _ => continue,
                    };
                    for path in paths {
                        match self.lookup(path, span) {
                            Ok(definition) => self
                                .output
                                .paths
                                .push((NameSite::Item, ResolvedName::Definition(definition))),
                            Err(e) => self.lookup_diagnostic(&e, path, span),
                        }
                    }
                }
                if let Some(paths) = authority {
                    for path in paths {
                        if path.package.is_none() && path.segments.len() == 1 {
                            if let Some(GenericName::Type(parameter)) =
                                self.generic(path.segments[0])
                            {
                                self.output
                                    .paths
                                    .push((NameSite::Item, ResolvedName::TypeParameter(parameter)));
                                continue;
                            }
                        }
                        self.extra_path(&path, item.origin.span, NameSite::Item);
                    }
                }
                // Opaque representation is not part of an external signature.
                self.exported = item.visibility == Visibility::Public;
                match body {
                    TypeBody::Record(fields) => {
                        let mut names = BTreeSet::new();
                        for field in &fields {
                            if !names.insert(field.name) {
                                self.diagnostic(
                                    const { RuleId::of("LANG-12") },
                                    field.origin.span,
                                    "duplicate field name",
                                );
                            }
                        }
                        // Leading-dot sibling paths have their own field namespace.
                        for (i, field) in fields.iter().enumerate() {
                            self.sibling_fields.entry(field.name).or_insert((
                                item.definition.clone(),
                                ty::FieldId(i as u32),
                                field.ty,
                            ));
                        }
                        for field in fields {
                            self.type_expr(field.ty);
                        }
                        self.sibling_fields.clear();
                    }
                    TypeBody::Variants(variants) => {
                        let mut names = BTreeSet::new();
                        let name_spans = self.syntax.tree.variant_name_spans(item.origin);
                        for (ordinal, variant) in variants.into_iter().enumerate() {
                            if !names.insert(variant.name) {
                                self.diagnostic(
                                    const { RuleId::of("LANG-7") },
                                    name_spans.get(ordinal).copied().unwrap_or(item.origin.span),
                                    "duplicate variant name",
                                );
                            }
                            let mut labels = BTreeSet::new();
                            for (label, ty) in variant.fields {
                                if let Some(label) = label
                                    && !labels.insert(label)
                                {
                                    self.diagnostic(
                                        const { RuleId::of("LANG-12") },
                                        item.origin.span,
                                        "duplicate payload field name",
                                    );
                                }
                                self.type_expr(ty);
                            }
                        }
                    }
                    TypeBody::Alias(ty) => {
                        self.type_expr(ty);
                    }
                    TypeBody::Distinct { field, fact } => {
                        let expected = self.type_expr(field.ty);
                        self.exported = false;
                        self.local(field.name, field.origin.span, expected);
                        if let Some(fact) = fact {
                            self.expr(fact, ExpectedType::Unknown);
                        }
                    }
                    TypeBody::Opaque => {}
                }
                self.exported = false;
                if let Some(drop) = drop {
                    self.scopes.push(Scope::default());
                    let binding = self.local(
                        drop.binding,
                        item.origin.span,
                        ExpectedType::nominal(item.definition),
                    );
                    self.output.drop_binding = Some(binding);
                    if !matches!(self.view, ResolveView::Header) {
                        self.expr(drop.body, ExpectedType::Unknown);
                    }
                    self.scopes.pop();
                }
            }
        }
    }
    fn function_header(
        &mut self,
        signature: &FunctionSyntax,
        span: Span,
        site: NameSite,
    ) -> ExpectedType {
        let enclosing_contract = self.contract_kind.take();
        let origin = self.origin(site);
        let diagnostics_before = self.output.query.diagnostics.len();
        self.scopes.last_mut().unwrap().owner = Some(site.clone());
        self.generics(&signature.generics, span);
        self.scopes
            .last_mut()
            .unwrap()
            .future
            .extend(signature.parameters.iter().filter_map(|p| p.name));
        for (i, parameter) in signature.parameters.iter().enumerate() {
            let declaring =
                std::mem::replace(&mut self.declaring, parameter.name.into_iter().collect());
            let expected = self.type_expr(parameter.ty);
            self.declaring = declaring;
            let own = std::mem::take(&mut self.deferred);
            if let Some(name) = parameter.name {
                self.scopes.last_mut().unwrap().future.remove(&name);
                self.bind(
                    name,
                    Binding {
                        name: ResolvedName::Parameter(ParamId(i as u32)),
                        expected,
                        span: self
                            .syntax
                            .tree
                            .parameter_name_span(origin, parameter)
                            .unwrap_or(span),
                        owner: Some(site.clone()),
                    },
                );
            }
            for (path, span, site) in own {
                self.extra_path(&path, span, site);
            }
        }
        let result = signature
            .result
            .map(|id| self.type_expr(id))
            .unwrap_or_default();
        for attribute in &signature.attributes {
            for argument in &attribute.arguments {
                if let AttributeArgument::StatePromise(s) = argument {
                    for path in &s.selectors {
                        self.extra_path(path, s.origin.span, site);
                    }
                }
            }
        }
        if let Some(paths) = &signature.uses {
            let path_spans = self.syntax.tree.uses_path_spans(origin);
            for (ordinal, path) in paths.iter().enumerate() {
                self.extra_path(
                    path,
                    path_spans.get(ordinal).copied().unwrap_or(span),
                    site.clone(),
                );
            }
        }
        let exported = self.exported;
        self.exported = false;
        for contract in &signature.contracts {
            self.contract_kind = Some(contract.kind);
            self.expr(contract.expression, ExpectedType::Unknown);
        }
        self.contract_kind = enclosing_contract;
        self.exported = exported;
        // Signature normalization runs only after a valid resolved header. Keep
        // that suppression policy when reporting its label obligation here.
        if self.output.query.diagnostics.len() == diagnostics_before {
            let mut labels = BTreeSet::new();
            for parameter in &signature.parameters {
                if let Some(label) = parameter.label
                    && !labels.insert(label)
                {
                    self.diagnostic(
                        const { RuleId::of("LANG-12") },
                        self.syntax
                            .tree
                            .parameter_label_span(origin, parameter)
                            .unwrap_or(span),
                        "duplicate parameter label",
                    );
                    break;
                }
            }
        }
        result
    }
    fn type_expr(&mut self, id: TypeExprId) -> ExpectedType {
        let node = self.syntax.tree.types[id.0 as usize].clone();
        match node.kind {
            TypeExprKind::Named { path, .. }
                if self.binder_member(&path, node.origin.span).is_some() =>
            {
                // Resolved again to read it; the first call reported a missing member.
                let member = self.binder_member_quiet(&path);
                let name = match member {
                    Some(BoundMember::Type(name) | BoundMember::TypeActual(name, _)) => name,
                    Some(BoundMember::SizeParameter(parameter)) => ResolvedName::Size(parameter),
                    Some(BoundMember::SizeConstant(_) | BoundMember::Operation(_)) => {
                        self.diagnostic(
                            const { RuleId::of("GEN-6") },
                            node.origin.span,
                            "an operation member is not a type",
                        );
                        return ExpectedType::Unknown;
                    }
                    None => return ExpectedType::Unknown,
                };
                self.output.query.value.types.push((id, name));
                ExpectedType::Unknown
            }
            TypeExprKind::Named { path, arguments } => {
                let expected = if path.package.is_none() && path.segments.len() == 1 {
                    let name = path.segments[0];
                    match self.generic(name) {
                        Some(GenericName::Type(parameter)) => {
                            self.output
                                .query
                                .value
                                .types
                                .push((id, ResolvedName::TypeParameter(parameter)));
                            ExpectedType::Unknown
                        }
                        Some(GenericName::Size(parameter)) => {
                            self.output
                                .query
                                .value
                                .types
                                .push((id, ResolvedName::Size(parameter)));
                            ExpectedType::Unknown
                        }
                        None => {
                            if let Some(primitive) = primitive(&self.spelling(name)) {
                                self.output
                                    .query
                                    .value
                                    .types
                                    .push((id, ResolvedName::Primitive(primitive)));
                                ExpectedType::Unknown
                            } else if self.spelling(name) == "any" {
                                self.diagnostic(
                                    const { RuleId::of("LANG-7") },
                                    node.origin.span,
                                    "any is not a type",
                                );
                                ExpectedType::Unknown
                            } else {
                                self.named_type(id, &path, node.origin.span)
                            }
                        }
                    }
                } else {
                    self.named_type(id, &path, node.origin.span)
                };
                let arguments = arguments
                    .into_iter()
                    .map(|argument| match argument {
                        TypeArgument::Type(ty) => self.type_expr(ty),
                        TypeArgument::SizeLiteral(_) => ExpectedType::Unknown,
                    })
                    .collect();
                match expected {
                    ExpectedType::Named { definition, .. } => ExpectedType::Named {
                        definition,
                        arguments,
                    },
                    expected => expected,
                }
            }
            TypeExprKind::Tuple(elements) => {
                ExpectedType::Tuple(elements.into_iter().map(|id| self.type_expr(id)).collect())
            }
            TypeExprKind::Box(ty) => self.type_expr(ty),
            TypeExprKind::Qualified {
                base,
                brand,
                relation,
                ..
            } => {
                let expected = self.type_expr(base);
                // Brand first, then relation places in written order: readers
                // match resolved paths to this order. `heap` and the
                // contextual `env` name no binding (MODE-1, PROV-4).
                match brand {
                    Some(BrandSyntax {
                        sibling: true,
                        path,
                    }) => self.sibling_path(&path, node.origin.span, NameSite::Type(id)),
                    Some(BrandSyntax {
                        sibling: false,
                        path,
                    }) => self.extra_path(&path, node.origin.span, NameSite::Type(id)),
                    None => {}
                }
                // A relation naming the binding being declared resolves after
                // it is bound; its other places wait with it to keep order.
                let paths: Vec<Path> = relation
                    .iter()
                    .flat_map(|r| &r.places)
                    .filter_map(|place| match place {
                        RelationPlace::Path(path) => Some(path.clone()),
                        _ => None,
                    })
                    .collect();
                let own = paths.iter().any(|p| {
                    p.package.is_none()
                        && p.segments
                            .first()
                            .is_some_and(|s| self.declaring.contains(s))
                });
                for path in paths {
                    if own {
                        self.deferred
                            .push((path, node.origin.span, NameSite::Type(id)));
                    } else {
                        self.extra_path(&path, node.origin.span, NameSite::Type(id));
                    }
                }
                expected
            }
            TypeExprKind::Function(signature) => {
                self.scopes.push(Scope::default());
                let result = self.function_header(&signature, node.origin.span, NameSite::Type(id));
                let parameters = signature
                    .parameters
                    .iter()
                    .map(|p| self.hint(p.ty))
                    .collect();
                self.scopes.pop();
                ExpectedType::Function {
                    parameters,
                    result: Box::new(result),
                }
            }
            TypeExprKind::Error => ExpectedType::Unknown,
        }
    }
    fn named_type(&mut self, id: TypeExprId, path: &Path, span: Span) -> ExpectedType {
        match self.lookup(path, span) {
            Ok(definition) => {
                if self.namespace(&definition) == Some(Namespace::Value) {
                    self.diagnostic(
                        const { RuleId::of("LANG-4") },
                        span,
                        "value name used in type context",
                    );
                    return ExpectedType::Unknown;
                }
                if self.exported
                    && self
                        .index
                        .entry(&definition)
                        .is_some_and(|e| e.visibility == Visibility::Private)
                {
                    self.diagnostic(
                        const { RuleId::of("PROG-2") },
                        span,
                        "public signature mentions a private type",
                    );
                }
                let native = self.signature(&definition).is_some_and(|signature| {
                    matches!(&signature.kind, ty::SignatureKind::Type(declaration)
                        if declaration.representation.is_some_and(|root|
                            matches!(self.types.get(root).map(|row| &row.kind),
                                Some(ty::TypeKind::Builtin { kind: ty::BuiltinType::Packed, .. }))))
                });
                self.output
                    .query
                    .value
                    .types
                    .push((id, ResolvedName::Definition(definition.clone())));
                // Packed storage has no nominal field shape. Its projections
                // depend on actual type arguments and are elaborated by H.
                if native {
                    ExpectedType::Unknown
                } else {
                    ExpectedType::nominal(definition)
                }
            }
            Err(LookupError::Missing) if native_type(path) => {
                // These constructors already have TypeKind::Builtin semantics.
                // No shared ResolvedName variant denotes them; keep their failed
                // reads, without inventing a nominal library identity. An indexed
                // declaration or provider result always takes precedence here.
                ExpectedType::Unknown
            }
            Err(e) => {
                self.lookup_diagnostic(&e, path, span);
                ExpectedType::Unknown
            }
        }
    }
    fn sibling_path(&mut self, path: &Path, span: Span, site: NameSite) {
        let Some((owner, field, written_type)) = path
            .segments
            .first()
            .and_then(|s| self.sibling_fields.get(s))
            .cloned()
            .filter(|_| path.package.is_none())
        else {
            self.diagnostic(
                const { RuleId::of("LANG-4") },
                span,
                "unresolved sibling field name",
            );
            if let Some(name) = path.segments.first() {
                let names = self
                    .sibling_fields
                    .keys()
                    .map(|s| self.spelling(*s))
                    .collect();
                self.suggest_field(span, *name, names, "");
            }
            return;
        };
        self.output.contextual.push((
            site.clone(),
            ContextualName::Field {
                owner,
                variant: None,
                field,
            },
        ));
        let mut expected = self.syntax_expected(written_type);
        for (segment, name) in path.segments.iter().enumerate().skip(1) {
            let ExpectedType::Named {
                definition: owner,
                arguments,
            } = expected
            else {
                self.defer(site.clone(), *name, span);
                return;
            };
            let shape = self.shape(&owner, &arguments);
            let field_names: Vec<_> = shape.fields.iter().map(|(name, _)| name.clone()).collect();
            let Some((i, (_, field_type))) = shape
                .fields
                .into_iter()
                .enumerate()
                .find(|(_, (n, _))| n == &self.spelling(*name))
            else {
                self.diagnostic(
                    const { RuleId::of("LANG-4") },
                    span,
                    "unresolved sibling projection field",
                );
                let name_span = self.path_name_span(path, span, segment);
                let spelling = self.spelling(*name);
                hirsel_diag::suggest_names(
                    self.output.query.diagnostics.last_mut().unwrap(),
                    &spelling,
                    field_names,
                    "",
                    name_span,
                );
                return;
            };
            if !self.representation_visible(&owner, span) {
                return;
            }
            self.output.contextual.push((
                site.clone(),
                ContextualName::Field {
                    owner,
                    variant: None,
                    field: ty::FieldId(i as u32),
                },
            ));
            expected = field_type;
        }
    }
    fn error_mapper(&mut self, id: ExprId, path: &Path, span: Span) {
        let site = NameSite::Expression(id);
        let expected = self
            .expected
            .error_mappers
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if path.package.is_none() && path.segments.len() == 1 {
            if let Some(binding) = self.value(path.segments[0]) {
                if let Some(owner) = binding.owner {
                    self.output.reference_scopes.push((site.clone(), owner));
                }
                self.output.paths.push((site, binding.name));
                return;
            }
            if let Some((variant, ..)) =
                self.variant(path.segments[0], &expected, site.clone(), span)
            {
                self.output
                    .paths
                    .push((site, ResolvedName::Variant(variant)));
                return;
            }
        }
        if path.package.is_none()
            && path.segments.len() == 1
            && self
                .scopes
                .iter()
                .any(|scope| scope.future.contains(&path.segments[0]))
        {
            let _ = self.lookup(path, span);
            self.diagnostic(
                const { RuleId::of("LANG-2") },
                span,
                "local and parameter definitions must precede their use",
            );
            return;
        }
        match self.lookup(path, span) {
            Ok(definition) => {
                self.signature(&definition);
                self.output
                    .paths
                    .push((site, ResolvedName::Definition(definition)));
            }
            Err(e) => {
                // The mapper has an expected error type supplied by checking,
                // rather than the success/result type of this Try expression.
                if path.package.is_none()
                    && path.segments.len() == 1
                    && expected == ExpectedType::Unknown
                    && matches!(e, LookupError::Missing | LookupError::Provider(_))
                {
                    self.defer(site, path.segments[0], span);
                } else {
                    self.lookup_diagnostic(&e, path, span);
                }
            }
        }
    }
    fn extra_path(&mut self, path: &Path, span: Span, site: NameSite) {
        if let Some((name, _)) = self.local_projection(path, site.clone(), span) {
            self.output.paths.push((site, name));
            return;
        }
        if path.package.is_none() && path.segments.len() == 1 {
            if let Some(binding) = self.value(path.segments[0]) {
                if let Some(owner) = binding.owner {
                    self.output.reference_scopes.push((site.clone(), owner));
                }
                self.output.paths.push((site, binding.name));
                return;
            }
            if let Some(GenericName::Size(id)) = self.generic(path.segments[0]) {
                self.output.paths.push((site, ResolvedName::Size(id)));
                return;
            }
        }
        if path.package.is_none()
            && path.segments.len() == 1
            && self
                .scopes
                .iter()
                .any(|scope| scope.future.contains(&path.segments[0]))
        {
            let _ = self.lookup(path, span);
            self.diagnostic(
                const { RuleId::of("LANG-2") },
                span,
                "local and parameter definitions must precede their use",
            );
            return;
        }
        match self.lookup(path, span) {
            Ok(definition) => {
                self.signature(&definition);
                self.output
                    .paths
                    .push((site, ResolvedName::Definition(definition)));
            }
            Err(e) => self.lookup_diagnostic(&e, path, span),
        }
    }
    /// Reads only a written type annotation, never an initializer/body.
    fn hint(&self, id: TypeExprId) -> ExpectedType {
        match &self.syntax.tree.types[id.0 as usize].kind {
            TypeExprKind::Named { path, arguments } => {
                if path.package.is_none()
                    && let [name] = path.segments[..]
                    && let Some((_, argument)) =
                        self.type_arguments.iter().find(|(n, _)| *n == name)
                {
                    return argument.clone();
                }
                self.output
                    .query
                    .value
                    .types
                    .iter()
                    .rev()
                    .find_map(|(ty, name)| {
                        if *ty == id
                            && let ResolvedName::Definition(d) = name
                        {
                            Some(ExpectedType::Named {
                                definition: d.clone(),
                                arguments: self.argument_hints(arguments),
                            })
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default()
            }
            TypeExprKind::Tuple(types) => {
                ExpectedType::Tuple(types.iter().map(|id| self.hint(*id)).collect())
            }
            // MEM-7 / #98: contextual field/pattern lookup sees box payloads.
            // This hint carries names only; Source records the physical projection.
            TypeExprKind::Box(id) | TypeExprKind::Qualified { base: id, .. } => self.hint(*id),
            TypeExprKind::Function(f) => ExpectedType::Function {
                parameters: f.parameters.iter().map(|p| self.hint(p.ty)).collect(),
                result: Box::new(f.result.map(|id| self.hint(id)).unwrap_or_default()),
            },
            _ => ExpectedType::Unknown,
        }
    }
    fn argument_hints(&self, arguments: &[TypeArgument]) -> Vec<ExpectedType> {
        arguments
            .iter()
            .map(|argument| match argument {
                TypeArgument::Type(ty) => self.hint(*ty),
                TypeArgument::SizeLiteral(_) => ExpectedType::Unknown,
            })
            .collect()
    }
    /// The owner's fields and variants with `arguments` substituted for its
    /// generic parameters, so a nested payload keeps its nominal context.
    fn shape(&mut self, owner: &DefPath, arguments: &[ExpectedType]) -> Shape {
        if !self.shaping.insert(owner.clone()) {
            return Shape::default();
        }
        let result = self.shape_inner(owner, arguments);
        self.shaping.remove(owner);
        result
    }
    fn shape_inner(&mut self, owner: &DefPath, arguments: &[ExpectedType]) -> Shape {
        if let Some(signature) = self.signature(owner)
            && let ty::SignatureKind::Type(definition) = &signature.kind
            && let Some(representation) = definition.representation
        {
            if let Some(ty::TypeKind::Named { .. }) =
                self.types.get(representation).map(|t| &t.kind)
                && let ExpectedType::Named {
                    definition,
                    arguments,
                } = semantic_expected(self.types, representation, arguments, 0, 0)
            {
                return self.shape(&definition, &arguments);
            }
            return shape_from_type(self.types, representation, arguments);
        }
        self.type_header(owner, arguments, |this, body| match body {
            TypeBody::Record(fields) => Shape {
                fields: fields
                    .into_iter()
                    .map(|f| (this.spelling(f.name), this.syntax_expected(f.ty)))
                    .collect(),
                variants: vec![],
            },
            TypeBody::Variants(variants) => Shape {
                fields: vec![],
                variants: variants
                    .into_iter()
                    .map(|v| {
                        (
                            this.spelling(v.name),
                            v.fields
                                .into_iter()
                                .map(|(n, t)| {
                                    (n.map(|n| this.spelling(n)), this.syntax_expected(t))
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            },
            TypeBody::Distinct { field, .. } => Shape {
                fields: vec![(this.spelling(field.name), this.syntax_expected(field.ty))],
                variants: vec![],
            },
            TypeBody::Alias(id) => {
                if let ExpectedType::Named {
                    definition,
                    arguments,
                } = this.syntax_expected(id)
                {
                    this.shape(&definition, &arguments)
                } else {
                    Shape::default()
                }
            }
            TypeBody::Opaque => Shape::default(),
        })
        .unwrap_or_default()
    }
    /// Header-only adapter for written same-file type signatures: runs `f` on
    /// the body of `owner`'s declaration with its generics bound to
    /// `arguments`. Type IDs here belong to the declaring item's arenas, not
    /// to a foreign store.
    fn type_header<R>(
        &mut self,
        owner: &DefPath,
        arguments: &[ExpectedType],
        f: impl FnOnce(&mut Self, TypeBody) -> R,
    ) -> Option<R> {
        let fixture = match self.view {
            ResolveView::File => self
                .siblings
                .iter()
                .map(|item| &**item)
                .find(|item| &item.item.definition == owner),
            _ => Some(self.syntax).filter(|item| &item.item.definition == owner),
        }?;
        let ItemKind::Type { generics, body, .. } = fixture.item.kind.clone() else {
            return None;
        };
        // Read from the declaring item's own arenas.
        let old_syntax = std::mem::replace(&mut self.syntax, fixture);
        let old_context = std::mem::replace(
            &mut self.context,
            FileContext {
                package: owner.0.package.clone(),
                modules: owner.0.modules.clone(),
            },
        );
        let old_arguments = std::mem::replace(
            &mut self.type_arguments,
            generics
                .iter()
                .enumerate()
                .map(|(i, g)| (g.name, arguments.get(i).cloned().unwrap_or_default()))
                .collect(),
        );
        let result = f(self, body);
        self.syntax = old_syntax;
        self.context = old_context;
        self.type_arguments = old_arguments;
        Some(result)
    }
    /// The nominal type a transparent alias chain represents, with its
    /// arguments. Constructors and variants belong to that nominal, never to
    /// an alias of it (LANG-7: aliases follow their representation).
    fn represented(
        &mut self,
        owner: &DefPath,
        arguments: &[ExpectedType],
    ) -> (DefPath, Vec<ExpectedType>) {
        let mut current = (owner.clone(), arguments.to_vec());
        let mut seen = BTreeSet::new();
        while seen.insert(current.0.clone()) {
            let next = match self.signature(&current.0) {
                Some(signature) => match &signature.kind {
                    ty::SignatureKind::Type(declaration) if signature.metadata.alias => declaration
                        .representation
                        .map(|r| semantic_expected(self.types, r, &current.1, 0, 0)),
                    _ => None,
                },
                None => self
                    .type_header(&current.0, &current.1, |this, body| match body {
                        TypeBody::Alias(id) => Some(this.syntax_expected(id)),
                        _ => None,
                    })
                    .flatten(),
            };
            match next {
                Some(ExpectedType::Named {
                    definition,
                    arguments,
                }) => current = (definition, arguments),
                _ => break,
            }
        }
        current
    }
    fn syntax_expected(&mut self, id: TypeExprId) -> ExpectedType {
        let hint = self.hint(id);
        match self.syntax.tree.types[id.0 as usize].kind.clone() {
            TypeExprKind::Named { path, arguments } => {
                let definition = match hint {
                    ExpectedType::Named { definition, .. } => definition,
                    // A referenced header need not itself have been resolved yet.
                    ExpectedType::Unknown => {
                        let span = self.syntax.tree.types[id.0 as usize].origin.span;
                        let Ok(definition) = self.lookup(&path, span) else {
                            return ExpectedType::Unknown;
                        };
                        definition
                    }
                    // A generic parameter of the type being shaped.
                    hint => return hint,
                };
                let arguments = arguments
                    .into_iter()
                    .map(|argument| match argument {
                        TypeArgument::Type(ty) => self.syntax_expected(ty),
                        TypeArgument::SizeLiteral(_) => ExpectedType::Unknown,
                    })
                    .collect();
                ExpectedType::Named {
                    definition,
                    arguments,
                }
            }
            _ if hint != ExpectedType::Unknown => hint,
            TypeExprKind::Tuple(types) => {
                ExpectedType::Tuple(types.into_iter().map(|t| self.syntax_expected(t)).collect())
            }
            TypeExprKind::Qualified { base, .. } | TypeExprKind::Box(base) => {
                self.syntax_expected(base)
            }
            TypeExprKind::Function(f) => ExpectedType::Function {
                parameters: f
                    .parameters
                    .iter()
                    .map(|p| self.syntax_expected(p.ty))
                    .collect(),
                result: Box::new(
                    f.result
                        .map(|id| self.syntax_expected(id))
                        .unwrap_or_default(),
                ),
            },
            _ => ExpectedType::Unknown,
        }
    }
    fn representation_visible(&mut self, owner: &DefPath, span: Span) -> bool {
        // PROG-2 applies to the represented nominal, including behind a local
        // transparent alias. Check privacy before asking shape/field lookup to
        // interpret an abstract signature as an empty representation.
        let mut owner = owner.clone();
        let mut seen = BTreeSet::new();
        while seen.insert(owner.clone()) {
            let external = owner.0.package != self.context.package;
            if self.index.access(&self.context, &owner).is_err()
                || (external
                    && self
                        .index
                        .entry(&owner)
                        .is_some_and(|e| e.visibility == Visibility::PublicOpaque))
            {
                self.diagnostic(
                    const { RuleId::of("PROG-2") },
                    span,
                    "opaque/private representation is hidden",
                );
                return false;
            }
            let signature = self.signature(&owner);
            if external
                && signature.as_ref().is_some_and(|s| {
                    matches!(
                        &s.kind,
                        ty::SignatureKind::Type(definition)
                            if definition.loop_source.is_none()
                                && definition.format.is_none()
                                && definition.representation.is_none()
                    )
                })
            {
                self.diagnostic(
                    const { RuleId::of("PROG-2") },
                    span,
                    "opaque/private representation is hidden",
                );
                return false;
            }
            if let Some(signature) = signature
                && signature.metadata.alias
                && let ty::SignatureKind::Type(declaration) = &signature.kind
                && let Some(root) = declaration.representation
                && let Some(ty::Type {
                    kind: ty::TypeKind::Named { definition, .. },
                    ..
                }) = self.types.get(root)
            {
                owner = definition.path().clone();
            } else {
                return true;
            }
        }
        // Cyclic aliases are a signature error; this visibility walk cannot
        // manufacture a shape or replace the diagnostic for that prerequisite.
        true
    }
    fn known_variant(&mut self, name: Symbol) -> bool {
        self.declared_variant(name, true)
    }
    /// A bare name that a non-core variant spells shadows a core type of that
    /// name: it resolves through the expected type, as an unresolved name does,
    /// and the core type stays reachable by its full path (LANG-4).
    fn shadows_core_type(&mut self, path: &Path, definition: &DefPath) -> bool {
        path.package.is_none()
            && path.segments.len() == 1
            && definition.package_is("core", "1")
            && self.namespace(definition) == Some(Namespace::Type)
            && self.declared_variant(path.segments[0], false)
    }
    fn declared_variant(&mut self, name: Symbol, core: bool) -> bool {
        let headers = self
            .siblings
            .iter()
            .map(|syntax| &syntax.item)
            .filter_map(|item| {
                if let ItemKind::Type {
                    body: TypeBody::Variants(variants),
                    ..
                } = &item.kind
                {
                    Some((
                        item.definition.clone(),
                        variants.iter().any(|v| v.name == name),
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let mut found = core
            && self
                .expected
                .available_variants
                .contains(&self.spelling(name));
        for (owner, contains) in headers {
            self.signature(&owner);
            found |= contains;
        }
        // The driver isolates sibling syntax. Follow only declared type edges:
        // a function result can introduce Option without a written annotation.
        let mut visited = BTreeSet::new();
        loop {
            let pending: BTreeSet<_> = self
                .signatures
                .values()
                .flatten()
                .flat_map(|s| {
                    ty::closure(self.types, signature_roots(s))
                        .iter()
                        .filter_map(|t| match &self.types.row(*t).kind {
                            ty::TypeKind::Named { definition, .. } => {
                                Some(definition.path().clone())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|d| !visited.contains(d))
                .collect();
            if pending.is_empty() {
                break;
            }
            for definition in pending {
                visited.insert(definition.clone());
                self.signature(&definition);
            }
        }
        found |= self.signatures.values().flatten().any(|signature| {
            if !core && signature.definition.path().package_is("core", "1") {
                false
            } else if let ty::SignatureKind::Type(definition) = &signature.kind {
                definition.representation.is_some_and(|id| {
                    shape_from_type(self.types, id, &[])
                        .variants
                        .iter()
                        .any(|(variant, _)| variant == &self.spelling(name))
                })
            } else {
                false
            }
        });
        found
    }
    /// Choose a declaration, never a type argument. H infers arguments from
    /// the call's payloads in source order, using ordinary constructor inference.
    fn inferred_variant_owner(&mut self, name: Symbol) -> Option<DefPath> {
        let mut declarations: BTreeSet<_> = self
            .index
            .unqualified_types(&self.context)
            .into_iter()
            .collect();
        // Types reached through declared signatures are also in view, as for
        // contextual variant deferral. No implementation body is consulted.
        declarations.extend(self.signatures.values().flatten().filter_map(|s| {
            matches!(s.kind, ty::SignatureKind::Type(_)).then(|| s.definition.path().clone())
        }));
        let spelling = self.spelling(name);
        let mut candidates = BTreeSet::new();
        for owner in declarations {
            let Some(signature) = self.signature(&owner) else {
                continue;
            };
            if signature.metadata.alias
                || self.index.access(&self.context, &owner).is_err()
                || (signature.metadata.visibility == Visibility::PublicOpaque
                    && owner.0.package != self.context.package)
            {
                continue;
            }
            if self
                .shape(&owner, &[])
                .variants
                .iter()
                .any(|(n, _)| n == &spelling)
            {
                candidates.insert(owner);
            }
        }
        // A user declaration shadows a core default; two user owners remain
        // ambiguous. Payload types never select between overloaded declarations.
        if candidates.iter().any(|p| !p.package_is("core", "1")) {
            candidates.retain(|p| !p.package_is("core", "1"));
        }
        (candidates.len() == 1).then(|| candidates.into_iter().next().unwrap())
    }
    fn variant(
        &mut self,
        name: Symbol,
        expected: &ExpectedType,
        site: NameSite,
        span: Span,
    ) -> Option<(ty::VariantId, Vec<(Option<String>, ExpectedType)>, DefPath)> {
        let ExpectedType::Named {
            definition: owner,
            arguments,
        } = expected
        else {
            return None;
        };
        let shape = self.shape(owner, arguments);
        let (i, (_, fields)) = shape
            .variants
            .into_iter()
            .enumerate()
            .find(|(_, (n, _))| n == &self.spelling(name))?;
        if !self.representation_visible(owner, span) {
            return None;
        }
        let variant = ty::VariantId(i as u32);
        let (owner, _) = self.represented(owner, arguments);
        self.output.contextual.push((
            site,
            ContextualName::Variant {
                owner: owner.clone(),
                variant,
            },
        ));
        Some((variant, fields, owner))
    }
    fn defer(&mut self, site: NameSite, name: Symbol, span: Span) {
        self.output.deferred.push(DeferredName {
            site,
            span,
            name: name.as_str().to_owned(),
        });
    }
    fn expr(&mut self, id: ExprId, expected: ExpectedType) -> ExpectedType {
        let expected = self
            .expected
            .expressions
            .get(&id)
            .cloned()
            .unwrap_or(expected);
        let node = self.syntax.tree.expressions[id.0 as usize].clone();
        let span = node.origin.span;
        match node.kind {
            ExprKind::Path(path) => self.expression_path(id, &path, expected, span),
            ExprKind::Literal(_)
            | ExprKind::Heap
            | ExprKind::Todo(_)
            | ExprKind::Error
            | ExprKind::Break { value: None, .. }
            | ExprKind::Continue(_) => expected,
            ExprKind::Break {
                label,
                value: Some(value),
            } => {
                let target = self
                    .block_labels
                    .iter()
                    .rev()
                    .find(|(name, _)| Some(*name) == label)
                    .map_or(ExpectedType::Unknown, |(_, ty)| ty.clone());
                self.expr(value, target);
                expected
            }
            // The body checker selects a formatter's operation (SEM-3).
            ExprKind::Formatter(_) => ExpectedType::Unknown,
            ExprKind::Result => {
                if self.contract_kind != Some(ContractKind::Ensures) {
                    self.diagnostic(
                        const { RuleId::of("FACT-1") },
                        span,
                        "result is available only inside ensures",
                    );
                }
                expected
            }
            ExprKind::Before(value) => {
                if self.contract_kind != Some(ContractKind::Ensures) {
                    self.diagnostic(
                        const { RuleId::of("FACT-1") },
                        span,
                        "before is available only inside ensures",
                    );
                    expected
                } else {
                    self.expr(value, expected)
                }
            }
            ExprKind::Tuple(elements) => {
                let types = if let ExpectedType::Tuple(types) = expected {
                    types
                } else {
                    vec![]
                };
                ExpectedType::Tuple(
                    elements
                        .into_iter()
                        .enumerate()
                        .map(|(i, e)| self.expr(e, types.get(i).cloned().unwrap_or_default()))
                        .collect(),
                )
            }
            ExprKind::Sequence(elements) => {
                for e in elements {
                    self.expr(e, ExpectedType::Unknown);
                }
                expected
            }
            ExprKind::Call { callee, arguments } => self.call(callee, arguments, expected),
            ExprKind::With { callee, bindings } => {
                self.callees.insert(callee);
                let hint = self.expr(callee, ExpectedType::Unknown);
                self.with_clause(id, callee, &bindings, span);
                hint
            }
            ExprKind::Field { base, field } => {
                if let Some(path) = self.dot_path(id)
                    && self.module_path(&path)
                {
                    return self.expression_path(id, &path, expected, span);
                }
                let base_type = self.expr(base, ExpectedType::Unknown);
                if let ExpectedType::Named {
                    definition: owner,
                    arguments,
                } = base_type
                {
                    if !self.representation_visible(&owner, span) {
                        return ExpectedType::Unknown;
                    }
                    let shape = self.shape(&owner, &arguments);
                    let field_names: Vec<_> =
                        shape.fields.iter().map(|(name, _)| name.clone()).collect();
                    if let Some((i, (_, field_type))) = shape
                        .fields
                        .into_iter()
                        .enumerate()
                        .find(|(_, (n, _))| n == &self.spelling(field))
                    {
                        if self.representation_visible(&owner, span) {
                            self.output.contextual.push((
                                NameSite::Expression(id),
                                ContextualName::Field {
                                    owner,
                                    variant: None,
                                    field: ty::FieldId(i as u32),
                                },
                            ));
                        }
                        return field_type;
                    }
                    if shape
                        .variants
                        .iter()
                        .any(|(n, _)| n == &self.spelling(field))
                    {
                        self.diagnostic(
                            const { RuleId::of("LANG-4") },
                            span,
                            "variants must be written bare",
                        );
                    } else {
                        self.diagnostic(
                            const { RuleId::of("LANG-4") },
                            span,
                            "unresolved field name",
                        );
                        let prefix = self
                            .dot_path(id)
                            .map(|path| {
                                path.segments[..path.segments.len() - 1]
                                    .iter()
                                    .map(|s| format!("{}.", self.spelling(*s)))
                                    .collect::<String>()
                            })
                            .unwrap_or_default();
                        self.suggest_field(span, field, field_names, &prefix);
                    }
                } else {
                    self.defer(NameSite::Expression(id), field, span);
                }
                expected
            }
            ExprKind::Index { base, index } => {
                self.expr(base, ExpectedType::Unknown);
                self.expr(index, ExpectedType::Unknown);
                expected
            }
            ExprKind::Unary { value, .. } | ExprKind::Comptime(value) => self.expr(value, expected),
            ExprKind::Binary { left, right, .. } => {
                self.expr(left, expected.clone());
                self.expr(right, expected.clone());
                expected
            }
            ExprKind::Try { value, map_error } => {
                self.expr(value, ExpectedType::Unknown);
                if let Some(path) = map_error {
                    self.error_mapper(id, &path, span);
                }
                expected
            }
            ExprKind::Block {
                label,
                statements,
                tail,
            } => {
                if let Some(label) = label {
                    self.block_labels.push((label, expected.clone()));
                }
                self.scopes.push(Scope::default());
                let future = statements
                    .iter()
                    .flat_map(|s| self.statement_names(*s))
                    .collect();
                self.scopes.last_mut().unwrap().future = future;
                for statement in statements {
                    self.statement(statement);
                }
                let result = tail
                    .map(|tail| self.expr(tail, expected))
                    .unwrap_or_default();
                self.scopes.pop();
                if label.is_some() {
                    self.block_labels.pop();
                }
                result
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(condition, ExpectedType::Unknown);
                self.expr(then_branch, expected.clone());
                if let Some(branch) = else_branch {
                    self.expr(branch, expected.clone());
                }
                expected
            }
            ExprKind::Match { value, arms } => {
                let value_type = self.expr(value, ExpectedType::Unknown);
                for arm in arms {
                    self.scopes.push(Scope::default());
                    self.pattern(arm.pattern, value_type.clone());
                    if let Some(guard) = arm.guard {
                        self.expr(guard, ExpectedType::Unknown);
                    }
                    self.expr(arm.body, expected.clone());
                    self.scopes.pop();
                }
                expected
            }
            ExprKind::Closure { signature, body } => {
                self.scopes.push(Scope::default());
                self.local_function_depth += 1;
                let result = self.function_header(&signature, span, NameSite::Expression(id));
                let result = if signature.result.is_some() {
                    result
                } else if let ExpectedType::Function { result, .. } = &expected {
                    (**result).clone()
                } else {
                    ExpectedType::Unknown
                };
                let old = std::mem::replace(&mut self.return_type, result.clone());
                let labels = std::mem::take(&mut self.block_labels);
                let enclosing_contract = self.contract_kind.take();
                self.expr(body, result);
                self.contract_kind = enclosing_contract;
                self.block_labels = labels;
                self.return_type = old;
                self.local_function_depth -= 1;
                self.scopes.pop();
                expected
            }
            ExprKind::Return(value) => {
                if let Some(value) = value {
                    self.expr(value, self.return_type.clone());
                }
                expected
            }
            ExprKind::Shape(ty) => {
                self.type_expr(ty);
                expected
            }
        }
    }
    fn expression_path(
        &mut self,
        id: ExprId,
        path: &Path,
        expected: ExpectedType,
        span: Span,
    ) -> ExpectedType {
        if let Some(member) = self.binder_member(path, span) {
            match member {
                Some((binding, BoundMember::Operation(target))) => {
                    if binding.implementation.is_none() {
                        let arguments = self.operation_arguments(&binding);
                        self.output.static_arguments.push((id, arguments));
                    }
                    self.signature(&target);
                    self.output
                        .query
                        .value
                        .expressions
                        .push((id, ResolvedName::Definition(target)));
                }
                // LANG-2: a size member used as a value is a `u64`.
                Some((_, BoundMember::SizeParameter(parameter))) => self
                    .output
                    .query
                    .value
                    .expressions
                    .push((id, ResolvedName::Size(parameter))),
                Some(_) => self.diagnostic(
                    const { RuleId::of("GEN-6") },
                    span,
                    "a type member is not a value",
                ),
                None => {}
            }
            return ExpectedType::Unknown;
        }
        if let Some((name, expected)) = self.local_projection(path, NameSite::Expression(id), span)
        {
            self.output.query.value.expressions.push((id, name));
            return expected;
        }
        if path.package.is_none() && path.segments.len() == 1 {
            let name = path.segments[0];
            if let Some(binding) = self.value(name) {
                if let Some(owner) = binding.owner {
                    self.output
                        .reference_scopes
                        .push((NameSite::Expression(id), owner));
                }
                self.output.query.value.expressions.push((id, binding.name));
                return binding.expected;
            }
            if let Some(generic) = self.generic(name) {
                self.output.query.value.expressions.push((
                    id,
                    match generic {
                        GenericName::Size(size) => ResolvedName::Size(size),
                        // A known type argument in expression brackets must
                        // reach H's LANG-2 judgment instead of LANG-4 lookup.
                        GenericName::Type(parameter) => ResolvedName::TypeParameter(parameter),
                    },
                ));
                return ExpectedType::Unknown;
            }
            // #54: retain expected-type variant precedence over a same-named
            // package function pending the namespace-priority ruling.
            if let Some((variant, ..)) =
                self.variant(name, &expected, NameSite::Expression(id), span)
            {
                self.output
                    .query
                    .value
                    .expressions
                    .push((id, ResolvedName::Variant(variant)));
                return expected;
            }
            if self.scopes.iter().any(|scope| scope.future.contains(&name)) {
                // Do not fall back to a global for a forbidden local reference.
                let _ = self.lookup(path, span);
                self.diagnostic(
                    if self.local_function_depth > 0 {
                        const { RuleId::of("LANG-10") }
                    } else {
                        const { RuleId::of("LANG-2") }
                    },
                    span,
                    "local definitions must precede their use",
                );
                return ExpectedType::Unknown;
            }
        }
        match self.lookup(path, span) {
            Ok(definition) if self.shadows_core_type(path, &definition) => {
                self.defer(NameSite::Expression(id), path.segments[0], span);
                ExpectedType::Unknown
            }
            Ok(definition) => {
                let namespace = self.namespace(&definition);
                // Qualified variants are never synthesized from nominal owners.
                self.signature(&definition);
                self.output
                    .query
                    .value
                    .expressions
                    .push((id, ResolvedName::Definition(definition.clone())));
                if namespace == Some(Namespace::Type) {
                    self.output.contextual.push((
                        NameSite::Expression(id),
                        ContextualName::Constructor(definition.clone()),
                    ));
                    if !self.representation_visible(&definition, span) {
                        return ExpectedType::Unknown;
                    }
                    return ExpectedType::nominal(definition);
                }
                if let Some(signature) = self.signature(&definition) {
                    match &signature.kind {
                        ty::SignatureKind::Function(f) => {
                            return ExpectedType::Function {
                                parameters: f
                                    .parameters
                                    .iter()
                                    .map(|p| semantic_expected(self.types, p.ty, &[], 0, 0))
                                    .collect(),
                                result: Box::new(semantic_expected(
                                    self.types,
                                    f.result,
                                    &[],
                                    0,
                                    0,
                                )),
                            };
                        }
                        ty::SignatureKind::Const { ty: ty, .. } => {
                            return semantic_expected(self.types, *ty, &[], 0, 0);
                        }
                        _ => {}
                    }
                }
                // Written same-file signatures work without a provider.
                if let Some(fixture) = self
                    .siblings
                    .iter()
                    .find(|i| i.item.definition == definition)
                {
                    let old_syntax = std::mem::replace(&mut self.syntax, fixture);
                    let written = match fixture.item.kind.clone() {
                        ItemKind::Function { signature, .. } => Some(ExpectedType::Function {
                            parameters: signature
                                .parameters
                                .iter()
                                .map(|p| self.syntax_expected(p.ty))
                                .collect(),
                            result: Box::new(
                                signature
                                    .result
                                    .map(|id| self.syntax_expected(id))
                                    .unwrap_or_default(),
                            ),
                        }),
                        ItemKind::Const { ty, .. } => Some(self.syntax_expected(ty)),
                        ItemKind::Type { .. }
                        | ItemKind::Signature { .. }
                        | ItemKind::Implementation { .. } => None,
                    };
                    self.syntax = old_syntax;
                    if let Some(written) = written {
                        return written;
                    }
                }
                expected
            }
            Err(e) => {
                if matches!(e, LookupError::Missing | LookupError::Provider(_))
                    && path.package.is_none()
                    && path.segments.len() == 1
                    && matches!(expected, ExpectedType::Unknown)
                    && self.callees.contains(&id)
                    && let Some(owner) = self.inferred_variant_owner(path.segments[0])
                {
                    let nominal = ExpectedType::nominal(owner);
                    if let Some((variant, ..)) =
                        self.variant(path.segments[0], &nominal, NameSite::Expression(id), span)
                    {
                        self.output
                            .query
                            .value
                            .expressions
                            .push((id, ResolvedName::Variant(variant)));
                        return nominal;
                    }
                }
                if matches!(e, LookupError::Missing | LookupError::Provider(_))
                    && path.package.is_none()
                    && path.segments.len() == 1
                    && self.known_variant(path.segments[0])
                {
                    self.defer(NameSite::Expression(id), path.segments[0], span);
                } else if path.package.is_none()
                    && path.segments.len() > 1
                    && self.value(path.segments[0]).is_some()
                    && self.module_path(path)
                    && matches!(
                        e,
                        LookupError::Missing
                            | LookupError::MissingMember(_)
                            | LookupError::Provider(_)
                    )
                {
                    let head = self.spelling(path.segments[0]);
                    self.diagnostic(const { RuleId::of("LANG-4") }, span,
                        &format!("before `.`, `{head}` names a module; rename the local `{head}` to access its fields"));
                } else {
                    self.lookup_diagnostic(&e, path, span);
                    if let Some(help) = self.qualified_variant_help(&e, path, &expected) {
                        self.output
                            .query
                            .diagnostics
                            .last_mut()
                            .unwrap()
                            .notes
                            .push(help);
                    }
                    if path.package.is_none()
                        && path.segments.len() == 1
                        && matches!(e, LookupError::Missing)
                        && let ExpectedType::Named {
                            definition,
                            arguments,
                        } = &expected
                        && self.representation_visible(definition, span)
                    {
                        let variants: Vec<_> = self
                            .shape(definition, arguments)
                            .variants
                            .into_iter()
                            .map(|(n, _)| n)
                            .collect();
                        let name_span = self.path_name_span(path, span, 0);
                        let spelling = self.spelling(path.segments[0]);
                        hirsel_diag::suggest_names(
                            self.output.query.diagnostics.last_mut().unwrap(),
                            &spelling,
                            variants,
                            "",
                            name_span,
                        );
                    }
                }
                ExpectedType::Unknown
            }
        }
    }
    /// A failed projection through Shared is still LANG-4. Inspect only the
    /// visible payload header to decide whether a read callback can help.
    fn shared_field_payload(
        &mut self,
        owner: &DefPath,
        arguments: &[ExpectedType],
        field: Symbol,
    ) -> Option<String> {
        if !owner.package_is("core", "1") || !owner.0.modules.is_empty() || owner.0.name != "Shared"
        {
            return None;
        }
        let [
            payload @ ExpectedType::Named {
                definition,
                arguments,
            },
        ] = arguments
        else {
            return None;
        };
        if self.index.access(&self.context, definition).is_err() {
            return None;
        }
        if definition.0.package != self.context.package
            && (self
                .index
                .entry(definition)
                .is_some_and(|e| e.visibility == Visibility::PublicOpaque)
                || self.signature(definition).is_some_and(|s| {
                    matches!(&s.kind,
                    ty::SignatureKind::Type(t) if t.representation.is_none())
                }))
        {
            return None;
        }
        let shape = self.shape(definition, arguments);
        shape
            .fields
            .iter()
            .any(|(name, _)| name == &self.spelling(field))
            .then(|| self.expected_spelling(payload))
            .flatten()
    }
    fn expected_spelling(&self, expected: &ExpectedType) -> Option<String> {
        match expected {
            ExpectedType::Named {
                definition,
                arguments,
            } => {
                let mut name = if definition.0.package == self.context.package
                    || definition.package_is("core", "1")
                {
                    definition
                        .0
                        .modules
                        .iter()
                        .chain([&definition.0.name])
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    definition.to_string()
                };
                if !arguments.is_empty() {
                    let arguments = arguments
                        .iter()
                        .map(|t| self.expected_spelling(t))
                        .collect::<Option<Vec<_>>>()?;
                    name.push_str(&format!("[{}]", arguments.join(", ")));
                }
                Some(name)
            }
            ExpectedType::Tuple(parts) => {
                let parts = parts
                    .iter()
                    .map(|t| self.expected_spelling(t))
                    .collect::<Option<Vec<_>>>()?;
                Some(if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                })
            }
            ExpectedType::Unknown | ExpectedType::Function { .. } => None,
        }
    }
    fn local_projection(
        &mut self,
        path: &Path,
        site: NameSite,
        span: Span,
    ) -> Option<(ResolvedName, ExpectedType)> {
        if path.package.is_some() || path.segments.len() < 2 || self.module_path(path) {
            return None;
        }
        let binding = self.value(path.segments[0])?;
        if let Some(owner) = binding.owner {
            self.output.reference_scopes.push((site.clone(), owner));
        }
        let mut expected = binding.expected;
        for (segment, field) in path.segments.iter().enumerate().skip(1) {
            if let ExpectedType::Named {
                definition: owner,
                arguments,
            } = expected
            {
                if !self.representation_visible(&owner, span) {
                    return Some((binding.name, ExpectedType::Unknown));
                }
                let shape = self.shape(&owner, &arguments);
                let field_names: Vec<_> =
                    shape.fields.iter().map(|(name, _)| name.clone()).collect();
                if let Some((i, (_, ty))) = shape
                    .fields
                    .into_iter()
                    .enumerate()
                    .find(|(_, (name, _))| name == &self.spelling(*field))
                {
                    self.output.contextual.push((
                        site.clone(),
                        ContextualName::Field {
                            owner,
                            variant: None,
                            field: ty::FieldId(i as u32),
                        },
                    ));
                    expected = ty;
                } else if shape
                    .variants
                    .iter()
                    .any(|(name, _)| name == &self.spelling(*field))
                {
                    // Payload access is a typing/layout judgment, including the
                    // ordinary-variant prohibition and Packed tag selection.
                    self.defer(site.clone(), *field, span);
                    expected = ExpectedType::Unknown;
                } else {
                    let rule = if matches!(site, NameSite::Expression(id) if self.callees.contains(&id))
                    {
                        const { RuleId::of("LANG-1") }
                    } else {
                        const { RuleId::of("LANG-4") }
                    };
                    let payload = self.shared_field_payload(&owner, &arguments, *field);
                    self.diagnostic(
                        rule,
                        span,
                        "unresolved field name; methods are not supported",
                    );
                    if let Some(payload) = payload {
                        let diagnostic = self.output.query.diagnostics.last_mut().unwrap();
                        *diagnostic = diagnostic.clone().with_shared_read_help(&payload);
                    }
                    let prefix = path.segments[..segment]
                        .iter()
                        .map(|s| format!("{}.", self.spelling(*s)))
                        .collect::<String>();
                    let name_span = self.path_name_span(path, span, segment);
                    let spelling = self.spelling(*field);
                    hirsel_diag::suggest_names(
                        self.output.query.diagnostics.last_mut().unwrap(),
                        &spelling,
                        field_names,
                        &prefix,
                        name_span,
                    );
                    expected = ExpectedType::Unknown;
                }
            } else {
                self.defer(site.clone(), *field, span);
                expected = ExpectedType::Unknown;
            }
        }
        Some((binding.name, expected))
    }
    fn dot_path(&self, id: ExprId) -> Option<Path> {
        match &self.syntax.tree.expressions[id.0 as usize].kind {
            ExprKind::Path(path) => Some(path.clone()),
            ExprKind::Field { base, field } => {
                let mut path = self.dot_path(*base)?;
                path.segments.push(*field);
                Some(path)
            }
            _ => None,
        }
    }
    fn module_path(&mut self, path: &Path) -> bool {
        if path.package.is_some() {
            return true;
        }
        let Some(head) = path.segments.first().map(|s| self.spelling(*s)) else {
            return false;
        };
        let exposed = self.index.has_head(&self.context, &head);
        std::sync::Arc::make_mut(&mut self.output.reads).push(
            DependencyRead::ModuleHead {
                context: (self.context.clone()).into(),
                head,
                manifest: self.index.revision,
                exposed,
            }
            .shared(),
        );
        exposed
    }
    fn call(
        &mut self,
        callee: ExprId,
        arguments: Vec<Argument>,
        expected: ExpectedType,
    ) -> ExpectedType {
        self.callees.insert(callee);
        let callee_type = self.expr(callee, expected.clone());
        let span = self.syntax.tree.expressions[callee.0 as usize].origin.span;
        let mut fields = vec![];
        let mut owner_variant = None;
        if let Some((_, ContextualName::Variant { owner, variant })) = self
            .output
            .contextual
            .iter()
            .rev()
            .find(|(site, _)| *site == NameSite::Expression(callee))
            .cloned()
        {
            // A contextual variant was selected by the expected type, whose
            // arguments instantiate its payload.
            let arguments = match &expected {
                ExpectedType::Named {
                    definition,
                    arguments,
                } => match self.represented(definition, arguments) {
                    (definition, arguments) if definition == owner => arguments,
                    _ => vec![],
                },
                _ => vec![],
            };
            fields = self
                .shape(&owner, &arguments)
                .variants
                .get(variant.0 as usize)
                .map(|(_, fields)| fields.clone())
                .unwrap_or_default();
            owner_variant = Some((owner, Some(variant)));
        } else if let ExpectedType::Named {
            definition: owner,
            arguments,
        } = &callee_type
        {
            fields = self
                .shape(owner, arguments)
                .fields
                .into_iter()
                .map(|(n, t)| (Some(n), t))
                .collect();
            owner_variant = Some((self.represented(owner, arguments).0, None));
        }
        let parameters = if let ExpectedType::Function { parameters, .. } = &callee_type {
            parameters.clone()
        } else {
            vec![]
        };
        for (i, argument) in arguments.into_iter().enumerate() {
            let field = if let Some(label) = argument.label {
                fields
                    .iter()
                    .enumerate()
                    .find(|(_, (n, _))| n.as_deref() == Some(&self.spelling(label)))
            } else {
                fields.get(i).map(|field| (i, field))
            };
            let arg_expected = field
                .map(|(_, (_, t))| t.clone())
                .or_else(|| parameters.get(i).cloned())
                .unwrap_or_default();
            if let (Some((owner, variant)), Some((index, _))) = (&owner_variant, field) {
                self.output.contextual.push((
                    NameSite::Expression(argument.value),
                    ContextualName::Field {
                        owner: owner.clone(),
                        variant: *variant,
                        field: ty::FieldId(index as u32),
                    },
                ));
            } else if owner_variant.is_some() && argument.label.is_some() {
                self.diagnostic(
                    const { RuleId::of("LANG-12") },
                    span,
                    "constructor label is not declared",
                );
            }
            self.expr(argument.value, arg_expected);
        }
        match callee_type {
            ExpectedType::Function { result, .. } => *result,
            named @ ExpectedType::Named { .. } => named,
            _ => expected,
        }
    }
    fn statement_names(&self, id: StmtId) -> Vec<Symbol> {
        match &self.syntax.tree.statements[id.0 as usize].kind {
            StmtKind::Function { name, .. } => vec![*name],
            StmtKind::Let { pattern, .. } => self.pattern_names(*pattern),
            _ => vec![],
        }
    }
    fn pattern_names(&self, id: PatternId) -> Vec<Symbol> {
        match &self.syntax.tree.patterns[id.0 as usize].kind {
            PatternKind::Binding(name) => vec![*name],
            PatternKind::Tuple(patterns) => patterns
                .iter()
                .flat_map(|p| self.pattern_names(*p))
                .collect(),
            PatternKind::Constructor { fields, .. } => fields
                .iter()
                .flat_map(|f| self.pattern_names(f.pattern))
                .collect(),
            PatternKind::Or(alternatives) => self.pattern_names(alternatives[0]),
            _ => vec![],
        }
    }
    fn statement(&mut self, id: StmtId) {
        let statement = self.syntax.tree.statements[id.0 as usize].clone();
        match statement.kind {
            StmtKind::Let {
                pattern,
                ty,
                value,
                otherwise,
                ..
            } => {
                let names = self.pattern_names(pattern);
                let declaring = std::mem::replace(&mut self.declaring, names);
                let expected = ty.map(|t| self.type_expr(t)).unwrap_or_default();
                self.declaring = declaring;
                let own = std::mem::take(&mut self.deferred);
                let value_type = self.expr(value, expected.clone());
                // LANG-13: the else block runs where the pattern bound nothing.
                if let Some(otherwise) = otherwise {
                    self.expr(otherwise, ExpectedType::Unknown);
                }
                self.pattern(
                    pattern,
                    if expected == ExpectedType::Unknown {
                        value_type
                    } else {
                        expected
                    },
                );
                for (path, span, site) in own {
                    self.extra_path(&path, span, site);
                }
            }
            StmtKind::Assign { place, value, .. } => {
                let expected = self.expr(place, ExpectedType::Unknown);
                self.expr(value, expected);
            }
            StmtKind::Expr(expr) => {
                self.expr(expr, ExpectedType::Unknown);
            }
            StmtKind::While {
                condition, body, ..
            } => {
                self.expr(condition, ExpectedType::Unknown);
                self.expr(body, ExpectedType::Unknown);
            }
            StmtKind::For {
                pattern,
                source,
                body,
                ..
            } => {
                self.expr(source, ExpectedType::Unknown);
                self.scopes.push(Scope::default());
                self.pattern(pattern, ExpectedType::Unknown);
                self.expr(body, ExpectedType::Unknown);
                self.scopes.pop();
            }
            StmtKind::Function {
                name,
                signature,
                body,
            } => {
                self.scopes.push(Scope::default());
                self.local_function_depth += 1;
                let result = self.function_header(
                    &signature,
                    statement.origin.span,
                    NameSite::Statement(id),
                );
                let old = std::mem::replace(&mut self.return_type, result.clone());
                let labels = std::mem::take(&mut self.block_labels);
                let enclosing_contract = self.contract_kind.take();
                self.expr(body, result.clone());
                self.contract_kind = enclosing_contract;
                self.block_labels = labels;
                self.return_type = old;
                self.local_function_depth -= 1;
                self.scopes.pop();
                let parameters = signature
                    .parameters
                    .iter()
                    .map(|p| self.syntax_expected(p.ty))
                    .collect();
                let local = self.local(
                    name,
                    statement.origin.span,
                    ExpectedType::Function {
                        parameters,
                        result: Box::new(result),
                    },
                );
                self.output.local_functions.push((id, local));
            }
        }
        for name in self.statement_names(id) {
            self.scopes.last_mut().unwrap().future.remove(&name);
        }
    }
    fn pattern(&mut self, id: PatternId, expected: ExpectedType) {
        let expected = self.expected.patterns.get(&id).cloned().unwrap_or(expected);
        let node = self.syntax.tree.patterns[id.0 as usize].clone();
        match node.kind {
            PatternKind::Binding(name) => {
                let variant = self
                    .variant(name, &expected, NameSite::Pattern(id), node.origin.span)
                    .is_some();
                if variant {
                    // A contextual nullary variant binds nothing.
                } else if let Some(first) = self.alternative_locals.last_mut() {
                    // LANG-13: a later alternative binds the first one's locals.
                    match first.get_mut(&name) {
                        Some((local, false)) => {
                            let local = *local;
                            first.insert(name, (local, true));
                            self.output.query.value.bindings.push((id, name, local));
                        }
                        Some((_, true)) => self.diagnostic(
                            const { RuleId::of("LANG-4") },
                            node.origin.span,
                            "a local cannot shadow another local",
                        ),
                        None => {
                            let message = format!(
                                "every alternative binds the same names; the first does not bind `{}`",
                                self.spelling(name)
                            );
                            self.diagnostic(
                                const { RuleId::of("LANG-13") },
                                node.origin.span,
                                message,
                            )
                        }
                    }
                } else {
                    let local = self.local(name, node.origin.span, expected);
                    self.output.query.value.bindings.push((id, name, local));
                }
            }
            PatternKind::Or(alternatives) => {
                let mark = self.output.query.value.bindings.len();
                self.pattern(alternatives[0], expected.clone());
                let first: BTreeMap<_, _> = self.output.query.value.bindings[mark..]
                    .iter()
                    .map(|(_, name, local)| (*name, (*local, false)))
                    .collect();
                for alternative in &alternatives[1..] {
                    self.alternative_locals.push(first.clone());
                    self.pattern(*alternative, expected.clone());
                    let bound = self.alternative_locals.pop().unwrap_or_default();
                    for (name, _) in bound.into_iter().filter(|(_, (_, used))| !used) {
                        let message = format!(
                            "every alternative binds the same names; this one does not bind `{}`",
                            self.spelling(name)
                        );
                        let span = self.syntax.tree.patterns[alternative.0 as usize]
                            .origin
                            .span;
                        self.diagnostic(const { RuleId::of("LANG-13") }, span, message);
                    }
                }
            }
            PatternKind::Tuple(patterns) => {
                let types = if let ExpectedType::Tuple(types) = expected {
                    types
                } else {
                    vec![]
                };
                for (i, pattern) in patterns.into_iter().enumerate() {
                    self.pattern(pattern, types.get(i).cloned().unwrap_or_default());
                }
            }
            PatternKind::Constructor { path, fields, .. } => {
                let mut field_types = vec![];
                let mut identity = None;
                if path.package.is_none()
                    && path.segments.len() == 1
                    && let Some((variant, types, owner)) = self.variant(
                        path.segments[0],
                        &expected,
                        NameSite::Pattern(id),
                        node.origin.span,
                    )
                {
                    field_types = types;
                    identity = Some((owner, Some(variant)));
                } else {
                    match self.lookup(&path, node.origin.span) {
                        Ok(owner)
                            if expected == ExpectedType::Unknown
                                && self.shadows_core_type(&path, &owner) =>
                        {
                            self.defer(NameSite::Pattern(id), path.segments[0], node.origin.span);
                        }
                        Ok(owner) => {
                            if self.namespace(&owner) != Some(Namespace::Type) {
                                self.diagnostic(
                                    const { RuleId::of("LANG-4") },
                                    node.origin.span,
                                    "pattern constructor is not a type",
                                );
                            } else if self.representation_visible(&owner, node.origin.span) {
                                let (owner, arguments) = self.represented(&owner, &[]);
                                self.output.contextual.push((
                                    NameSite::Pattern(id),
                                    ContextualName::Constructor(owner.clone()),
                                ));
                                field_types = self
                                    .shape(&owner, &arguments)
                                    .fields
                                    .into_iter()
                                    .map(|(n, t)| (Some(n), t))
                                    .collect();
                                identity = Some((owner, None));
                            }
                        }
                        Err(e) => {
                            if matches!(e, LookupError::Missing | LookupError::Provider(_))
                                && path.package.is_none()
                                && path.segments.len() == 1
                                && expected == ExpectedType::Unknown
                                && self.known_variant(path.segments[0])
                            {
                                self.defer(
                                    NameSite::Pattern(id),
                                    path.segments[0],
                                    node.origin.span,
                                );
                            } else {
                                self.lookup_diagnostic(&e, &path, node.origin.span);
                                if matches!(e, LookupError::Missing)
                                    && path.package.is_none()
                                    && path.segments.len() == 1
                                    && let ExpectedType::Named {
                                        definition,
                                        arguments,
                                    } = &expected
                                    && self.representation_visible(definition, node.origin.span)
                                {
                                    let names: Vec<_> = self
                                        .shape(definition, arguments)
                                        .variants
                                        .into_iter()
                                        .map(|(n, _)| n)
                                        .collect();
                                    let name_span = self.path_name_span(&path, node.origin.span, 0);
                                    let spelling = self.spelling(path.segments[0]);
                                    hirsel_diag::suggest_names(
                                        self.output.query.diagnostics.last_mut().unwrap(),
                                        &spelling,
                                        names,
                                        "",
                                        name_span,
                                    );
                                }
                            }
                        }
                    }
                }
                for (i, field) in fields.into_iter().enumerate() {
                    let found = if let Some(label) = field.label {
                        field_types
                            .iter()
                            .enumerate()
                            .find(|(_, (n, _))| n.as_deref() == Some(&self.spelling(label)))
                    } else {
                        field_types.get(i).map(|f| (i, f))
                    };
                    if let (Some((owner, variant)), Some((index, _))) = (&identity, found) {
                        self.output.contextual.push((
                            NameSite::Pattern(field.pattern),
                            ContextualName::Field {
                                owner: owner.clone(),
                                variant: *variant,
                                field: ty::FieldId(index as u32),
                            },
                        ));
                    } else if identity.is_some() && field.label.is_some() {
                        self.diagnostic(
                            const { RuleId::of("LANG-4") },
                            node.origin.span,
                            "unresolved pattern field name",
                        );
                        let names: Vec<_> =
                            field_types.iter().filter_map(|(n, _)| n.clone()).collect();
                        let label = field.label.unwrap();
                        let before = self.syntax.tree.patterns[field.pattern.0 as usize]
                            .origin
                            .span
                            .start;
                        let label_span =
                            Span::new(node.origin.span.anchor, node.origin.span.start, before)
                                .and_then(|span| self.name_span(span, label, true));
                        let spelling = self.spelling(label);
                        hirsel_diag::suggest_names(
                            self.output.query.diagnostics.last_mut().unwrap(),
                            &spelling,
                            names,
                            "",
                            label_span,
                        );
                    }
                    self.pattern(
                        field.pattern,
                        found.map(|(_, (_, t))| t.clone()).unwrap_or_default(),
                    );
                }
            }
            _ => {}
        }
    }
}

const MAX_EXPECTED_DEPTH: usize = 4096;
/// `arguments` instantiate the generic parameters bound `binders` function
/// types outside `id`; any other parameter is unknown context.
fn semantic_expected(
    store: &ty::TypeInterner,
    id: ty::TypeId,
    arguments: &[ExpectedType],
    binders: u32,
    depth: usize,
) -> ExpectedType {
    // Valid graphs are acyclic; this bounds a malformed (cyclic) one.
    if depth > MAX_EXPECTED_DEPTH {
        return ExpectedType::Unknown;
    }
    let nested =
        |id: ty::TypeId, binders: u32| semantic_expected(store, id, arguments, binders, depth + 1);
    match store.get(id).map(|ty| &ty.kind) {
        Some(ty::TypeKind::Named {
            definition,
            arguments: written,
        }) => ExpectedType::Named {
            definition: definition.path().clone(),
            arguments: written
                .iter()
                .map(|argument| match argument {
                    ty::GenericArgument::Type(t) => nested(*t, binders),
                    ty::GenericArgument::Size(_) => ExpectedType::Unknown,
                })
                .collect(),
        },
        Some(ty::TypeKind::Parameter(ty::BinderRef::Bound {
            depth: binder,
            parameter,
        })) if *binder == binders => arguments
            .get(parameter.0 as usize)
            .cloned()
            .unwrap_or_default(),
        Some(ty::TypeKind::Tuple(types)) => {
            ExpectedType::Tuple(types.iter().map(|t| nested(*t, binders)).collect())
        }
        Some(ty::TypeKind::Function(f)) => ExpectedType::Function {
            parameters: f
                .parameters
                .iter()
                .map(|p| nested(p.ty, binders + 1))
                .collect(),
            result: Box::new(nested(f.result, binders + 1)),
        },
        Some(ty::TypeKind::Box(id)) => nested(*id, binders),
        _ => ExpectedType::Unknown,
    }
}
fn shape_from_type(store: &ty::TypeInterner, id: ty::TypeId, arguments: &[ExpectedType]) -> Shape {
    let expected = |t: ty::TypeId| semantic_expected(store, t, arguments, 0, 0);
    match store.get(id).map(|t| &t.kind) {
        Some(ty::TypeKind::Record(fields)) => Shape {
            fields: fields
                .iter()
                .map(|f| (f.name.as_str().to_owned(), expected(f.ty)))
                .collect(),
            variants: vec![],
        },
        Some(ty::TypeKind::Variant(variants)) => Shape {
            fields: vec![],
            variants: variants
                .iter()
                .map(|v| {
                    (
                        v.name.as_str().to_owned(),
                        v.fields
                            .iter()
                            .map(|(n, t)| (n.map(|n| n.as_str().to_owned()), expected(*t)))
                            .collect(),
                    )
                })
                .collect(),
        },
        _ => Shape::default(),
    }
}
/// The type rows a signature's declaration roots: its header or constant
/// type, or a type's representation.
fn signature_roots(signature: &ty::Signature) -> Vec<ty::TypeId> {
    match &signature.kind {
        ty::SignatureKind::Function(f) => f
            .parameters
            .iter()
            .map(|p| p.ty)
            .chain([f.result])
            .chain(f.error)
            .collect(),
        ty::SignatureKind::Type(declaration) => declaration.representation.into_iter().collect(),
        ty::SignatureKind::Const { ty, .. } => vec![*ty],
        ty::SignatureKind::Family(_) => vec![],
        ty::SignatureKind::Implementation(implementation) => implementation
            .members
            .iter()
            .filter_map(|(_, actual)| match actual {
                ty::MemberActual::Type(ty) => Some(*ty),
                _ => None,
            })
            .collect(),
    }
}
fn primitive(name: &str) -> Option<ty::Primitive> {
    use ty::Primitive::*;
    Some(match name {
        "bool" => Bool,
        "u8" => U8,
        "u16" => U16,
        "u32" => U32,
        "u64" => U64,
        "u128" => U128,
        "i8" => I8,
        "i16" => I16,
        "i32" => I32,
        "i64" => I64,
        "i128" => I128,
        "f32" => F32,
        "f64" => F64,
        "char" => Char,
        "String" => String,
        "Bytes" => Bytes,
        "Never" => Never,
        _ => return None,
    })
}

// Compiler-native type constructors, matching the semantic TypeKind vocabulary.
// Qualified library names remain ordinary lookups and never use this fallback.
fn native_type(path: &Path) -> bool {
    path.package.is_none()
        && path.segments.len() == 1
        && matches!(
            path.segments[0].as_str(),
            "Array"
                | "List"
                | "Map"
                | "Set"
                | "SortedMap"
                | "Inline"
                | "Simd"
                | "Packed"
                | "Shared"
                | "Slots"
                | "SlotId"
                | "Region"
                | "Iter"
                | "Task"
                | "Sender"
                | "Receiver"
                | "Mutex"
                | "Atomic"
                | "Ptr"
                | "FnPtr"
                | "TypeShape"
        )
}

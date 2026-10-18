use crate::{LookupError, ManifestIndex, Namespace};
use hirsel_diag::{Diagnostic, Label, RuleId};
use hirsel_intern::{Interner, Symbol};
use hirsel_span::Span;
use hirsel_syntax::*;
use hirsel_types::{
    self as ty, LocalId, ParamId, Resolution, ResolvedName, SignatureProvider, SizeParamId,
    SizeParameterRef, TypeParamId, TypeParameterRef,
};
use std::collections::{BTreeMap, BTreeSet};

pub use hirsel_types::{
    ContextualName, DeferredName, DependencyRead, GenericBinding, GenericParameterName, NameSite,
};

/// Expected nominal/aggregate context supplied by syntax or later checking.
/// No body inference is performed here; external hints can finish deferred uses.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ExpectedType {
    Named(DefPath),
    Tuple(Vec<ExpectedType>),
    Function {
        parameters: Vec<ExpectedType>,
        result: Box<ExpectedType>,
    },
    #[default]
    Unknown,
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
    future: BTreeSet<Symbol>,
    owner: Option<NameSite>,
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
/// SyntaxFile may include other headers, but resolution never walks their bodies.
pub fn resolve_item(
    file: &SyntaxFile,
    item: ItemId,
    symbols: &Interner,
    index: &ManifestIndex,
    provider: &mut (impl SignatureProvider + ?Sized),
) -> ResolveOutput {
    resolve_item_with_expected(
        file,
        item,
        symbols,
        index,
        provider,
        &ExpectedTypes::default(),
    )
}

/// Syntax read domain. File permits local header fixtures; Item and Header
/// require every other declaration to come through the signature provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveView {
    File(ItemId),
    Item(ItemId),
    Header(ItemId),
}
impl ResolveView {
    fn item(self) -> ItemId {
        match self {
            Self::File(item) | Self::Item(item) | Self::Header(item) => item,
        }
    }
}

pub fn resolve_item_with_expected(
    file: &SyntaxFile,
    item: ItemId,
    symbols: &Interner,
    index: &ManifestIndex,
    provider: &mut (impl SignatureProvider + ?Sized),
    expected: &ExpectedTypes,
) -> ResolveOutput {
    resolve_view(
        file,
        ResolveView::File(item),
        symbols,
        index,
        provider,
        expected,
    )
}

/// Resolve the selected domain without modifying syntax or its arena identities.
pub fn resolve_view(
    file: &SyntaxFile,
    view: ResolveView,
    symbols: &Interner,
    index: &ManifestIndex,
    provider: &mut (impl SignatureProvider + ?Sized),
    expected: &ExpectedTypes,
) -> ResolveOutput {
    let item = view.item();
    let definition = &file.tree.items[item.0 as usize].definition;
    let mut resolver = Resolver {
        file,
        view,
        symbols,
        index,
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
        signatures: BTreeMap::new(),
        next_local: 0,
        callees: BTreeSet::new(),
        local_function_depth: 0,
        return_type: ExpectedType::Unknown,
        contract_kind: None,
        exported: false,
        shaping: BTreeSet::new(),
        sibling_fields: BTreeMap::new(),
        declaring: vec![],
        deferred: vec![],
    };
    resolver.item(item);
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
        .sort_by(|a, b| a.definition.cmp(&b.definition));
    resolver.output
}

struct Resolver<'a, P: ?Sized> {
    file: &'a SyntaxFile,
    view: ResolveView,
    symbols: &'a Interner,
    index: &'a ManifestIndex,
    provider: &'a mut P,
    expected: &'a ExpectedTypes,
    context: FileContext,
    output: ResolveOutput,
    scopes: Vec<Scope>,
    signatures: BTreeMap<DefPath, Option<ty::SignatureRef>>,
    next_local: u32,
    callees: BTreeSet<ExprId>,
    local_function_depth: usize,
    return_type: ExpectedType,
    contract_kind: Option<ContractKind>,
    exported: bool,
    shaping: BTreeSet<DefPath>,
    sibling_fields: BTreeMap<Symbol, (DefPath, ty::FieldId, TypeExprId)>,
    /// Names a let pattern or parameter is declaring: a relation naming one
    /// stands for that binding's own scope (MODE-1, LANG-15).
    declaring: Vec<Symbol>,
    /// Relation places that name a binding being declared, resolved once it is.
    deferred: Vec<(Path, Span, NameSite)>,
}

impl<P: SignatureProvider + ?Sized> Resolver<'_, P> {
    fn spelling(&self, symbol: Symbol) -> String {
        self.symbols
            .resolve(symbol)
            .unwrap_or("<invalid symbol>")
            .to_owned()
    }
    fn diagnostic(&mut self, rule: RuleId, span: Span, message: impl Into<String>) {
        self.output
            .query
            .diagnostics
            .push(Diagnostic::error(rule, span, message));
    }
    fn lookup(&mut self, path: &Path, _span: Span) -> Result<DefPath, LookupError> {
        self.lookup_using(path, self.symbols)
    }
    fn lookup_using(&mut self, path: &Path, symbols: &Interner) -> Result<DefPath, LookupError> {
        let segments = path
            .segments
            .iter()
            .map(|s| symbols.resolve(*s).unwrap_or("<invalid symbol>").to_owned())
            .collect::<Vec<_>>();
        let mut result = self.index.lookup(&self.context, path, symbols);
        // The provider can supply builtin/driver-owned names absent from this
        // manifest snapshot. It is never allowed to override ambiguity/privacy.
        if matches!(
            result,
            Err(LookupError::Missing | LookupError::MissingMember(_))
        ) {
            let lookup = self.provider.lookup(&self.context, path, symbols);
            self.output.reads.push(DependencyRead::ProviderLookup {
                context: self.context.clone(),
                package: path.package.clone(),
                segments: path
                    .segments
                    .iter()
                    .map(|s| symbols.resolve(*s).unwrap_or("").to_owned())
                    .collect(),
                output: lookup.clone(),
            });
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
        self.output.reads.push(DependencyRead::Lookup {
            context: self.context.clone(),
            package: path.package.clone(),
            segments,
            manifest: self.index.revision,
            result: result.clone(),
        });
        if let Ok(definition) = &result {
            self.output
                .query
                .value
                .dependencies
                .push(definition.clone());
        }
        result
    }
    fn lookup_diagnostic(&mut self, result: &LookupError, span: Span) {
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
            LookupError::Missing => {
                self.diagnostic(const { RuleId::of("LANG-4") }, span, "unresolved name")
            }
            LookupError::MissingMember(candidates) => self.diagnostic(
                const { RuleId::of("LANG-4") },
                span,
                format!(
                    "unresolved module member; looked for {}",
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
                    "ambiguous module head: {}; write a full package/version path",
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
    }
    fn signature(&mut self, definition: &DefPath) -> Option<ty::SignatureRef> {
        if let Some(signature) = self.signatures.get(definition) {
            return signature.clone();
        }
        let result = self.provider.signature(definition);
        let signature = match result {
            Ok(signature) => {
                self.output
                    .query
                    .dependencies
                    .push(ty::SignatureDependency {
                        definition: definition.clone(),
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
        self.output.reads.push(DependencyRead::Signature {
            definition: definition.clone(),
            fingerprint: signature.as_ref().map(|s| s.fingerprint),
        });
        self.output
            .query
            .value
            .dependencies
            .push(definition.clone());
        self.signatures
            .insert(definition.clone(), signature.clone());
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
        let mut symbols = Interner::default();
        let mut segments: Vec<_> = self
            .context
            .modules
            .iter()
            .map(|m| symbols.intern(m))
            .collect();
        segments.push(symbols.intern(&self.spelling(name)));
        let path = Path {
            package: Some(self.context.package.clone()),
            segments,
        };
        let read = self.lookup_using(&path, &symbols);
        let provider_value = if let Ok(definition) = &read {
            definition.0.package == self.context.package
                && definition.0.modules == self.context.modules
                && self.namespace(definition) == Some(Namespace::Value)
        } else {
            false
        };
        if provider_value {
            self.diagnostic(
                const { RuleId::of("LANG-4") },
                binding.span,
                "a local cannot shadow a top-level value in the same module",
            );
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
            NameSite::Item(id) => self.file.tree.items[id.0 as usize].origin,
            NameSite::Expression(id) => self.file.tree.expressions[id.0 as usize].origin,
            NameSite::Statement(id) => self.file.tree.statements[id.0 as usize].origin,
            NameSite::Type(id) => self.file.tree.types[id.0 as usize].origin,
            NameSite::Pattern(id) => self.file.tree.patterns[id.0 as usize].origin,
        }
    }
    fn generics(&mut self, generics: &[GenericParam], span: Span) {
        let owner = self
            .scopes
            .last()
            .unwrap()
            .owner
            .expect("generic declaration has an owner");
        let name_spans = self.file.generic_name_spans(self.origin(owner));
        let declarations = generics
            .iter()
            .map(|p| match &p.kind {
                GenericKind::Type(bounds) => ty::GenericParameter::Type {
                    name: self.symbols.resolve(p.name).unwrap_or("").to_owned(),
                    bounds: bounds.clone().into(),
                },
                GenericKind::Size => ty::GenericParameter::Size {
                    name: self.symbols.resolve(p.name).unwrap_or("").to_owned(),
                },
            })
            .collect();
        if !generics.is_empty() {
            self.output.generic_declarations.insert(owner, declarations);
        }
        for (ordinal, parameter) in generics.iter().enumerate() {
            if self.generic(parameter.name).is_some() || self.value(parameter.name).is_some() {
                self.diagnostic(
                    const { RuleId::of("GEN-1") },
                    name_spans.get(ordinal).copied().unwrap_or(span),
                    "duplicate generic parameter",
                );
                continue;
            }
            let name = match parameter.kind {
                GenericKind::Type(_) => {
                    let id = TypeParameterRef::body(owner, TypeParamId(ordinal as u32));
                    GenericName::Type(id)
                }
                GenericKind::Size => {
                    let id = SizeParameterRef::body(owner, SizeParamId(ordinal as u32));
                    GenericName::Size(id)
                }
            };
            let owner = self
                .scopes
                .last()
                .unwrap()
                .owner
                .clone()
                .expect("generic declaration has an owner");
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
    }
    fn item(&mut self, id: ItemId) {
        let item = self.file.tree.items[id.0 as usize].clone();
        self.scopes.last_mut().unwrap().owner = Some(NameSite::Item(id));
        self.exported = item.visibility != Visibility::Private;
        match item.kind {
            ItemKind::Function {
                signature, body, ..
            } => {
                let result = self.function_header(&signature, item.origin.span, NameSite::Item(id));
                self.exported = false;
                self.return_type = result.clone();
                if let Some(body) = body
                    && !matches!(self.view, ResolveView::Header(_))
                {
                    self.expr(body, result);
                }
            }
            ItemKind::Const { ty, value } => {
                let expected = self.type_expr(ty);
                self.exported = false;
                if !matches!(self.view, ResolveView::Header(_)) {
                    self.expr(value, expected);
                }
            }
            ItemKind::Type {
                generics,
                authority,
                body,
                drop,
                ..
            } => {
                let path = Path {
                    package: None,
                    segments: vec![item.name],
                };
                let spelling = self.spelling(item.name);
                if !item.definition.package_is("core", "1")
                    && (native_type(&path, self.symbols) || primitive(&spelling).is_some())
                {
                    self.diagnostic(const { RuleId::of("LANG-7") }, item.name_span,
                        &format!("type `{spelling}` conflicts with a builtin type name; choose a domain name for the type"));
                }
                self.generics(&generics, item.origin.span);
                for attribute in &item.attributes {
                    if self.symbols.resolve(attribute.name) == Some("loop_source") {
                        for argument in &attribute.arguments {
                            if let AttributeArgument::LoopSource(s) = argument {
                                // Stage one records operation identities only. Reading
                                // their signatures here cycles through the receiver's
                                // header; stage two validates them after publication.
                                for path in [&s.open, &s.next] {
                                    match self.lookup(path, s.origin.span) {
                                        Ok(definition) => self.output.paths.push((
                                            NameSite::Item(id),
                                            ResolvedName::Definition(definition),
                                        )),
                                        Err(e) => self.lookup_diagnostic(&e, s.origin.span),
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(paths) = authority {
                    for path in paths {
                        if path.package.is_none() && path.segments.len() == 1 {
                            if let Some(GenericName::Type(parameter)) =
                                self.generic(path.segments[0])
                            {
                                self.output.paths.push((
                                    NameSite::Item(id),
                                    ResolvedName::TypeParameter(parameter),
                                ));
                                continue;
                            }
                        }
                        self.extra_path(&path, item.origin.span, NameSite::Item(id));
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
                        let name_spans = self.file.variant_name_spans(item.origin);
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
                        ExpectedType::Named(item.definition),
                    );
                    self.output.drop_binding = Some(binding);
                    if !matches!(self.view, ResolveView::Header(_)) {
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
                            .file
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
            let path_spans = self.file.uses_path_spans(origin);
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
                        self.file
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
        let node = self.file.tree.types[id.0 as usize].clone();
        match node.kind {
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
                for argument in arguments {
                    if let TypeArgument::Type(ty) = argument {
                        self.type_expr(ty);
                    }
                }
                expected
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
                            matches!(signature.types.get(root).map(|row| &row.kind),
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
                    ExpectedType::Named(definition)
                }
            }
            Err(LookupError::Missing) if native_type(path, self.symbols) => {
                // These constructors already have TypeKind::Builtin semantics.
                // No shared ResolvedName variant denotes them; keep their failed
                // reads, without inventing a nominal library identity. An indexed
                // declaration or provider result always takes precedence here.
                ExpectedType::Unknown
            }
            Err(e) => {
                self.lookup_diagnostic(&e, span);
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
        for name in &path.segments[1..] {
            let ExpectedType::Named(owner) = expected else {
                self.defer(site.clone(), *name, span);
                return;
            };
            let shape = self.shape(&owner);
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
            if let Some((variant, _)) =
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
                    self.lookup_diagnostic(&e, span);
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
            Err(e) => self.lookup_diagnostic(&e, span),
        }
    }
    /// Reads only a written type annotation, never an initializer/body.
    fn hint(&self, id: TypeExprId) -> ExpectedType {
        match &self.file.tree.types[id.0 as usize].kind {
            TypeExprKind::Named { .. } => self
                .output
                .query
                .value
                .types
                .iter()
                .rev()
                .find_map(|(ty, name)| {
                    if *ty == id
                        && let ResolvedName::Definition(d) = name
                    {
                        Some(ExpectedType::Named(d.clone()))
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
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
    fn shape(&mut self, owner: &DefPath) -> Shape {
        if !self.shaping.insert(owner.clone()) {
            return Shape::default();
        }
        let result = self.shape_inner(owner);
        self.shaping.remove(owner);
        result
    }
    fn shape_inner(&mut self, owner: &DefPath) -> Shape {
        if let Some(signature) = self.signature(owner)
            && let ty::SignatureKind::Type(definition) = &signature.kind
            && let Some(representation) = definition.representation
        {
            if let Some(ty::Type {
                kind: ty::TypeKind::Named { definition, .. },
                ..
            }) = signature.types.get(representation)
            {
                return self.shape(definition);
            }
            return shape_from_type(&signature.types, representation);
        }
        if let Some(item) = self
            .file
            .tree
            .items
            .iter()
            .enumerate()
            .find(|(index, item)| {
                &item.definition == owner
                    && (matches!(self.view, ResolveView::File(_))
                        || *index == self.view.item().0 as usize)
            })
            .map(|(_, item)| item)
            .cloned()
            && let ItemKind::Type { body, .. } = item.kind
        {
            // Header-only adapter for written same-file signatures.
            // Type IDs here belong to this SyntaxFile, not to a foreign store.
            let old_context = std::mem::replace(
                &mut self.context,
                FileContext {
                    package: owner.0.package.clone(),
                    modules: owner.0.modules.clone(),
                },
            );
            let shape = match body {
                TypeBody::Record(fields) => Shape {
                    fields: fields
                        .into_iter()
                        .map(|f| (self.spelling(f.name), self.syntax_expected(f.ty)))
                        .collect(),
                    variants: vec![],
                },
                TypeBody::Variants(variants) => Shape {
                    fields: vec![],
                    variants: variants
                        .into_iter()
                        .map(|v| {
                            (
                                self.spelling(v.name),
                                v.fields
                                    .into_iter()
                                    .map(|(n, t)| {
                                        (n.map(|n| self.spelling(n)), self.syntax_expected(t))
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                },
                TypeBody::Distinct { field, .. } => Shape {
                    fields: vec![(self.spelling(field.name), self.syntax_expected(field.ty))],
                    variants: vec![],
                },
                TypeBody::Alias(id) => {
                    if let ExpectedType::Named(d) = self.syntax_expected(id) {
                        self.shape(&d)
                    } else {
                        Shape::default()
                    }
                }
                TypeBody::Opaque => Shape::default(),
            };
            self.context = old_context;
            return shape;
        }
        Shape::default()
    }
    fn syntax_expected(&mut self, id: TypeExprId) -> ExpectedType {
        let hint = self.hint(id);
        if hint != ExpectedType::Unknown {
            return hint;
        }
        // A referenced header need not itself have been resolved yet.
        match self.file.tree.types[id.0 as usize].kind.clone() {
            TypeExprKind::Named { path, .. } => {
                let span = self.file.tree.types[id.0 as usize].origin.span;
                self.lookup(&path, span)
                    .map(ExpectedType::Named)
                    .unwrap_or_default()
            }
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
                        s.kind,
                        ty::SignatureKind::Type(ty::TypeDefinition {
                            loop_source: None,
                            representation: None,
                            ..
                        })
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
                }) = signature.types.get(root)
            {
                owner = definition.clone();
            } else {
                return true;
            }
        }
        // Cyclic aliases are a signature error; this visibility walk cannot
        // manufacture a shape or replace the diagnostic for that prerequisite.
        true
    }
    fn known_variant(&mut self, name: Symbol) -> bool {
        let headers = self
            .file
            .tree
            .items
            .iter()
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
        let mut found = self
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
                    s.types.iter().filter_map(|(_, t)| match &t.kind {
                        ty::TypeKind::Named { definition, .. } => Some(definition.clone()),
                        _ => None,
                    })
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
            if let ty::SignatureKind::Type(definition) = &signature.kind {
                definition.representation.is_some_and(|id| {
                    shape_from_type(&signature.types, id)
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
    fn variant(
        &mut self,
        name: Symbol,
        expected: &ExpectedType,
        site: NameSite,
        span: Span,
    ) -> Option<(ty::VariantId, Vec<(Option<String>, ExpectedType)>)> {
        let ExpectedType::Named(owner) = expected else {
            return None;
        };
        let shape = self.shape(owner);
        let (i, (_, fields)) = shape
            .variants
            .into_iter()
            .enumerate()
            .find(|(_, (n, _))| n == &self.spelling(name))?;
        if !self.representation_visible(owner, span) {
            return None;
        }
        let variant = ty::VariantId(i as u32);
        self.output.contextual.push((
            site,
            ContextualName::Variant {
                owner: owner.clone(),
                variant,
            },
        ));
        Some((variant, fields))
    }
    fn defer(&mut self, site: NameSite, name: Symbol, span: Span) {
        self.output.deferred.push(DeferredName {
            site,
            span,
            name: self
                .symbols
                .resolve(name)
                .unwrap_or("<invalid symbol>")
                .to_owned(),
        });
    }
    fn expr(&mut self, id: ExprId, expected: ExpectedType) -> ExpectedType {
        let expected = self
            .expected
            .expressions
            .get(&id)
            .cloned()
            .unwrap_or(expected);
        let node = self.file.tree.expressions[id.0 as usize].clone();
        let span = node.origin.span;
        match node.kind {
            ExprKind::Path(path) => self.expression_path(id, &path, expected, span),
            ExprKind::Literal(_)
            | ExprKind::Heap
            | ExprKind::Todo(_)
            | ExprKind::Error
            | ExprKind::Break
            | ExprKind::Continue => expected,
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
            ExprKind::Field { base, field } => {
                if let Some(path) = self.dot_path(id)
                    && self.module_path(&path)
                {
                    return self.expression_path(id, &path, expected, span);
                }
                let base_type = self.expr(base, ExpectedType::Unknown);
                if let ExpectedType::Named(owner) = base_type {
                    if !self.representation_visible(&owner, span) {
                        return ExpectedType::Unknown;
                    }
                    let shape = self.shape(&owner);
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
            ExprKind::Block { statements, tail } => {
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
                let enclosing_contract = self.contract_kind.take();
                self.expr(body, result);
                self.contract_kind = enclosing_contract;
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
            if let Some((variant, _)) =
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
                    return ExpectedType::Named(definition);
                }
                if let Some(signature) = self.signature(&definition) {
                    match &signature.kind {
                        ty::SignatureKind::Function(f) => {
                            return ExpectedType::Function {
                                parameters: f
                                    .parameters
                                    .iter()
                                    .map(|p| semantic_expected(&signature.types, p.ty, 0))
                                    .collect(),
                                result: Box::new(semantic_expected(&signature.types, f.result, 0)),
                            };
                        }
                        ty::SignatureKind::Const(ty) => {
                            return semantic_expected(&signature.types, *ty, 0);
                        }
                        _ => {}
                    }
                }
                // Written same-file signatures work without a provider.
                if let Some(item) = self
                    .file
                    .tree
                    .items
                    .iter()
                    .find(|i| i.definition == definition)
                    .cloned()
                {
                    match item.kind {
                        ItemKind::Function { signature, .. } => {
                            return ExpectedType::Function {
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
                            };
                        }
                        ItemKind::Const { ty, .. } => return self.syntax_expected(ty),
                        _ => {}
                    }
                }
                expected
            }
            Err(e) => {
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
                    self.lookup_diagnostic(&e, span);
                }
                ExpectedType::Unknown
            }
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
        for field in &path.segments[1..] {
            if let ExpectedType::Named(owner) = expected {
                if !self.representation_visible(&owner, span) {
                    return Some((binding.name, ExpectedType::Unknown));
                }
                let shape = self.shape(&owner);
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
                    self.diagnostic(
                        rule,
                        span,
                        "unresolved field name; methods are not supported",
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
        match &self.file.tree.expressions[id.0 as usize].kind {
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
        self.output.reads.push(DependencyRead::ModuleHead {
            context: self.context.clone(),
            head,
            manifest: self.index.revision,
            exposed,
        });
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
        let span = self.file.tree.expressions[callee.0 as usize].origin.span;
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
            fields = self
                .shape(&owner)
                .variants
                .get(variant.0 as usize)
                .map(|(_, fields)| fields.clone())
                .unwrap_or_default();
            owner_variant = Some((owner, Some(variant)));
        } else if let ExpectedType::Named(owner) = &callee_type {
            fields = self
                .shape(owner)
                .fields
                .into_iter()
                .map(|(n, t)| (Some(n), t))
                .collect();
            owner_variant = Some((owner.clone(), None));
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
            ExpectedType::Named(owner) => ExpectedType::Named(owner),
            _ => expected,
        }
    }
    fn statement_names(&self, id: StmtId) -> Vec<Symbol> {
        match &self.file.tree.statements[id.0 as usize].kind {
            StmtKind::Function { name, .. } => vec![*name],
            StmtKind::Let { pattern, .. } => self.pattern_names(*pattern),
            _ => vec![],
        }
    }
    fn pattern_names(&self, id: PatternId) -> Vec<Symbol> {
        match &self.file.tree.patterns[id.0 as usize].kind {
            PatternKind::Binding(name) => vec![*name],
            PatternKind::Tuple(patterns) => patterns
                .iter()
                .flat_map(|p| self.pattern_names(*p))
                .collect(),
            PatternKind::Constructor { fields, .. } => fields
                .iter()
                .flat_map(|f| self.pattern_names(f.pattern))
                .collect(),
            _ => vec![],
        }
    }
    fn statement(&mut self, id: StmtId) {
        let statement = self.file.tree.statements[id.0 as usize].clone();
        match statement.kind {
            StmtKind::Let {
                pattern, ty, value, ..
            } => {
                let names = self.pattern_names(pattern);
                let declaring = std::mem::replace(&mut self.declaring, names);
                let expected = ty.map(|t| self.type_expr(t)).unwrap_or_default();
                self.declaring = declaring;
                let own = std::mem::take(&mut self.deferred);
                let value_type = self.expr(value, expected.clone());
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
            StmtKind::While { condition, body } => {
                self.expr(condition, ExpectedType::Unknown);
                self.expr(body, ExpectedType::Unknown);
            }
            StmtKind::For {
                pattern,
                source,
                body,
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
                let enclosing_contract = self.contract_kind.take();
                self.expr(body, result.clone());
                self.contract_kind = enclosing_contract;
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
        let node = self.file.tree.patterns[id.0 as usize].clone();
        match node.kind {
            PatternKind::Binding(name) => {
                if self
                    .variant(name, &expected, NameSite::Pattern(id), node.origin.span)
                    .is_none()
                {
                    let local = self.local(name, node.origin.span, expected);
                    self.output.query.value.bindings.push((id, name, local));
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
                    && let Some((variant, types)) = self.variant(
                        path.segments[0],
                        &expected,
                        NameSite::Pattern(id),
                        node.origin.span,
                    )
                {
                    field_types = types;
                    if let ExpectedType::Named(owner) = expected {
                        identity = Some((owner, Some(variant)));
                    }
                } else {
                    match self.lookup(&path, node.origin.span) {
                        Ok(owner) => {
                            if self.namespace(&owner) != Some(Namespace::Type) {
                                self.diagnostic(
                                    const { RuleId::of("LANG-4") },
                                    node.origin.span,
                                    "pattern constructor is not a type",
                                );
                            } else if self.representation_visible(&owner, node.origin.span) {
                                self.output.contextual.push((
                                    NameSite::Pattern(id),
                                    ContextualName::Constructor(owner.clone()),
                                ));
                                field_types = self
                                    .shape(&owner)
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
                                self.lookup_diagnostic(&e, node.origin.span);
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

fn semantic_expected(store: &ty::TypeStore, id: ty::TypeId, depth: usize) -> ExpectedType {
    if depth > store.len() {
        return ExpectedType::Unknown;
    }
    match store.get(id).map(|ty| &ty.kind) {
        Some(ty::TypeKind::Named { definition, .. }) => ExpectedType::Named(definition.clone()),
        Some(ty::TypeKind::Tuple(types)) => ExpectedType::Tuple(
            types
                .iter()
                .map(|t| semantic_expected(store, *t, depth + 1))
                .collect(),
        ),
        Some(ty::TypeKind::Function(f)) => ExpectedType::Function {
            parameters: f
                .parameters
                .iter()
                .map(|p| semantic_expected(store, p.ty, depth + 1))
                .collect(),
            result: Box::new(semantic_expected(store, f.result, depth + 1)),
        },
        Some(ty::TypeKind::Box(id)) => semantic_expected(store, *id, depth + 1),
        _ => ExpectedType::Unknown,
    }
}
fn shape_from_type(store: &ty::TypeStore, id: ty::TypeId) -> Shape {
    match store.get(id).map(|t| &t.kind) {
        Some(ty::TypeKind::Record(fields)) => Shape {
            fields: fields
                .iter()
                .map(|f| (f.name.clone(), semantic_expected(store, f.ty, 0)))
                .collect(),
            variants: vec![],
        },
        Some(ty::TypeKind::Variant(variants)) => Shape {
            fields: vec![],
            variants: variants
                .iter()
                .map(|v| {
                    (
                        v.name.clone(),
                        v.fields
                            .iter()
                            .map(|(n, t)| (n.clone(), semantic_expected(store, *t, 0)))
                            .collect(),
                    )
                })
                .collect(),
        },
        _ => Shape::default(),
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
        "String" => String,
        "Bytes" => Bytes,
        "Never" => Never,
        _ => return None,
    })
}

// Compiler-native type constructors, matching the semantic TypeKind vocabulary.
// Qualified library names remain ordinary lookups and never use this fallback.
fn native_type(path: &Path, symbols: &Interner) -> bool {
    path.package.is_none()
        && path.segments.len() == 1
        && matches!(
            symbols.resolve(path.segments[0]),
            Some(
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
        )
}

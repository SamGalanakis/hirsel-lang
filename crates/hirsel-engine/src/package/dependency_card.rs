//! Dependency facts from the same checked closure and prechecks used by a build.
use super::*;
use hirsel_syntax::{ItemKind, Visibility};
use hirsel_types::{
    AuthoritySummary, AuthorityTerm, GenericDeclarations, Signature, SignatureKind, TypeId,
    TypeKind, Uses,
};
use serde_json::json;

fn spelling(id: &PackageId) -> String {
    format!("{}@{}", id.name, id.version)
}

#[derive(Default)]
struct Authority {
    capabilities: BTreeSet<String>,
    operations: BTreeSet<String>,
    unknown: BTreeSet<String>,
}
impl Authority {
    fn summary(&mut self, summary: &AuthoritySummary, owner: &DefPath) {
        match summary {
            AuthoritySummary::Known(terms) => {
                for term in terms.iter() {
                    match term {
                        AuthorityTerm::Capability(path) => {
                            self.capabilities.insert(path.to_string());
                        }
                        _ => {
                            self.unknown.insert(owner.to_string());
                        }
                    }
                }
            }
            AuthoritySummary::Unknown => {
                self.unknown.insert(owner.to_string());
            }
        }
    }
    fn json(self) -> Value {
        json!({"capabilities": self.capabilities, "operations": self.operations,
            "unresolved": self.unknown})
    }
}

/// Expand a callable's symbolic uses through its parameter types, using the
/// compiler's held-authority algorithm, including generic and opaque summaries.
fn authority(
    engine: &mut QueryEngine,
    signature: &Signature,
    out: &mut Authority,
) -> Result<bool, QueryError> {
    let SignatureKind::Function(function) = &signature.kind else {
        return Ok(false);
    };
    let uses = match &function.uses {
        Uses::Only(summary) => summary.clone(),
        Uses::Unrestricted => {
            out.unknown.insert(signature.definition.to_string());
            // Unrestricted is not evidence of purity. Parameters still provide
            // useful positive evidence; the unresolved marker remains.
            AuthoritySummary::Known(
                (0..function.parameters.len())
                    .map(|i| {
                        AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                            hirsel_types::ParamId(i as u32),
                        ))
                    })
                    .collect::<Vec<_>>()
                    .into(),
            )
        }
    };
    let mut local = Authority::default();
    if let AuthoritySummary::Known(terms) = uses {
        for term in terms.iter() {
            match term {
                AuthorityTerm::Parameter(parameter) => {
                    if parameter.binder() != hirsel_types::GenericBinder::Bound(0) {
                        local.unknown.insert(signature.definition.to_string());
                        continue;
                    }
                    let Some(parameter) = function.parameters.get(parameter.parameter().0 as usize)
                    else {
                        local.unknown.insert(signature.definition.to_string());
                        continue;
                    };
                    let summary = hirsel_types::structural_authority(
                        &signature.types,
                        parameter.ty,
                        &signature.metadata.roles,
                        &GenericDeclarations::signature(&function.generics),
                        &mut |_, path| {
                            path.map(|p| engine.item_signature(p).map(|s| Some(s.value)))
                                .unwrap_or(Ok(None))
                        },
                    )?;
                    local.summary(&summary, &signature.definition);
                }
                _ => local.summary(
                    &AuthoritySummary::Known(vec![term.clone()].into()),
                    &signature.definition,
                ),
            }
        }
    } else {
        local.unknown.insert(signature.definition.to_string());
    }
    let effectful = !local.capabilities.is_empty();
    out.capabilities.extend(local.capabilities);
    out.unknown.extend(local.unknown);
    Ok(effectful)
}

#[derive(Clone)]
struct Declaration {
    file: FileId,
    public: bool,
    function: bool,
    test: bool,
    tokens: usize,
}

fn closure(
    roots: impl IntoIterator<Item = DefPath>,
    edges: &BTreeMap<DefPath, BTreeSet<DefPath>>,
) -> BTreeSet<DefPath> {
    let mut seen = BTreeSet::new();
    let mut pending: Vec<_> = roots.into_iter().collect();
    while let Some(path) = pending.pop() {
        if seen.insert(path.clone()) {
            pending.extend(edges.get(&path).into_iter().flatten().cloned());
        }
    }
    seen
}

fn facts(
    engine: &mut QueryEngine,
    paths: &BTreeSet<DefPath>,
    edges: &BTreeMap<DefPath, BTreeSet<DefPath>>,
) -> Result<(Value, Value), QueryError> {
    let mut auth = Authority::default();
    let mut externs = BTreeSet::new();
    let mut unsafe_operations = BTreeSet::new();
    let mut unmodeled = BTreeSet::new();
    let unsafe_set: BTreeSet<_> = hirsel_types::core_roles::intrinsic_roles()
        .into_iter()
        .filter(|(_, r)| *r == hirsel_types::core_roles::Intrinsic::UnsafeOperation)
        .map(|(p, _)| p)
        .collect();
    for path in paths {
        let signature = engine.item_signature(path)?.value;
        if authority(engine, &signature, &mut auth)? && path.package_is("core", "1") {
            auth.operations.insert(path.to_string());
        }
        if unsafe_set.contains(path) {
            unsafe_operations.insert(path.to_string());
        }
        if matches!(&signature.kind, SignatureKind::Function(f) if f.foreign.is_some()) {
            externs.insert(path.clone());
        }
    }
    for path in paths.iter().filter(|p| !externs.contains(*p)) {
        if edges
            .get(path)
            .into_iter()
            .flatten()
            .any(|p| externs.contains(p))
            && !engine
                .item_signature(path)?
                .value
                .metadata
                .wrapper
                .is_some_and(|w| w.deterministic)
        {
            unmodeled.insert(path.to_string());
        }
    }
    // Runtime provider implementations are selected by a run record, not by
    // source packages. These are the canonical capability contracts they need.
    let providers = auth.capabilities.clone();
    Ok((
        auth.json(),
        json!({"externs": externs.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "unsafe_operations": unsafe_operations, "unmodeled_foreign_callers": unmodeled,
        "replay": if unmodeled.is_empty() { "no_unmodeled_ffi" } else { "unreplayable" },
        "provider_requirements": providers}),
    ))
}

/// Only type rows rooted in a public signature count as exposure. In particular,
/// an opaque type's private representation and local body types are not API.
fn exposed_types(signature: &Signature) -> BTreeSet<DefPath> {
    let mut pending: Vec<TypeId> = match &signature.kind {
        SignatureKind::Function(f) => f
            .parameters
            .iter()
            .map(|p| p.ty)
            .chain([f.result])
            .chain(f.error)
            .collect(),
        SignatureKind::Type(t) if signature.metadata.visibility != Visibility::PublicOpaque => {
            t.representation.into_iter().collect()
        }
        SignatureKind::Const { ty, .. } => vec![*ty],
        _ => vec![],
    };
    let mut seen = BTreeSet::new();
    let mut paths = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = signature.types.get(id) else {
            continue;
        };
        match &row.kind {
            TypeKind::Named {
                definition,
                arguments,
            } => {
                paths.insert(definition.clone());
                pending.extend(arguments.iter().filter_map(|a| match a {
                    hirsel_types::GenericArgument::Type(t) => Some(*t),
                    _ => None,
                }));
            }
            TypeKind::Builtin { arguments, .. } => {
                pending.extend(arguments.iter().filter_map(|a| match a {
                    hirsel_types::GenericArgument::Type(t) => Some(*t),
                    _ => None,
                }))
            }
            TypeKind::Tuple(ts) => pending.extend(ts.iter().copied()),
            TypeKind::Record(fs) => pending.extend(fs.iter().map(|f| f.ty)),
            TypeKind::Variant(vs) => {
                pending.extend(vs.iter().flat_map(|v| v.fields.iter().map(|f| f.1)))
            }
            TypeKind::Box(t) => pending.push(*t),
            TypeKind::Function(f) => pending.extend(
                f.parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain([f.result])
                    .chain(f.error),
            ),
            _ => (),
        }
    }
    paths
}

/// Package-owned corpus cases live beside its manifest under `corpus/`.
/// External consumer/conformance suites are not this package's own tests.
fn corpus_cases(manifest: &Path) -> Result<usize, ToolError> {
    let directory = manifest.parent().unwrap().join("corpus");
    if !directory.exists() {
        return Ok(0);
    }
    let mut pending = vec![directory];
    let mut count = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                pending.push(entry.path());
            } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == "hir") {
                count += 1;
            }
        }
    }
    Ok(count)
}

impl Compiler {
    /// Cards computed with the normal shared package cache and prechecks.
    pub fn dependency_cards(
        &self,
        manifest: &Path,
        selection: Option<&str>,
    ) -> Result<Value, ToolError> {
        self.load_test_package(manifest)?
            .dependency_cards(manifest, selection)
    }
}

impl LoadedProgram {
    /// Version 1 dependency cards from this manifest-selected program. Keeps the
    /// engine available for optional code-store hash/provenance inspection.
    pub fn dependency_cards(
        &mut self,
        manifest: &Path,
        selection: Option<&str>,
    ) -> Result<Value, ToolError> {
        let mut graph = Graph::default();
        let root = graph.visit(manifest, None)?;
        if root != self.entry.0.package
            || graph
                .packages
                .values()
                .flat_map(|p| &p.sources)
                .any(|(path, context)| {
                    !self.files.iter().any(|(file, c)| {
                        c == context
                            && self
                                .engine
                                .sources
                                .file(*file)
                                .is_some_and(|s| s.path == *path)
                    })
                })
        {
            return Err(invalid(
                "dependency card manifest does not match the loaded program",
            ));
        }
        let mut selected = Vec::new();
        for id in graph.packages.keys() {
            let name = spelling(id);
            if let Some(filter) = selection {
                if filter == name {
                    selected.push((id.clone(), vec![]));
                } else if let Some(module) = filter.strip_prefix(&(name + "/")) {
                    let modules: Vec<String> = module.split('/').map(str::to_owned).collect();
                    if !module.is_empty()
                        && graph.packages[id]
                            .sources
                            .iter()
                            .any(|(_, c)| c.modules.starts_with(&modules))
                    {
                        selected.push((id.clone(), modules));
                    }
                }
            } else if *id != root {
                selected.push((id.clone(), vec![]));
            }
        }
        if selection.is_some() && selected.is_empty() {
            return Err(invalid(
                "--package must name a selected package or module, e.g. std@1/json",
            ));
        }
        let program = self;
        if let Some(entry) = &graph.packages[&root].entry {
            program.entry = entry.clone();
        }
        let checked = program
            .check(LoweringPolicy::Reachable)
            .map_err(|e| program.error(e))?;
        if checked
            .diagnostics
            .iter()
            .any(|d| d.severity == hirsel_diag::Severity::Error)
        {
            return Err(program.error(QueryError::Diagnostics(checked.diagnostics)));
        }
        if checked.completion != CheckCompletion::Complete {
            return Err(program.error(QueryError::NotImplemented));
        }
        let mut declarations = BTreeMap::new();
        let mut explicit_exposures: BTreeMap<DefPath, BTreeSet<String>> = BTreeMap::new();
        for (file, context) in program.files.clone() {
            let parsed = program
                .engine
                .parse_file(file, &context)
                .map_err(|e| program.error(e))?
                .value;
            let viewer = FileContext {
                package: PackageId {
                    name: "$card".into(),
                    version: "0".into(),
                },
                modules: vec![],
            };
            let index = program
                .engine
                .declarations
                .frozen()
                .index(context.package.clone());
            for syntax in &parsed.items {
                // One item's spans share its anchor, so they compare directly.
                let (item, tree) = (&syntax.item, &syntax.tree);
                if context.package == root
                    && index.access(&viewer, &item.definition).is_ok()
                    && !item.attributes.iter().any(|a| a.name.as_str() == "test")
                {
                    let end = match &item.kind {
                        ItemKind::Function {
                            body: Some(body), ..
                        } => tree.expressions[body.0 as usize].origin.span.start,
                        ItemKind::Type { .. } if item.visibility == Visibility::PublicOpaque => {
                            item.name_span.end
                        }
                        ItemKind::Const { value, .. } => {
                            tree.expressions[value.0 as usize].origin.span.start
                        }
                        _ => item.origin.span.end,
                    };
                    for ty in &tree.types {
                        if ty.origin.span.start < item.origin.span.start || ty.origin.span.end > end
                        {
                            continue;
                        }
                        if let hirsel_syntax::TypeExprKind::Named { path, .. } = &ty.kind
                            && let Ok(path) = index.lookup(&context, path)
                        {
                            explicit_exposures
                                .entry(path)
                                .or_default()
                                .insert(item.definition.to_string());
                        }
                    }
                }
                declarations.insert(
                    item.definition.clone(),
                    Declaration {
                        file,
                        public: index.access(&viewer, &item.definition).is_ok(),
                        function: matches!(item.kind, ItemKind::Function { .. }),
                        test: item.attributes.iter().any(|a| a.name.as_str() == "test"),
                        tokens: tree
                            .tokens
                            .get(item.origin.tokens.start as usize..item.origin.tokens.end as usize)
                            .unwrap_or_default()
                            .iter()
                            .filter(|t| {
                                !matches!(
                                    t.kind,
                                    hirsel_syntax::TokenKind::Trivia(_)
                                        | hirsel_syntax::TokenKind::Eof
                                )
                            })
                            .count(),
                    },
                );
            }
        }
        let roots: Vec<_> = if graph.packages[&root].entry.is_some() {
            vec![program.entry.clone()]
        } else {
            declarations
                .iter()
                .filter(|(p, d)| p.0.package == root && d.public && !d.test)
                .map(|(p, _)| p.clone())
                .collect()
        };
        let reach = program
            .engine
            .semantic_program(roots.clone())
            .map_err(|e| program.error(e))?;
        let mut exposures = explicit_exposures;
        for (path, declaration) in &declarations {
            if path.0.package == root && declaration.public && !declaration.test {
                let signature = program
                    .engine
                    .item_signature(path)
                    .map_err(|e| program.error(e))?
                    .value;
                for ty in exposed_types(&signature) {
                    exposures.entry(ty).or_default().insert(path.to_string());
                }
            }
        }
        let mut cards = Vec::new();
        for (id, modules) in selected {
            let belongs =
                |path: &DefPath| path.0.package == id && path.0.modules.starts_with(&modules);
            let own: BTreeSet<_> = declarations
                .keys()
                .filter(|p| belongs(p))
                .cloned()
                .collect();
            let api: Vec<_> = own
                .iter()
                .filter(|p| {
                    let d = &declarations[*p];
                    d.public && d.function && !d.test
                })
                .cloned()
                .collect();
            let whole = program
                .engine
                .semantic_program(api.clone())
                .map_err(|e| program.error(e))?;
            let reached: BTreeSet<_> = own.intersection(&reach.definitions).cloned().collect();
            // Attribute authority to this dependency's reached definitions, not
            // unrelated effects elsewhere in the importing program.
            let reached_closure = closure(reached.iter().cloned(), &reach.uses);
            let (whole_authority, whole_safety) =
                facts(&mut program.engine, &whole.definitions, &whole.uses)
                    .map_err(|e| program.error(e))?;
            let (reached_authority, reached_safety) =
                facts(&mut program.engine, &reached_closure, &reach.uses)
                    .map_err(|e| program.error(e))?;
            let units = |paths: &BTreeSet<DefPath>| {
                paths
                    .iter()
                    .map(|p| declarations[p].file)
                    .collect::<BTreeSet<_>>()
                    .len()
            };
            let token_count: usize = reached.iter().map(|p| declarations[p].tokens).sum();
            let prechecked = program.engine.prechecked(&id);
            let exposure: Vec<_> = exposures
                .iter()
                .filter(|(p, _)| belongs(p))
                .map(|(p, users)| json!({"type":p.to_string(), "signatures":users}))
                .collect();
            let dependencies: Vec<_> = graph
                .closure(&id)
                .iter()
                .filter(|p| **p != id)
                .map(spelling)
                .collect();
            let name = if modules.is_empty() {
                spelling(&id)
            } else {
                format!("{}/{}", spelling(&id), modules.join("/"))
            };
            cards.push(json!({"package":name, "public_functions":api.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "authority":{"whole_public_api":whole_authority,"reached_by_you":reached_authority},
                "weight":{"definitions":{"total":own.len(),"reached":reached.len()},
                    "units":{"total": graph.packages[&id].sources.iter().filter(|(_,c)| c.modules.starts_with(&modules)).count(), "reached":units(&reached)},
                    "transitive_dependencies":dependencies,
                    "cost":{"basis":"source_token_work_estimate", "reached_source_tokens":token_count,
                        "check_source_tokens":if prechecked {0} else {token_count}, "lower_source_tokens":if prechecked {0} else {token_count}, "native_build_source_tokens":token_count,
                        "prechecked_definitions":if prechecked {reached.len()} else {0},
                        "check_ms":Value::Null,"build_ms":Value::Null}},
                "exposure":exposure,
                "determinism":{"whole_public_api":whole_safety,"reached_by_you":reached_safety,
                    "unsafe_ffi_package":graph.packages[&id].unsafe_ffi.is_some()},
                "health":{"tests":own.iter().filter(|p| declarations[*p].test).count(),
                    "corpus_cases":corpus_cases(&graph.packages[&id].path)?,"last_gate":Value::Null,"provenance":Value::Null,
                    "unavailable":{"last_gate":"no content-bound package test receipt in the build cache",
                        "provenance":"source manifests do not select a code store"}}}));
        }
        Ok(json!({"schema_version":1,"root_package":spelling(&root),
            "reach_basis":if graph.packages[&root].entry.is_some() {"entry"} else {"public_api"},
            "roots":roots.iter().map(ToString::to_string).collect::<Vec<_>>(),"cards":cards}))
    }
}

/// A compact text projection of the versioned machine-readable card.
pub fn render_dependency_cards(report: &Value) -> String {
    fn list(value: &Value) -> String {
        let items: Vec<_> = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if items.is_empty() {
            "none".into()
        } else {
            items.join(", ")
        }
    }
    let mut out = format!(
        "Dependency cards for {} (reach: {})\n",
        report["root_package"].as_str().unwrap(),
        report["reach_basis"].as_str().unwrap()
    );
    for card in report["cards"].as_array().unwrap() {
        out.push_str(&format!("\n{}\n", card["package"].as_str().unwrap()));
        for (key, label) in [
            ("whole_public_api", "whole public API"),
            ("reached_by_you", "reached by you"),
        ] {
            let a = &card["authority"][key];
            out.push_str(&format!(
                "  Authority ({label}): {}\n    operations: {}\n",
                list(&a["capabilities"]),
                list(&a["operations"])
            ));
            if !a["unresolved"].as_array().unwrap().is_empty() {
                out.push_str(&format!(
                    "    unresolved authority: {}\n",
                    list(&a["unresolved"])
                ));
            }
        }
        let w = &card["weight"];
        out.push_str(&format!("  Weight: {} / {} definitions, {} / {} source units reached\n    dependencies: {}\n    cost estimate: {} source tokens; check {}, lower {}, native build {}; {} prechecked definitions (timings unavailable)\n",
            w["definitions"]["reached"], w["definitions"]["total"], w["units"]["reached"], w["units"]["total"], list(&w["transitive_dependencies"]),
            w["cost"]["reached_source_tokens"], w["cost"]["check_source_tokens"], w["cost"]["lower_source_tokens"], w["cost"]["native_build_source_tokens"], w["cost"]["prechecked_definitions"]));
        let exposed: Vec<_> = card["exposure"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                format!(
                    "{} in {}",
                    e["type"].as_str().unwrap(),
                    list(&e["signatures"])
                )
            })
            .collect();
        out.push_str(&format!(
            "  Exposure: {}\n",
            if exposed.is_empty() {
                "none".into()
            } else {
                exposed.join("; ")
            }
        ));
        for (key, label) in [
            ("whole_public_api", "whole public API"),
            ("reached_by_you", "reached by you"),
        ] {
            let d = &card["determinism"][key];
            out.push_str(&format!("  Replay ({label}): {}; externs: {}; Unsafe operations: {}\n    provider capability contracts: {} (implementations selected by run record)\n",
                d["replay"].as_str().unwrap(), list(&d["externs"]), list(&d["unsafe_operations"]), list(&d["provider_requirements"])));
        }
        out.push_str(&format!(
            "  Health: {} own tests; {} corpus cases; last gate unavailable\n",
            card["health"]["tests"], card["health"]["corpus_cases"]
        ));
        let provenance = &card["health"]["provenance"];
        if provenance.is_null() {
            out.push_str("    code-store provenance: unavailable (no store selected)\n");
        } else {
            out.push_str(&format!(
                "    code store: {}; signature policy: {}; {} objects inspected\n",
                provenance["provider"].as_str().unwrap(),
                provenance["signature_policy"].as_str().unwrap(),
                provenance["objects"].as_array().unwrap().len()
            ));
        }
    }
    out
}

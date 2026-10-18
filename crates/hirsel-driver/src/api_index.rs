//! Documentation from public declarations, without checking or executing bodies.
use hirsel_engine::{DefinitionQueries, PackageDeclarations, ToolError, registry};
use hirsel_span::SourceMap;
use hirsel_syntax::{self as ast, DefPath, FileContext, ItemKind, TokenKind, Visibility};
use hirsel_types::*;
use std::{collections::BTreeMap, path::Path};

/// One public definition or constructor. Paths retain package identity for coverage.
#[derive(Debug)]
pub struct Entry {
    pub identity: String,
    pub name: String,
    pub declaration: String,
    pub description: String,
    pub links: Vec<Link>,
}

/// A resolved definition used by a public declaration.
#[derive(Debug)]
pub struct Link {
    pub name: String,
    pub identity: String,
    pub url: String,
}

/// Injective, HTML-safe encoding of the versioned definition path.
pub fn anchor(identity: &str) -> String {
    let mut out = String::from("def-");
    for byte in identity.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("_{byte:02x}"));
        }
    }
    out
}

fn short(path: &DefPath) -> String {
    path.0
        .modules
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(path.0.name.as_str()))
        .collect::<Vec<_>>()
        .join(".")
}
fn sentence(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = text
        .char_indices()
        .find_map(|(i, c)| {
            (matches!(c, '.' | '!' | '?') && text[i + c.len_utf8()..].starts_with(' '))
                .then_some(i + c.len_utf8())
        })
        .unwrap_or(text.len());
    text[..end].to_owned()
}
fn query_error(error: hirsel_engine::QueryError, sources: &SourceMap) -> ToolError {
    ToolError::Query {
        error,
        phase: None,
        sources: sources.clone(),
    }
}

/// Read a selected package's public declarations. std@1 also publishes core.
/// No signature is inferred from a body, and no body is checked or evaluated.
pub fn entries(manifest: &Path) -> Result<Vec<Entry>, ToolError> {
    let mut package = crate::COMPILER.load_package_declarations(manifest)?;
    let mut out = Vec::new();
    let std = package.package.package_is("std", "1");
    let repo = manifest
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent);
    let catalogue = |name: &str| {
        repo.and_then(|p| std::fs::read_to_string(p.join("docs/spec").join(name)).ok())
            .unwrap_or_default()
    };
    let std_docs = if std {
        catalogue("std-library.md")
    } else {
        String::new()
    };
    if std {
        out.extend(core_entries(&catalogue("core-library.md")));
    }
    source_entries(&mut package, &std_docs, &mut out)?;
    out.sort_by(|a, b| {
        module(&a.identity)
            .cmp(&module(&b.identity))
            .then(a.name.cmp(&b.name))
            .then(a.identity.cmp(&b.identity))
    });
    Ok(out)
}

fn core_entries(core_docs: &str) -> Vec<Entry> {
    let registry = registry::BuiltinRegistry::new();
    let ledger: BTreeMap<_, _> = registry::SECTION22
        .lines()
        .filter_map(|line| {
            let declaration = line.strip_prefix("N ").unwrap_or(line);
            if !declaration.starts_with("fn ") && !declaration.starts_with('@') {
                return None;
            }
            let name = declaration.split_once("fn ")?.1.split(['[', '(']).next()?;
            Some((name, line.replacen("N fn ", "@noalloc fn ", 1)))
        })
        .collect();
    let mut out = Vec::new();
    for s in registry.signatures().filter(|s| {
        s.metadata.visibility != Visibility::Private
            && !s.definition.0.modules.iter().any(|m| m.starts_with('$'))
    }) {
        let name = short(&s.definition);
        let printer = CorePrinter {
            signature: s,
            generics: s.generics(),
            parameters: Vec::new(),
        };
        let declaration = match &s.kind {
            SignatureKind::Function(f) => ledger.get(name.as_str()).cloned().unwrap_or_else(|| {
                format!(
                    "{}fn {name}{}{}",
                    if f.allocation == Allocation::NoAlloc {
                        "@noalloc "
                    } else {
                        ""
                    },
                    printer.generics(),
                    printer.function(f)
                )
            }),
            SignatureKind::Type(t) => {
                let representation = if s.metadata.visibility == Visibility::PublicOpaque {
                    None
                } else {
                    t.representation
                        .filter(|id| match &s.types.get(*id).unwrap().kind {
                            TypeKind::Builtin { kind, .. } => format!("{kind:?}") != name,
                            TypeKind::Primitive(_) => false,
                            _ => true,
                        })
                };
                let capability = match t.capability {
                    Some(CapabilityKind::Global) => "@capability(global) ",
                    Some(CapabilityKind::Scoped) => "@capability(local) ",
                    None => "",
                };
                let bounds = if t.bounds.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        t.bounds
                            .iter()
                            .map(bound_name)
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )
                };
                let authority = match &t.authority {
                    AuthoritySummary::Unknown => String::new(),
                    AuthoritySummary::Known(terms) => format!(
                        " uses {{{}}}",
                        terms
                            .iter()
                            .map(|term| match term {
                                AuthorityTerm::Capability(path) => short(path),
                                AuthorityTerm::Parameter(_) =>
                                    unreachable!("type authority cannot name a value parameter"),
                                AuthorityTerm::TypeParameter(p) =>
                                    printer.generic_name(p.parameter().0 as usize).into(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                format!(
                    "{capability}{}{}type {name}{}{bounds}{authority}{}",
                    if s.metadata.visibility == Visibility::PublicOpaque {
                        "opaque "
                    } else {
                        ""
                    },
                    if t.resource { "resource " } else { "" },
                    printer.generics(),
                    representation
                        .map(|id| format!(" = {}", printer.ty(id)))
                        .unwrap_or_default()
                )
            }
            SignatureKind::Const(id) => format!("const {name}: {}", printer.ty(*id)),
        };
        out.push(Entry {
            identity: s.definition.to_string(),
            description: catalogue_sentence(core_docs, &name),
            links: Vec::new(),
            name: name.clone(),
            declaration,
        });
        if let SignatureKind::Type(t) = &s.kind {
            if s.metadata.visibility != Visibility::PublicOpaque {
                if let Some(Type {
                    kind: TypeKind::Variant(variants),
                    ..
                }) = t.representation.and_then(|id| s.types.get(id))
                {
                    for variant in variants.iter() {
                        out.push(Entry {
                            identity: format!("{}#{}", s.definition, variant.name),
                            name: format!("{name}.{}", variant.name),
                            declaration: format!(
                                "{}{} -> {name}{}",
                                variant.name,
                                printer.payload(variant),
                                printer.generic_arguments()
                            ),
                            description: format!("Variant of {name}."),
                            links: Vec::new(),
                        });
                    }
                }
            }
        }
    }
    out
}

fn source_entries(
    package: &mut PackageDeclarations,
    catalogue: &str,
    out: &mut Vec<Entry>,
) -> Result<(), ToolError> {
    for (file, context) in package.files.clone() {
        if context.package != package.package {
            continue;
        }
        let engine = &mut package.engine;
        let parsed = engine
            .parse_file(file, &context)
            .map_err(|e| query_error(e, &engine.sources))?;
        if !parsed.diagnostics.is_empty() {
            return Err(query_error(
                hirsel_engine::QueryError::Diagnostics(parsed.diagnostics),
                &engine.sources,
            ));
        }
        let syntax = parsed.value;
        let engine = &package.engine;
        for id in &syntax.roots {
            let item = &syntax.tree.items[id.0 as usize];
            if !package.is_exported(&item.definition) {
                continue;
            }
            let name = short(&item.definition);
            let mut end = item.origin.span.end;
            match &item.kind {
                ItemKind::Function {
                    body: Some(body), ..
                } => end = syntax.tree.expressions[body.0 as usize].origin.span.start,
                ItemKind::Const { value, .. } => {
                    end = syntax.tree.expressions[value.0 as usize].origin.span.start;
                    // Exclude the initializer's equals sign as well.
                    if let Some(token) = syntax.tokens.iter().rev().find(|t| {
                        t.span.end <= end
                            && t.span.start >= item.name_span.end
                            && t.kind == TokenKind::Punct(ast::Punct::Eq)
                    }) {
                        end = token.span.start;
                    }
                }
                ItemKind::Type {
                    drop: Some(drop), ..
                } => {
                    end = syntax.tree.expressions[drop.body.0 as usize]
                        .origin
                        .span
                        .start
                }
                _ => {}
            }
            if item.visibility == Visibility::PublicOpaque {
                let mut depth = 0usize;
                let mut uses = false;
                for token in &syntax.tokens {
                    if token.span.start < item.name_span.end || token.span.start >= end {
                        continue;
                    }
                    match token.kind {
                        TokenKind::Keyword(ast::Keyword::Uses) if depth == 0 => uses = true,
                        TokenKind::Punct(ast::Punct::LBrace) if depth == 0 && !uses => {
                            end = token.span.start;
                            break;
                        }
                        TokenKind::Punct(
                            ast::Punct::LBrace | ast::Punct::LBracket | ast::Punct::LParen,
                        ) => depth += 1,
                        TokenKind::Punct(
                            ast::Punct::RBrace | ast::Punct::RBracket | ast::Punct::RParen,
                        ) => {
                            depth -= 1;
                            if depth == 0 {
                                uses = false;
                            }
                        }
                        TokenKind::Punct(ast::Punct::Eq) if depth == 0 => {
                            end = token.span.start;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            let declaration = source_declaration(&syntax, item, end, &engine.sources, &name);
            let comments = source_comments(&syntax, item, &engine.sources);
            let description = if comments.is_empty() {
                catalogue_sentence(catalogue, &name)
            } else {
                comments
            };
            let links = source_links(
                package,
                &context,
                &syntax,
                item,
                item.origin.span.start,
                end,
            );
            out.push(Entry {
                identity: item.definition.to_string(),
                name: name.clone(),
                declaration,
                description,
                links,
            });
            if let ItemKind::Type {
                body: ast::TypeBody::Variants(variants),
                generics,
                ..
            } = &item.kind
            {
                if item.visibility == Visibility::PublicOpaque {
                    continue;
                }
                let args = if generics.is_empty() {
                    String::new()
                } else {
                    format!(
                        "[{}]",
                        generics
                            .iter()
                            .map(|g| syntax.symbols.resolve(g.name).unwrap())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let spans = syntax.variant_name_spans(item.origin);
                for (i, variant) in variants.iter().enumerate() {
                    let variant_name = syntax.symbols.resolve(variant.name).unwrap();
                    let start = spans[i].start;
                    let end = spans.get(i + 1).map_or(end, |s| s.start);
                    let text = token_text(&syntax, start, end, &engine.sources)
                        .trim_end_matches([' ', '|'])
                        .to_owned();
                    out.push(Entry {
                        identity: format!("{}#{variant_name}", item.definition),
                        name: format!("{name}.{variant_name}"),
                        declaration: format!("{text} -> {name}{args}"),
                        description: format!("Variant of {name}."),
                        links: source_links(package, &context, &syntax, item, start, end),
                    });
                }
            }
        }
    }
    Ok(())
}

fn source_links(
    package: &PackageDeclarations,
    context: &FileContext,
    syntax: &ast::SyntaxFile,
    item: &ast::Item,
    start: u32,
    end: u32,
) -> Vec<Link> {
    // Only AST nodes in the published header are considered. In particular,
    // neither opaque representations nor types inside function bodies leak links.
    let paths = syntax.tree.types.iter().filter_map(|ty| {
        if ty.origin.span.start < start || ty.origin.span.end > end {
            return None;
        }
        match &ty.kind {
            ast::TypeExprKind::Named { path, .. } => Some(path),
            _ => None,
        }
    });
    let attributes = item
        .attributes
        .iter()
        .filter(|a| a.origin.span.start >= start && a.origin.span.end <= end)
        .flat_map(|a| &a.arguments)
        .filter_map(|a| match a {
            ast::AttributeArgument::Format(f) => Some(&f.function),
            _ => None,
        });
    let uses = match &item.kind {
        ItemKind::Function { signature, .. } => signature.uses.as_deref().unwrap_or_default(),
        ItemKind::Type { authority, .. } => authority.as_deref().unwrap_or_default(),
        _ => &[],
    };
    let generics = match &item.kind {
        ItemKind::Function { signature, .. } => signature.generics.as_slice(),
        ItemKind::Type { generics, .. } => generics.as_slice(),
        _ => &[],
    };
    let parameters = match &item.kind {
        ItemKind::Function { signature, .. } => signature.parameters.as_slice(),
        _ => &[],
    };
    let contracts = syntax.tree.expressions.iter().filter_map(|expr| {
        if expr.origin.span.start < start || expr.origin.span.end > end {
            return None;
        }
        match &expr.kind {
            ast::ExprKind::Path(path) => Some(path),
            _ => None,
        }
    });
    let mut links = BTreeMap::new();
    for path in paths.chain(attributes).chain(uses).chain(contracts) {
        if path.package.is_none()
            && path.segments.first().is_some_and(|head| {
                generics.iter().any(|g| g.name == *head)
                    || parameters.iter().any(|p| p.name == Some(*head))
            })
        {
            continue;
        }
        let Ok(definition) = package.lookup(context, path, &syntax.symbols) else {
            continue;
        };
        if definition == item.definition || !package.is_exported(&definition) {
            continue;
        }
        let identity = definition.to_string();
        let target = anchor(&identity);
        let url = if definition.0.package == package.package
            || (package.package.package_is("std", "1")
                && definition.0.package.package_is("core", "1"))
        {
            format!("#{target}")
        } else if definition.0.package.package_is("core", "1")
            || definition.0.package.package_is("std", "1")
        {
            format!("https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#{target}")
        } else {
            format!(
                "../{}@{}/api.md#{target}",
                definition.0.package.name, definition.0.package.version
            )
        };
        links.insert(
            identity.clone(),
            Link {
                name: short(&definition),
                identity,
                url,
            },
        );
    }
    links.into_values().collect()
}

fn token_text(syntax: &ast::SyntaxFile, start: u32, end: u32, sources: &SourceMap) -> String {
    let mut text = String::new();
    let mut space = false;
    for token in &syntax.tokens {
        if token.span.start < start || token.span.end > end {
            continue;
        }
        match token.kind {
            TokenKind::Trivia(_) => space = !text.is_empty(),
            TokenKind::Eof => {}
            _ => {
                if space {
                    text.push(' ');
                }
                text.push_str(sources.slice(token.span).unwrap());
                space = false;
            }
        }
    }
    text
}
fn source_declaration(
    syntax: &ast::SyntaxFile,
    item: &ast::Item,
    end: u32,
    sources: &SourceMap,
    name: &str,
) -> String {
    let before = token_text(
        syntax,
        item.origin.span.start,
        item.name_span.start,
        sources,
    );
    let after = token_text(syntax, item.name_span.end, end, sources);
    let after = if after.starts_with(['[', '(', ':']) {
        after
    } else {
        format!(" {after}")
    };
    format!("{before} {name}{after}").trim().to_owned()
}
fn source_comments(syntax: &ast::SyntaxFile, item: &ast::Item, sources: &SourceMap) -> String {
    let mut comments = Vec::new();
    // Owned trivia contains only the comments attached to this declaration.
    for token in &syntax.tokens[item.owned_tokens.start as usize..item.owned_tokens.end as usize] {
        if token.span.start >= item.name_span.start {
            break;
        }
        let raw = sources.slice(token.span).unwrap();
        match token.kind {
            TokenKind::Trivia(ast::TriviaKind::DocLine) => {
                let line = raw.strip_prefix("///").unwrap();
                comments.push(line.strip_prefix(' ').unwrap_or(line).trim_end());
            }
            TokenKind::Trivia(ast::TriviaKind::DocBlock) => {
                // /**/ shares the prefix's star with the closing delimiter.
                let inner = &raw[2..raw.len() - 2];
                comments.push(inner.strip_prefix('*').unwrap_or(inner).trim_matches('\n'));
            }
            _ => {}
        }
    }
    comments.join("\n")
}
fn catalogue_sentence(docs: &str, name: &str) -> String {
    docs.lines()
        .filter(|line| line.starts_with('|'))
        .find_map(|line| {
            let cells: Vec<_> = line.split('|').map(str::trim).collect();
            (cells.get(1)?.starts_with(&format!("`{name}(")))
                .then(|| sentence(cells[cells.len() - 2]))
        })
        .unwrap_or_default()
}

fn bound_name(bound: &Bound) -> String {
    if *bound == Bound::NoallocDrop {
        "noalloc_drop".into()
    } else {
        format!("{bound:?}").to_lowercase()
    }
}

fn module(identity: &str) -> String {
    identity
        .split_once('/')
        .and_then(|(_, path)| path.rsplit_once('/'))
        .map_or_else(String::new, |(modules, _)| modules.replace('/', "."))
}

/// One renderer for std and user packages; signatures remain verbatim token text.
pub fn render(entries: &[Entry]) -> String {
    let definitions = entries.iter().filter(|e| !e.identity.contains('#')).count();
    let mut page = format!(
        "# Public API\n\nGenerated from manifest-selected public declarations. Do not edit by hand.\nSignatures retain modes, brands, attributes, relations and `uses` clauses.\nOpaque representations stay hidden; bodies are not checked or evaluated.\n\n{definitions} exported definitions; {} variants.\n",
        entries.len() - definitions
    );
    let mut head = String::new();
    for entry in entries {
        let module = module(&entry.identity);
        let module = if module.is_empty() {
            "Types and roots"
        } else {
            &module
        };
        if module != head {
            page += &format!("\n## {module}\n\n");
            head = module.into();
        }
        page += &format!(
            "<a id=\"{}\"></a>\n\n- {}\n",
            anchor(&entry.identity),
            code_span(&entry.declaration)
        );
        if !entry.description.is_empty() {
            page += &format!("\n{}\n", entry.description);
        }
        if !entry.links.is_empty() {
            page += "\nReferences: ";
            page += &entry
                .links
                .iter()
                .map(|l| format!("[`{}`]({})", l.identity, l.url))
                .collect::<Vec<_>>()
                .join(", ");
            page += "\n";
        }
        page += "\n";
    }
    page.truncate(page.trim_end().len());
    page.push('\n');
    page
}

fn code_span(text: &str) -> String {
    let ticks = text.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1;
    let delimiter = "`".repeat(ticks);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{delimiter}{pad}{text}{pad}{delimiter}")
}

/// Versioned, deterministic JSON; anchors and links agree with Markdown.
pub fn render_json(entries: &[Entry]) -> String {
    let entries: Vec<_> = entries.iter().map(|e| serde_json::json!({
        "identity": e.identity, "anchor": anchor(&e.identity), "name": e.name,
        "declaration": e.declaration, "description": e.description,
        "links": e.links.iter().map(|l| serde_json::json!({"name": l.name, "identity": l.identity, "url": l.url})).collect::<Vec<_>>()
    })).collect();
    format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({"schema": 1, "entries": entries}))
            .unwrap()
    )
}

struct CorePrinter<'a> {
    signature: &'a Signature,
    generics: &'a [GenericParameter],
    /// The parameter names of the function being printed, which brands name (PROV-1).
    parameters: Vec<String>,
}
impl CorePrinter<'_> {
    fn generic_name(&self, n: usize) -> &str {
        match &self.generics[n] {
            GenericParameter::Type { name, .. } | GenericParameter::Size { name } => name,
        }
    }
    fn generic_arguments(&self) -> String {
        if self.generics.is_empty() {
            String::new()
        } else {
            format!(
                "[{}]",
                (0..self.generics.len())
                    .map(|n| self.generic_name(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
    fn generics(&self) -> String {
        if self.generics.is_empty() {
            return String::new();
        }
        format!(
            "[{}]",
            self.generics
                .iter()
                .map(|g| match g {
                    GenericParameter::Size { name } => format!("{name}: size"),
                    GenericParameter::Type { name, bounds } =>
                        if bounds.is_empty() {
                            name.clone()
                        } else {
                            format!(
                                "{name}: {}",
                                bounds
                                    .iter()
                                    .map(bound_name)
                                    .collect::<Vec<_>>()
                                    .join(" + ")
                            )
                        },
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
    fn payload(&self, variant: &Variant) -> String {
        format!(
            "({})",
            variant
                .fields
                .iter()
                .map(|(name, id)| format!(
                    "{}{}",
                    name.as_ref().map(|n| format!("{n}: ")).unwrap_or_default(),
                    self.ty(*id)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
    fn function(&self, f: &FunctionType) -> String {
        let nested = CorePrinter {
            signature: self.signature,
            generics: if f.generics.is_empty() {
                self.generics
            } else {
                &f.generics
            },
            parameters: f
                .parameters
                .iter()
                .map(|p| p.name.clone().unwrap_or_default())
                .collect(),
        };
        format!(
            "({}) -> {}{}",
            f.parameters
                .iter()
                .map(|p| {
                    let mode = match p.passing {
                        PassingMode::Take => "take ",
                        PassingMode::Mut => "mut ",
                        _ => "",
                    };
                    format!(
                        "{mode}{}{}",
                        p.name
                            .as_ref()
                            .map(|name| format!("{name}: "))
                            .unwrap_or_default(),
                        nested.ty(p.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            nested.ty(f.result),
            if f.uses == Uses::Only(AuthoritySummary::inert()) {
                " uses {}"
            } else {
                ""
            }
        )
    }
    fn ty(&self, id: TypeId) -> String {
        self.branded(id, &[])
    }
    /// The type with its brand, `siblings` naming the fields of the record it is a field of
    /// (PROV-5).
    fn branded(&self, id: TypeId, siblings: &[String]) -> String {
        let t = self.signature.types.get(id).unwrap();
        let base = self.unbranded(t);
        if t.brand.alternatives.is_empty() {
            return base;
        }
        let origins = t
            .brand
            .alternatives
            .iter()
            .map(|origin| {
                let root = match &origin.root {
                    OriginRoot::Sibling(field) => {
                        siblings.get(field.0 as usize).map(|n| format!(".{n}"))
                    }
                    OriginRoot::Parameter(ParameterRef::Bound {
                        depth: 0,
                        parameter,
                    }) => self
                        .parameters
                        .get(parameter.0 as usize)
                        .filter(|n| !n.is_empty())
                        .cloned(),
                    _ => None,
                };
                match root {
                    Some(root) if origin.fields.is_empty() => root,
                    _ => panic!("a core brand origin with no spelling: {origin:?}"),
                }
            })
            .collect::<Vec<_>>()
            .join(" | ");
        format!("{base} of {origins}")
    }
    fn unbranded(&self, t: &Type) -> String {
        match &t.kind {
            TypeKind::Primitive(p) => match p {
                Primitive::String | Primitive::Bytes | Primitive::Never => format!("{p:?}"),
                _ => format!("{p:?}").to_lowercase(),
            },
            TypeKind::Parameter(p) => self.generic_name(p.parameter().0 as usize).into(),
            TypeKind::Tuple(ids) => format!(
                "({}{})",
                ids.iter()
                    .map(|id| self.ty(*id))
                    .collect::<Vec<_>>()
                    .join(", "),
                if ids.len() == 1 { "," } else { "" }
            ),
            TypeKind::Named {
                definition,
                arguments,
            } => self.applied(&short(definition), arguments),
            TypeKind::Builtin { kind, arguments } => self.applied(&format!("{kind:?}"), arguments),
            TypeKind::Box(id) => format!("box {}", self.ty(*id)),
            TypeKind::Record(fields) => {
                let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                format!(
                    "{{ {} }}",
                    fields
                        .iter()
                        .map(|f| format!(
                            "{}{}: {}",
                            if f.growable { "growable " } else { "" },
                            f.name,
                            self.branded(f.ty, &names)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            TypeKind::Variant(variants) => variants
                .iter()
                .map(|v| format!("{}{}", v.name, self.payload(v)))
                .collect::<Vec<_>>()
                .join(" | "),
            TypeKind::Function(f) => format!(
                "{}fn{}",
                if f.allocation == Allocation::NoAlloc {
                    "@noalloc "
                } else {
                    ""
                },
                self.function(f)
            ),
            TypeKind::Error => panic!("recovery type in core declaration"),
        }
    }
    fn applied(&self, name: &str, arguments: &[GenericArgument]) -> String {
        if arguments.is_empty() {
            return name.into();
        }
        format!(
            "{name}[{}]",
            arguments
                .iter()
                .map(|arg| match arg {
                    GenericArgument::Type(id) => self.ty(*id),
                    GenericArgument::Size(Size::Constant(n)) => n.to_string(),
                    GenericArgument::Size(Size::Parameter(p)) =>
                        self.generic_name(p.parameter().0 as usize).into(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

//! Documentation from public declarations, without checking or executing bodies.
use hirsel_engine::{DefinitionQueries, QueryEngine, ToolError, default_environment, registry};
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

/// Read exactly the sources selected by std's manifest, using the compiler parser.
/// The root is a Hirsel repository; it is explicit so installed tools can select a checkout.
pub fn entries(root: &Path) -> Result<Vec<Entry>, ToolError> {
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
    let core_docs = std::fs::read_to_string(root.join("docs/spec/core-library.md"))?;
    let std_docs = std::fs::read_to_string(root.join("docs/spec/std-library.md"))?;
    let mut out = Vec::new();
    for s in registry.signatures().filter(|s| {
        s.metadata.visibility != Visibility::Private
            && !s.definition.0.modules.iter().any(|m| m.starts_with('$'))
    }) {
        let name = short(&s.definition);
        let printer = CorePrinter {
            signature: s,
            generics: s.generics(),
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
            description: catalogue_sentence(&core_docs, &name),
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
                        });
                    }
                }
            }
        }
    }
    let manifest_path = root.join("stdlib/std@1/hirsel.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)
            .map_err(|e| ToolError::Runtime(format!("std manifest: {e}")))?;
    let sources = manifest["sources"]
        .as_array()
        .ok_or_else(|| ToolError::Runtime("std manifest requires sources".into()))?;
    let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters());
    for source in sources {
        let path = source["path"]
            .as_str()
            .ok_or_else(|| ToolError::Runtime("std source requires path".into()))?;
        let modules = serde_json::from_value(source["module"].clone())
            .map_err(|e| ToolError::Runtime(format!("std module: {e}")))?;
        let path = manifest_path.parent().unwrap().join(path);
        let file = engine
            .sources
            .add(&path, std::fs::read_to_string(&path)?)
            .ok_or(ToolError::SourceTooLarge)?;
        let context = FileContext {
            package: ast::PackageId {
                name: "std".into(),
                version: "1".into(),
            },
            modules,
        };
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
        for id in &syntax.roots {
            let item = &syntax.tree.items[id.0 as usize];
            if item.visibility == Visibility::Private {
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
                catalogue_sentence(&std_docs, &name)
            } else {
                sentence(&comments)
            };
            out.push(Entry {
                identity: item.definition.to_string(),
                name: name.clone(),
                declaration,
                description,
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
                    let end = spans.get(i + 1).map_or(item.origin.span.end, |s| s.start);
                    let text = token_text(&syntax, start, end, &engine.sources)
                        .trim_end_matches([' ', '|'])
                        .to_owned();
                    out.push(Entry {
                        identity: format!("{}#{variant_name}", item.definition),
                        name: format!("{name}.{variant_name}"),
                        declaration: format!("{text} -> {name}{args}"),
                        description: format!("Variant of {name}."),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        module(&a.identity)
            .cmp(module(&b.identity))
            .then(a.name.cmp(&b.name))
            .then(a.identity.cmp(&b.identity))
    });
    Ok(out)
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
        if matches!(
            token.kind,
            TokenKind::Trivia(
                ast::TriviaKind::DocLine
                    | ast::TriviaKind::LineComment
                    | ast::TriviaKind::DocBlock
                    | ast::TriviaKind::BlockComment
            )
        ) {
            comments.push(
                sources
                    .slice(token.span)
                    .unwrap()
                    .trim_start_matches('/')
                    .trim_start_matches('*')
                    .trim_end_matches("*/")
                    .trim(),
            );
        }
    }
    comments.join(" ")
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

fn module(identity: &str) -> &str {
    identity
        .split_once('/')
        .and_then(|(_, path)| path.split_once('/'))
        .map_or("", |(head, _)| head)
}

/// One line per item, including constructors, with module heads shared by core and std.
pub fn render(entries: &[Entry]) -> String {
    let definitions = entries.iter().filter(|e| !e.identity.contains('#')).count();
    let mut page = format!(
        "# Full signature index\n\nGenerated from the core registry and std manifest declarations. Do not edit by hand.\nRegenerate from the repository root: `hirsel doc --index > docs/api.md`.\n\nShort module names are the spellings programs use; core and std share module heads.\nSignatures retain declared modes, brands, attributes and `uses` clauses; whitespace is folded.\nOpaque representations stay hidden. Descriptions quote the first sentence of an attached\ncomment or catalogue row when one exists. Constructor rows also appear in their type.\nCore types show their registry bounds and authority; intrinsic storage is not a recursive alias.\nCore schema variables `I`/`J` mean sized integers, `F` floats, and `A` integer/float/bool scalars.\n\n{definitions} exported definitions; {} variants.\n\n",
        entries.len() - definitions
    );
    let mut head = "";
    for entry in entries {
        let module = match module(&entry.identity) {
            "" => "Types and roots",
            head => head,
        };
        if module != head {
            page += &format!("\n## {module}\n\n");
            head = module;
        }
        page += &format!("- `{}`", entry.declaration);
        if !entry.description.is_empty() {
            page += &format!(" — {}", entry.description.replace('`', "'"));
        }
        page += &format!(" <!-- {} -->\n", entry.identity);
    }
    page
}

struct CorePrinter<'a> {
    signature: &'a Signature,
    generics: &'a [GenericParameter],
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
        let t = self.signature.types.get(id).unwrap();
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
            TypeKind::Record(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|f| format!(
                        "{}{}: {}",
                        if f.growable { "growable " } else { "" },
                        f.name,
                        self.ty(f.ty)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
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

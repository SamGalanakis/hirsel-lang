use hirsel_diag::Diagnostic;
use hirsel_engine::DefinitionQueries;
use hirsel_engine::registry::{BuiltinRegistry, nominal_roles};
use hirsel_intern::Interner;
use hirsel_resolve::{ManifestFixture, ManifestIndex};
use hirsel_span::SourceMap;
use hirsel_syntax as ast;
use hirsel_types::*;
use std::collections::BTreeMap;
use std::sync::Arc;
pub(super) struct Registry {
    pub signatures: BTreeMap<ast::DefPath, SignatureRef>,
    pub index: ManifestIndex,
    pub reads: Vec<ast::DefPath>,
    pub builtins: BuiltinRegistry,
}
pub(super) fn path(name: &str) -> ast::DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().unwrap();
    ast::DefPath(ast::ItemPath {
        package: ast::PackageId {
            name: "test".into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
}
pub(super) fn context() -> ast::FileContext {
    ast::FileContext {
        package: path("main").0.package,
        modules: vec![],
    }
}
impl SignatureProvider for Registry {
    fn signature(&mut self, path: &ast::DefPath) -> Result<SignatureRef, QueryError> {
        self.reads.push(path.clone());
        self.signatures
            .get(path)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    fn lookup(
        &mut self,
        context: &ast::FileContext,
        path: &ast::Path,
        symbols: &Interner,
    ) -> LookupOutput {
        let result = self
            .index
            .lookup(context, path, symbols)
            .ok()
            .or_else(|| {
                path.package.as_ref().and_then(|package| {
                    let mut parts: Vec<_> = path
                        .segments
                        .iter()
                        .map(|s| symbols.resolve(*s).unwrap().to_string())
                        .collect();
                    let name = parts.pop()?;
                    let candidate = ast::DefPath(ast::ItemPath {
                        package: package.clone(),
                        modules: parts,
                        name,
                    });
                    self.signatures
                        .contains_key(&candidate)
                        .then_some(candidate)
                })
            })
            .or_else(|| self.builtins.lookup_name(path, symbols))
            .ok_or(QueryError::NotImplemented);
        LookupOutput {
            manifest: self.builtins.revision(),
            result,
        }
    }
}
pub(super) fn parse(
    source: &str,
    symbol_seed: &[String],
) -> (ast::SyntaxFile, SourceMap, Vec<Diagnostic>) {
    let mut map = SourceMap::default();
    let file = map.add("body.hir", source.into()).unwrap();
    let mut symbols = Interner::default();
    for s in symbol_seed {
        symbols.intern(s);
    }
    let lex = hirsel_lexer::lex(file, &map, &mut symbols);
    let parsed = hirsel_parser::parse(file, lex.tokens, &context(), &map, &symbols);
    let mut ds = lex.diagnostics;
    ds.extend(parsed.diagnostics);
    (parsed.syntax, map, ds)
}
/// Source corpus uses the production core declarations, including implicit loop
/// operations. No library type or callback signature is reconstructed here.
pub(super) fn registry(file: &ast::SyntaxFile) -> Registry {
    let (index, ds) = ManifestIndex::new(ManifestFixture::from_files(
        context().package,
        [file],
        ast::ContentHash::default(),
    ));
    assert!(ds.is_empty(), "{ds:?}");
    let builtins = BuiltinRegistry::new();
    let mut signatures: BTreeMap<_, _> = builtins
        .signatures()
        .map(|s| (s.definition.clone(), s.clone()))
        .collect();
    static STD: std::sync::OnceLock<Vec<SignatureRef>> = std::sync::OnceLock::new();
    for signature in STD.get_or_init(|| {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_source/main.hir");
        let mut loaded = hirsel_driver::load_program(&fixture).unwrap();
        [
            "bytes.replace",
            "map.same_entries",
            "u64.to_string",
            "unicode.graphemes",
            "unicode.GraphemeView",
            "unicode.GraphemeCursor",
            "unicode.graphemes_open",
            "unicode.graphemes_next",
            "u8.to_string",
            "test.Source",
            "test.choice",
            "test.any_u64",
            "test.range_u64",
            "test.ascii_string",
            "test.quoted",
            "test.assert_eq",
            "test.property",
            "test.property_cases",
            "test.equal",
            "test.approx",
        ]
        .into_iter()
        .map(|name| {
            let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
            let path = ast::DefPath(ast::ItemPath {
                package: ast::PackageId {
                    name: "std".into(),
                    version: "1".into(),
                },
                name: parts.pop().unwrap(),
                modules: parts,
            });
            loaded.engine.item_signature(&path).unwrap().value
        })
        .collect()
    }) {
        signatures.insert(signature.definition.clone(), signature.clone());
    }
    Registry {
        signatures,
        index,
        reads: vec![],
        builtins,
    }
}
pub(super) fn normalize_file(
    file: &ast::SyntaxFile,
    registry: &mut Registry,
) -> Result<(), String> {
    let metadata = Metadata {
        result_type: nominal_roles().result_type,
        unsafe_capability: nominal_roles().unsafe_capability,
        heap: nominal_roles().heap,
        definitions: registry
            .signatures
            .iter()
            .map(|(p, s)| {
                (
                    p.clone(),
                    DefinitionMetadata {
                        public: true,
                        alias: s.metadata.alias,
                    },
                )
            })
            .chain(file.tree.items.iter().map(|i| {
                (
                    i.definition.clone(),
                    DefinitionMetadata {
                        public: true,
                        alias: matches!(
                            &i.kind,
                            ast::ItemKind::Type {
                                body: ast::TypeBody::Alias(_),
                                ..
                            }
                        ),
                    },
                )
            }))
            .collect(),
        ..Default::default()
    };
    let mut pending = file.roots.clone();
    let mut last = String::new();
    for _ in 0..file.roots.len() + 1 {
        let mut next = vec![];
        for id in pending {
            let r = hirsel_resolve::resolve_item(
                file,
                id,
                &file.symbols,
                &registry.index.clone(),
                registry,
            );
            let item = &file.tree.items[id.0 as usize];
            let ctx = ast::FileContext {
                package: item.definition.0.package.clone(),
                modules: item.definition.0.modules.clone(),
            };
            match hirsel_signature::normalize(
                hirsel_check_protocol::header::Input {
                    tree: &file.tree,
                    item: id,
                    symbols: &file.symbols,
                    context: &ctx,
                    resolution: &r,
                    metadata: &metadata,
                    declaration_sites: &metadata.declarations,
                },
                registry,
            ) {
                Ok(out) => {
                    registry
                        .signatures
                        .insert(item.definition.clone(), Arc::new(out.value));
                }
                Err(e) => {
                    next.push(id);
                    last = format!("{e:?}");
                }
            }
        }
        if next.is_empty() {
            return Ok(());
        }
        pending = next;
    }
    Err(last)
}

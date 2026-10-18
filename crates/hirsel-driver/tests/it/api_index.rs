//! Documentation coverage is checked independently against the compiler's exports.
use hirsel_driver::api_index;
use hirsel_engine::{DefinitionQueries, registry::BuiltinRegistry};
use hirsel_syntax::{ItemKind, TypeBody, Visibility};
use hirsel_types::{SignatureKind, TypeKind};
use std::{collections::BTreeSet, path::Path};

#[test]
fn published_index_matches_every_core_and_std_export() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entries = api_index::entries(&root).unwrap();
        let actual: BTreeSet<_> = entries.iter().map(|e| e.identity.clone()).collect();
        assert_eq!(actual.len(), entries.len(), "duplicate index row");
        let mut exports = BTreeSet::new();
        let mut definitions = 0;
        for s in BuiltinRegistry::new().signatures().filter(|s| {
            s.metadata.visibility != Visibility::Private
                && !s.definition.0.modules.iter().any(|m| m.starts_with('$'))
        }) {
            exports.insert(s.definition.to_string());
            definitions += 1;
            if s.metadata.visibility == Visibility::PublicOpaque {
                continue;
            }
            if let SignatureKind::Type(t) = &s.kind {
                if let Some(repr) = t.representation {
                    if let TypeKind::Variant(variants) = &s.types.get(repr).unwrap().kind {
                        for variant in variants.iter() {
                            exports.insert(format!("{}#{}", s.definition, variant.name));
                        }
                    }
                }
            }
        }
        // Use the real package loader, independently of the generator's manifest reader.
        let program = hirsel_driver::load_program_source(
            &root.join("crates/hirsel-driver/tests/fixtures/std_source/main.hir"),
            Some(""),
        )
        .unwrap();
        let mut engine = program.engine;
        for (file, context) in program.files {
            if !context.package.package_is("std", "1") {
                continue;
            }
            let syntax = engine.parse_file(file, &context).unwrap().value;
            for id in &syntax.roots {
                let item = &syntax.tree.items[id.0 as usize];
                if item.visibility == Visibility::Private {
                    continue;
                }
                definitions += 1;
                assert!(exports.insert(item.definition.to_string()));
                if item.visibility == Visibility::PublicOpaque {
                    continue;
                }
                if let ItemKind::Type {
                    body: TypeBody::Variants(variants),
                    ..
                } = &item.kind
                {
                    for variant in variants {
                        exports.insert(format!(
                            "{}#{}",
                            item.definition,
                            syntax.symbols.resolve(variant.name).unwrap()
                        ));
                    }
                }
            }
        }
        assert_eq!(
            entries.iter().filter(|e| !e.identity.contains('#')).count(),
            definitions,
            "export count"
        );
        assert_eq!(actual, exports, "export identities (including variants)");
        let page = api_index::render(&entries);
        let headings: Vec<_> = page
            .lines()
            .filter(|line| line.starts_with("## "))
            .collect();
        assert_eq!(
            headings.len(),
            headings.iter().collect::<BTreeSet<_>>().len(),
            "each module has one group"
        );
        assert_eq!(
            page.lines().filter(|l| l.starts_with("- `")).count(),
            exports.len()
        );
        assert_eq!(
            page,
            include_str!("../../../../docs/api.md"),
            "run hirsel doc --index > docs/api.md"
        );
    });
}

#[test]
fn index_retains_source_contracts_comments_and_public_visibility() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("api-index-{}", std::process::id()));
        std::fs::create_dir_all(root.join("stdlib/std@1")).unwrap();
        std::fs::create_dir_all(root.join("docs/spec")).unwrap();
        for page in ["core-library.md", "std-library.md"] {
            std::fs::write(root.join("docs/spec").join(page), "").unwrap();
        }
        std::fs::write(
            root.join("stdlib/std@1/hirsel.json"),
            r#"{"sources":[{"path":"probe.hir","module":["probe"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("stdlib/std@1/probe.hir"),
            r#"
/// Retains the source brand. A second sentence is omitted.
@noalloc
pub fn retain[T: data](source: Dir, take value: T,
    callback: fn(T) -> bool uses {}) -> File of source uses {} { unreachable() }
fn private_function() -> () {}
/// A public record.
pub type Record { name: String, bytes: Bytes }
pub type Choice[T] = Empty | Value(T)
pub opaque type Hidden: inert uses {} { secret: u64 }
pub const LIMIT: u64 = 7
"#,
        )
        .unwrap();
        let entries = api_index::entries(&root).unwrap();
        let find = |name: &str| {
            entries
                .iter()
                .find(|e| e.identity == format!("std@1/probe/{name}"))
                .unwrap()
        };
        assert_eq!(
            find("retain").declaration,
            "@noalloc pub fn probe.retain[T: data](source: Dir, take value: T, callback: fn(T) -> bool uses {}) -> File of source uses {}"
        );
        assert_eq!(find("retain").description, "Retains the source brand.");
        assert_eq!(
            find("Record").declaration,
            "pub type probe.Record { name: String, bytes: Bytes }"
        );
        assert_eq!(
            find("Hidden").declaration,
            "pub opaque type probe.Hidden: inert uses {}"
        );
        assert_eq!(find("LIMIT").declaration, "pub const probe.LIMIT: u64");
        assert_eq!(
            find("Choice#Value").declaration,
            "Value(T) -> probe.Choice[T]"
        );
        assert!(!entries.iter().any(|e| e.name.contains("private_function")));
        std::fs::remove_dir_all(root).unwrap();
    });
}

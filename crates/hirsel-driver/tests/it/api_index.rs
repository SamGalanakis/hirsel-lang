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
        let entries = api_index::entries(&root.join("stdlib/std@1/hirsel.json")).unwrap();
        assert_eq!(
            entries
                .iter()
                .find(|e| e.identity == "core@1/slots/insert")
                .unwrap()
                .declaration,
            "@preserves_unique(s) fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s"
        );
        // A record field's brand names its sibling field (PROV-5).
        assert!(
            entries
                .iter()
                .find(|e| e.identity == "core@1/Deadline")
                .unwrap()
                .declaration
                .ends_with("= { clock: MonotonicClock, at: Instant of .clock }")
        );
        let actual: BTreeSet<_> = entries.iter().map(|e| e.identity.clone()).collect();
        assert_eq!(actual.len(), entries.len(), "duplicate index row");
        let mut exports = BTreeSet::new();
        let mut definitions = 0;
        for s in BuiltinRegistry::new().signatures().filter(|s| {
            s.metadata.visibility != Visibility::Private
                && !s
                    .definition
                    .path()
                    .0
                    .modules
                    .iter()
                    .any(|m| m.starts_with('$'))
        }) {
            exports.insert(s.definition.to_string());
            definitions += 1;
            if s.metadata.visibility == Visibility::PublicOpaque {
                continue;
            }
            if let SignatureKind::Type(t) = &s.kind {
                if let Some(repr) = t.representation {
                    if let TypeKind::Variant(variants) =
                        &BuiltinRegistry::new().types().get(repr).unwrap().kind
                    {
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
            if !context.package.package_is("std", "1")
                || context.modules.iter().any(|m| m == "internal")
            {
                continue;
            }
            let syntax = engine.parse_file(file, &context).unwrap().value;
            for id in &syntax.items {
                let item = &id.item;
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
                        exports.insert(format!("{}#{}", item.definition, variant.name.as_str()));
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
            r#"{"schema":1,"package":{"name":"std","version":"1"},"sources":[{"path":"probe.hir","module":["probe"]}],"dependencies":[]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("stdlib/std@1/probe.hir"),
            r#"
/// Retains the source brand. A second sentence is retained.
@noalloc
pub fn retain[T: data](source: Dir, take value: T,
    callback: fn(T) -> bool uses {}) -> File of source uses {} { unreachable() }
fn private_function() -> () {}
/// A public record.
pub type Record { name: String, bytes: Bytes }
pub type Choice[T] = Empty | Value(T)
pub opaque type Hidden: inert uses {} { secret: u64 }
pub const LIMIT: u64 = 7
/**/
pub fn literal(text: String) -> bool uses {}
  requires text == "two  spaces and `ticks`" { true }
pub fn comment(text: String) -> String/* relation */in text uses {} { text }
"#,
        )
        .unwrap();
        let entries = api_index::entries(&root.join("stdlib/std@1/hirsel.json")).unwrap();
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
        assert_eq!(
            find("retain").description,
            "Retains the source brand. A second sentence is retained."
        );
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
            find("literal").declaration,
            "pub fn probe.literal(text: String) -> bool uses {} requires text == \"two  spaces and `ticks`\""
        );
        assert_eq!(
            find("comment").declaration,
            "pub fn probe.comment(text: String) -> String in text uses {}"
        );
        assert_eq!(
            find("Choice#Value").declaration,
            "Value(T) -> probe.Choice[T]"
        );
        assert!(api_index::render(&entries).contains("- ``pub fn probe.literal"));
        assert!(!entries.iter().any(|e| e.name.contains("private_function")));
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn package_public_api_matches_readable_golden() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entries = api_index::entries(&root.join("packages/doc_api@1/hirsel.json")).unwrap();
        let page = api_index::render(&entries);
        let expected = root.join("packages/doc_api@1/api.md");
        if std::env::var_os("HIRSEL_BLESS").is_some() {
            std::fs::write(expected, page).unwrap();
            return;
        }
        assert_eq!(page, include_str!("../../../../packages/doc_api@1/api.md"));
        let identities: BTreeSet<_> = entries.iter().map(|entry| &entry.identity).collect();
        assert_eq!(
            identities.len(),
            entries.len(),
            "duplicate package index row"
        );
        assert_eq!(
            entries
                .iter()
                .find(|e| e.name == "api.token")
                .unwrap()
                .links[0]
                .identity,
            "doc_dep@2/model/Token"
        );
    });
}

#[test]
fn package_documentation_does_not_query_bodies_or_initializers() {
    hirsel_engine::with_compiler_stack(|| {
        let root =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("api-body-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hirsel.json"), r#"{"schema":1,"package":{"name":"probe","version":"1"},"sources":[{"path":"api.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let source = root.join("api.hir");
        std::fs::write(
            &source,
            "pub fn value() -> u64 { 1 }\npub const VALUE: u64 = 1\n",
        )
        .unwrap();
        let before =
            api_index::render_json(&api_index::entries(&root.join("hirsel.json")).unwrap());
        // Syntactically valid bodies with unavailable names/types and a const
        // cycle must not enter signature extraction, checking or evaluation.
        std::fs::write(&source, "pub fn value() -> u64 { let hidden: Missing = unknown(); hidden }\npub const VALUE: u64 = VALUE\n").unwrap();
        let after = api_index::render_json(&api_index::entries(&root.join("hirsel.json")).unwrap());
        assert_eq!(before, after);
        std::fs::remove_dir_all(root).unwrap();
    });
}

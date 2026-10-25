//! Fresh request loading from admitted source-only carry.
use hirsel_driver::*;
use hirsel_engine::{runtime::ProcessBudget, *};
use hirsel_syntax::{DefId, DefPath, ItemPath, PackageId};
use std::sync::Arc;

#[test]
fn source_index_loader_preserves_headers_and_retires_removed_sources() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let manifest = root.join("hirsel.json");
        let write_manifest = |leaf: bool| {
            let mut sources = vec![serde_json::json!({"path":"main.hir", "module":[]})];
            if leaf {
                sources.push(serde_json::json!({"path":"leaf.hir", "module":[]}));
            }
            std::fs::write(
                &manifest,
                serde_json::json!({
                    "schema":1, "package":{"name":"carry", "version":"1"},
                    "sources":sources, "dependencies":[],
                    "entry":{"module":[], "name":"main"}
                })
                .to_string(),
            )
            .unwrap();
        };
        write_manifest(true);
        std::fs::write(
            root.join("main.hir"),
            "fn unrelated()->u64 { 7u64 }\npub fn main()->u64 { leaf() }\n",
        )
        .unwrap();
        std::fs::write(root.join("leaf.hir"), "fn leaf()->u64 { 1u64 }\n").unwrap();
        let compiler = COMPILER.clone().with_config(CompileConfig {
            retention: memo::Retention::Resident,
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        let path = |name: &str| {
            DefPath(ItemPath {
                package: PackageId {
                    name: "carry".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: name.into(),
            })
        };
        let budget = ProcessBudget::new(512 << 20);
        let carry = |program: LoadedProgram| {
            let bytes = program.engine.source_index_bytes().unwrap();
            program
                .engine
                .into_source_index(budget.reserve(bytes).unwrap())
                .unwrap()
        };
        let load = |state| {
            compiler
                .load_check_package_inputs_with_source_index(
                    &manifest,
                    &loader_inputs::DiskInputs,
                    state,
                )
                .unwrap()
        };
        let mut first = compiler.load_check_package(&manifest).unwrap();
        let header = first
            .engine
            .signature(DefId::intern(&path("unrelated")))
            .unwrap();
        let leaf_header = first
            .engine
            .signature(DefId::intern(&path("leaf")))
            .unwrap();
        let mut unchanged = load(carry(first));
        let original_sources = unchanged.engine.sources.clone();
        assert!(
            unchanged
                .engine
                .indexed_definitions()
                .contains(&path("leaf"))
        );
        assert!(Arc::ptr_eq(
            &header,
            &unchanged
                .engine
                .signature(DefId::intern(&path("unrelated")))
                .unwrap()
        ));
        assert_eq!(
            unchanged
                .engine
                .recompute_count(&path("unrelated"), QueryKind::Signature),
            0
        );
        // Checking must still visit every carried body; dropping ParsedFile is
        // not permission to return an empty file inventory.
        assert!(
            unchanged
                .check(LoweringPolicy::Comptime)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        for name in ["unrelated", "main", "leaf"] {
            assert!(
                unchanged
                    .engine
                    .recompute_count(&path(name), QueryKind::Check)
                    > 0,
                "carried source {name} was omitted from checking"
            );
        }
        std::fs::write(root.join("leaf.hir"), "fn leaf()->u64 { 2u64 }\n").unwrap();
        let mut edited = load(carry(unchanged));
        assert!(Arc::ptr_eq(
            &header,
            &edited
                .engine
                .signature(DefId::intern(&path("unrelated")))
                .unwrap()
        ));
        assert!(Arc::ptr_eq(
            &leaf_header,
            &edited
                .engine
                .signature(DefId::intern(&path("leaf")))
                .unwrap()
        ));
        assert_eq!(
            edited
                .engine
                .recompute_count(&path("unrelated"), QueryKind::Signature),
            0
        );
        assert!(
            edited
                .check(LoweringPolicy::Comptime)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        // A manifest removal retires both the declaration and its old memo.
        write_manifest(false);
        std::fs::write(
            root.join("main.hir"),
            "fn unrelated()->u64 { 7u64 }\npub fn main()->u64 { unrelated() }\n",
        )
        .unwrap();
        let mut removed = load(carry(edited));
        assert!(!removed.engine.indexed_definitions().contains(&path("leaf")));
        assert!(
            removed
                .engine
                .signature(DefId::intern(&path("leaf")))
                .is_err()
        );
        assert!(
            removed
                .check(LoweringPolicy::Comptime)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        drop(removed);
        assert!(
            budget.reserved() > 0,
            "detached original SourceMap retains source admission only after capture decoration"
        );
        drop(original_sources);
        assert_eq!(budget.reserved(), 0);
    });
}

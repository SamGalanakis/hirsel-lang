use hirsel_driver::*;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

// Corpus output checks cannot distinguish source bodies from privileged fallbacks.
#[test]
fn deque_is_an_ordinary_source_native_container() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-5-std-deque-run.hir");
        let mut loaded = load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let mut found = BTreeSet::new();
        for unit in &ready.units {
            let header = &unit.unit().header;
            if header.definition.0.package.name_is("std") {
                assert_eq!(header.origin, UnitOrigin::Source);
                if header.definition.0.modules == ["deque"] {
                    found.insert(header.definition.0.name.as_str());
                }
            }
        }
        for name in [
            "empty",
            "len",
            "push_front",
            "push_back",
            "pop_front",
            "pop_back",
        ] {
            assert!(
                found.contains(name),
                "missing source deque.{name}: {found:?}"
            );
        }
    });
}

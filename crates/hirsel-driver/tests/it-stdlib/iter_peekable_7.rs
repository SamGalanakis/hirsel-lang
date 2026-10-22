use hirsel_driver::*;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

// Executed corpus vectors establish behavior; native headers establish that the
// wrapper uses ordinary source functions rather than privileged operations.
#[test]
fn peekable_is_ordinary_source_with_owned_cache() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-5-iter-peekable-7-vectors-run.hir");
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
            if header.definition.0.package.name_is("std")
                && header.definition.0.modules == ["peekable"]
            {
                assert_eq!(header.origin, UnitOrigin::Source);
                found.insert(header.definition.0.name.as_str());
            }
        }
        for name in ["new", "fill", "peek", "next"] {
            assert!(
                found.contains(name),
                "missing source peekable.{name}: {found:?}"
            );
        }
    });
}

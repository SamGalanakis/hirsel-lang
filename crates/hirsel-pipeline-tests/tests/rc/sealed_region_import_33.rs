//! Original source imports preserve the opaque Region handle representation.
use crate::rc_support::{lower, violations};
use hirsel_lir::{ir::ValueRepr, layout_types::LayoutKind};

#[test]
fn sealed_region_handles_are_preserved_in_source_and_bounded_library_imports() {
    for source in [
        include_str!("../../../../conformance/MEM/MEM-4-sealed-empty-unit-run.hir"),
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
    ] {
        let (program, lowered) = lower(source);
        let rejected = violations(&program, &lowered);
        assert!(rejected.is_empty(), "{rejected:#?}");
        let mut source_handles = 0;
        let mut library_handles = 0;
        for unit in &lowered.units {
            for ty in &unit.types {
                if matches!(
                    lowered.table.layout(ty.layout).map(|l| &l.kind),
                    Some(LayoutKind::SealedRegion { .. })
                ) {
                    assert_eq!(ty.repr, ValueRepr::Handle);
                    match unit.header.origin {
                        hirsel_lir::ir::UnitOrigin::Source => source_handles += 1,
                        hirsel_lir::ir::UnitOrigin::Library => library_handles += 1,
                    }
                }
            }
        }
        assert!(
            source_handles > 0 && library_handles > 0,
            "source {source_handles}, library {library_handles}"
        );
    }
}

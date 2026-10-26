//! Portable flow facts keep their meaning after the source units are released.
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_rc::seal_demand::{
    Demand,
    equations::{LocalDemandEquations, solve},
};
use hirsel_types::TypeKind;

#[test]
fn original_scalar_demand_archive_roundtrips_without_units_and_refuses_bad_indices() {
    let (h, _, _) = super::sealed_region_h_source_33::original(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
    );
    let entry = h
        .units
        .iter()
        .position(|u| u.header.definition == crate::rc_support::path("main"))
        .unwrap();
    let expected = Demand::analyze(&h.units, &[entry]).unwrap();
    let archives = h
        .units
        .iter()
        .map(LocalDemandEquations::extract)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let bytes = serde_json::to_vec(&archives).unwrap();
    drop(archives);
    drop(h);
    let restored: Vec<LocalDemandEquations> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(solve(&restored, &[entry]).unwrap(), expected);
    let mut malformed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    malformed[entry]["seals"] = serde_json::json!([u32::MAX]);
    let malformed: Vec<LocalDemandEquations> = serde_json::from_value(malformed).unwrap();
    assert!(
        solve(&malformed, &[entry])
            .unwrap_err()
            .contains("invalid seal-demand local equations")
    );
}

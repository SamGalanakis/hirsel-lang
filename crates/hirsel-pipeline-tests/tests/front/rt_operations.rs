//! hirsel-rt's native operation dispositions checked against the driver's registry inventory.
use hirsel_core_contract::*;
use hirsel_engine::registry::BuiltinRegistry;
use hirsel_rt::operations;
use hirsel_types::{DropAllocation, PassingMode};
use std::collections::BTreeSet;
fn cleanup(value: &DropAllocation) -> CleanupEffect {
    match value {
        DropAllocation::NoAlloc => CleanupEffect::NoAlloc,
        DropAllocation::MayAllocate => CleanupEffect::MayAllocate,
        DropAllocation::DependsOn(ps) => CleanupEffect::DependsOn(
            ps.iter()
                .map(|p| {
                    assert!(matches!(p, hirsel_types::BinderRef::Bound { depth: 0, .. }));
                    p.parameter().0
                })
                .collect(),
        ),
    }
}
fn rows() -> Vec<OperationRow> {
    BuiltinRegistry::new()
        .operation_inventory()
        .into_values()
        .map(|r| {
            let mut row = OperationRow::unclassified(OperationId {
                definition: r.key.definition,
                declaration_digest: r.key.declaration_digest,
            });
            row.ownership = Some(CallOwnership {
                parameters: r
                    .parameters
                    .iter()
                    .map(|p| ParameterOwnership {
                        ordinal: p.ordinal.0,
                        passing: match p.passing {
                            PassingMode::Borrow => Passing::Borrow,
                            PassingMode::Mut => Passing::Mut,
                            PassingMode::Take => Passing::Take,
                        },
                        cleanup: cleanup(&p.cleanup),
                    })
                    .collect(),
                result_cleanup: cleanup(&r.result_cleanup),
            });
            row.effects = SemanticEffects {
                reads: r.effects.reads,
                writes: r.effects.writes,
                owns: r.effects.owns,
                may_fail: r.effects.may_fail,
                allocates: r.effects.allocates,
                observable: r.effects.observable,
            };
            row
        })
        .collect()
}
#[test]
fn inventory_set_equality_and_digest_control() {
    let rows = rows();
    let native = operations::manifest(rows.clone()).unwrap();
    native
        .check_coverage(rows.iter().map(|r| r.id.clone()))
        .unwrap();
    assert_eq!(native.unclassified(Engine::Native).count(), 0);
    let native_paths: BTreeSet<_> = operations::NATIVE_PATHS
        .iter()
        .map(|n| hirsel_syntax::DefPath::core(n))
        .collect();
    assert_eq!(
        native_paths,
        rows.iter().map(|r| r.id.definition.clone()).collect()
    );
    let mut new = OperationRow::unclassified(OperationId {
        definition: hirsel_syntax::DefPath::core("new.operation"),
        declaration_digest: Default::default(),
    });
    assert!(operations::manifest(rows.iter().cloned().chain([new.clone()])).is_err());
    assert!(
        native
            .check_coverage(rows.iter().map(|r| r.id.clone()).chain([new.id.clone()]))
            .is_err()
    );
    new.id = rows[0].id.clone();
    new.id.declaration_digest.0[0] ^= 1;
    assert!(
        native
            .check_coverage(rows.iter().skip(1).map(|r| r.id.clone()).chain([new.id]))
            .is_err()
    );
    let narrow = native
        .rows()
        .find(|r| r.id.definition == hirsel_syntax::DefPath::core("file.narrow"))
        .unwrap();
    assert_eq!(
        narrow.ownership.as_ref().unwrap().parameters[1].passing,
        Passing::Take
    );
    for name in [
        "string.from_bytes",
        "string.slice",
        "io.stdout",
        "out.print",
        "io.dir",
        "dir.narrow",
        "file.open",
        "file.narrow",
        "file.create_atomic",
        "file.commit",
        "file.size",
        "file.remove",
        "file.write_bytes",
        "file.write_all",
        "path.of",
        "arg.path",
    ] {
        let row = native
            .rows()
            .find(|r| r.id.definition == hirsel_syntax::DefPath::core(name))
            .unwrap();
        assert_eq!(
            row.failure.as_ref().map(|r| r.0.as_str()),
            match name {
                "string.slice" => Some("SEM-3"),
                "string.from_bytes" | "out.print" => None,
                _ => Some("CAP-2"),
            }
        );
        let symbol = format!("hirsel_rt_v2_{}", name.replace('.', "_"));
        assert_eq!(
            row.dispositions.native,
            Disposition::RuntimePrimitive(symbol.clone())
        );
        assert!(
            operations::HIRSEL_ABI_EXPORTS
                .iter()
                .any(|e| e.symbol == symbol)
        );
    }
    for (name, symbol) in [
        (
            "$intrinsic.UnicodeScalars_open",
            "hirsel_rt_v2_unicode_scalars_open",
        ),
        (
            "$intrinsic.UnicodeScalars_next",
            "hirsel_rt_v2_unicode_scalars_next",
        ),
        (
            "$intrinsic.ScalarPositions_open",
            "hirsel_rt_v2_scalar_positions_open",
        ),
        (
            "$intrinsic.ScalarPositions_next",
            "hirsel_rt_v2_scalar_positions_next",
        ),
    ] {
        assert_eq!(
            native
                .rows()
                .find(|r| r.id.definition == hirsel_syntax::DefPath::core(name))
                .unwrap()
                .dispositions
                .native,
            Disposition::RuntimePrimitive(symbol.into())
        );
        assert!(
            operations::HIRSEL_ABI_EXPORTS
                .iter()
                .any(|e| e.symbol == symbol)
        );
    }
    for name in [
        "arena.with",
        "slots.insert",
        "slots.get",
        "slots.remove",
        "iter.from",
        "iter.map",
        "$intrinsic.ArrayConsume_open",
        "$intrinsic.ArrayConsume_next",
        "$intrinsic.ListConsume_open",
        "$intrinsic.ListConsume_next",
        "$intrinsic.InlineConsume_open",
        "$intrinsic.InlineConsume_next",
    ] {
        assert_eq!(
            native
                .rows()
                .find(|r| r.id.definition == hirsel_syntax::DefPath::core(name))
                .unwrap()
                .dispositions
                .native,
            Disposition::Body
        );
    }
    for name in [
        "task.async",
        "channel.open",
        "mutex.with",
        "atomic.load",
        "shared.thaw",
    ] {
        assert!(matches!(
            native
                .rows()
                .find(|r| r.id.definition == hirsel_syntax::DefPath::core(name))
                .unwrap()
                .dispositions
                .native,
            Disposition::Unavailable(Unavailable::M2(_))
        ));
    }
    // Scalar intrinsics and the inline library operations are instructions; the array
    // reductions name a float family.
    for &name in operations::SCALAR_INTRINSICS
        .iter()
        .chain(operations::INLINE_OPERATIONS)
    {
        let row = native
            .rows()
            .find(|r| r.id.definition == hirsel_syntax::DefPath::core(name))
            .unwrap();
        assert_eq!(
            row.dispositions.native,
            Disposition::Instruction(name.into())
        );
    }
    assert_eq!(
        native
            .rows()
            .find(|r| r.id.definition == hirsel_syntax::DefPath::core("list.push"))
            .unwrap()
            .dispositions
            .native,
        Disposition::Body
    );
    for name in [
        "sorted_map.insert",
        "$intrinsic.SortedMapOrder_open",
        "$intrinsic.SortedMapOrder_next",
    ] {
        let row = native
            .rows()
            .find(|r| r.id.definition == hirsel_syntax::DefPath::core(name))
            .unwrap();
        assert_eq!(row.dispositions.native, Disposition::Body);
    }
    let comparators: Vec<_> = native
        .rows()
        .filter(|r| {
            r.id.definition.0.modules == ["$intrinsic"]
                && r.id.definition.0.name.starts_with("OrderCompare_")
        })
        .collect();
    assert_eq!(comparators.len(), 10);
    assert!(
        comparators
            .iter()
            .all(|r| r.dispositions.native == Disposition::Body)
    );
    let mut altered = rows
        .iter()
        .find(|r| r.id.definition == hirsel_syntax::DefPath::core("file.narrow"))
        .unwrap()
        .clone();
    altered.ownership.as_mut().unwrap().parameters[1].passing = Passing::Borrow;
    assert!(operations::classify(altered).is_err());
    let mut altered = rows[0].clone();
    altered.id.declaration_digest.0[0] ^= 1;
    assert!(operations::classify(altered).is_err());
    assert!(operations::manifest(rows.iter().cloned().chain([rows[0].clone()])).is_err());
    let mut foreign = rows[0].clone();
    foreign.id.definition.0.package.name = "app".into();
    assert!(operations::classify(foreign).is_err());
    println!("{} inventory rows have native dispositions", native.len());
}
#[test]
fn declaration_digests_match_the_reviewed_native_inventory() {
    let encoded = rows()
        .iter()
        .map(|r| {
            let p = &r.id.definition.0;
            let name = p
                .modules
                .iter()
                .chain(std::iter::once(&p.name))
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(".");
            let digest: String =
                r.id.declaration_digest
                    .0
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
            format!(
                "{name}\t{digest}\t{}\n",
                operations::ownership_encoding(r.ownership.as_ref().unwrap())
            )
        })
        .collect::<String>();
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../native-tests/library/operations.tsv"
            ),
            &encoded,
        )
        .unwrap();
        return;
    }
    assert_eq!(
        encoded,
        include_str!("../../../../native-tests/library/operations.tsv")
    );
}

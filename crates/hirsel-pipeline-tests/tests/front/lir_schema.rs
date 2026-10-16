//! The lir operation manifest checked against the driver's registry inventory
//! and interface version; moved from hirsel-lir's schema tests so lir does not
//! dev-depend on the driver.
use hirsel_codec::{self as snapshot, Snapshot};
use hirsel_core_contract::{
    Disposition, Engine, Manifest, OperationId, OperationRow, SemanticEffects, Unavailable,
};
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};

fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "0".into(),
        },
        modules: vec!["m".into()],
        name: name.into(),
    })
}

fn hash(label: &str) -> ContentHash {
    ContentHash::of_bytes(label.as_bytes())
}

fn assert_round_trip<T: Snapshot + PartialEq + std::fmt::Debug>(value: &T) -> String {
    let text = snapshot::to_text(value);
    let back: T = snapshot::from_text(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    assert_eq!(&back, value);
    assert_eq!(
        snapshot::to_text(&back),
        text,
        "re-encoding is not deterministic"
    );
    assert!(!text.contains("TypeId"), "a dump contains a TypeId");
    assert!(!text.contains("0x"), "a dump contains an address");
    text
}

mod manifest {
    use super::*;

    fn inventory() -> Vec<OperationId> {
        hirsel_engine::registry::BuiltinRegistry::new()
            .operation_inventory()
            .into_keys()
            .map(|key| OperationId {
                definition: key.definition,
                declaration_digest: key.declaration_digest,
            })
            .collect()
    }

    #[test]
    fn the_manifest_skeleton_has_a_row_for_every_registry_operation() {
        let ids = inventory();
        assert!(ids.len() > 50, "registry inventory is unexpectedly small");
        let mut manifest = Manifest::skeleton(ids.clone());
        assert_eq!(manifest.len(), ids.len());
        assert_eq!(manifest.check_coverage(ids.clone()), Ok(()));
        for row in manifest.rows() {
            assert_eq!(row.ownership, None);
            assert_eq!(row.effects, SemanticEffects::CONSERVATIVE);
            for engine in Engine::ALL {
                assert_eq!(
                    row.dispositions.get(engine),
                    &Disposition::Unavailable(Unavailable::Unclassified)
                );
            }
        }
        assert_eq!(manifest.unclassified(Engine::Native).count(), ids.len());
        println!("manifest skeleton: {} operation rows", manifest.len());

        // Rows can be classified but never added or removed.
        let stranger = OperationId {
            definition: path("not-registered"),
            declaration_digest: hash("x"),
        };
        assert!(
            manifest
                .replace(OperationRow::unclassified(stranger.clone()))
                .is_err()
        );
        let mut first = manifest.rows().next().unwrap().clone();
        first.dispositions.native = Disposition::Unavailable(Unavailable::NotImplemented);
        manifest.replace(first).unwrap();
        assert_eq!(manifest.unclassified(Engine::Native).count(), ids.len() - 1);

        // A registry operation without a row, or a row without an operation, fails coverage.
        let missing = manifest.check_coverage(ids.iter().cloned().chain([stranger.clone()]));
        assert_eq!(
            missing.unwrap_err().missing.into_iter().collect::<Vec<_>>(),
            [stranger]
        );
        let extra = manifest
            .check_coverage(ids.iter().skip(1).cloned())
            .unwrap_err();
        assert_eq!(extra.extra.len(), 1);

        assert_round_trip(&manifest);
    }

    #[test]
    fn the_schema_interface_version_matches_the_driver() {
        assert_eq!(
            hirsel_engine::default_environment().interface_version,
            hirsel_lir::INTERFACE_VERSION
        );
    }
}

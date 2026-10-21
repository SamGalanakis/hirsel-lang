use hirsel_driver::{COMPILER, dependency_card};
use hirsel_engine::{DefinitionQueries, QueryKind, with_compiler_stack};
use serde_json::json;
use std::path::Path;

fn manifest() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/dep_card_fixture/app/hirsel.json")
}

#[test]
fn dependency_card_reuses_checked_queries_and_does_not_recheck_bodies() {
    with_compiler_stack(|| {
        let manifest = manifest();
        let mut program = COMPILER.load_test_package(&manifest).unwrap();
        let first = program.dependency_cards(&manifest, None).unwrap();
        let mut keys = Vec::new();
        for (file, context) in program.files.clone() {
            let parsed = program.engine.parse_file(file, &context).unwrap().value;
            for syntax in &parsed.items {
                let path = syntax.item.definition.clone();
                for kind in [
                    QueryKind::Signature,
                    QueryKind::Check,
                    QueryKind::Modes,
                    QueryKind::Lower,
                ] {
                    keys.push((
                        path.clone(),
                        kind,
                        program.engine.recompute_count(&path, kind),
                    ));
                }
            }
        }
        assert_eq!(program.dependency_cards(&manifest, None).unwrap(), first);
        for (path, kind, count) in keys {
            assert_eq!(
                program.engine.recompute_count(&path, kind),
                count,
                "{path}: {kind:?}"
            );
        }
    });
}

#[test]
fn dependency_card_verifies_signed_code_store_objects() {
    with_compiler_stack(|| {
        use hirsel_code_store::{Bundle, Signer};
        let manifest = manifest();
        let mut program = COMPILER.load_test_package(&manifest).unwrap();
        program.dependency_cards(&manifest, Some("leaf@1")).unwrap();
        let roots = program
            .files
            .clone()
            .into_iter()
            .flat_map(|(file, context)| {
                let parsed = program.engine.parse_file(file, &context).unwrap().value;
                parsed
                    .items
                    .iter()
                    .map(|syntax| syntax.item.definition.clone())
                    .collect::<Vec<_>>()
            })
            .filter(|p| p.package_is("leaf", "1"))
            .collect::<Vec<_>>();
        let groups = program.engine.definition_groups(roots).unwrap();
        let signer = Signer::from_seed(&[34; 32]);
        let mut bundle = Bundle::of_groups(&groups);
        bundle.sign(&signer);
        let dir = std::env::temp_dir().join(format!("hirsel-card-signed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = json!({"provider":"code.dir@1", "path":dir.join("objects"),
            "signatures":{"provider":"sign.ed25519@1","keys":[signer.public_key().hex()]}});
        let store = hirsel_code_store::config::open(&config).unwrap();
        assert!(matches!(
            store.add(&bundle).unwrap(),
            hirsel_code_store::Added::Ok
        ));
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, config.to_string()).unwrap();
        let report = dependency_card::cards(&manifest, Some("leaf@1"), Some(&config_path)).unwrap();
        let p = &report["cards"][0]["health"]["provenance"];
        assert_eq!(p["signature_policy"], "sign.ed25519@1");
        assert!(!p["objects"].as_array().unwrap().is_empty());
        for object in p["objects"].as_array().unwrap() {
            assert_eq!(object["present"], true);
            assert_eq!(object["signature_verified"], true);
        }
        let hash = p["objects"][0]["hash"].as_str().unwrap();
        std::fs::write(
            dir.join(format!("objects/sig/{hash}/{}", signer.public_key().hex())),
            [0u8; 64],
        )
        .unwrap();
        let error =
            dependency_card::cards(&manifest, Some("leaf@1"), Some(&config_path)).unwrap_err();
        assert!(format!("{error:?}").contains("Unsigned"));
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn dependency_cards_preserve_facts_when_a_matching_image_saves_body_work() {
    with_compiler_stack(|| {
        use hirsel_engine::{StdImage, StdImages};
        let manifest = manifest();
        let leaf = manifest.parent().unwrap().join("../leaf/hirsel.json");
        let path =
            std::env::temp_dir().join(format!("hirsel-card-image-{}.bin", std::process::id()));
        COMPILER.image_package(&leaf, &path).unwrap();
        let images = StdImages::of(vec![StdImage::open(&path).unwrap()]);
        let mut source = COMPILER
            .load_program_source_with_images(&manifest, None, &StdImages::none())
            .unwrap();
        let mut cached = COMPILER
            .load_program_source_with_images(&manifest, None, &images)
            .unwrap();
        let mut a = source.dependency_cards(&manifest, Some("leaf@1")).unwrap();
        let mut b = cached.dependency_cards(&manifest, Some("leaf@1")).unwrap();
        assert_eq!(a["cards"][0]["weight"]["cost"]["prechecked_definitions"], 0);
        assert!(
            b["cards"][0]["weight"]["cost"]["prechecked_definitions"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert_eq!(b["cards"][0]["weight"]["cost"]["check_source_tokens"], 0);
        assert_eq!(b["cards"][0]["weight"]["cost"]["lower_source_tokens"], 0);
        a["cards"][0]["weight"]
            .as_object_mut()
            .unwrap()
            .remove("cost");
        b["cards"][0]["weight"]
            .as_object_mut()
            .unwrap()
            .remove("cost");
        assert_eq!(a, b);
        std::fs::remove_file(path).unwrap();
    });
}

use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::{SignatureKind, TypeKind};
fn engine(text: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("private.hir", text.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}
#[test]
fn opaque_private_edits_invalidate_owner_record_without_changing_public_signature() {
    let path = DefPath(hirsel_syntax::ItemPath {
        name: "Hidden".into(),
        ..entry_path(&default_context()).0
    });
    let mut engine = engine("pub opaque type Hidden: () uses {} { value: u64 }\n");
    let public = engine.item_signature(&path).unwrap().value;
    assert!(matches!(&public.kind, SignatureKind::Type(t) if t.representation.is_none()));
    assert!(
        public.types.is_empty(),
        "public signatures retain no hidden rows"
    );
    let private = engine.owner_representation(&path, &path).unwrap();
    assert!(matches!(
        private.types.get(private.private_shape).unwrap().kind,
        TypeKind::Record(_)
    ));
    assert!(
        engine
            .owner_representation(&entry_path(&default_context()), &path)
            .is_err()
    );
    let file = engine
        .sources
        .add(
            "private.hir",
            "pub opaque type Hidden: () uses {} { value: u32 }\n".into(),
        )
        .unwrap();
    check_progress(&mut engine, file, &default_context()).unwrap();
    let changed = engine.owner_representation(&path, &path).unwrap();
    assert_ne!(
        private.representation_revision,
        changed.representation_revision
    );
    assert_eq!(
        public.fingerprint,
        engine.item_signature(&path).unwrap().value.fingerprint
    );
}
#[test]
fn source_less_opaque_shapes_remain_explicitly_unavailable() {
    let mut engine = engine("@capability(global) pub opaque type Cap: plain uses {}\n");
    let path = DefPath(hirsel_syntax::ItemPath {
        name: "Cap".into(),
        ..entry_path(&default_context()).0
    });
    assert!(matches!(
        engine.owner_representation(&path, &path),
        Err(QueryError::NotImplemented)
    ));
}

#[test]
fn opaque_owner_bodies_construct_project_destructure_and_match() {
    with_compiler_stack(|| {
        let mut engine = engine(
            "pub opaque type Hidden: data uses {} { value: u64 }\n\
             pub opaque type Choice: data uses {} = Empty() | Full(u64)\n\
             fn build() -> Hidden { Hidden(value: 1) }\n\
             fn read(h: Hidden) -> u64 { h.value }\n\
             fn unpack(take h: Hidden) -> u64 { let Hidden(value: n) = take h; n }\n\
             fn choose() -> Choice { Full(2) }\n\
             fn inspect(c: Choice) -> u64 { match c { Empty() => 0, Full(n) => n } }\n",
        );
        for name in ["build", "read", "unpack", "choose", "inspect"] {
            let mut path = entry_path(&default_context());
            path.0.name = name.into();
            engine.lower_to_tir(&path).unwrap();
        }
    });
}

#[test]
fn private_field_edits_recheck_owner_bodies_with_one_public_fingerprint() {
    with_compiler_stack(|| {
        let original = "pub opaque type Hidden: data uses {} { value: u64 }\n\
                        fn read(h: Hidden) -> u64 { h.value }\n";
        let mut engine = engine(original);
        let mut hidden = entry_path(&default_context());
        hidden.0.name = "Hidden".into();
        let mut read = hidden.clone();
        read.0.name = "read".into();
        let fingerprint = engine.item_signature(&hidden).unwrap().value.fingerprint;
        engine.lower_to_tir(&read).unwrap();
        let counts = [
            QueryKind::Resolve,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ]
        .map(|kind| engine.recompute_count(&read, kind));
        let expanded = original.replace("value: u64", "value: u64, spare: u32");
        let file = engine.sources.add("private.hir", expanded).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        engine.lower_to_tir(&read).unwrap();
        assert_eq!(
            fingerprint,
            engine.item_signature(&hidden).unwrap().value.fingerprint
        );
        for (kind, count) in [
            QueryKind::Resolve,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ]
        .into_iter()
        .zip(counts)
        {
            assert!(engine.recompute_count(&read, kind) > count, "{kind:?}");
        }
        let file = engine
            .sources
            .add("private.hir", original.replace("value: u64", "value: u32"))
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert_eq!(
            fingerprint,
            engine.item_signature(&hidden).unwrap().value.fingerprint
        );
        assert!(
            matches!(engine.lower_to_tir(&read), Err(QueryError::Diagnostics(ds))
            if ds[0].rule.as_str() == "LANG-7")
        );
        for (kind, count) in [QueryKind::Resolve, QueryKind::Check]
            .into_iter()
            .zip(counts)
        {
            assert!(engine.recompute_count(&read, kind) > count, "{kind:?}");
        }
        // Restore the shape without changing read's text or its public signature.
        let file = engine.sources.add("private.hir", original.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        engine.lower_to_tir(&read).unwrap();
    });
}

#[test]
fn owner_constant_materialization_tracks_private_shape_edits() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden: data uses {} { value: u64 }\n\
                      const seed: Hidden = Hidden(value: 1)\n";
        let mut engine = engine(source);
        let mut seed = entry_path(&default_context());
        seed.0.name = "seed".into();
        let fingerprint = engine.item_signature(&seed).unwrap().value.fingerprint;
        for (source, primitive) in [
            (source.to_owned(), hirsel_types::Primitive::U64),
            (
                source.replace("value: u64", "value: u32"),
                hirsel_types::Primitive::U32,
            ),
        ] {
            let file = engine.sources.add("private.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let value = engine.materialize_value(&seed).unwrap();
            let hirsel_tir::ValueKind::Construct { fields, .. } = value.root.kind else {
                panic!("record")
            };
            assert_eq!(
                value.types.get(fields[0].ty).unwrap().kind,
                TypeKind::Primitive(primitive)
            );
            assert_eq!(
                fingerprint,
                engine.item_signature(&seed).unwrap().value.fingerprint
            );
        }
    });
}

#[test]
fn opaque_package_controls_preserve_private_operations_and_diagnostic_sites() {
    with_compiler_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for slug in [
            "owner-run",
            "package-run",
            "client-construct-ok",
            "client-read-ok",
            "client-destructure-ok",
            "client-alias-chain-ok",
        ] {
            let path = root.join(format!("conformance/PROG/PROG-2-opaque-{slug}.hir"));
            let checked = check_path(&path).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{slug}: {:?}",
                checked.diagnostics
            );
            assert_eq!(checked.completion, CheckCompletion::Complete);
            if slug.ends_with("run") {
                let mut output = vec![];
                assert!(matches!(
                    run_path_with_simulation(&path, SimulationConfig::default(), &mut output)
                        .unwrap(),
                    RunOutcome::Exit(0)
                ));
                assert_eq!(
                    output,
                    if slug == "owner-run" {
                        b"opaque\n".as_slice()
                    } else {
                        b""
                    }
                );
                let mut loaded = load_program(&path).unwrap();
                let snapshot = loaded.engine.export_native_input().unwrap();
                for public in snapshot.signatures.values() {
                    if public.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                        assert!(public.types.is_empty(), "{:?}", public.definition);
                    }
                }
                loaded.engine.import_native_input(snapshot).unwrap();
            }
        }
        for (slug, line) in [
            ("construct", 4),
            ("read", 4),
            ("destructure", 6),
            ("alias-chain", 8),
        ] {
            let path = root.join(format!(
                "conformance/PROG/PROG-2-opaque-client-{slug}-bad.hir"
            ));
            let checked = check_path(&path).unwrap();
            let diagnostic = &checked.diagnostics[0];
            assert_eq!(
                diagnostic.rule.as_str(),
                "PROG-2",
                "{slug}: {:?}",
                checked.diagnostics
            );
            assert_eq!(
                checked.sources.location(diagnostic.primary).unwrap().line,
                line,
                "{slug}"
            );
        }
    });
}

#[test]
fn opaque_fingerprint_excludes_private_nominal_field_dependencies() {
    with_compiler_stack(|| {
        let source =
            "type Inner { value: u64 }\npub opaque type Hidden: data uses {} { value: Inner }\n";
        let mut engine = engine(source);
        let mut hidden = entry_path(&default_context());
        hidden.0.name = "Hidden".into();
        let fingerprint = engine.item_signature(&hidden).unwrap().value.fingerprint;
        let file = engine
            .sources
            .add("private.hir", source.replace("value: Inner", "value: u64"))
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert_eq!(
            fingerprint,
            engine.item_signature(&hidden).unwrap().value.fingerprint
        );
    });
}

#[test]
fn adding_a_private_shape_retries_a_cached_owner_rejection() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden: data uses {}\n\
                      fn build() -> Hidden { Hidden(value: 1) }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("private.hir", source.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut hidden = entry_path(&default_context());
        hidden.0.name = "Hidden".into();
        let mut build = hidden.clone();
        build.0.name = "build".into();
        let public = engine.item_signature(&hidden).unwrap().value.fingerprint;
        assert!(matches!(
            engine.lower_to_tir(&build),
            Err(QueryError::Diagnostics(_))
        ));
        let changed = source.replace("uses {}\n", "uses {} { value: u64 }\n");
        let file = engine.sources.add("private.hir", changed).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert_eq!(
            public,
            engine.item_signature(&hidden).unwrap().value.fingerprint
        );
        engine.lower_to_tir(&build).unwrap();
    });
}

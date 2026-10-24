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
        hirsel_types::encoding::signature_types(engine.types(), &public).is_empty(),
        "public signatures retain no hidden rows"
    );
    let private = engine.owner_representation(&path, &path).unwrap();
    assert!(matches!(
        engine.types().get(private.private_shape).unwrap().kind,
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
                engine.types().get(fields[0].ty).unwrap().kind,
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
fn owner_constant_shapes_track_private_field_edits() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden: data uses {} { value: u64 }\n\
                      fn first(take info: ShapeInfo) -> String {\n\
                      match info.body { Record(fields) => copy fields[0].ty, Choice(_) => \"\" }\n\
                      }\n\
                      const field: String = first(type_shape.info(shape(Hidden)))\n";
        let mut engine = engine(source);
        let mut field = entry_path(&default_context());
        field.0.name = "field".into();
        for (source, spelling) in [
            (source.to_owned(), "u64"),
            (source.replace("value: u64", "value: u32"), "u32"),
        ] {
            let file = engine.sources.add("private.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            assert_eq!(
                engine.materialize_constant(&field).unwrap(),
                hirsel_tir::Constant::String(spelling.into())
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
                    run_path(&path, &mut output).unwrap(),
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
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
                    if public.definition.path().is_core("Region") {
                        // 2a938017 retains only this intrinsic descriptor for independent V.
                        assert_eq!(
                            public.metadata.visibility,
                            hirsel_syntax::Visibility::PublicOpaque
                        );
                        let hirsel_types::SignatureKind::Type(declaration) = &public.kind else {
                            panic!("core Region must remain a type")
                        };
                        assert!(declaration.resource);
                        let representation = declaration.representation.unwrap();
                        assert_eq!(
                            hirsel_types::encoding::signature_types(&snapshot.types, public),
                            vec![representation]
                        );
                        let hirsel_types::TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Region,
                            arguments,
                        } = &snapshot.types.get(representation).unwrap().kind
                        else {
                            panic!("core Region exports its intrinsic descriptor only")
                        };
                        let [hirsel_types::GenericArgument::Type(item)] = arguments.as_slice()
                        else {
                            panic!("Region retains exactly its original payload type argument")
                        };
                        assert!(matches!(snapshot.types.get(*item).unwrap().kind,
                            hirsel_types::TypeKind::Parameter(parameter)
                            if parameter == hirsel_types::TypeParameterRef::bound(hirsel_types::TypeParamId(0))));
                    } else if public.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
                    {
                        assert!(
                            hirsel_types::encoding::signature_types(&snapshot.types, public)
                                .is_empty(),
                            "{:?}",
                            public.definition
                        );
                    }
                }
                loaded.engine.import_native_input(snapshot).unwrap();
            }
        }
        for (slug, line) in [
            ("construct", 5),
            ("read", 5),
            ("destructure", 7),
            ("alias-chain", 9),
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

/// A constant in another file reflects the opaque type through a helper: no
/// text it executed and no public fingerprint changes when the private field
/// does. Its evaluation records `Representation(Hidden)` with the digest of
/// the private view it reflected, which the edit changes and reverting it
/// restores.
#[test]
fn constants_reflecting_an_opaque_type_through_a_helper_track_private_field_edits() {
    with_compiler_stack(|| {
        let hidden = "pub opaque type Hidden: data uses {} { value: u64 }\n";
        let reflect = "fn first(take info: ShapeInfo) -> String {\n\
                       match info.body { Record(fields) => copy fields[0].ty, Choice(_) => \"\" }\n\
                       }\n\
                       fn first_field[T](s: TypeShape[T]) -> String { first(type_shape.info(s)) }\n\
                       const field: String = first_field(shape(Hidden))\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("reflect.hir", reflect.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut field = entry_path(&default_context());
        field.0.name = "field".into();
        let mut hidden_path = field.clone();
        hidden_path.0.name = "Hidden".into();
        let mut digests = vec![];
        for (source, spelling) in [
            (hidden.to_owned(), "u64"),
            (hidden.replace("value: u64", "value: u32"), "u32"),
            (hidden.to_owned(), "u64"),
        ] {
            let file = engine.sources.add("hidden.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let public = engine
                .item_signature(&hidden_path)
                .unwrap()
                .value
                .fingerprint;
            assert_eq!(
                engine.materialize_constant(&field).unwrap(),
                hirsel_tir::Constant::String(spelling.into())
            );
            let reads = engine.representation_reads(&field).unwrap();
            let [(read, digest)] = &reads[..] else {
                panic!("{spelling}: {reads:?}")
            };
            assert_eq!(read, &hidden_path);
            assert_ne!(*digest, hirsel_syntax::ContentHash::default());
            digests.push((public, *digest));
        }
        // The public fingerprint never moves; the reflected view's digest does.
        assert_eq!(digests[0].0, digests[1].0);
        assert_ne!(digests[0].1, digests[1].1);
        assert_eq!(digests[0].1, digests[2].1);
    });
}

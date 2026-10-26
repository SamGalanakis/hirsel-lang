//! Owner bodies validate against the private signature input of their own package.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_span::Span;
use hirsel_tir::*;
use hirsel_types::{Destination, Primitive, SignatureKind, Type, TypeKind, UseKind};
use hirsel_validate::SignatureMap;

#[test]
fn hand_built_opaque_construction_needs_the_owner_input_and_checks_fields() {
    with_compiler_stack(|| {
        let context = default_context();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("owner.hir", "pub opaque type Hidden: plain uses {} { value: u64 }\nfn build() -> Hidden { Hidden(value: 1) }\n".into()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut hidden = entry_path(&context);
        hidden.0.name = "Hidden".into();
        let mut build = hidden.clone();
        build.0.name = "build".into();
        let public = engine.item_signature(&hidden).unwrap().value;
        assert!(matches!(&public.kind, SignatureKind::Type(t) if t.representation.is_none()));
        let function = engine.item_signature(&build).unwrap().value;
        let types = engine.types().clone();
        let mut row = Type::unit();
        row.kind = TypeKind::Primitive(Primitive::U64);
        let number = types.intern(row);
        let SignatureKind::Function(header) = &function.kind else {
            unreachable!()
        };
        let nominal = header.result;
        let span = Span::default();
        let instruction = |kind| Instruction {
            kind,
            span,
            on_failure: CleanupId(0),
        };
        let take = |place| Operand {
            place: PlaceId(place),
            usage: UseKind::Take,
        };
        let mut definition = Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            path: hirsel_syntax::DefId::intern(&build),
            inferred_unique: Default::default(),
            signature_hash: function.fingerprint,
            types: std::sync::Arc::from([]),
            entry: BodyId(0),
            contracts: vec![],
            bodies: vec![Body {
                arena_scope_grant: None,
                scoped_parameters: Vec::new(),
                parameter_scope: hirsel_types::LexicalOwner::first(&build)
                    .site(hirsel_types::NameSite::Item),
                locals: vec![
                    Local {
                        ty: number,
                        storage: Storage::Temporary,
                        scope: ScopeId(0),
                        span,
                    },
                    Local {
                        ty: nominal,
                        storage: Storage::ReturnSlot,
                        scope: ScopeId(0),
                        span,
                    },
                ],
                places: [number, nominal]
                    .into_iter()
                    .enumerate()
                    .map(|(i, ty)| Place {
                        loan: None,
                        binding: None,
                        root: LocalId(i as u32),
                        projection: Default::default(),
                        ty,
                    })
                    .collect(),
                scopes: vec![Scope { parent: None, span }],
                blocks: vec![Block {
                    instructions: vec![
                        instruction(InstructionKind::StorageLive(LocalId(0))),
                        instruction(InstructionKind::Assign {
                            destination: PlaceId(0),
                            value: RvalueId(0),
                        }),
                        instruction(InstructionKind::Assign {
                            destination: PlaceId(1),
                            value: RvalueId(1),
                        }),
                    ],
                    terminator: Terminator::Return {
                        value: take(1),
                        cleanup: CleanupId(0),
                    },
                    span,
                }],
                rvalues: vec![
                    Rvalue::Constant(Constant::Integer(1)),
                    Rvalue::Construct {
                        constructor: Constructor::Record(hidden.clone()),
                        fields: vec![ConstructField {
                            field: 0,
                            value: take(0),
                        }],
                        destination: Destination::Heap,
                    },
                ],
                cleanup_nodes: vec![],
                cleanups: vec![Cleanup::default()],
                entry: BlockId(0),
                parameters: vec![],
                result: nominal,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
            }],
        };
        definition.types = hirsel_tir::definition_types(&types, &definition);
        // This program fails with the previous public-only transport.
        let mut public_input = SignatureMap::new(types.clone());
        public_input.insert(&hidden, (*public).clone());
        public_input.insert(function.definition, (*function).clone());
        definition.types = hirsel_tir::definition_types(engine.types(), &definition);
        assert!(
            !hirsel_validate::validate(&definition, engine.types(), &public_input, &mut vec![])
                .violations
                .is_empty()
        );
        let snapshot = engine.signature_snapshot();
        let owner_input = snapshot.validation(&context.package);
        definition.types = hirsel_tir::definition_types(engine.types(), &definition);
        let report =
            hirsel_validate::validate(&definition, engine.types(), owner_input, &mut vec![]);
        assert!(report.violations.is_empty(), "{:?}", report.violations);
        let foreign = hirsel_syntax::PackageId {
            name: context.package.name.clone(),
            version: "1".into(),
        };
        definition.types = hirsel_tir::definition_types(engine.types(), &definition);
        assert!(
            !hirsel_validate::validate(
                &definition,
                engine.types(),
                snapshot.validation(&foreign),
                &mut vec![]
            )
            .violations
            .is_empty()
        );
        let mut forged = definition;
        let Rvalue::Construct { fields, .. } = &mut forged.bodies[0].rvalues[1] else {
            unreachable!()
        };
        fields[0].field = 1;
        forged.types = hirsel_tir::definition_types(engine.types(), &forged);
        assert!(
            !hirsel_validate::validate(&forged, engine.types(), owner_input, &mut vec![])
                .violations
                .is_empty()
        );
        assert_eq!(public, engine.item_signature(&hidden).unwrap().value);
    });
}

/// The frozen signature universe changes only with its inputs; ordinary body
/// invalidation must retain its Arc and all the per-view V verdicts.
#[test]
fn snapshot_reuses_body_edits_and_tracks_signature_inputs() {
    with_compiler_stack(|| {
        use std::sync::Arc;
        let context = default_context();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let parse = |engine: &mut QueryEngine, source: &str| {
            let file = engine
                .sources
                .add("snapshot-edits.hir", source.into())
                .unwrap();
            engine.parse_file(file, &context).unwrap();
        };
        parse(&mut engine, "fn main() -> u64 { 1 }\n");
        let first = engine.signature_snapshot();
        parse(&mut engine, "fn main() -> u64 { 2 }\n");
        let body = engine.signature_snapshot();
        assert!(Arc::ptr_eq(&first, &body));
        assert_eq!(
            engine.recompute_count(&entry_path(&context), QueryKind::Signature),
            1
        );

        parse(&mut engine, "fn main() -> u8 { 2u8 }\n");
        let header = engine.signature_snapshot();
        assert!(!Arc::ptr_eq(&body, &header));
        assert_ne!(
            body.signature(hirsel_syntax::DefId::intern(&entry_path(&context))),
            header.signature(hirsel_syntax::DefId::intern(&entry_path(&context)))
        );

        // The default may be unlimited. Still select a distinct finite input
        // rather than overflowing while testing snapshot invalidation.
        engine.comptime_limits.call_depth = engine
            .comptime_limits
            .call_depth
            .checked_add(1)
            .unwrap_or(32);
        let limits = engine.signature_snapshot();
        assert!(!Arc::ptr_eq(&header, &limits));
        engine.module_inputs_mut().manifest_revision =
            hirsel_syntax::ContentHash::of_bytes(b"new manifest");
        let manifest = engine.signature_snapshot();
        assert!(!Arc::ptr_eq(&limits, &manifest));
        assert!(Arc::ptr_eq(&manifest, &engine.signature_snapshot()));

        // A new later candidate preserves the first header, but affects its
        // duplicate diagnostic and the declaration-candidate read.
        parse(
            &mut engine,
            "fn main() -> u8 { 2u8 }\nfn main() -> u8 { 3u8 }\n",
        );
        let duplicate = engine.signature_snapshot();
        assert!(!Arc::ptr_eq(&manifest, &duplicate));
        assert!(
            duplicate
                .signature(hirsel_syntax::DefId::intern(&entry_path(&context)))
                .unwrap()
                .is_err()
        );
    });
}

#[test]
fn snapshot_private_shape_edits_change_the_view_without_a_public_fingerprint_change() {
    with_compiler_stack(|| {
        use std::sync::Arc;
        let context = default_context();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let mut hidden = entry_path(&context);
        hidden.0.name = "Hidden".into();
        let hidden = hirsel_syntax::DefId::intern(&hidden);
        let mut previous = None;
        for field in ["u64", "u8"] {
            let file = engine
                .sources
                .add(
                    "opaque-edits.hir",
                    format!("pub opaque type Hidden: plain uses {{}} {{ value: {field} }}\n"),
                )
                .unwrap();
            engine.parse_file(file, &context).unwrap();
            let snapshot = engine.signature_snapshot();
            if let Some(old) = previous {
                let old: Arc<hirsel_engine::SignatureSnapshot> = old;
                assert!(!Arc::ptr_eq(&old, &snapshot));
                assert_eq!(
                    old.signature(hidden).unwrap().unwrap().fingerprint,
                    snapshot.signature(hidden).unwrap().unwrap().fingerprint
                );
                assert_ne!(
                    old.representation(hidden).unwrap().unwrap().1,
                    snapshot.representation(hidden).unwrap().unwrap().1
                );
            }
            previous = Some(snapshot);
        }
    });
}

#[test]
fn snapshot_retires_staged_signatures_when_only_the_owner_body_changes() {
    with_compiler_stack(|| {
        use std::sync::Arc;
        let context = default_context();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let mut owner = entry_path(&context);
        owner.0.name = "value".into();
        let source = "fn value[n: size]() -> u64 { comptime (n + 1) }\n";
        let file = engine
            .sources
            .add("staged-edits.hir", source.into())
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let analyzed = engine.mode_definition(&owner).unwrap().value;
        let expression = analyzed
            .typed
            .checked
            .expressions
            .iter()
            .find(|row| {
                matches!(
                    analyzed.typed.syntax.tree.expressions[row.expression.0 as usize].kind,
                    hirsel_syntax::ExprKind::Comptime(_)
                )
            })
            .unwrap()
            .expression;
        let staged =
            hirsel_check_protocol::body::comptime_path(&owner, &analyzed.typed.checked, expression);
        let staged = hirsel_syntax::DefId::intern(&staged);
        let initial = engine.signature_snapshot();
        assert!(initial.signature(staged).is_none());
        engine.lower_to_tir(&owner).unwrap();
        let published = engine.signature_snapshot();
        assert!(!Arc::ptr_eq(&initial, &published));
        assert!(published.signature(staged).unwrap().is_ok());

        let file = engine
            .sources
            .add(
                "staged-edits.hir",
                "fn value[n: size]() -> u64 { n + 1 }\n".into(),
            )
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let retired = engine.signature_snapshot();
        assert!(!Arc::ptr_eq(&published, &retired));
        assert!(retired.signature(staged).is_none());
        assert!(published.signature(staged).unwrap().is_ok());
    });
}

#[test]
fn live_and_frozen_bodies_record_the_same_private_view_receipt() {
    with_compiler_stack(|| {
        let context = default_context();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let mut hidden = entry_path(&context);
        hidden.0.name = "Hidden".into();
        let mut build = hidden.clone();
        build.0.name = "build".into();
        let hidden = hirsel_syntax::DefId::intern(&hidden);
        let mut previous = None;
        for field in ["u64", "u8"] {
            let source = format!(
                "pub opaque type Hidden: plain uses {{}} {{ value: {field} }}\nfn build() -> Hidden {{ Hidden(value: 1) }}\n"
            );
            let file = engine.sources.add("private-receipts.hir", source).unwrap();
            engine.parse_file(file, &context).unwrap();
            engine.check_definition(&build).unwrap();
            let serial = engine.memo_reads(&build, QueryKind::Check).unwrap().clone();
            let snapshot = engine.signature_snapshot();
            let mut frozen = ReadsBuilder::new();
            let mut provider =
                FrozenProvider::for_body(&snapshot, context.package.clone(), &mut frozen);
            let view = provider.signature(hidden).unwrap();
            assert!(matches!(&view.kind, SignatureKind::Type(t) if t.representation.is_some()));
            let frozen = frozen.finish().unwrap();
            for (key, value) in frozen.iter() {
                assert!(
                    serial
                        .iter()
                        .any(|(read, observed)| read == key && observed == value),
                    "{key:?}"
                );
            }
            let digest = frozen
                .iter()
                .find_map(|(key, value)| {
                    (*key == ReadKey::Representation(hidden)).then_some(*value)
                })
                .unwrap();
            assert_eq!(
                digest,
                snapshot.representation(hidden).unwrap().unwrap().1.0
            );
            if let Some(old) = previous {
                assert_ne!(old, digest);
            }
            previous = Some(digest);
        }
        assert_eq!(engine.recompute_count(&build, QueryKind::Check), 2);
    });
}

/// A source edit's private-view subscriptions and warm reads survive real
/// concurrent phases, rather than merely exercising the provider in isolation.
#[test]
fn frozen_frontier_matches_serial_private_edits_and_tir() {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        for jobs in [1, 2, 8] {
            let mut serial = QueryEngine::new(default_environment(), compiler_adapters());
            let mut parallel = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let paths: Vec<_> = ["read", "build", "main"]
                .into_iter()
                .map(|name| {
                    let mut path = entry_path(&context);
                    path.0.name = name.into();
                    path
                })
                .collect();
            for (field, value) in [("u64", 1), ("u8", 1), ("u8", 2)] {
                let text = format!(
                    "pub opaque type Hidden: plain uses {{}} {{ value: {field} }}\nfn build() -> Hidden {{ Hidden(value: {value}) }}\nfn read(x: Hidden) -> {field} {{ x.value }}\nfn main() -> {field} {{ read(build()) }}\n"
                );
                for engine in [&mut serial, &mut parallel] {
                    let file = engine
                        .sources
                        .add("parallel-private.hir", text.clone())
                        .unwrap();
                    engine.parse_file(file, &context).unwrap();
                }
                let expected: Vec<_> = paths
                    .iter()
                    .map(|path| serial.mode_definition(path))
                    .collect();
                let actual = parallel.mode_definitions(paths.clone(), Jobs::new(jobs));
                for ((path, expected), actual) in paths.iter().zip(expected).zip(actual) {
                    expected.unwrap();
                    actual.result.unwrap();
                    for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                        assert_eq!(
                            serial.memo_reads(path, kind),
                            parallel.memo_reads(path, kind),
                            "{jobs}/{field}/{value}/{path:?}/{kind:?}"
                        );
                    }
                    let a = serial.definition_summary(DefId::intern(path)).unwrap();
                    let b = parallel.definition_summary(DefId::intern(path)).unwrap();
                    assert_eq!(a.summary.frames, b.summary.frames);
                    assert_eq!(a.summary.calls.len(), b.summary.calls.len());
                    for (a, b) in a.summary.calls.iter().zip(&b.summary.calls) {
                        assert_eq!(
                            (a.callee, a.binder, a.class, a.owning_frame, a.target_frame),
                            (b.callee, b.binder, b.class, b.owning_frame, b.target_frame)
                        );
                        let a = serial.sources.resolve(a.site);
                        let b = parallel.sources.resolve(b.site);
                        assert_eq!((a.start, a.end), (b.start, b.end));
                    }
                    assert_eq!(a.growth.edges.len(), b.growth.edges.len());
                    assert_eq!(
                        serial.export_tir(path).unwrap(),
                        parallel.export_tir(path).unwrap()
                    );
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_diagnostics_and_tir_are_identical_at_three_salts() {
    fn run() -> String {
        with_compiler_stack(|| {
            use hirsel_intern::jobs::Jobs;
            fn transcript(engine: &mut QueryEngine, results: Vec<ModeResult>) -> String {
                let mut text = String::new();
                let accepted = results.iter().all(|result| result.result.is_ok());
                for result in results {
                    text.push_str(&format!("{:?} {:?}\n", result.definition, result.phase));
                    let diagnostics = match result.result {
                        Ok(output) => {
                            if accepted {
                                let tir = engine.export_tir(&result.definition).unwrap();
                                text.push_str(&tir);
                            }
                            output.diagnostics.clone()
                        }
                        Err(QueryError::Diagnostics(ds)) => ds,
                        Err(error) => panic!("unexpected operational failure {error:?}"),
                    };
                    for diagnostic in diagnostics {
                        text.push_str(&diagnostic.to_json(&engine.sources).to_string());
                        text.push('\n');
                    }
                }
                text
            }
            let programs = [
                "fn id[T](take v: T) -> T { v }\nfn left() -> u64 { id(1u64) }\nfn right() -> u8 { id(2u8) }\n",
                "pub opaque type Hidden: plain uses {} { value: u64 }\nfn make() -> Hidden { Hidden(value: 1) }\nfn read(v: Hidden) -> u64 { v.value }\n",
                "fn bad() -> u64 { absent }\nfn good() -> u64 { 1 }\n",
            ];
            let mut all = String::new();
            for source in programs {
                let mut serial = QueryEngine::new(default_environment(), compiler_adapters());
                let file = serial
                    .sources
                    .add("frozen-oracle.hir", source.into())
                    .unwrap();
                let parsed = serial.parse_file(file, &default_context()).unwrap();
                let paths: Vec<_> = parsed
                    .value
                    .items
                    .iter()
                    .filter(|s| matches!(s.item.kind, hirsel_syntax::ItemKind::Function { .. }))
                    .map(|s| s.item.definition.clone())
                    .rev()
                    .collect();
                let expected: Vec<_> = paths
                    .iter()
                    .map(|path| {
                        serial.last_phase = None;
                        serial.unavailable = None;
                        let result = serial.mode_definition(path).map(std::sync::Arc::new);
                        ModeResult {
                            summary: None,
                            reads: std::sync::Arc::new(Reads::empty()),
                            diagnostic_phases: vec![],
                            definition: path.clone(),
                            phase: serial
                                .unavailable
                                .or(serial.last_phase)
                                .or(Some(QueryKind::Modes)),
                            result,
                        }
                    })
                    .collect();
                let expected = transcript(&mut serial, expected);
                for jobs in [1, 2, 8] {
                    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                    let file = engine
                        .sources
                        .add("frozen-oracle.hir", source.into())
                        .unwrap();
                    engine.parse_file(file, &default_context()).unwrap();
                    for _ in 0..2 {
                        let results = engine.mode_definitions(paths.clone(), Jobs::new(jobs));
                        assert_eq!(
                            transcript(&mut engine, results),
                            expected,
                            "jobs {jobs}: {source}"
                        );
                    }
                }
                all.push_str(&expected);
            }
            all
        })
    }
    let Some(outputs) = hirsel_intern::under_salts(
        "signature_input::frozen_frontier_diagnostics_and_tir_are_identical_at_three_salts",
        &[1, 2, 3],
        run,
    ) else {
        return;
    };
    assert!(outputs[0].contains("(Export\n"));
    assert!(outputs[0].contains("\"severity\":\"error\""));
    assert!(outputs.iter().all(|output| output == &outputs[0]));
}

#[test]
fn frozen_frontier_keeps_serial_generation_between_source_frontiers() {
    with_compiler_stack(|| {
        let source = "fn make() -> Result[Body[fn() -> u64 uses {}], String] uses {} { Ok(codegen.body(\"fn() -> u64 uses {} { 7 }\")) }\nfn first() -> u64 uses {} { 1 }\nfn generated() -> u64 uses {} = comptime make()\nfn last() -> u64 uses {} { generated() }\n";
        for jobs in [1, 2, 8] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("parallel-generated.hir", source.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let paths: Vec<_> = ["first", "generated", "last"]
                .into_iter()
                .map(|name| {
                    let mut p = entry_path(&default_context());
                    p.0.name = name.into();
                    p
                })
                .collect();
            let results =
                engine.mode_definitions(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            for result in results {
                result.result.unwrap();
            }
            let mut serial = QueryEngine::new(default_environment(), compiler_adapters());
            let file = serial
                .sources
                .add("parallel-generated.hir", source.into())
                .unwrap();
            serial.parse_file(file, &default_context()).unwrap();
            for p in paths {
                serial.mode_definition(&p).unwrap();
                assert_eq!(
                    serial.export_tir(&p).unwrap(),
                    engine.export_tir(&p).unwrap()
                );
                assert_eq!(
                    serial.memo_reads(&p, QueryKind::Modes),
                    engine.memo_reads(&p, QueryKind::Modes)
                );
            }
        }
    });
}

#[test]
fn validation_universe_receipts_distinguish_valid_changes_and_originating_private_packages() {
    with_compiler_stack(|| {
        use hirsel_engine::memo::PackageKey;
        use hirsel_validate::SignatureLookup;
        let context = default_context();
        let foreign = hirsel_syntax::PackageId {
            name: "foreign".into(),
            version: "1".into(),
        };
        let base = "pub opaque type Hidden: plain uses {} { value: u8 }\nfn f() -> u8 { 1u8 }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("universe.hir", base.into()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let initial = engine.signature_snapshot();
        let initial_errors = initial.validation(&context.package).universe_errors().0;
        let mut reads = ReadsBuilder::new();
        let own = FrozenProvider::for_body(&initial, context.package.clone(), &mut reads)
            .validation_universe();
        let foreign_before =
            FrozenProvider::for_body(&initial, foreign.clone(), &mut reads).validation_universe();
        let reads = reads.finish().unwrap();
        assert_eq!(
            reads.get(&ReadKey::ValidationUniverse(PackageKey::of(
                &context.package
            ))),
            Some(own)
        );
        assert_eq!(
            reads.get(&ReadKey::ValidationUniverse(PackageKey::of(&foreign))),
            Some(foreign_before)
        );
        assert_ne!(
            own, foreign_before,
            "private views have an explicit originating package"
        );
        let body = base.replace("{ 1u8 }", "{ 2u8 }");
        let file = engine.sources.add("universe.hir", body.clone()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let body_snapshot = engine.signature_snapshot();
        assert_eq!(body_snapshot.validation_input_digest(&context.package), own);
        let private = body.replace("value: u8", "value: u16");
        let file = engine.sources.add("universe.hir", private.clone()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let private_snapshot = engine.signature_snapshot();
        assert_eq!(
            private_snapshot
                .validation(&context.package)
                .universe_errors()
                .0,
            initial_errors
        );
        assert_ne!(
            private_snapshot.validation_input_digest(&context.package),
            own
        );
        assert_eq!(
            private_snapshot.validation_input_digest(&foreign),
            foreign_before
        );
        let header = private.replace("fn f() -> u8 { 2u8 }", "fn f() -> u16 { 2u16 }");
        let file = engine.sources.add("universe.hir", header).unwrap();
        engine.parse_file(file, &context).unwrap();
        let changed = engine.signature_snapshot();
        assert_eq!(
            changed.validation(&context.package).universe_errors().0,
            initial_errors
        );
        assert_ne!(
            changed.validation_input_digest(&foreign),
            foreign_before,
            "valid changed rows change the input receipt even with the same empty error digest"
        );
    });
}

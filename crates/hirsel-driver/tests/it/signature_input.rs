//! Owner bodies validate against the private signature input of their own package.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_span::Span;
use hirsel_tir::*;
use hirsel_types::{Destination, Primitive, SignatureKind, Type, TypeKind, TypeStore, UseKind};
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
        let mut types = TypeStore::default();
        let mut row = Type::unit();
        row.kind = TypeKind::Primitive(Primitive::U64);
        let number = types.push(row);
        let SignatureKind::Function(header) = &function.kind else {
            unreachable!()
        };
        let nominal =
            hirsel_types::import_type(&function.types, header.result, &mut types).unwrap();
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
        let definition = Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            path: build.clone(),
            signature_hash: function.fingerprint,
            types,
            entry: BodyId(0),
            contracts: vec![],
            bodies: vec![Body {
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
                tail_sites: vec![],
                matches: vec![],
            }],
        };
        // This program fails with the previous public-only transport.
        let public_input: SignatureMap = [
            (hidden.clone(), (*public).clone()),
            (function.definition.clone(), (*function).clone()),
        ]
        .into_iter()
        .collect();
        assert!(
            !hirsel_validate::validate(&definition, &public_input, &mut vec![])
                .violations
                .is_empty()
        );
        let snapshot = engine.signature_snapshot();
        let owner_input = snapshot.validation(&context.package);
        let report = hirsel_validate::validate(&definition, owner_input, &mut vec![]);
        assert!(report.violations.is_empty(), "{:?}", report.violations);
        let foreign = hirsel_syntax::PackageId {
            name: context.package.name.clone(),
            version: "1".into(),
        };
        assert!(
            !hirsel_validate::validate(&definition, snapshot.validation(&foreign), &mut vec![])
                .violations
                .is_empty()
        );
        let mut forged = definition;
        let Rvalue::Construct { fields, .. } = &mut forged.bodies[0].rvalues[1] else {
            unreachable!()
        };
        fields[0].field = 1;
        assert!(
            !hirsel_validate::validate(&forged, owner_input, &mut vec![])
                .violations
                .is_empty()
        );
        assert_eq!(public, engine.item_signature(&hidden).unwrap().value);
    });
}

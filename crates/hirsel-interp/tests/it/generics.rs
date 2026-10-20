use crate::support;
use hirsel_interp::{Limits, Outcome, Value, run};
use hirsel_tir::*;
use hirsel_types::*;
use support::*;

#[test]
fn size_bindings_preserve_u64_boundaries_and_reject_missing_values() {
    for size in [0, u64::MAX] {
        let mut reader = Builder::new();
        let result = reader.local(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        reader.assign(
            result,
            Rvalue::Size(SizeParameterRef::body(item_scope(), SizeParamId(1))),
            1,
            CleanupId(0),
        );
        let reader = reader.finish("read_size", result);
        let mut caller = Builder::new();
        let scalar = caller.type_id(TypeKind::Primitive(Primitive::U8));
        let result = caller.local(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        caller.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("read_size"),
                    arguments: vec![
                        GenericArgument::Type(scalar),
                        GenericArgument::Size(Size::Constant(size)),
                    ],
                },
                arguments: vec![],
                returns: Some(CallReturn {
                    destination: result,
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            },
            1,
        );
        let caller = caller.finish("main", result);
        let definitions = vec![caller, reader.clone()];
        let mut signatures = hirsel_validate::SignatureMap::new(session());
        signatures.extend(fixture_signatures(&definitions, &runtime()));
        let SignatureKind::Function(f) = &mut signatures.get_mut(&path("read_size")).unwrap().kind
        else {
            unreachable!()
        };
        f.generics = vec![
            GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            },
            GenericParameter::Size { name: "n".into() },
        ]
        .into();
        for definition in &definitions {
            hirsel_validate::assert_valid(definition, &session(), &signatures);
        }
        let execution = run(
            &mut Provider {
                definitions,
                queries: vec![],
            }, &crate::support::session(),
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            execution.outcome,
            Outcome::Returned {
                value: Value::Integer(
                    hirsel_interp::Integer::new(Primitive::U64, size as u128).unwrap()
                ),
                exit_code: 0
            }
        );
        let error = run(
            &mut Provider {
                definitions: vec![reader],
                queries: vec![],
            }, &crate::support::session(),
            &sources(),
            &RunConfiguration {
                entry: path("read_size"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Limits::default(),
        )
        .unwrap_err();
        assert!(
            matches!(error.error,hirsel_interp::Error::InvalidTir(message) if message=="unbound generic size parameter")
        );
    }
}

//! STORE-6 controls manufacture calls without source checking or schema bytes.
use super::*;

fn table_call(kind: Option<BuiltinType>, function_value: bool) -> Fixture {
    let mut f = Fixture::unit();
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U32)));
    let key = if let Some(kind) = kind {
        let arguments = match kind {
            BuiltinType::Map => vec![GenericArgument::Type(scalar), GenericArgument::Type(scalar)],
            BuiltinType::Simd => vec![
                GenericArgument::Type(scalar),
                GenericArgument::Size(Size::Constant(4)),
            ],
            _ => vec![GenericArgument::Type(scalar)],
        };
        let inner = f.definition.types.push(ty(TypeKind::Builtin {
            kind,
            arguments: arguments.into(),
        }));
        // The prohibition walks nested containers, not just the actual's root.
        f.definition
            .types
            .push(ty(TypeKind::Tuple(vec![inner].into())))
    } else {
        scalar
    };
    let mut signature = function_signature("get", vec![], TypeKind::Tuple(vec![].into()));
    signature.definition.0.package.name = "std".into();
    signature.definition.0.modules = vec!["table".into()];
    let path = signature.definition.clone();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.generics = vec![GenericParameter::Type {
        name: "K".into(),
        bounds: vec![Bound::Serializable].into(),
    }]
    .into();
    let mut function = fun.clone();
    function.result =
        import_type(&signature.types, function.result, &mut f.definition.types).unwrap();
    f.signatures.insert(path.clone(), signature);
    if function_value {
        let slot = f.slot(TypeKind::Function(function), Storage::Temporary);
        // A specialized function value has no remaining generic binder.
        let old = f.body().places[slot as usize].ty;
        let mut row = f.definition.types.get(old).unwrap().clone();
        if let TypeKind::Function(fun) = &mut row.kind {
            fun.generics = vec![].into();
        }
        let specialized = f.definition.types.push(row);
        f.body().places[slot as usize].ty = specialized;
        f.body().locals[slot as usize].ty = specialized;
        let cleanup = f.cleanup(vec![drop(slot)]);
        f.assign(
            slot,
            Rvalue::FunctionReference {
                definition: path,
                arguments: vec![GenericArgument::Type(key)],
            },
            0,
        );
        f.body().blocks[0].terminator = ret(0, cleanup);
    } else {
        let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path,
                arguments: vec![GenericArgument::Type(key)],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(destination),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(destination, 0)));
    }
    f
}

#[test]
fn table_keys_exclude_map_set_and_simd_in_calls_and_function_values() {
    for function_value in [false, true] {
        table_call(None, function_value).valid();
        table_call(Some(BuiltinType::List), function_value).valid();
        for kind in [BuiltinType::Map, BuiltinType::Set, BuiltinType::Simd] {
            table_call(Some(kind), function_value).broken(vec![ViolationKind::TypeMismatch {
                entity: "table key schema",
            }]);
        }
    }
}

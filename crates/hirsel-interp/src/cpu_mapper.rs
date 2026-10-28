//! Independent checked-TIR certificate for owned CPU mapper data and operations.
//! This walk never consumes a native layout, mapper graph or execution plan.
use crate::{AsyncTirProvider, Error, Value, aggregate::RuntimeTypes};
use hirsel_tir::{Callee, Constant, Constructor, InstructionKind, Rvalue, Storage, Terminator};
use hirsel_type_analysis::logical::{Logical, LogicalNominal, Resolved, Scope, type_arguments};
use hirsel_types::{BuiltinType, Destination, Primitive, TypeId, TypeKind};
use std::collections::{BTreeSet, HashSet};

fn scalar(runtime: &RuntimeTypes, ty: TypeId) -> bool {
    fn visit(runtime: &RuntimeTypes, ty: TypeId, active: &mut HashSet<TypeId>) -> bool {
        if !active.insert(ty) {
            return false;
        }
        let valid = runtime.types.get(ty).is_some_and(|row| match &row.kind {
            TypeKind::Primitive(p) => matches!(
                p,
                Primitive::Bool
                    | Primitive::U8
                    | Primitive::U16
                    | Primitive::U32
                    | Primitive::U64
                    | Primitive::I8
                    | Primitive::I16
                    | Primitive::I32
                    | Primitive::I64
                    | Primitive::F32
                    | Primitive::F64
            ),
            TypeKind::Tuple(fields) => fields.iter().all(|ty| visit(runtime, *ty, active)),
            TypeKind::Record(fields) => fields.iter().all(|field| visit(runtime, field.ty, active)),
            TypeKind::Variant(variants) => variants.iter().all(|variant| {
                variant
                    .fields
                    .iter()
                    .all(|(_, ty)| visit(runtime, *ty, active))
            }),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(nominal) = runtime.get(definition.path()) else {
                    return false;
                };
                if nominal.resource || nominal.finalizer.is_some() {
                    return false;
                }
                let Some(representation) = nominal.representation else {
                    return false;
                };
                // Close the checked declaration exactly as retained-value shape
                // traversal does, without consulting native receipt metadata.
                let signature = runtime.serialization.get(definition.path());
                let default_roles = hirsel_types::NominalRoles::default();
                let roles = signature
                    .map(|s| &*s.metadata.roles)
                    .unwrap_or(&default_roles);
                hirsel_types::substitute(
                    &runtime.types,
                    &runtime.memo,
                    representation,
                    &hirsel_types::Substitution {
                        binder: Some(hirsel_types::GenericBinder::Bound(0)),
                        roles,
                        arguments,
                        authorities: &vec![
                            hirsel_types::AuthoritySummary::Unknown;
                            arguments.len()
                        ],
                    },
                )
                .is_ok_and(|closed| visit(runtime, closed, active))
            }
            _ => false,
        });
        active.remove(&ty);
        valid
    }
    visit(runtime, ty, &mut HashSet::new())
}

fn owned(runtime: &RuntimeTypes, ty: TypeId) -> bool {
    let nominal = |definition: hirsel_syntax::DefId| {
        runtime.get(definition.path()).map(|row| {
            match (row.resource, row.finalizer.is_some(), row.representation) {
                (false, false, Some(shape)) => LogicalNominal::Representation { shape },
                _ => LogicalNominal::Opaque,
            }
        })
    };
    fn visit<'a>(logical: &Logical<'a, '_>, ty: TypeId, scope: &Scope<'a>, depth: usize) -> bool {
        if depth >= 64 {
            return false;
        }
        let Ok(Resolved::Structural(row, scope)) = logical.resolve(ty, scope) else {
            return false;
        };
        let part = |ty| visit(logical, ty, &scope, depth + 1);
        match &row.kind {
            TypeKind::Primitive(p) => matches!(
                p,
                Primitive::Bool
                    | Primitive::U8
                    | Primitive::U16
                    | Primitive::U32
                    | Primitive::U64
                    | Primitive::I8
                    | Primitive::I16
                    | Primitive::I32
                    | Primitive::I64
                    | Primitive::F32
                    | Primitive::F64
            ),
            TypeKind::Tuple(fields) => fields.iter().copied().all(part),
            TypeKind::Record(fields) => fields.iter().all(|field| part(field.ty)),
            TypeKind::Variant(variants) => variants
                .iter()
                .all(|variant| variant.fields.iter().all(|(_, ty)| part(*ty))),
            TypeKind::Box(payload) => part(*payload),
            TypeKind::Builtin {
                kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline,
                arguments,
            } => {
                let arguments = type_arguments(arguments);
                matches!(arguments.as_slice(), [element] if part(*element))
            }
            _ => false,
        }
    }
    visit(&Logical::new(&nominal), ty, &Scope::root(&runtime.types), 0)
}

fn pure(path: &hirsel_syntax::DefPath) -> bool {
    [
        "int.wrapping_add",
        "int.wrapping_sub",
        "int.wrapping_mul",
        "int.saturating_add",
        "int.saturating_sub",
        "int.leading_zeros",
        "int.trailing_zeros",
        "int.count_ones",
        "int.swap_bytes",
        "int.reverse_bits",
        "f32.bits",
        "f32.from_bits",
        "f64.bits",
        "f64.from_bits",
        "list.len",
        "list.capacity",
        "list.reserve",
        "list.push",
        "failure.check",
    ]
    .iter()
    .any(|name| path.is_core(name))
}

pub(crate) async fn certified(
    provider: &mut (impl AsyncTirProvider + ?Sized),
    runtime: &RuntimeTypes,
    mapper: &Value,
) -> Result<bool, Error> {
    let (path, body) = match mapper {
        Value::Closure {
            definition,
            body,
            environment,
            ..
        } => {
            if !environment
                .capture_types
                .iter()
                .all(|ty| scalar(runtime, *ty))
            {
                return Ok(false);
            }
            (definition.path().clone(), Some(*body))
        }
        Value::FunctionReference(reference) => (reference.definition.clone(), None),
        _ => return Ok(false),
    };
    let mut todo = vec![(path, body, false)];
    let mut active = BTreeSet::new();
    let mut checked = BTreeSet::new();
    let mut instructions = 0;
    while let Some((path, selected, leave)) = todo.pop() {
        let definition = match provider.definition(&path).await {
            Ok(definition) => definition,
            Err(hirsel_types::QueryError::NotImplemented) => return Ok(false),
            Err(error) => return Err(Error::Query(error)),
        };
        let body_id = selected.unwrap_or(definition.entry);
        let key = (path.clone(), body_id.0);
        if leave {
            active.remove(&key);
            checked.insert(key);
            continue;
        }
        if checked.contains(&key) {
            continue;
        }
        if !active.insert(key) {
            return Ok(false);
        }
        todo.push((path, Some(body_id), true));
        // Keep the native graph limits and refuse unknown representations.
        if checked.len() + active.len() >= 64 {
            return Ok(false);
        }
        let Some(body) = definition.bodies.get(body_id.0 as usize) else {
            return Ok(false);
        };
        instructions += body
            .blocks
            .iter()
            .map(|block| block.instructions.len() + 1)
            .sum::<usize>();
        if instructions > 4096
            || body.arena_scope_grant.is_some()
            || !owned(runtime, body.result)
            || !body.locals.iter().all(|local| owned(runtime, local.ty))
        {
            return Ok(false);
        }
        if body.rvalues.iter().any(|value| {
            !matches!(
                value,
                Rvalue::Constant(
                    Constant::Unit
                        | Constant::Bool(_)
                        | Constant::Integer(_)
                        | Constant::F32(_)
                        | Constant::F64(_)
                ) | Rvalue::Use(_)
                    | Rvalue::Unary {
                        op: hirsel_syntax::UnaryOp::Neg
                            | hirsel_syntax::UnaryOp::Not
                            | hirsel_syntax::UnaryOp::BitNot,
                        ..
                    }
                    | Rvalue::Binary { .. }
                    | Rvalue::Size(_)
                    | Rvalue::Construct {
                        constructor: Constructor::Tuple
                            | Constructor::Record(_)
                            | Constructor::Variant { .. }
                            | Constructor::Sequence
                            | Constructor::Box,
                        destination: Destination::Heap | Destination::Inferred,
                        ..
                    }
            )
        }) {
            return Ok(false);
        }
        for block in &body.blocks {
            if block.instructions.iter().any(|instruction| {
                matches!(&instruction.kind, InstructionKind::BeginLoan { place, mutable: true, .. }
                    if body.places.get(place.0 as usize).and_then(|place| body.locals.get(place.root.0 as usize))
                        .is_none_or(|local| local.storage == Storage::Capture))
            }) {
                return Ok(false);
            }
            if let Terminator::Call { callee, .. } = &block.terminator {
                match callee {
                    Callee::Definition { path, .. } if pure(path) => (),
                    Callee::Definition { path, .. } if path.0.package.name != "core" => {
                        todo.push((path.clone(), None, false))
                    }
                    _ => return Ok(false),
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_span::Span;
    use hirsel_tir::*;
    use hirsel_types::{GenericArgument, QueryError, Type, TypeInterner};
    use std::{
        future::Future,
        sync::Arc,
        task::{Context, Poll, Waker},
    };

    struct Program(DefinitionRef);
    impl TirProvider for Program {
        fn definition(&mut self, _: &hirsel_syntax::DefPath) -> Result<DefinitionRef, QueryError> {
            Ok(self.0.clone())
        }
        fn constant(&mut self, _: &hirsel_syntax::DefPath) -> Result<MaterializedRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn constant_definitions(&mut self) -> Result<Vec<hirsel_syntax::DefPath>, QueryError> {
            Ok(vec![])
        }
    }
    fn certificate(runtime: &RuntimeTypes, ty: TypeId, value: Rvalue) -> bool {
        let path = hirsel_syntax::DefPath::core("mapper");
        let span = Span::default();
        let mut program = Program(Arc::new(Definition {
            inferred_unique: Default::default(),
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: vec![ty].into(),
            path: (&path).into(),
            signature_hash: Default::default(),
            bodies: vec![Body {
                arena_scope_grant: None,
                parameter_scope: hirsel_types::LexicalOwner::first(&path)
                    .site(hirsel_types::NameSite::Item),
                locals: vec![Local {
                    ty,
                    storage: Storage::ReturnSlot,
                    scope: ScopeId(0),
                    span,
                }],
                places: vec![Place {
                    loan: None,
                    binding: None,
                    root: LocalId(0),
                    projection: vec![].into(),
                    ty,
                }],
                scopes: vec![hirsel_tir::Scope { parent: None, span }],
                blocks: vec![Block {
                    instructions: vec![Instruction {
                        kind: InstructionKind::Assign {
                            destination: PlaceId(0),
                            value: RvalueId(0),
                        },
                        span,
                        on_failure: CleanupId(0),
                    }],
                    terminator: Terminator::Return {
                        value: Operand {
                            place: PlaceId(0),
                            usage: hirsel_types::UseKind::Take,
                        },
                        cleanup: CleanupId(0),
                    },
                    span,
                }],
                rvalues: vec![value],
                cleanups: vec![Cleanup::default()],
                cleanup_nodes: vec![],
                entry: BlockId(0),
                parameters: vec![],
                result: ty,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
                scoped_parameters: vec![],
            }],
            entry: BodyId(0),
            contracts: vec![],
        }));
        let mapper = Value::FunctionReference(Box::new(crate::value::NamedFunction {
            definition: path,
            arguments: vec![],
            types: runtime.types.clone(),
            ty,
        }));
        let mut provider = crate::ImmediateTirProvider::new(&mut program);
        let mut future = std::pin::pin!(certified(&mut provider, runtime, &mapper));
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => result.unwrap(),
            Poll::Pending => panic!("immediate checked program suspended"),
        }
    }
    #[test]
    fn allocating_sequence_certificate_refuses_deep_copy_and_scoped_storage() {
        let runtime = RuntimeTypes::new(TypeInterner::new(None));
        let item = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let list = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(item)].into(),
        }));
        let construction = |destination| Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields: vec![],
            destination,
        };
        assert!(certificate(&runtime, list, construction(Destination::Heap)));
        assert!(certificate(
            &runtime,
            list,
            construction(Destination::Inferred)
        ));
        assert!(!certificate(
            &runtime,
            list,
            construction(Destination::Region(hirsel_types::RegionId(0)))
        ));
        assert!(!certificate(
            &runtime,
            list,
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value: Operand {
                    place: PlaceId(0),
                    usage: hirsel_types::UseKind::Copy
                }
            }
        ));
        let shared = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(item)].into(),
        }));
        assert!(!certificate(
            &runtime,
            shared,
            construction(Destination::Heap)
        ));
    }
    #[test]
    fn owned_results_resolve_closed_nominals_and_refuse_resource_cleanup() {
        let mut runtime = RuntimeTypes::new(TypeInterner::new(None));
        let item = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let list = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(item)].into(),
        }));
        let path = hirsel_syntax::DefPath::core("Rows");
        let named = runtime.types.intern(Type::with_kind(TypeKind::Named {
            definition: (&path).into(),
            arguments: vec![].into(),
        }));
        runtime.nominals.push(crate::aggregate::Nominal {
            definition: path,
            resource: false,
            distinct: false,
            finalizer: None,
            fields: 1,
            representation: Some(list),
        });
        assert!(owned(&runtime, named));
        runtime.nominals[0].resource = true;
        assert!(!owned(&runtime, named));
        runtime.nominals[0].resource = false;
        runtime.nominals[0].representation = Some(named);
        assert!(!owned(&runtime, named));
    }
}

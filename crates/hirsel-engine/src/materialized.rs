//! Reify interpreter values with checked and declared types, never source reflection.
use crate::*;
use hirsel_interp::Value;
use hirsel_span::Span;
use hirsel_tir::{Constant, Constructor};
use hirsel_tir::{Materialized, ValueKind, ValueNode};
use hirsel_type_analysis::cleanup::{CleanupLeaf, cleanup_summary};
use hirsel_types::{GenericArgument, SignatureKind, Substitution, TypeId, TypeKind};
fn invalid(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "compile-time materialization".into(),
        message: message.into(),
    })
}
// Spec #44 leaves callable and resource constant semantics unsettled. Until
// executable constant environments have a contract, only data is transported.
fn non_data(site: Span) -> QueryError {
    let mut diagnostic = Diagnostic::error(
        const { hirsel_diag::RuleId::of("CT-2") },
        site,
        "compile-time values cannot materialize resources, capabilities, closures, type shapes or generated bodies",
    );
    diagnostic
        .notes
        .push("constant value semantics are tracked by spec issue #44".into());
    QueryError::Diagnostics(vec![diagnostic])
}
/// The declared views reification needs. Implementations record the public
/// signature and, when the owning package sees an opaque shape, its private
/// representation. The iterative value conversion itself has no engine state.
pub(crate) trait MaterializationSource {
    fn signature(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError>;
    fn body_view(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError>;
}

struct EngineMaterialization<'a> {
    engine: &'a mut QueryEngine,
    package: &'a hirsel_syntax::PackageId,
}
impl MaterializationSource for EngineMaterialization<'_> {
    fn signature(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.engine
            .item_signature(definition.path())
            .map(|output| output.value)
    }
    fn body_view(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        let public = self.signature(definition)?;
        let (signature, revision) = self.engine.body_signature(self.package, public)?;
        if revision.is_some() {
            self.engine.read_implementation(definition.path());
        }
        Ok(signature)
    }
}
impl MaterializationSource for FrozenProvider<'_> {
    fn signature(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        SignatureProvider::signature(self, definition)
    }
    fn body_view(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        FrozenProvider::body_view(self, definition)
    }
}

impl QueryEngine {
    /// Reify an evaluated value as compile-time data of the checked type `ty`.
    pub fn reify_value(
        &mut self,
        package: &hirsel_syntax::PackageId,
        value: Value,
        ty: TypeId,
        site: Span,
    ) -> Result<Materialized, QueryError> {
        let types = self.types.clone();
        let memo = self.memo.clone();
        reify_value(
            &types,
            &memo,
            &mut EngineMaterialization {
                engine: self,
                package,
            },
            value,
            ty,
            site,
        )
    }
}

/// Reify a task's interpreter result over its immutable views and local read
/// recorder. Both serial evaluation and S1 workers use this conversion.
pub(crate) fn reify_value(
    types: &hirsel_types::TypeInterner,
    memo: &hirsel_types::TypeMemo,
    source: &mut impl MaterializationSource,
    value: Value,
    ty: TypeId,
    site: Span,
) -> Result<Materialized, QueryError> {
    let root = Materializer {
        types,
        memo,
        source,
    }
    .reify_node(value, ty, site)?;
    Ok(Materialized { root })
}

struct Materializer<'a, S> {
    types: &'a hirsel_types::TypeInterner,
    memo: &'a hirsel_types::TypeMemo,
    source: &'a mut S,
}
impl<S: MaterializationSource> Materializer<'_, S> {
    fn reify_node(
        &mut self,
        value: Value,
        ty: TypeId,
        site: Span,
    ) -> Result<ValueNode, QueryError> {
        let (types, memo) = (self.types, self.memo);
        let source = &mut *self.source;
        enum Work {
            Enter(Option<Value>, TypeId),
            Construct {
                ty: TypeId,
                constructor: Constructor,
                fields: usize,
            },
        }
        let mut pending = vec![Work::Enter(Some(value), ty)];
        let mut nodes = vec![];
        while let Some(work) = pending.pop() {
            let (value, ty) = match work {
                Work::Enter(value, ty) => (
                    value.ok_or_else(|| invalid("materialized field was moved out"))?,
                    ty,
                ),
                Work::Construct {
                    ty,
                    constructor,
                    fields,
                } => {
                    let fields = nodes.split_off(nodes.len() - fields);
                    nodes.push(ValueNode {
                        ty,
                        kind: ValueKind::Construct {
                            constructor,
                            fields,
                        },
                    });
                    continue;
                }
            };
            // CT-3: a shape token or a generated body serves compile-time code only.
            if types
                .get(ty)
                .is_some_and(|t| hirsel_types::compile_time_only(&t.kind))
            {
                return Err(non_data(site));
            }
            let kind = match value {
                Value::Unit => ValueKind::Constant(Constant::Unit),
                Value::Bool(v) => ValueKind::Constant(Constant::Bool(v)),
                Value::Integer(v) => ValueKind::Constant(Constant::Integer(v.bits())),
                Value::Char(v) => ValueKind::Constant(Constant::Integer(v.into())),
                Value::F32(v) => ValueKind::Constant(Constant::F32(v)),
                Value::F64(v) => ValueKind::Constant(Constant::F64(v)),
                Value::String(v) => ValueKind::Constant(Constant::String(v)),
                Value::Bytes(v) => ValueKind::Constant(Constant::Bytes(v)),
                value => {
                    let root = types
                        .get(ty)
                        .ok_or_else(|| invalid("missing checked value type"))?
                        .clone();
                    let mut representation = ty;
                    if let TypeKind::Named {
                        definition,
                        arguments,
                    } = &root.kind
                    {
                        let signature = source.body_view(*definition)?;
                        let SignatureKind::Type(declaration) = &signature.kind else {
                            return Err(invalid("aggregate identity is not a type declaration"));
                        };
                        if declaration.resource || declaration.capability.is_some() {
                            match &value {
                                Value::Resource {
                                    definition, fields, ..
                                }
                                | Value::ResourceVariant {
                                    definition, fields, ..
                                } if declaration.resource => {
                                    require_identity(&root.kind, definition.path())?;
                                    if fields.iter().any(Option::is_none) {
                                        return Err(invalid("materialized field was moved out"));
                                    }
                                }
                                Value::Handle { .. } => (),
                                _ => {
                                    return Err(invalid(
                                        "evaluated value disagrees with its non-data declaration",
                                    ));
                                }
                            }
                            return Err(non_data(site));
                        }
                        // Only the owning package's closed initializer has a
                        // private shape; materialization is not shape reflection.
                        let declared = declaration.representation.ok_or_else(|| {
                            invalid("opaque representation is unavailable to this initializer")
                        })?;
                        let authorities = arguments
                            .iter()
                            .map(|_| hirsel_types::AuthoritySummary::Unknown)
                            .collect::<Vec<_>>();
                        representation = hirsel_types::substitute(
                            types,
                            memo,
                            declared,
                            &Substitution {
                                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                                roles: &signature.metadata.roles,
                                arguments,
                                authorities: &authorities,
                            },
                        )
                        .map_err(|e| invalid(format!("aggregate representation import: {e:?}")))?;
                    }
                    if matches!(
                        (&value, &root.kind),
                        (
                            Value::Closure { .. } | Value::FunctionReference(_),
                            TypeKind::Function(_)
                        )
                    ) {
                        return Err(non_data(site));
                    }
                    // Keyed collections may hide non-data payloads or dictionaries.
                    // Materialized Map/Set values use the data-only constructors;
                    // dictionary-bearing SortedMap remains outside this contract.
                    if matches!(
                        (&root.kind, &value),
                        (
                            TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::Map,
                                ..
                            },
                            Value::Map { .. }
                        ) | (
                            TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::Set,
                                ..
                            },
                            Value::Set { .. }
                        ) | (
                            TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::SortedMap,
                                ..
                            },
                            Value::Record { .. }
                        )
                    ) && !hirsel_types::structural_equality(
                        types,
                        ty,
                        hirsel_types::Bound::Data,
                        &hirsel_types::GenericDeclarations::default(),
                        &mut |d| source.signature(d).map(Some),
                    )? {
                        return Err(non_data(site));
                    }
                    let layout = types.row(representation).kind.clone();
                    let (constructor, values, field_types) = match (value, layout) {
                        (Value::Tuple(values), TypeKind::Tuple(fields)) => {
                            (Constructor::Tuple, values, fields)
                        }
                        (
                            Value::Record {
                                definition,
                                fields: values,
                            },
                            TypeKind::Record(fields),
                        ) => {
                            require_identity(&root.kind, definition.path())?;
                            (
                                Constructor::Record(definition.path().clone()),
                                values,
                                fields.into_iter().map(|f| f.ty).collect(),
                            )
                        }
                        (
                            Value::Distinct {
                                definition,
                                fields: values,
                            },
                            TypeKind::Record(fields),
                        ) => {
                            require_identity(&root.kind, definition.path())?;
                            (
                                Constructor::Record(definition.path().clone()),
                                values,
                                fields.into_iter().map(|f| f.ty).collect(),
                            )
                        }
                        (
                            Value::Variant {
                                variant,
                                fields: values,
                            },
                            TypeKind::Variant(variants),
                        ) => {
                            let TypeKind::Named { definition, .. } = root.kind else {
                                return Err(invalid("variant needs a declared nominal identity"));
                            };
                            let fields = variants
                                .get(variant as usize)
                                .ok_or_else(|| invalid("evaluated variant ordinal is invalid"))?
                                .fields
                                .iter()
                                .map(|(_, t)| *t)
                                .collect();
                            (
                                Constructor::Variant {
                                    definition: definition.path().clone(),
                                    variant,
                                },
                                values,
                                fields,
                            )
                        }
                        (
                            Value::Sequence { kind, elements, .. },
                            TypeKind::Builtin {
                                kind: expected,
                                arguments,
                            },
                        ) if kind == expected => {
                            let Some(GenericArgument::Type(element)) = arguments.first() else {
                                return Err(invalid("sequence element type is missing"));
                            };
                            let fields = vec![*element; elements.len()];
                            (Constructor::Sequence, elements.into_vec(), fields.into())
                        }
                        (
                            Value::Map { entries, .. },
                            layout @ TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::Map,
                                ..
                            },
                        ) => {
                            let fields = layout
                                .map_entry_fields()
                                .ok_or_else(|| invalid("map entry type is missing"))?;
                            let kind = hirsel_types::TypeKind::Tuple(fields.to_vec().into());
                            let drop_allocation = cleanup_summary(&kind, |leaf| match leaf {
                                CleanupLeaf::Component(t) => types.row(t).drop_allocation.clone(),
                                _ => unreachable!("tuple cleanup contains only components"),
                            });
                            let entry = types.intern(hirsel_types::Type {
                                drop_allocation,
                                ..hirsel_types::Type::with_kind(kind)
                            });
                            let entry = hirsel_types::normalize_type_properties(
                                types,
                                entry,
                                &hirsel_types::GenericDeclarations::default(),
                                &mut |d| source.signature(d).map(Some),
                            )?;
                            // Removed entries are holes in the simulation's insertion
                            // ordered storage, not partially moved materialized values.
                            let entries: Vec<_> = entries.into_iter().flatten().map(Some).collect();
                            let fields = vec![entry; entries.len()];
                            (Constructor::Map, entries, fields.into())
                        }
                        (
                            Value::Set {
                                entries: elements, ..
                            },
                            TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::Set,
                                arguments,
                            },
                        ) => {
                            let Some(GenericArgument::Type(element)) = arguments.first() else {
                                return Err(invalid("set element type is missing"));
                            };
                            let elements: Vec<_> =
                                elements.into_iter().flatten().map(Some).collect();
                            let fields = vec![*element; elements.len()];
                            (Constructor::Set, elements, fields.into())
                        }
                        (Value::Box(values), TypeKind::Box(element)) => {
                            (Constructor::Box, values, vec![element].into())
                        }
                        _ => {
                            return Err(invalid(
                                "evaluated value has no supported declared data layout",
                            ));
                        }
                    };
                    if values.len() != field_types.len() {
                        return Err(invalid(
                            "evaluated field arity disagrees with its declared type",
                        ));
                    }
                    pending.push(Work::Construct {
                        ty,
                        constructor,
                        fields: values.len(),
                    });
                    pending.extend(
                        values
                            .into_iter()
                            .zip(field_types)
                            .rev()
                            .map(|(value, ty)| Work::Enter(value, ty)),
                    );
                    continue;
                }
            };
            nodes.push(ValueNode { ty, kind });
        }
        Ok(nodes.pop().expect("evaluated value tree has a root"))
    }
}

fn require_identity(kind: &TypeKind, path: &DefPath) -> Result<(), QueryError> {
    if matches!(kind, TypeKind::Named { definition, .. } if definition.path() == path) {
        Ok(())
    } else {
        Err(invalid(
            "evaluated nominal identity disagrees with its checked type",
        ))
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn frozen_materialization_matches_serial_and_records_owning_private_views() {
        let types = registry::session_types();
        let memo = hirsel_types::TypeMemo::new();
        let registry = registry::BuiltinRegistry::new();
        let path = DefPath::core("Option");
        let id = DefId::intern(&path);
        let private = registry.get(id).unwrap();
        let mut public = (*private).clone();
        public.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        let SignatureKind::Type(declaration) = &mut public.kind else {
            panic!("Option is a type");
        };
        declaration.representation = None;
        public.fingerprint = ContentHash::of_bytes(b"opaque Option fixture");
        let snapshot = SignatureSnapshot::new(
            Revision::FIRST,
            types.clone(),
            [(id, Ok(Arc::new(public)))],
            [private],
            None,
        );
        let boolean = types.intern(hirsel_types::Type::boolean());
        let ty = types.intern(hirsel_types::Type::with_kind(TypeKind::Named {
            definition: id,
            arguments: vec![GenericArgument::Type(boolean)].into(),
        }));
        let value = || Value::Variant {
            variant: 1,
            fields: vec![Some(Value::Bool(true))],
        };
        let mut engine =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters())
                .with_types(types.clone());
        let serial = engine
            .reify_value(&path.0.package, value(), ty, Span::default())
            .unwrap();
        let mut recorded = vec![];
        for _ in 0..2 {
            let mut reads = ReadsBuilder::new();
            let mut source = FrozenProvider::new(&snapshot, path.0.package.clone(), &mut reads);
            let frozen =
                reify_value(&types, &memo, &mut source, value(), ty, Span::default()).unwrap();
            assert_eq!(frozen.root, serial.root);
            let reads = reads.finish().unwrap();
            assert!(reads.iter().any(|(key, _)| *key == ReadKey::Signature(id)));
            assert!(reads.iter().any(|(key, digest)| {
                *key == ReadKey::Representation(id)
                    && *digest == snapshot.representation(id).unwrap().unwrap().1.0
            }));
            recorded.push(reads);
        }
        assert_eq!(
            recorded[0], recorded[1],
            "reusing immutable views replays the reads"
        );
        let mut reads = ReadsBuilder::new();
        let mut outsider = FrozenProvider::new(&snapshot, default_context().package, &mut reads);
        assert!(matches!(
            reify_value(&types, &memo, &mut outsider, value(), ty, Span::default()),
            Err(QueryError::Operational(_))
        ));
        assert!(
            !reads
                .finish()
                .unwrap()
                .iter()
                .any(|(key, _)| *key == ReadKey::Representation(id))
        );
    }
}

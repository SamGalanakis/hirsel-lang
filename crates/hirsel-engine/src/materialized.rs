//! Reify interpreter values with checked and declared types, never source reflection.
use crate::*;
use hirsel_interp::Value;
use hirsel_span::Span;
use hirsel_tir::{Constant, Constructor};
use hirsel_tir::{Materialized, ValueKind, ValueNode};
use hirsel_types::{GenericArgument, SignatureKind, Substitution, TypeId, TypeKind, TypeStore};
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
        "compile-time values cannot materialize resources, capabilities or closures",
    );
    diagnostic
        .notes
        .push("constant value semantics are tracked by spec issue #44".into());
    QueryError::Diagnostics(vec![diagnostic])
}
impl QueryEngine {
    /// Reify an evaluated value as compile-time data of the checked type `ty`.
    pub fn reify_value(
        &mut self,
        package: &hirsel_syntax::PackageId,
        value: Value,
        source: &TypeStore,
        ty: TypeId,
        site: Span,
    ) -> Result<Materialized, QueryError> {
        let mut types = source.clone();
        let root = self.reify_node(package, value, ty, &mut types, site)?;
        Ok(Materialized { types, root })
    }
    fn reify_node(
        &mut self,
        package: &hirsel_syntax::PackageId,
        value: Value,
        ty: TypeId,
        types: &mut TypeStore,
        site: Span,
    ) -> Result<ValueNode, QueryError> {
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
            let kind = match value {
                Value::Unit => ValueKind::Constant(Constant::Unit),
                Value::Bool(v) => ValueKind::Constant(Constant::Bool(v)),
                Value::Integer(v) => ValueKind::Constant(Constant::Integer(v.bits())),
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
                        let public = self.item_signature(definition)?.value;
                        let (signature, revision) = self.body_signature(package, public)?;
                        if revision.is_some() {
                            self.read_implementation(definition);
                        }
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
                                    require_identity(&root.kind, definition)?;
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
                        representation = hirsel_types::substitute_type(
                            &signature.types,
                            declared,
                            types,
                            Substitution {
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
                        (Value::Closure { .. }, TypeKind::Function(_))
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
                        &mut |path| self.item_signature(path).map(|output| Some(output.value)),
                    )? {
                        return Err(non_data(site));
                    }
                    let layout = types.get(representation).unwrap().kind.clone();
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
                            require_identity(&root.kind, &definition)?;
                            (
                                Constructor::Record(definition),
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
                            require_identity(&root.kind, &definition)?;
                            (
                                Constructor::Record(definition),
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
                                    definition,
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
                            (Constructor::Sequence, elements, fields.into())
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
                            let drop_allocation =
                                hirsel_types::cleanup_summary(&kind, |leaf| match leaf {
                                    hirsel_types::CleanupLeaf::Component(t) => {
                                        types.get(t).unwrap().drop_allocation.clone()
                                    }
                                    _ => unreachable!("tuple cleanup contains only components"),
                                });
                            let entry = types.push(hirsel_types::Type {
                                drop_allocation,
                                ..hirsel_types::Type::with_kind(kind)
                            });
                            let entry = hirsel_types::normalize_type_properties(
                                types,
                                entry,
                                &hirsel_types::GenericDeclarations::default(),
                                &mut |path| {
                                    self.item_signature(path).map(|output| Some(output.value))
                                },
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
    if matches!(kind, TypeKind::Named { definition, .. } if definition == path) {
        Ok(())
    } else {
        Err(invalid(
            "evaluated nominal identity disagrees with its checked type",
        ))
    }
}

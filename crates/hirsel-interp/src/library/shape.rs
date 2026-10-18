//! CT-3 `type_shape.info`: a nominal type's declared representation as core data.
//! It reads the checked declaration of the call's type actual, never a body; the
//! `shape(T)` site already proved the representation visible (GEN-3, PROG-2).
use super::dispatch::variant;
use super::{none, some};
use crate::{Error, OperationCall, Value, invalid};
use hirsel_syntax::{DefPath, Visibility};
use hirsel_types::{BuiltinType, CanonicalSpelling, GenericArgument, SignatureKind, TypeKind};

pub(super) fn info(call: &OperationCall<'_>) -> Result<Value, Error> {
    let [GenericArgument::Type(ty)] = call.generics else {
        return Err(invalid("type_shape.info type actual"));
    };
    let Some(TypeKind::Named {
        definition,
        arguments,
    }) = call.types.get(*ty).map(|t| &t.kind)
    else {
        return Err(invalid("type_shape.info requires a nominal type actual"));
    };
    let signature = call
        .runtime
        .serialization
        .get(definition)
        .ok_or_else(|| invalid("type_shape.info declaration is not loaded"))?;
    let SignatureKind::Type(declaration) = &signature.kind else {
        return Err(invalid("type_shape.info actual is not a type"));
    };
    let representation = declaration
        .representation
        .and_then(|r| signature.types.get(r))
        .ok_or_else(|| invalid("type_shape.info representation is hidden"))?;
    let actual = CanonicalSpelling {
        types: call.types,
        arguments: &[],
        outer: None,
    };
    let declared = CanonicalSpelling {
        types: &signature.types,
        arguments,
        outer: Some(&actual),
    };
    let body = match &representation.kind {
        TypeKind::Record(fields) => variant(
            0,
            vec![list(
                fields
                    .iter()
                    .map(|f| {
                        record(
                            "FieldShape",
                            vec![
                                Value::String(f.name.clone()),
                                Value::String(declared.text(f.ty)),
                            ],
                        )
                    })
                    .collect(),
            )],
        ),
        TypeKind::Variant(variants) => variant(
            1,
            vec![list(
                variants
                    .iter()
                    .map(|v| {
                        let payload = v
                            .fields
                            .iter()
                            .map(|(label, ty)| {
                                record(
                                    "PayloadShape",
                                    vec![
                                        label.clone().map_or_else(none, |l| some(Value::String(l))),
                                        Value::String(declared.text(*ty)),
                                    ],
                                )
                            })
                            .collect();
                        record(
                            "VariantShape",
                            vec![Value::String(v.name.clone()), list(payload)],
                        )
                    })
                    .collect(),
            )],
        ),
        _ => return Err(invalid("type_shape.info requires a record or variant type")),
    };
    let visibility = match signature.metadata.visibility {
        Visibility::Private => 0,
        Visibility::Public => 1,
        Visibility::PublicOpaque => 2,
    };
    Ok(record(
        "ShapeInfo",
        vec![
            Value::String(actual.text(*ty)),
            variant(visibility, vec![]),
            body,
        ],
    ))
}

fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: DefPath::core(name),
        fields: fields.into_iter().map(Some).collect(),
    }
}
fn list(elements: Vec<Value>) -> Value {
    Value::Sequence {
        kind: BuiltinType::List,
        elements: elements.into_iter().map(Some).collect(),
        arena: None,
    }
}

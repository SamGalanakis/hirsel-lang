//! CT-3 `type_shape.info`: a nominal type's declared representation as core data.
//! It reads the checked declaration of the call's type actual, never a body; the
//! `shape(T)` site already proved the representation visible (GEN-3, PROG-2).
use super::dispatch::variant;
use super::{none, some};
use crate::{Error, OperationCall, Value, invalid};
use hirsel_syntax::{DefPath, Visibility};
use hirsel_types::{
    BuiltinType, GenericArgument, PassingMode, Primitive, SignatureKind, Size, TypeId, TypeKind,
    TypeStore,
};

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
    let actual = Spelling {
        types: call.types,
        arguments: &[],
        outer: None,
    };
    let declared = Spelling {
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
                            vec![Value::String(f.name.clone()), declared.text(f.ty)],
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
                                        declared.text(*ty),
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
        vec![actual.text(*ty), variant(visibility, vec![]), body],
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

/// Canonical type spelling: valid source in every module, so every nominal
/// type is its full versioned path. A declaration's own parameters spell
/// through the actual arguments, which `outer` owns.
struct Spelling<'a> {
    types: &'a TypeStore,
    arguments: &'a [GenericArgument],
    outer: Option<&'a Spelling<'a>>,
}
impl Spelling<'_> {
    fn text(&self, ty: TypeId) -> Value {
        let mut out = String::new();
        self.write(ty, &mut out);
        Value::String(out)
    }
    fn write(&self, ty: TypeId, out: &mut String) {
        let Some(row) = self.types.get(ty) else {
            out.push_str("<unknown>");
            return;
        };
        match &row.kind {
            TypeKind::Primitive(p) => out.push_str(match p {
                Primitive::String => "String",
                Primitive::Bytes => "Bytes",
                Primitive::Never => "Never",
                Primitive::Bool => "bool",
                Primitive::Char => "char",
                Primitive::U8 => "u8",
                Primitive::U16 => "u16",
                Primitive::U32 => "u32",
                Primitive::U64 => "u64",
                Primitive::U128 => "u128",
                Primitive::I8 => "i8",
                Primitive::I16 => "i16",
                Primitive::I32 => "i32",
                Primitive::I64 => "i64",
                Primitive::I128 => "i128",
                Primitive::F32 => "f32",
                Primitive::F64 => "f64",
            }),
            TypeKind::Tuple(items) => {
                out.push('(');
                for (i, item) in items.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    self.write(*item, out);
                }
                if items.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            }
            TypeKind::Parameter(p) => {
                match (self.arguments.get(p.parameter().0 as usize), self.outer) {
                    (Some(GenericArgument::Type(actual)), Some(outer)) => outer.write(*actual, out),
                    _ => out.push_str("<parameter>"),
                }
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let item = &definition.0;
                out.push_str(&format!("{}@{}/", item.package.name, item.package.version));
                if !item.modules.is_empty() {
                    out.push_str(&item.modules.join("/"));
                    out.push('.');
                }
                out.push_str(&item.name);
                self.arguments_text(arguments, out);
            }
            TypeKind::Builtin { kind, arguments } => {
                out.push_str(&format!("{kind:?}"));
                self.arguments_text(arguments, out);
            }
            TypeKind::Box(inner) => {
                out.push_str("box ");
                self.write(*inner, out);
            }
            TypeKind::Function(f) => {
                out.push_str("fn(");
                for (i, parameter) in f.parameters.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(match parameter.passing {
                        PassingMode::Take => "take ",
                        PassingMode::Mut => "mut ",
                        _ => "",
                    });
                    self.write(parameter.ty, out);
                }
                out.push_str(") -> ");
                self.write(f.result, out);
            }
            TypeKind::Record(_) | TypeKind::Variant(_) | TypeKind::Error => {
                out.push_str("<unknown>")
            }
        }
    }
    fn arguments_text(&self, arguments: &[GenericArgument], out: &mut String) {
        if arguments.is_empty() {
            return;
        }
        out.push('[');
        for (i, argument) in arguments.iter().enumerate() {
            if i != 0 {
                out.push_str(", ");
            }
            match argument {
                GenericArgument::Type(ty) => self.write(*ty, out),
                GenericArgument::Size(Size::Constant(n)) => out.push_str(&n.to_string()),
                GenericArgument::Size(Size::Parameter(p)) => {
                    match (self.arguments.get(p.parameter().0 as usize), self.outer) {
                        (Some(GenericArgument::Size(Size::Constant(n))), Some(_)) => {
                            out.push_str(&n.to_string())
                        }
                        _ => out.push_str("<size>"),
                    }
                }
            }
        }
        out.push(']');
    }
}

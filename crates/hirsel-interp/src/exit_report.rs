//! PROG-3 host exit text, separate from semantic program output and observations.
use crate::Value;
use crate::aggregate::RuntimeTypes;
use hirsel_types::logical::{Logical, LogicalNominal, Resolved, Scope, type_arguments};
use hirsel_types::{Signature, SignatureKind, TypeId, TypeKind};

/// A host report's position among captured program stream bytes. The report
/// itself is excluded from semantic observations and Io tapes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub text: String,
    pub stdout_len: usize,
    pub stderr_len: usize,
}

/// Format a borrowed returned Error before the host consumes its owner.
pub fn returned_error(
    value: &Value,
    signature: &Signature,
    runtime: &RuntimeTypes,
) -> Option<String> {
    let SignatureKind::Function(function) = &signature.kind else {
        return None;
    };
    let nominal = |path: &hirsel_syntax::DefPath| {
        runtime.get(path).map(|n| match &n.representation {
            Some((types, shape)) => LogicalNominal::Representation {
                types,
                shape: *shape,
            },
            None => LogicalNominal::Opaque,
        })
    };
    let logical = Logical::new(&nominal);
    let TypeKind::Named { arguments, .. } = &signature.types.get(function.result)?.kind else {
        return None;
    };
    let hirsel_types::GenericArgument::Type(ty) = arguments.get(1)? else {
        return None;
    };
    let ty = *ty;
    let scope = Scope::root(&signature.types);
    let Value::Variant { variant: 1, fields } = value else {
        return None;
    };
    let value = fields.first()?.as_ref()?;
    let mut out = String::from("error: ");
    enum Work<'a> {
        Text(String),
        Value(&'a Value, TypeId, Scope<'a>, bool),
    }
    let mut work = vec![Work::Value(value, ty, scope, true)];
    let mut retained_scopes = vec![];
    while let Some(item) = work.pop() {
        let Work::Value(value, ty, scope, top) = item else {
            if let Work::Text(text) = item {
                out.push_str(&text);
            }
            continue;
        };
        let resolved = logical.resolve(ty, &scope).ok();
        let (kind, scope) = match &resolved {
            Some(Resolved::Structural(row, scope)) => (Some(&row.kind), scope.clone()),
            _ => (None, scope),
        };
        // Retain parent frames until their children have finished, then release
        // them youngest first so a deep recursive value cannot recurse in Rc drop.
        retained_scopes.push(scope.clone());
        if let Some(TypeKind::Builtin { kind, arguments }) = kind {
            use hirsel_types::BuiltinType::*;
            if *kind == Packed {
                if let Some(ty) = type_arguments(arguments).first() {
                    work.push(Work::Value(value, *ty, scope, top));
                    continue;
                }
            } else if !matches!(kind, List | Array | Inline | Simd) {
                out.push_str(&format!("<{kind:?}>"));
                continue;
            }
        }
        match value {
            Value::Unit => out.push_str("()"),
            Value::Bool(v) => out.push_str(&v.to_string()),
            Value::Integer(v) => out.push_str(&if v.is_signed() {
                v.signed().to_string()
            } else {
                v.bits().to_string()
            }),
            // The scalar value, as the native report reads a char's word.
            Value::Char(v) => out.push_str(&u32::from(*v).to_string()),
            Value::F32(v) => out.push_str(&format!("{:?}", f32::from_bits(*v))),
            Value::F64(v) => out.push_str(&format!("{:?}", f64::from_bits(*v))),
            Value::String(v) if top => out.push_str(v),
            Value::String(v) => out.push_str(&format!("{v:?}")),
            Value::Bytes(v) => out.push_str(&format!("{v:?}")),
            Value::Handle { kind, .. } => out.push_str(&format!("<{kind}>")),
            Value::FunctionReference(_) | Value::Closure { .. } => out.push_str("<function>"),
            _ => {
                let fields = value.fields().expect("aggregate value");
                let (open, close, specs): (String, &str, Vec<(Option<String>, TypeId)>) = match kind
                {
                    Some(TypeKind::Tuple(ts)) => (
                        "(".into(),
                        if ts.len() == 1 { ",)" } else { ")" },
                        ts.iter().map(|t| (None, *t)).collect(),
                    ),
                    Some(TypeKind::Record(fs)) => (
                        "{ ".into(),
                        " }",
                        fs.iter().map(|f| (Some(f.name.clone()), f.ty)).collect(),
                    ),
                    Some(TypeKind::Variant(vs)) => {
                        let ordinal = match value {
                            Value::Variant { variant, .. }
                            | Value::ResourceVariant { variant, .. } => *variant,
                            _ => 0,
                        };
                        let v = &vs[ordinal as usize];
                        let named = v.fields.first().is_some_and(|(n, _)| n.is_some());
                        if v.fields.is_empty() {
                            (v.name.clone(), "", vec![])
                        } else {
                            (
                                format!("{}{}", v.name, if named { " { " } else { "(" }),
                                if named { " }" } else { ")" },
                                v.fields.to_vec(),
                            )
                        }
                    }
                    Some(TypeKind::Box(t)) => ("box ".into(), "", vec![(None, *t)]),
                    Some(TypeKind::Builtin { arguments, .. }) => {
                        let ts = type_arguments(arguments);
                        (
                            "[".into(),
                            "]",
                            fields
                                .iter()
                                .enumerate()
                                .map(|(i, _)| (None, ts[i % ts.len()]))
                                .collect(),
                        )
                    }
                    _ => ("(".into(), ")", vec![]),
                };
                out.push_str(&open);
                work.push(Work::Text(close.into()));
                for (index, ((label, ty), field)) in specs.iter().zip(fields).enumerate().rev() {
                    if let Some(field) = field {
                        work.push(Work::Value(field, *ty, scope.clone(), false));
                    } else {
                        work.push(Work::Text("<moved>".into()));
                    }
                    if let Some(label) = label {
                        work.push(Work::Text(format!("{label}: ")));
                    }
                    if index != 0 {
                        work.push(Work::Text(", ".into()));
                    }
                }
            }
        }
    }
    while retained_scopes.pop().is_some() {}
    out.push('\n');
    Some(out)
}

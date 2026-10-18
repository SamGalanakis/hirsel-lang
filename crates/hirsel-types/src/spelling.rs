//! Source-shaped type names for diagnostics. This never queries a body.
use crate::*;
use hirsel_syntax::PackageId;
use std::collections::BTreeSet;

/// Source-shaped spelling of `root` for diagnostics. Core definitions and
/// those of `package` print by module path, others by versioned path. With
/// `modes`, each row also shows its written `unique`, `once` and `sendable`
/// modifiers, so types that differ only in a mode are told apart.
pub fn type_spelling(
    store: &TypeStore,
    root: TypeId,
    names: &GenericDeclarations,
    package: &PackageId,
    modes: bool,
) -> String {
    enum Part {
        Type(TypeId),
        Text(String),
        Leave(TypeId),
    }
    fn arguments(parts: &mut Vec<Part>, args: &[GenericArgument], names: &GenericDeclarations) {
        if args.is_empty() {
            return;
        }
        parts.push(Part::Text("[".into()));
        for (i, arg) in args.iter().enumerate() {
            if i != 0 {
                parts.push(Part::Text(", ".into()));
            }
            parts.push(match arg {
                GenericArgument::Type(ty) => Part::Type(*ty),
                GenericArgument::Size(Size::Constant(n)) => Part::Text(n.to_string()),
                GenericArgument::Size(Size::Parameter(p)) => Part::Text(match names.get(*p) {
                    Some(GenericParameter::Size { name }) => name.clone(),
                    _ => format!("n{}", p.parameter().0),
                }),
            });
        }
        parts.push(Part::Text("]".into()));
    }
    let mut output = String::new();
    let mut active = BTreeSet::new();
    let mut pending = vec![Part::Type(root)];
    while let Some(part) = pending.pop() {
        let id = match part {
            Part::Text(text) => {
                output.push_str(&text);
                continue;
            }
            Part::Leave(id) => {
                active.remove(&id);
                continue;
            }
            Part::Type(id) => id,
        };
        if !active.insert(id) {
            output.push('…');
            continue;
        }
        pending.push(Part::Leave(id));
        let Some(row) = store.get(id) else {
            output.push_str("<unknown>");
            continue;
        };
        let mut parts = Vec::new();
        if modes {
            if row.modes.uniqueness == Uniqueness::Unique {
                parts.push(Part::Text("unique ".into()));
            }
            if row.modes.multiplicity == Multiplicity::Once {
                parts.push(Part::Text("once ".into()));
            }
            if row.sendability_requirement == SendabilityRequirement::Sendable {
                parts.push(Part::Text("sendable ".into()));
            }
        }
        match &row.kind {
            TypeKind::Primitive(p) => parts.push(Part::Text(match p {
                Primitive::String => "String".into(),
                Primitive::Bytes => "Bytes".into(),
                Primitive::Never => "Never".into(),
                _ => format!("{p:?}").to_ascii_lowercase(),
            })),
            TypeKind::Tuple(items) => {
                parts.push(Part::Text("(".into()));
                for (i, item) in items.iter().enumerate() {
                    if i != 0 {
                        parts.push(Part::Text(", ".into()));
                    }
                    parts.push(Part::Type(*item));
                }
                if items.len() == 1 {
                    parts.push(Part::Text(",".into()));
                }
                parts.push(Part::Text(")".into()));
            }
            TypeKind::Parameter(p) => parts.push(Part::Text(match names.get(*p) {
                Some(GenericParameter::Type { name, .. }) => name.clone(),
                _ => format!("T{}", p.parameter().0),
            })),
            TypeKind::Named {
                definition,
                arguments: args,
            } => {
                let name = if definition.package_is("core", "1") || definition.0.package == *package
                {
                    definition
                        .0
                        .modules
                        .iter()
                        .chain(std::iter::once(&definition.0.name))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    definition.to_string()
                };
                parts.push(Part::Text(name));
                arguments(&mut parts, args, names);
            }
            TypeKind::Builtin {
                kind,
                arguments: args,
            } => {
                parts.push(Part::Text(format!("{kind:?}")));
                arguments(&mut parts, args, names);
            }
            TypeKind::Box(inner) => {
                parts.push(Part::Text("box ".into()));
                parts.push(Part::Type(*inner));
            }
            TypeKind::Record(fields) => {
                parts.push(Part::Text("{ ".into()));
                for (i, field) in fields.iter().enumerate() {
                    if i != 0 {
                        parts.push(Part::Text(", ".into()));
                    }
                    parts.push(Part::Text(format!("{}: ", field.name)));
                    parts.push(Part::Type(field.ty));
                }
                parts.push(Part::Text(" }".into()));
            }
            TypeKind::Variant(variants) => {
                for (i, variant) in variants.iter().enumerate() {
                    if i != 0 {
                        parts.push(Part::Text(" | ".into()));
                    }
                    parts.push(Part::Text(format!("{}(", variant.name)));
                    for (j, (name, ty)) in variant.fields.iter().enumerate() {
                        if j != 0 {
                            parts.push(Part::Text(", ".into()));
                        }
                        if let Some(name) = name {
                            parts.push(Part::Text(format!("{name}: ")));
                        }
                        parts.push(Part::Type(*ty));
                    }
                    parts.push(Part::Text(")".into()));
                }
            }
            TypeKind::Function(f) => {
                parts.push(Part::Text("fn(".into()));
                for (i, parameter) in f.parameters.iter().enumerate() {
                    if i != 0 {
                        parts.push(Part::Text(", ".into()));
                    }
                    let passing = match parameter.passing {
                        hirsel_syntax::PassingMode::Borrow => "",
                        hirsel_syntax::PassingMode::Take => "take ",
                        hirsel_syntax::PassingMode::Mut => "mut ",
                    };
                    parts.push(Part::Text(passing.into()));
                    parts.push(Part::Type(parameter.ty));
                }
                parts.push(Part::Text(") -> ".into()));
                parts.push(Part::Type(f.result));
            }
            TypeKind::Error => parts.push(Part::Text("<unknown>".into())),
        }
        pending.extend(parts.into_iter().rev());
    }
    output
}

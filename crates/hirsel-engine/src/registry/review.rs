//! The review table's text rendering of declared types and functions
//! (`review-table.txt`), shared by tests that compare normalized signatures.
use hirsel_types::*;

pub fn describe_type(types: &TypeInterner, id: TypeId) -> String {
    let t = types.row(id);
    let mut out = match &t.kind {
        TypeKind::Primitive(p) => format!("{p:?}"),
        TypeKind::Tuple(items) => format!(
            "({})",
            items
                .iter()
                .map(|id| describe_type(types, *id))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Parameter(p) => format!("T{}", p.parameter().0),
        TypeKind::Named {
            definition,
            arguments,
        } => format!(
            "{}[{}]",
            definition.path().0.name,
            arguments
                .iter()
                .map(|a| describe_arg(types, a))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Builtin { kind, arguments } => format!(
            "{kind:?}[{}]",
            arguments
                .iter()
                .map(|a| describe_arg(types, a))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Box(id) => format!("box {}", describe_type(types, *id)),
        TypeKind::Function(f) => format!("fn{}", describe_function(types, f)),
        TypeKind::Record(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|f| format!("{}:{}", f.name, describe_type(types, f.ty)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Variant(variants) => variants
            .iter()
            .map(|v| {
                format!(
                    "{}({})",
                    v.name,
                    v.fields
                        .iter()
                        .map(|(_, id)| describe_type(types, *id))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join("|"),
        TypeKind::Error => panic!("recovery type"),
    };
    if let Some(target) = t.allocation_target {
        out += &format!(" allocator={target:?}");
    }
    if !t.modes.relation.is_global() {
        out += &format!("@{:?}", t.modes.relation);
    }
    if t.modes.uniqueness == Uniqueness::Unique {
        out += "@unique";
    }
    if t.modes.multiplicity == Multiplicity::Once {
        out += "@once";
    }
    if let Some(captures) = &t.erased_captures {
        out += &format!(" captures={captures:?}");
    }
    for origin in &t.brand.alternatives {
        out += &format!(" of {:?}", origin.root);
    }
    out
}
pub fn describe_arg(types: &TypeInterner, arg: &GenericArgument) -> String {
    match arg {
        GenericArgument::Type(id) => describe_type(types, *id),
        GenericArgument::Size(size) => format!("{size:?}"),
    }
}
pub fn describe_function(types: &TypeInterner, f: &FunctionType) -> String {
    format!(
        "({})->{} ; {:?} ; authority={:?} ; uses={:?}",
        f.parameters
            .iter()
            .map(|p| format!(
                "{:?} {}:{}",
                p.passing,
                p.name.as_deref().unwrap_or("_"),
                describe_type(types, p.ty)
            ))
            .collect::<Vec<_>>()
            .join(","),
        describe_type(types, f.result),
        f.allocation,
        f.authority,
        f.uses
    )
}

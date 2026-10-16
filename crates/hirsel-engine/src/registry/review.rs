//! The review table's text rendering of declared types and functions
//! (`review-table.txt`), shared by tests that compare normalized signatures.
use hirsel_types::*;

pub fn describe_type(s: &Signature, id: TypeId) -> String {
    let t = s.types.get(id).unwrap();
    let mut out = match &t.kind {
        TypeKind::Primitive(p) => format!("{p:?}"),
        TypeKind::Tuple(items) => format!(
            "({})",
            items
                .iter()
                .map(|id| describe_type(s, *id))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Parameter(p) => format!("T{}", p.parameter().0),
        TypeKind::Named {
            definition,
            arguments,
        } => format!(
            "{}[{}]",
            definition.0.name,
            arguments
                .iter()
                .map(|a| describe_arg(s, a))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Builtin { kind, arguments } => format!(
            "{kind:?}[{}]",
            arguments
                .iter()
                .map(|a| describe_arg(s, a))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeKind::Box(id) => format!("box {}", describe_type(s, *id)),
        TypeKind::Function(f) => format!("fn{}", describe_function(s, f)),
        TypeKind::Record(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|f| format!("{}:{}", f.name, describe_type(s, f.ty)))
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
                        .map(|(_, id)| describe_type(s, *id))
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
pub fn describe_arg(s: &Signature, arg: &GenericArgument) -> String {
    match arg {
        GenericArgument::Type(id) => describe_type(s, *id),
        GenericArgument::Size(size) => format!("{size:?}"),
    }
}
pub fn describe_function(s: &Signature, f: &FunctionType) -> String {
    format!(
        "({})->{} ; {:?} ; authority={:?} ; uses={:?}",
        f.parameters
            .iter()
            .map(|p| format!(
                "{:?} {}:{}",
                p.passing,
                p.name.as_deref().unwrap_or("_"),
                describe_type(s, p.ty)
            ))
            .collect::<Vec<_>>()
            .join(","),
        describe_type(s, f.result),
        f.allocation,
        f.authority,
        f.uses
    )
}

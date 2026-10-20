//! Source-shaped type names for diagnostics and the canonical spelling of
//! CT-3 shapes and CT-5 instances. This never queries a body.
use crate::*;
use hirsel_syntax::PackageId;
use std::collections::{BTreeMap, BTreeSet};

/// Source-shaped spelling of `root` for diagnostics. Core definitions and
/// those of `package` print by module path, others by versioned path. With
/// `modes`, each row also shows its written `unique`, `once` and `sendable`
/// modifiers, and a function type the relation of each parameter and its
/// result (`fn() -> T in env`), so types that differ only in a mode are told
/// apart.
pub fn type_spelling(
    store: &TypeStore,
    root: TypeId,
    names: &GenericDeclarations,
    package: &PackageId,
    modes: bool,
) -> String {
    spelling(store, root, names, package, modes, &BTreeSet::new())
}

/// The spellings of two types a diagnostic sets side by side. A short name
/// that stands for two definitions across them, such as a user `Map` and
/// core's, prints as each one's full path (`app@1/Map`, `core@1/Map`), so the
/// two never read alike.
pub fn type_spellings_apart(
    store: &TypeStore,
    [left, right]: [TypeId; 2],
    names: &GenericDeclarations,
    package: &PackageId,
    modes: bool,
) -> [String; 2] {
    let mut heads: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![left, right];
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = store.get(id) else { continue };
        let arguments = |args: &[GenericArgument]| {
            args.iter()
                .filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                })
                .collect::<Vec<_>>()
        };
        match &row.kind {
            TypeKind::Named {
                definition,
                arguments: args,
            } => {
                heads
                    .entry(short_name(definition, package))
                    .or_default()
                    .insert(definition.to_string());
                pending.extend(arguments(args));
            }
            TypeKind::Builtin {
                kind,
                arguments: args,
            } => {
                heads
                    .entry(format!("{kind:?}"))
                    .or_default()
                    .insert(format!("core@1/{kind:?}"));
                pending.extend(arguments(args));
            }
            TypeKind::Tuple(items) => pending.extend(items.iter().copied()),
            TypeKind::Box(inner) => pending.push(*inner),
            TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| f.ty)),
            TypeKind::Variant(variants) => pending.extend(
                variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|(_, t)| *t)),
            ),
            TypeKind::Function(f) => {
                pending.extend(f.parameters.iter().map(|p| p.ty).chain([f.result]))
            }
            TypeKind::Primitive(_) | TypeKind::Parameter(_) | TypeKind::Error => {}
        }
    }
    let qualify = heads
        .into_iter()
        .filter(|(_, identities)| identities.len() > 1)
        .map(|(name, _)| name)
        .collect();
    [left, right].map(|root| spelling(store, root, names, package, modes, &qualify))
}

/// A definition's diagnostic name: its module path for core and `package`,
/// its full path otherwise.
fn short_name(definition: &DefPath, package: &PackageId) -> String {
    if definition.package_is("core", "1") || definition.0.package == *package {
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
    }
}

fn spelling(
    store: &TypeStore,
    root: TypeId,
    names: &GenericDeclarations,
    package: &PackageId,
    modes: bool,
    qualify: &BTreeSet<String>,
) -> String {
    enum Part {
        Type(TypeId),
        Text(String),
        Leave(TypeId),
        /// A function type's relation, spelled inside its own binder.
        Relation(TypeId),
        /// Enter or leave a function type's parameter binder.
        Bind(Vec<Option<String>>),
        Unbind,
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
    let mut binders: Vec<Vec<Option<String>>> = vec![];
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
            Part::Bind(names) => {
                binders.push(names);
                continue;
            }
            Part::Unbind => {
                binders.pop();
                continue;
            }
            Part::Relation(id) => {
                if let Some(row) = store.get(id) {
                    output.push_str(&relation_spelling(&row.modes.relation, &binders));
                }
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
                let name = short_name(definition, package);
                parts.push(Part::Text(if qualify.contains(&name) {
                    definition.to_string()
                } else {
                    name
                }));
                arguments(&mut parts, args, names);
            }
            TypeKind::Builtin {
                kind,
                arguments: args,
            } => {
                let name = format!("{kind:?}");
                parts.push(Part::Text(if qualify.contains(&name) {
                    format!("core@1/{name}")
                } else {
                    name
                }));
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
                // A parameter a relation of this header names is spelled with
                // its name, so `in x` refers to something visible.
                let named: BTreeSet<u32> = if modes {
                    f.parameters
                        .iter()
                        .map(|p| p.ty)
                        .chain([f.result])
                        .filter_map(|ty| store.get(ty))
                        .flat_map(|row| row.modes.relation.reaches().to_vec())
                        .filter_map(|reach| match reach {
                            Reach::Place(Origin {
                                root:
                                    OriginRoot::Parameter(ParameterRef::Bound {
                                        depth: 0,
                                        parameter,
                                    }),
                                ..
                            }) => Some(parameter.0),
                            _ => None,
                        })
                        .collect()
                } else {
                    BTreeSet::new()
                };
                parts.push(Part::Bind(
                    f.parameters.iter().map(|p| p.name.clone()).collect(),
                ));
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
                    if let Some(name) = &parameter.name
                        && named.contains(&(i as u32))
                    {
                        parts.push(Part::Text(format!("{name}: ")));
                    }
                    parts.push(Part::Type(parameter.ty));
                    if modes {
                        parts.push(Part::Relation(parameter.ty));
                    }
                }
                parts.push(Part::Text(") -> ".into()));
                parts.push(Part::Type(f.result));
                if modes {
                    parts.push(Part::Relation(f.result));
                }
                parts.push(Part::Unbind);
            }
            TypeKind::Error => parts.push(Part::Text("<unknown>".into())),
        }
        pending.extend(parts.into_iter().rev());
    }
    output
}

/// ` in S` for a non-global relation inside function-type binders, innermost
/// last: `env`, a parameter's name, or `{env, ..}` for several. A field path
/// is not spelled, since field names live in declarations; a place this
/// binder cannot name (a body local or region) is `_`.
fn relation_spelling(relation: &Relation, binders: &[Vec<Option<String>>]) -> String {
    let mut reaches = relation.reaches().to_vec();
    reaches.sort_by_key(|reach| !matches!(reach, Reach::Environment));
    let places: Vec<String> = reaches
        .iter()
        .map(|reach| match reach {
            Reach::Environment => "env".into(),
            Reach::Place(Origin {
                root: OriginRoot::Parameter(ParameterRef::Bound { depth, parameter }),
                ..
            }) => binders
                .len()
                .checked_sub(*depth as usize + 1)
                .and_then(|binder| binders[binder].get(parameter.0 as usize))
                .cloned()
                .flatten()
                .unwrap_or_else(|| "_".into()),
            Reach::Place(_) | Reach::Region(_) => "_".into(),
        })
        .collect();
    match places.as_slice() {
        [] => String::new(),
        [place] => format!(" in {place}"),
        _ => format!(" in {{{}}}", places.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::PassingMode;

    fn function(store: &mut TypeStore, parameters: Vec<Parameter>, result: TypeId) -> TypeId {
        store.push(Type::with_kind(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: parameters.into(),
            result,
            error: None,
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::Unknown,
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        })))
    }
    fn related(store: &mut TypeStore, relation: Relation) -> TypeId {
        let mut row = Type::with_kind(TypeKind::Primitive(Primitive::String));
        row.modes.relation = relation;
        store.push(row)
    }
    fn spell(store: &TypeStore, root: TypeId) -> String {
        let package = PackageId {
            name: "fixture".into(),
            version: "1".into(),
        };
        type_spelling(store, root, &Default::default(), &package, true)
    }

    /// Two headers that differ only in their result relation spell
    /// differently.
    #[test]
    fn function_types_spell_their_relations() {
        let mut store = TypeStore::default();
        let global = related(&mut store, Relation::global());
        let environment = related(&mut store, Relation::environment());
        let plain = function(&mut store, vec![], global);
        let local = function(&mut store, vec![], environment);
        assert_eq!(spell(&store, plain), "fn() -> String");
        assert_eq!(spell(&store, local), "fn() -> String in env");

        let x = Reach::Place(Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(1))),
            fields: Default::default(),
        });
        let both = related(&mut store, Relation::new([Reach::Environment, x]));
        let parameter = |name: &str, passing| Parameter {
            name: Some(name.into()),
            label: None,
            passing,
            ty: global,
        };
        let borrowing = function(
            &mut store,
            vec![
                parameter("w", PassingMode::Take),
                parameter("x", PassingMode::Mut),
            ],
            both,
        );
        assert_eq!(
            spell(&store, borrowing),
            "fn(take String, mut x: String) -> String in {env, x}"
        );
    }
}

/// Canonical type spelling (CT-3): valid source in every module, so every
/// nominal type is its full versioned path. A declaration's own parameters
/// spell through the actual arguments, which `outer` owns. Modes are not
/// spelled.
pub struct CanonicalSpelling<'a> {
    pub types: &'a TypeStore,
    pub arguments: &'a [GenericArgument],
    pub outer: Option<&'a CanonicalSpelling<'a>>,
}
impl CanonicalSpelling<'_> {
    /// The spelling of a type with no free parameters of its own.
    pub fn closed(types: &TypeStore, ty: TypeId) -> String {
        CanonicalSpelling {
            types,
            arguments: &[],
            outer: None,
        }
        .text(ty)
    }
    pub fn text(&self, ty: TypeId) -> String {
        let mut out = String::new();
        self.write(ty, &mut out);
        out
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

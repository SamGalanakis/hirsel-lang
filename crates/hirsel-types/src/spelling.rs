//! Canonical spelling of CT-3 shapes and CT-5 instances.
use crate::*;

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

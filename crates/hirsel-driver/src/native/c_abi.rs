//! The published native data ABI (MEM-7, PROG-4): every non-generic `@layout(c)` record and
//! variant and every `@soa` element of a program, keyed by target, runtime ABI version and
//! layout identity. `hirsel export --c-header` renders the C types; `--native-abi` renders
//! the descriptor. Each C type's identity is one link symbol: the program's launcher object
//! defines it, and the generated header references it, so a foreign object compiled from a
//! header of another ABI version, target or layout leaves it undefined and the link refuses.
use std::collections::BTreeMap;
use std::fmt::Write;

use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind, SizeSpec, TagForm};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{Type, TypeKind};

use super::{NativeError, StageName, TargetSpec};

/// Every published symbol starts with this prefix; a linker naming an undefined one is a
/// data ABI mismatch.
pub(crate) const SYMBOL_PREFIX: &str = "hirsel_data_abi_";

/// One published type: its C spelling, layout and identity.
#[derive(Clone, Debug)]
pub struct DataType {
    pub definition: DefPath,
    /// The C tag name: the module path and type name joined by `_`.
    pub name: String,
    pub layout: LayoutId,
    pub shape: Shape,
    /// The C declaration and its static assertions, the identity's preimage.
    pub declaration: String,
    pub identity: ContentHash,
}

#[derive(Clone, Debug)]
pub enum Shape {
    /// A `@layout(c)` record: field names in declaration order.
    Record(Vec<String>),
    /// A `@layout(c)` variant: each variant's name and field names.
    Variant(Vec<(String, Vec<String>)>),
    /// An `@soa` element: published as columns, with no C declaration.
    Soa,
}

impl DataType {
    /// The link symbol for this identity: the runtime ABI version stays readable so a
    /// refusal can name it.
    pub fn symbol(&self) -> String {
        symbol(&self.name, hirsel_rt::ABI_VERSION, &self.identity)
    }
}

fn symbol(name: &str, abi: u32, identity: &ContentHash) -> String {
    let hex: String = identity.0[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{SYMBOL_PREFIX}v{abi}_{name}_{hex}")
}

/// The data ABI of a program: its target and published types, in dependency order.
#[derive(Clone, Debug)]
pub struct DataAbi {
    pub triple: String,
    pub types: Vec<DataType>,
    pub table: LayoutTable,
}

/// Collect the program's published types. Generic declarations are not published: C has
/// no instantiation to name.
pub(crate) fn data_abi<'a>(
    types: &hirsel_types::TypeInterner,
    signatures: &std::collections::BTreeMap<hirsel_syntax::DefPath, hirsel_types::SignatureRef>,
    representations: impl IntoIterator<
        Item = (
            &'a hirsel_syntax::DefPath,
            Option<&'a hirsel_engine::RepresentationRecord>,
        ),
    >,
    target: &TargetSpec,
) -> Result<DataAbi, NativeError> {
    let error = |message: String| NativeError::new(StageName::Import, message);
    let mut declared = hirsel_layout::layout::Owners::default();
    for (path, record) in representations {
        super::adapt::add_owner(&mut declared, path, &signatures[path], record);
    }
    let owners = hirsel_rc::library::owners(types, signatures).over(&declared);
    let mut layouts = LayoutBuilder::new(target, &owners);
    let mut found = Vec::new();
    for (definition, owner) in &owners.nominals {
        let OwnerRepresentation::Defined(record) = owner else {
            continue;
        };
        let promises = &record.layout_promises;
        if !(promises.c || promises.soa) || !record.generics.is_empty() || record.resource {
            continue;
        }
        let named = types.intern(Type::with_kind(TypeKind::Named {
            definition: *definition,
            arguments: Default::default(),
        }));
        let layout = layouts
            .layout(types, named)
            .map_err(|e| error(format!("{}: {e:?}", record.definition.0.name)))?;
        let shape = match &types.get(record.private_shape).map(|t| &t.kind) {
            _ if promises.soa => Shape::Soa,
            Some(TypeKind::Record(fields)) => {
                Shape::Record(fields.iter().map(|f| f.name.as_str().to_owned()).collect())
            }
            Some(TypeKind::Variant(variants)) => Shape::Variant(
                variants
                    .iter()
                    .map(|v| {
                        let fields = v.fields.iter().enumerate();
                        let names = fields
                            .map(|(i, (name, _))| match name {
                                Some(name) => name.as_str().to_owned(),
                                None => format!("_{i}"),
                            })
                            .collect();
                        (v.name.as_str().to_owned(), names)
                    })
                    .collect(),
            ),
            _ => continue,
        };
        let path = &record.definition.0;
        let mut name = path.modules.join("_");
        if !name.is_empty() {
            name.push('_');
        }
        name.push_str(&path.name);
        found.push((record.definition.clone(), name, layout, shape));
    }
    let table = layouts.finish();
    let names: BTreeMap<LayoutId, String> = found
        .iter()
        .filter(|(_, _, _, shape)| !matches!(shape, Shape::Soa))
        .map(|(_, name, layout, _)| (*layout, name.clone()))
        .collect();
    let mut seen = BTreeMap::new();
    for (definition, name, ..) in &found {
        if let Some(other) = seen.insert(name.clone(), definition.clone()) {
            return Err(error(format!(
                "`{}` and `{}` publish the same C name `{name}`",
                other.0.name, definition.0.name
            )));
        }
    }
    let mut types = Vec::new();
    for (definition, name, layout, shape) in found {
        let declaration = declaration(&table, &names, &name, layout, &shape)
            .map_err(|e| error(format!("{name}: {e}")))?;
        let identity = ContentHash::of_parts(
            "hirsel.data-abi.v1",
            [
                target.triple.as_bytes(),
                &hirsel_rt::ABI_VERSION.to_le_bytes(),
                declaration.as_bytes(),
            ],
        );
        types.push(DataType {
            definition,
            name,
            layout,
            shape,
            declaration,
            identity,
        });
    }
    // Nested C types precede their holders.
    let mut ordered: Vec<DataType> = Vec::new();
    while !types.is_empty() {
        let ready = types.iter().position(|t| {
            uses(&table, t.layout, &names)
                .iter()
                .all(|used| *used == t.layout || ordered.iter().any(|o| o.layout == *used))
        });
        ordered.push(types.remove(ready.unwrap_or(0)));
    }
    Ok(DataAbi {
        triple: target.triple.clone(),
        types: ordered,
        table,
    })
}

/// The published C types a layout's fields name.
fn uses(
    table: &LayoutTable,
    layout: LayoutId,
    names: &BTreeMap<LayoutId, String>,
) -> Vec<LayoutId> {
    let Some(record) = table.layout(layout) else {
        return Vec::new();
    };
    let mut fields: Vec<LayoutId> = record.fields.iter().map(|f| f.layout).collect();
    if let Some(tag) = &record.tag {
        fields.extend(
            tag.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.layout)),
        );
    }
    let mut used = Vec::new();
    while let Some(field) = fields.pop() {
        match table.layout(field).map(|r| &r.kind) {
            Some(LayoutKind::Inline { element, .. }) => fields.push(*element),
            _ if names.contains_key(&field) => used.push(field),
            _ => {}
        }
    }
    used
}

fn size(table: &LayoutTable, layout: LayoutId) -> Result<u64, String> {
    match table.layout(layout).map(|r| &r.size) {
        Some(SizeSpec::Fixed(size)) => Ok(*size),
        _ => Err("a published type without a fixed size".into()),
    }
}

/// A field's C spelling: the type before the name and the array suffix after it.
fn c_type(
    table: &LayoutTable,
    names: &BTreeMap<LayoutId, String>,
    layout: LayoutId,
) -> Result<(String, String), String> {
    if let Some(name) = names.get(&layout) {
        return Ok((format!("struct {name}"), String::new()));
    }
    let record = table.layout(layout).ok_or("an unknown field layout")?;
    Ok(match &record.kind {
        LayoutKind::Scalar(scalar) => (scalar_type(*scalar).into(), String::new()),
        LayoutKind::Inline { element, count } => {
            let (ty, suffix) = c_type(table, names, *element)?;
            (ty, format!("[{count}]{suffix}"))
        }
        other => return Err(format!("a field of layout {other:?} has no C spelling")),
    })
}

fn scalar_type(scalar: ScalarKind) -> &'static str {
    match scalar {
        ScalarKind::Bool => "_Bool",
        ScalarKind::U8 => "uint8_t",
        ScalarKind::U16 => "uint16_t",
        ScalarKind::U32 => "uint32_t",
        ScalarKind::U64 => "uint64_t",
        ScalarKind::U128 => "unsigned __int128",
        ScalarKind::I8 => "int8_t",
        ScalarKind::I16 => "int16_t",
        ScalarKind::I32 => "int32_t",
        ScalarKind::I64 => "int64_t",
        ScalarKind::I128 => "__int128",
        ScalarKind::F32 => "float",
        ScalarKind::F64 => "double",
        ScalarKind::Pointer => "void *",
    }
}

/// C keywords a Hirsel field name may collide with get a trailing underscore.
fn c_name(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "auto", "char", "const", "default", "double", "enum", "extern", "float", "goto", "int",
        "long", "register", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
        "union", "unsigned", "void", "volatile",
    ];
    if KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_owned()
    }
}

/// The C declaration of one published type, with static assertions of its size,
/// alignment and every field offset. An `@soa` element publishes column macros instead.
fn declaration(
    table: &LayoutTable,
    names: &BTreeMap<LayoutId, String>,
    name: &str,
    layout: LayoutId,
    shape: &Shape,
) -> Result<String, String> {
    let record = table.layout(layout).ok_or("an unknown layout")?;
    let total = size(table, layout)?;
    let mut out = String::new();
    let mut asserts = Vec::new();
    let mut member = |out: &mut String, indent: &str, field: &str, at: LayoutId| {
        c_type(table, names, at).map(|(ty, suffix)| {
            writeln!(out, "{indent}{ty} {}{suffix};", c_name(field)).unwrap();
        })
    };
    match shape {
        Shape::Record(fields) => {
            writeln!(out, "struct {name} {{").unwrap();
            for field in &record.fields {
                let field_name = &fields[field.ordinal as usize];
                member(&mut out, "    ", field_name, field.layout)?;
                asserts.push((c_name(field_name), field.offset));
            }
            writeln!(out, "}};").unwrap();
        }
        Shape::Variant(variants) => {
            let tag = record.tag.as_ref().ok_or("a C variant without a tag")?;
            if tag.offset != 0 || tag.form != TagForm::Ordinal {
                return Err("a C variant whose tag is not an ordinal at offset 0".into());
            }
            for (ordinal, (variant, _)) in variants.iter().enumerate() {
                writeln!(out, "#define {name}_{variant} {ordinal}u").unwrap();
            }
            writeln!(out, "struct {name} {{\n    uint32_t tag;").unwrap();
            let arms: Vec<_> = tag
                .variants
                .iter()
                .filter(|v| !v.fields.is_empty())
                .collect();
            if !arms.is_empty() {
                writeln!(out, "    union {{").unwrap();
                let start = arms
                    .iter()
                    .flat_map(|v| &v.fields)
                    .map(|f| f.offset)
                    .min()
                    .unwrap();
                for arm in arms {
                    let (variant, fields) = &variants[arm.ordinal as usize];
                    writeln!(out, "        struct {{").unwrap();
                    for field in &arm.fields {
                        let field_name = &fields[field.ordinal as usize];
                        member(&mut out, "            ", field_name, field.layout)?;
                        asserts.push((
                            format!("payload.{}.{}", c_name(variant), c_name(field_name)),
                            field.offset,
                        ));
                    }
                    writeln!(out, "        }} {};", c_name(variant)).unwrap();
                }
                writeln!(out, "    }} payload;").unwrap();
                asserts.push(("payload".into(), start));
            }
            writeln!(out, "}};").unwrap();
        }
        Shape::Soa => {
            let shape = table.soa_shape(layout).map_err(|e| format!("{e:?}"))?;
            writeln!(
                out,
                "/* @soa {name}: row {total} bytes; column c of capacity n starts at \
                 {} + n * prefix(c) */",
                shape.data_offset
            )
            .unwrap();
            writeln!(out, "#define {name}_SOA_QUANTUM {}u", shape.quantum).unwrap();
            for (index, span) in shape.spans.iter().enumerate() {
                writeln!(
                    out,
                    "#define {name}_SOA_COLUMN_{index}(n) ({}u + (n) * {}u) /* stride {} */",
                    shape.data_offset, span.prefix, span.stride
                )
                .unwrap();
            }
            return Ok(out);
        }
    }
    writeln!(
        out,
        "_Static_assert(sizeof(struct {name}) == {total} && _Alignof(struct {name}) == {}, \
         \"{name} layout\");",
        record.align
    )
    .unwrap();
    for (field, offset) in asserts {
        writeln!(
            out,
            "_Static_assert(offsetof(struct {name}, {field}) == {offset}, \"{name}.{field}\");"
        )
        .unwrap();
    }
    Ok(out)
}

impl DataAbi {
    /// The C header: every published declaration and a reference to its identity symbol,
    /// kept by `used` and `retain` so an object compiled from it links only against the same ABI.
    pub fn header(&self) -> String {
        let mut out = format!(
            "/* Hirsel native data ABI v{abi} for {triple}: generated by `hirsel export \
             --c-header`. */\n#pragma once\n#include <stddef.h>\n#include <stdint.h>\n\n\
             #define HIRSEL_DATA_ABI {abi}u\n\n",
            abi = hirsel_rt::ABI_VERSION,
            triple = self.triple,
        );
        for ty in &self.types {
            out.push_str(&ty.declaration);
            if !matches!(ty.shape, Shape::Soa) {
                let symbol = ty.symbol();
                writeln!(
                    out,
                    "extern const char {symbol};\n\
                     __attribute__((used, retain)) static const char *const hirsel_abi_{name} = &{symbol};",
                    name = ty.name
                )
                .unwrap();
            }
            out.push('\n');
        }
        out
    }

    /// The descriptor: target, ABI version and, per type, its identity, symbol, size,
    /// alignment, field offsets, tag and columns.
    pub fn descriptor(&self) -> String {
        let types: Vec<_> = self
            .types
            .iter()
            .map(|ty| {
                let record = self.table.layout(ty.layout).unwrap();
                let fields: Vec<_> = record
                    .fields
                    .iter()
                    .map(|f| serde_json::json!({"ordinal": f.ordinal, "offset": f.offset}))
                    .collect();
                let mut value = serde_json::json!({
                    "name": ty.name,
                    "definition": format!("{}", ty.definition.0.name),
                    "identity": ty.identity.0.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    "size": size(&self.table, ty.layout).ok(),
                    "align": record.align,
                    "fields": fields,
                });
                if let Some(tag) = &record.tag {
                    value["tag"] = serde_json::json!({
                        "offset": tag.offset,
                        "width": 4,
                        "variants": tag.variants.iter().map(|v| serde_json::json!({
                            "ordinal": v.ordinal,
                            "fields": v.fields.iter().map(|f| serde_json::json!(
                                {"ordinal": f.ordinal, "offset": f.offset})).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                    });
                }
                match ty.shape {
                    Shape::Soa => {
                        let shape = self.table.soa_shape(ty.layout).unwrap();
                        let tag = shape.tag().map(|(_, scalar)| match scalar {
                            ScalarKind::U8 => 1,
                            ScalarKind::U16 => 2,
                            _ => 4,
                        });
                        value["soa"] = serde_json::json!({
                            "data_offset": shape.data_offset,
                            "quantum": shape.quantum,
                            "tag_width": tag,
                            "columns": shape.spans.iter().map(|s| serde_json::json!({
                                "row_offset": s.row_offset,
                                "prefix": s.prefix,
                                "stride": s.stride,
                                "align": s.align,
                            })).collect::<Vec<_>>(),
                        });
                    }
                    _ => value["symbol"] = serde_json::json!(ty.symbol()),
                }
                value
            })
            .collect();
        let descriptor = serde_json::json!({
            "schema": 1,
            "target": self.triple,
            "abi_version": hirsel_rt::ABI_VERSION,
            "types": types,
        });
        serde_json::to_string_pretty(&descriptor).unwrap() + "\n"
    }

    /// A relocatable object defining every published identity symbol, linked with the
    /// program's objects so foreign code compiled from the matching header resolves.
    pub(crate) fn object(&self) -> Result<Vec<u8>, NativeError> {
        use object::write::{Object, Symbol, SymbolSection};
        let architecture = if self.triple.starts_with("aarch64") {
            object::Architecture::Aarch64
        } else {
            object::Architecture::X86_64
        };
        let mut object = Object::new(
            object::BinaryFormat::Elf,
            architecture,
            object::Endianness::Little,
        );
        let section = object.add_section(
            Vec::new(),
            b".rodata.hirsel_data_abi".to_vec(),
            object::SectionKind::ReadOnlyData,
        );
        for ty in self.types.iter().filter(|t| !matches!(t.shape, Shape::Soa)) {
            let offset = object.append_section_data(section, &[0], 1);
            object.add_symbol(Symbol {
                name: ty.symbol().into_bytes(),
                value: offset,
                size: 1,
                kind: object::SymbolKind::Data,
                scope: object::SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(section),
                flags: object::SymbolFlags::None,
            });
        }
        object
            .write()
            .map_err(|e| NativeError::new(StageName::Link, format!("the data ABI object: {e}")))
    }
}

/// A linker failure that names an undefined identity symbol: the foreign objects were
/// compiled from a header of another ABI version, target or layout (PROG-4).
pub(crate) fn refusal(abi: &DataAbi, linker: &str) -> Option<String> {
    let start = linker.find(SYMBOL_PREFIX)?;
    let symbol: String = linker[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let rest = symbol.strip_prefix(SYMBOL_PREFIX)?.strip_prefix('v')?;
    let (version, rest) = rest.split_once('_')?;
    let name = rest.rsplit_once('_').map_or(rest, |(name, _)| name);
    let ours = hirsel_rt::ABI_VERSION;
    let reason = if version != ours.to_string() {
        format!("data ABI v{version}, but this build publishes v{ours}")
    } else if let Some(ty) = abi.types.iter().find(|t| t.name == name) {
        format!(
            "another layout or target of `{}`; this build publishes {} for {}",
            ty.name,
            ty.symbol(),
            abi.triple
        )
    } else {
        format!("a type `{name}` this program does not publish")
    };
    Some(format!(
        "PROG-4: foreign code was compiled from a Hirsel header for {reason} \
         (undefined {symbol}); regenerate it with `hirsel export --c-header`"
    ))
}

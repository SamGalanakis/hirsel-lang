//! PROG-3: derive immutable diagnostic views from checked types and actual Ready layouts.
use super::root_source::LinkSource;
use super::{NativeError, ReadyProgram, StageName};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, SizeSpec, TagForm};
use hirsel_syntax::DefPath;
use hirsel_type_analysis::logical::{Logical, LogicalNominal, Resolved, Scope, type_arguments};
use hirsel_types::{Primitive, SignatureKind, TypeId, TypeKind};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(crate) struct Report {
    pub declarations: String,
    /// The nodes `declarations` defines, for a launcher Cranelift emits.
    pub nodes: Vec<hirsel_cranelift::ReportNode>,
    pub offset: u64,
    pub root: usize,
    pub result: LayoutId,
    pub return_kind: u64,
    pub success_offset: u64,
}
#[derive(Default)]
struct Node {
    kind: u64,
    a: u64,
    b: u64,
    c: u64,
    text: String,
    fields: Vec<(u64, usize, String)>,
}

pub(crate) fn derive(
    input: &LinkSource<'_>,
    program: &ReadyProgram,
    entry: &DefPath,
) -> Result<Report, NativeError> {
    let error = |s: String| NativeError::new(StageName::Launch, format!("exit report: {s}"));
    let signature = &input.signatures[entry];
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(error("not a function".into()));
    };
    let nominal = |definition: hirsel_syntax::DefId| {
        let path = definition.path();
        if let Some(Some(r)) = input.representations.get(path) {
            return Some(LogicalNominal::Representation {
                shape: r.private_shape,
            });
        }
        let s = input.signatures.get(path)?;
        let SignatureKind::Type(t) = &s.kind else {
            return None;
        };
        Some(match t.representation {
            Some(shape) => LogicalNominal::Representation { shape },
            None => LogicalNominal::Opaque,
        })
    };
    let logical = Logical::new(&nominal);
    let instance = program.functions[entry];
    let unit = program
        .summaries
        .iter()
        .find(|s| s.definition == *entry && s.instance == instance)
        .ok_or_else(|| error("no entry unit".into()))?;
    let result = program.table.fn_abis[unit.abi.0 as usize]
        .result
        .as_ref()
        .ok_or_else(|| error("no entry result".into()))?
        .layout;
    let return_kind = match hirsel_types::entry::entry_return(&input.types, function.result) {
        Some(hirsel_types::entry::EntryReturn::UnitResult) => 0,
        Some(hirsel_types::entry::EntryReturn::ExitCode) => 1,
        Some(hirsel_types::entry::EntryReturn::ExitCodeResult) => 2,
        None => return Err(error("invalid entry return type".into())),
    };
    // The offset is derived from the checked layout, never the opaque source shape.
    let mut success_layout = result;
    let mut success_offset = 0;
    if return_kind == 2 {
        let field = program
            .table
            .layout(result)
            .and_then(|r| r.tag.as_ref())
            .and_then(|t| t.variants.iter().find(|v| v.ordinal == 0))
            .and_then(|v| v.fields.first())
            .ok_or_else(|| error("no Ok payload layout".into()))?;
        success_offset = field.offset;
        success_layout = field.layout;
    }
    if return_kind != 0 {
        let mut layout = program
            .table
            .layout(success_layout)
            .ok_or_else(|| error("missing ExitCode layout".into()))?;
        while let Some(field) = layout.fields.first().filter(|_| layout.fields.len() == 1) {
            success_offset += field.offset;
            layout = program
                .table
                .layout(field.layout)
                .ok_or_else(|| error("missing code layout".into()))?;
        }
    }
    if return_kind == 1 {
        return Ok(Report {
            declarations: "%hirsel.report.node = type { i64, i64, i64, i64, ptr, i64, ptr, i64 }\n@hirsel.report.node.0 = private constant %hirsel.report.node zeroinitializer\n".into(),
            nodes: vec![hirsel_cranelift::ReportNode::default()],
            offset: 0, root: 0, result, return_kind, success_offset,
        });
    }
    let TypeKind::Named { arguments, .. } = &input.types.get(function.result).unwrap().kind else {
        unreachable!()
    };
    let Some(hirsel_types::GenericArgument::Type(ty)) = arguments.get(1) else {
        return Err(error("missing Error type".into()));
    };
    let ty = *ty;
    let scope = Scope::root(&input.types);
    let field = program
        .table
        .layout(result)
        .and_then(|r| r.tag.as_ref())
        .and_then(|t| t.variants.iter().find(|v| v.ordinal == 1))
        .and_then(|v| v.fields.first())
        .ok_or_else(|| error("no Error payload layout".into()))?;
    let mut nodes = vec![];
    let mut memo = BTreeMap::new();
    let mut pending = vec![];
    fn child<'a>(
        ty: TypeId,
        scope: Scope<'a>,
        layout: LayoutId,
        nodes: &mut Vec<Node>,
        pending: &mut Vec<(usize, TypeId, Scope<'a>, LayoutId)>,
    ) -> usize {
        let n = nodes.len();
        nodes.push(Node::default());
        pending.push((n, ty, scope, layout));
        n
    }
    let root = child(ty, scope, field.layout, &mut nodes, &mut pending);
    while let Some((index, ty, scope, layout)) = pending.pop() {
        let resolved = logical
            .resolve(ty, &scope)
            .map_err(|e| error(format!("{e:?}")))?;
        let Resolved::Structural(row, scope) = resolved else {
            let name = program
                .table
                .layout(layout)
                .and_then(|r| r.key.owner_revision)
                .and_then(|revision| {
                    input
                        .signatures
                        .values()
                        .find(|s| s.fingerprint == revision)
                        .map(|s| s.definition.path().0.name.as_str())
                })
                .unwrap_or("opaque");
            nodes[index].text = format!("<{name}>");
            continue;
        };
        let key = (row as *const hirsel_types::Type as usize, layout);
        if let Some(previous) = memo.get(&key).copied() {
            nodes[index].kind = 12;
            nodes[index].fields.push((0, previous, String::new()));
            continue;
        }
        memo.insert(key, index);
        let mut record = program
            .table
            .layout(layout)
            .ok_or_else(|| error("missing layout".into()))?;
        if !matches!(
            &row.kind,
            TypeKind::Record(_) | TypeKind::Tuple(_) | TypeKind::Variant(_)
        ) {
            while matches!(record.kind, LayoutKind::Aggregate) && record.fields.len() == 1 {
                record = program
                    .table
                    .layout(record.fields[0].layout)
                    .ok_or_else(|| error("missing represented layout".into()))?;
            }
        }
        let mut node = Node::default();
        macro_rules! part {
            ($ty:expr, $layout:expr) => {
                child($ty, scope.clone(), $layout, &mut nodes, &mut pending)
            };
        }
        match &row.kind {
            TypeKind::Primitive(p) => {
                use Primitive::*;
                node.kind = match p {
                    Bool => 1,
                    U8 | U16 | U32 | U64 | U128 | Char => 2,
                    I8 | I16 | I32 | I64 | I128 => 3,
                    F32 | F64 => 4,
                    String => 5,
                    Bytes => 6,
                    Never => 0,
                };
                node.a = hirsel_type_analysis::logical::primitive(*p);
            }
            TypeKind::Tuple(ts) => {
                node.kind = 7;
                node.a = 1;
                node.b = 1;
                node.text = "(".into();
                for (ty, field) in ts.iter().zip(&record.fields) {
                    node.fields
                        .push((field.offset, part!(*ty, field.layout), String::new()));
                }
            }
            TypeKind::Record(fs)
                if !matches!(record.kind, LayoutKind::Block { .. } | LayoutKind::Flat) =>
            {
                node.kind = 7;
                node.a = 2;
                node.text = "{ ".into();
                for (f, field) in fs.iter().zip(&record.fields) {
                    node.fields
                        .push((field.offset, part!(f.ty, field.layout), f.name.to_string()));
                }
            }
            TypeKind::Variant(vs) if record.tag.is_some() => {
                let tag = record
                    .tag
                    .as_ref()
                    .ok_or_else(|| error("variant has no tag".into()))?;
                node.kind = 8;
                node.a = tag.offset;
                (node.b, node.c) = match tag.form {
                    TagForm::Ordinal => (0, 0),
                    TagForm::Niche { nullary, dataful } => {
                        (1, u64::from(nullary) | (u64::from(dataful) << 32))
                    }
                    TagForm::LiveBit { bit } => (2, u64::from(bit)),
                };
                for variant in &tag.variants {
                    let v = &vs[variant.ordinal as usize];
                    let named = v.fields.first().is_some_and(|(n, _)| n.is_some());
                    let mut branch = Node {
                        kind: 7,
                        a: if v.fields.is_empty() {
                            0
                        } else if named {
                            2
                        } else {
                            1
                        },
                        text: format!(
                            "{}{}",
                            v.name,
                            if v.fields.is_empty() {
                                ""
                            } else if named {
                                " { "
                            } else {
                                "("
                            }
                        ),
                        ..Default::default()
                    };
                    for ((label, ty), field) in v.fields.iter().zip(&variant.fields) {
                        branch.fields.push((
                            field.offset,
                            part!(*ty, field.layout),
                            label.map(|l| l.to_string()).unwrap_or_default(),
                        ));
                    }
                    let branch_id = nodes.len();
                    nodes.push(branch);
                    node.fields
                        .push((u64::from(variant.ordinal), branch_id, String::new()));
                }
            }
            TypeKind::Box(t) => {
                let payload = match record.kind {
                    LayoutKind::Box { payload } | LayoutKind::Block { payload } => payload,
                    _ => return Err(error("box layout".into())),
                };
                node.kind = 9;
                node.a = 8;
                node.text = "box ".into();
                node.fields.push((0, part!(*t, payload), String::new()));
            }
            TypeKind::Builtin { kind, arguments } => {
                use hirsel_types::BuiltinType::*;
                let types = type_arguments(arguments);
                match (&record.kind, kind, types.first()) {
                    (_, Packed, Some(ty)) => {
                        node.kind = 12;
                        node.fields.push((0, part!(*ty, layout), String::new()));
                    }
                    (LayoutKind::Block { payload }, List | Array, Some(ty)) => {
                        let p = program
                            .table
                            .layout(*payload)
                            .ok_or_else(|| error("sequence payload".into()))?;
                        if let LayoutKind::Sequence { element } = p.kind {
                            let e = program.table.layout(element).unwrap();
                            let SizeSpec::Fixed(stride) = e.size else {
                                return Err(error("sequence stride".into()));
                            };
                            node.kind = 10;
                            node.a = 8 + 16u64.div_ceil(u64::from(e.align)) * u64::from(e.align);
                            node.b = stride;
                            node.c = 8;
                            node.fields.push((0, part!(*ty, element), String::new()));
                        } else {
                            node.text = format!("<{kind:?}>");
                        }
                    }
                    (LayoutKind::Inline { element, count }, _, Some(ty)) => {
                        node.kind = 11;
                        node.a = *count;
                        node.b = program
                            .table
                            .layout(*element)
                            .unwrap()
                            .size
                            .storage()
                            .unwrap();
                        node.fields.push((0, part!(*ty, *element), String::new()));
                    }
                    (LayoutKind::Simd { element, lanes }, _, Some(ty)) => {
                        node.kind = 11;
                        node.a = u64::from(*lanes);
                        node.b = program
                            .table
                            .layout(*element)
                            .unwrap()
                            .size
                            .storage()
                            .unwrap();
                        node.fields.push((0, part!(*ty, *element), String::new()));
                    }
                    _ => node.text = format!("<{kind:?}>"),
                }
            }
            TypeKind::Function(_) => node.text = "<function>".into(),
            _ => node.text = "<opaque>".into(),
        }
        nodes[index] = node;
    }
    let mut declarations = String::from(
        "%hirsel.report.node = type { i64, i64, i64, i64, ptr, i64, ptr, i64 }\n%hirsel.report.field = type { i64, ptr, ptr, i64 }\n",
    );
    fn literal(out: &mut String, name: &str, text: &str) {
        let bytes = text
            .as_bytes()
            .iter()
            .map(|b| format!("\\{b:02X}"))
            .collect::<String>();
        out.push_str(&format!(
            "@{name} = private constant [{} x i8] c\"{bytes}\"\n",
            text.len()
        ));
    }
    for (i, n) in nodes.iter().enumerate() {
        literal(
            &mut declarations,
            &format!("hirsel.report.text.{i}"),
            &n.text,
        );
        let mut fields = vec![];
        for (j, (offset, node, label)) in n.fields.iter().enumerate() {
            literal(
                &mut declarations,
                &format!("hirsel.report.label.{i}.{j}"),
                label,
            );
            fields.push(format!("%hirsel.report.field {{ i64 {offset}, ptr @hirsel.report.node.{node}, ptr @hirsel.report.label.{i}.{j}, i64 {} }}", label.len()));
        }
        declarations.push_str(&format!(
            "@hirsel.report.fields.{i} = private constant [{} x %hirsel.report.field] [{}]\n",
            fields.len(),
            fields.join(", ")
        ));
        declarations.push_str(&format!("@hirsel.report.node.{i} = private constant %hirsel.report.node {{ i64 {}, i64 {}, i64 {}, i64 {}, ptr @hirsel.report.text.{i}, i64 {}, ptr @hirsel.report.fields.{i}, i64 {} }}\n", n.kind, n.a, n.b, n.c, n.text.len(), fields.len()));
    }
    let nodes = nodes
        .into_iter()
        .map(|n| hirsel_cranelift::ReportNode {
            kind: n.kind,
            a: n.a,
            b: n.b,
            c: n.c,
            text: n.text.into_bytes(),
            fields: n
                .fields
                .into_iter()
                .map(|(offset, node, label)| (offset, node, label.into_bytes()))
                .collect(),
        })
        .collect();
    Ok(Report {
        declarations,
        nodes,
        offset: field.offset,
        root,
        result,
        return_kind,
        success_offset,
    })
}

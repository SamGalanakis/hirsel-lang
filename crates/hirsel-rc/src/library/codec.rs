//! `codec.encode` and `codec.decode` (SER-2; LIR schema revision 29): each call becomes one
//! `Encode` or `Decode` of a codec plan that pairs the call's concrete schema with the layouts
//! of its values. The schema is proved and described by `hirsel_types::serialization`, its
//! descriptor bytes by `hirsel_types::wire`; this module only walks schema and layout together.
use super::{CallShape, InlineLowering, R};
use crate::PassError;
use crate::import::ImportCx;
use hirsel_lir::codec::{
    CONTAINER_ENTRIES, CONTAINER_INDEX, CodecCheck, CodecNode, CodecNodeId, CodecPlan,
    DECODE_ERRORS,
};
use hirsel_lir::ir::Inst;
use hirsel_lir::layout_types::{CopyRecipe, LayoutId, LayoutKind, LayoutRecord, LayoutTable};
use hirsel_syntax::DefPath;
use hirsel_types::serialization::{SchemaArgument, SchemaId, SchemaNode, SerializableSchema};
use hirsel_types::{BuiltinType, GenericArgument, Primitive, UseKind};
use std::collections::BTreeMap;

pub static ENCODE: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("codec.encode"),
    lower: encode,
};

pub static DECODE: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("codec.decode"),
    lower: decode,
};

fn encode(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let ([argument], [GenericArgument::Type(ty)]) = (call.arguments, call.generics) else {
        return Ok(false);
    };
    let root = cx.place_layout(argument.value.place)?;
    if !read_only(cx, argument, root) {
        return Ok(false);
    }
    let bytes = cx.place_layout(call.destination)?;
    let schema = schema(cx, *ty)?;
    let plan = Plan::new(cx.layout_table(), &schema, None).build(root, bytes, None)?;
    let value = cx.operand(&argument.value)?;
    let codec = cx.codec(plan);
    let result = cx.fresh(bytes);
    cx.push(Inst::Encode {
        result,
        value,
        codec,
    });
    cx.consumed(value);
    cx.finish(call, result)?;
    Ok(true)
}

fn decode(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let ([argument], [GenericArgument::Type(ty)]) = (call.arguments, call.generics) else {
        return Ok(false);
    };
    let bytes = cx.place_layout(argument.value.place)?;
    if !read_only(cx, argument, bytes) {
        return Ok(false);
    }
    let result_layout = cx.place_layout(call.destination)?;
    let schema = schema(cx, *ty)?;
    let checks = checks(cx, &schema)?;
    let table = cx.layout_table();
    // `Result[T, DecodeError]`: `Ok(T)` at 0, `Error(DecodeError)` at 1.
    let arm = |ordinal: u32| -> R<LayoutId> {
        let tag = table.layouts[result_layout.0 as usize]
            .tag
            .as_ref()
            .ok_or_else(|| PassError::Malformed("a decode result without a tag".into()))?;
        tag.variants
            .iter()
            .find(|v| v.ordinal == ordinal)
            .and_then(|v| v.fields.first())
            .map(|f| f.layout)
            .ok_or_else(|| PassError::Malformed("a decode result arm without its field".into()))
    };
    let (root, error) = (arm(0)?, arm(1)?);
    let plan =
        Plan::new(table, &schema, Some(checks)).build(root, bytes, Some((result_layout, error)))?;
    let value = cx.operand(&argument.value)?;
    let codec = cx.codec(plan);
    let result = cx.fresh(result_layout);
    cx.push(Inst::Decode {
        result,
        value,
        codec,
    });
    cx.consumed(value);
    cx.finish(call, result)?;
    Ok(true)
}

/// The operand is only read: a borrow, or a copy of plain bits that holds nothing.
fn read_only(cx: &ImportCx<'_>, argument: &hirsel_tir::CallArgument, layout: LayoutId) -> bool {
    match argument.value.usage {
        UseKind::Borrow => true,
        UseKind::Copy => cx.record(layout).copy == CopyRecipe::Bits,
        UseKind::Take | UseKind::Mut => false,
    }
}

fn schema(cx: &mut ImportCx<'_>, ty: hirsel_types::TypeId) -> R<SerializableSchema> {
    cx.codec_schema(ty)?.ok_or_else(|| {
        PassError::Malformed("a codec call without a checked serializable schema".into())
    })
}

/// Every distinct check the schema's checked opaque types name, in node order, as the
/// direct-call symbol and `FnAbi` a decode calls.
fn checks(cx: &mut ImportCx<'_>, schema: &SerializableSchema) -> R<Vec<(DefPath, CodecCheck)>> {
    let mut out: Vec<(DefPath, CodecCheck)> = vec![];
    for node in &schema.nodes {
        if let SchemaNode::Nominal {
            check: Some(path), ..
        } = node
            && !out.iter().any(|(p, _)| p == path)
        {
            let (symbol, abi) = cx.function(path)?;
            out.push((path.clone(), CodecCheck { symbol, abi }));
        }
    }
    Ok(out)
}

fn unsupported<T>(what: impl Into<String>) -> R<T> {
    Err(PassError::Unsupported(format!(
        "SER-2 codec: {}",
        what.into()
    )))
}

/// The plan under construction: one node per (schema node, layout) pair reached.
struct Plan<'a> {
    table: &'a LayoutTable,
    schema: &'a SerializableSchema,
    /// A decode's checks; None for an encode, which treats a checked type as its
    /// representation.
    checks: Option<Vec<(DefPath, CodecCheck)>>,
    nodes: Vec<CodecNode>,
    memo: BTreeMap<(u32, LayoutId), CodecNodeId>,
    pending: Vec<(CodecNodeId, SchemaId, LayoutId)>,
}

impl<'a> Plan<'a> {
    fn new(
        table: &'a LayoutTable,
        schema: &'a SerializableSchema,
        checks: Option<Vec<(DefPath, CodecCheck)>>,
    ) -> Self {
        Self {
            table,
            schema,
            checks,
            nodes: vec![],
            memo: BTreeMap::new(),
            pending: vec![],
        }
    }

    fn record(&self, layout: LayoutId) -> R<&'a LayoutRecord> {
        self.table
            .layout(layout)
            .ok_or_else(|| PassError::Malformed(format!("layout {} outside the table", layout.0)))
    }

    fn push(&mut self, node: CodecNode) -> CodecNodeId {
        self.nodes.push(node);
        CodecNodeId(self.nodes.len() as u32 - 1)
    }

    /// A nominal over a record, tuple or variant has its representation's layout: the
    /// wrapper is the schema's alone.
    fn transparent(&self, mut s: SchemaId) -> SchemaId {
        while let SchemaNode::Nominal {
            representation,
            check,
            ..
        } = &self.schema.nodes[s.0 as usize]
        {
            // A decoded checked type keeps its node: it calls the check.
            if check.is_some() && self.checks.is_some() {
                break;
            }
            match &self.schema.nodes[representation.0 as usize] {
                SchemaNode::Record(_) | SchemaNode::Variant(_) => s = *representation,
                SchemaNode::Tuple(items) if !items.is_empty() => s = *representation,
                _ => break,
            }
        }
        s
    }

    fn node(&mut self, s: SchemaId, layout: LayoutId) -> CodecNodeId {
        let s = self.transparent(s);
        if let Some(id) = self.memo.get(&(s.0, layout)) {
            return *id;
        }
        let id = self.push(CodecNode::Fields {
            layout,
            fields: vec![],
        });
        self.memo.insert((s.0, layout), id);
        self.pending.push((id, s, layout));
        id
    }

    fn build(
        mut self,
        root: LayoutId,
        bytes: LayoutId,
        result: Option<(LayoutId, LayoutId)>,
    ) -> R<CodecPlan> {
        let root = self.node(self.schema.root, root);
        while let Some((id, s, layout)) = self.pending.pop() {
            self.nodes[id.0 as usize] = self.describe(s, layout)?;
        }
        let bytes = self.text(bytes, false)?;
        let bytes = self.push(bytes);
        let result = match result {
            None => None,
            Some((result, error)) => {
                let error = self.push(CodecNode::Variant {
                    layout: error,
                    arms: vec![vec![]; DECODE_ERRORS.len()],
                });
                Some(self.push(CodecNode::Variant {
                    layout: result,
                    arms: vec![vec![root], vec![error]],
                }))
            }
        };
        let descriptor = hirsel_types::wire::Codec::new(self.schema)
            .map_err(|e| PassError::Malformed(format!("SER-2 descriptor: {e:?}")))?
            .descriptor()
            .to_vec();
        let plan = CodecPlan {
            descriptor,
            nodes: self.nodes,
            root,
            bytes,
            result,
            checks: self
                .checks
                .into_iter()
                .flatten()
                .map(|(_, check)| check)
                .collect(),
        };
        if let Err(error) = plan.check(self.table) {
            return unsupported(error);
        }
        Ok(plan)
    }

    fn text(&self, layout: LayoutId, utf8: bool) -> R<CodecNode> {
        let LayoutKind::Slice { element } = self.record(layout)?.kind else {
            return unsupported("String or Bytes off a slice layout");
        };
        let sequence = LayoutKind::Sequence { element };
        let Some(storage) = self.table.layouts.iter().position(|r| r.kind == sequence) else {
            return unsupported("a byte slice without its sequence storage");
        };
        Ok(CodecNode::Text {
            layout,
            storage: LayoutId(storage as u32),
            utf8,
        })
    }

    /// Every field of `layout`, in declaration order, at `items`.
    fn fields(&mut self, layout: LayoutId, items: &[SchemaId]) -> R<CodecNode> {
        let r = self.record(layout)?;
        if r.fields.len() != items.len() {
            return unsupported(format!("{:?} fields off their schema", r.kind));
        }
        let mut fields = vec![];
        for (ordinal, item) in items.iter().enumerate() {
            let Some(f) = r.fields.iter().find(|f| f.ordinal == ordinal as u32) else {
                return unsupported("a field off its layout");
            };
            fields.push(self.node(*item, f.layout));
        }
        Ok(CodecNode::Fields { layout, fields })
    }

    fn variant(&mut self, layout: LayoutId, arms: &[Vec<SchemaId>]) -> R<CodecNode> {
        let Some(tag) = &self.record(layout)?.tag else {
            return unsupported("a variant without a tag");
        };
        if tag.variants.len() != arms.len() {
            return unsupported("variant arms off their schema");
        }
        let mut out = vec![];
        for (ordinal, arm) in arms.iter().enumerate() {
            let Some(v) = tag.variants.iter().find(|v| v.ordinal == ordinal as u32) else {
                return unsupported("a variant arm off its layout");
            };
            if v.fields.len() != arm.len() {
                return unsupported("variant fields off their schema");
            }
            let mut fields = vec![];
            for (i, item) in arm.iter().enumerate() {
                let Some(f) = v.fields.iter().find(|f| f.ordinal == i as u32) else {
                    return unsupported("a variant field off its layout");
                };
                fields.push(self.node(*item, f.layout));
            }
            out.push(fields);
        }
        Ok(CodecNode::Variant { layout, arms: out })
    }

    fn argument(arguments: &[SchemaArgument], i: usize) -> R<SchemaId> {
        match arguments.get(i) {
            Some(SchemaArgument::Type(t)) => Ok(*t),
            _ => Err(PassError::Malformed(
                "a codec builtin without its type".into(),
            )),
        }
    }

    fn arms(&self, s: SchemaId) -> R<Vec<Vec<SchemaId>>> {
        match &self.schema.nodes[self.transparent(s).0 as usize] {
            SchemaNode::Variant(arms) => Ok(arms
                .iter()
                .map(|(_, fields)| fields.iter().map(|(_, t)| *t).collect())
                .collect()),
            _ => Err(PassError::Malformed(
                "Packed of a schema that is not a variant".into(),
            )),
        }
    }

    /// The block payload of `layout`, which must be a block.
    fn payload(&self, layout: LayoutId) -> R<LayoutId> {
        match self.record(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Ok(payload),
            ref kind => unsupported(format!("{kind:?} where a block was expected")),
        }
    }

    fn describe(&mut self, s: SchemaId, layout: LayoutId) -> R<CodecNode> {
        let r = self.record(layout)?;
        if let LayoutKind::Block { payload } = r.kind
            && self.record(payload)?.kind == LayoutKind::Flat
        {
            return Ok(CodecNode::Box {
                layout,
                payload: self.node(s, payload),
            });
        }
        let text = matches!(
            self.schema.nodes[s.0 as usize],
            SchemaNode::Scalar(Primitive::String | Primitive::Bytes)
        );
        let sequence = matches!(
            self.schema.nodes[s.0 as usize],
            SchemaNode::Builtin {
                kind: BuiltinType::List | BuiltinType::Array,
                ..
            }
        );
        if r.kind == LayoutKind::Flat
            && !text
            && !sequence
            && let [field] = r.fields.as_slice()
            && field.offset == 8
            && matches!(r.size, hirsel_lir::layout_types::SizeSpec::Dynamic(_))
        {
            return Ok(CodecNode::Framed {
                layout,
                value: self.node(s, field.layout),
            });
        }
        Ok(match &self.schema.nodes[s.0 as usize] {
            SchemaNode::Scalar(Primitive::String) if r.kind == LayoutKind::Flat => {
                CodecNode::FlatText { layout, utf8: true }
            }
            SchemaNode::Scalar(Primitive::String) => self.text(layout, true)?,
            SchemaNode::Scalar(Primitive::Bytes) if r.kind == LayoutKind::Flat => {
                CodecNode::FlatText {
                    layout,
                    utf8: false,
                }
            }
            SchemaNode::Scalar(Primitive::Bytes) => self.text(layout, false)?,
            SchemaNode::Scalar(Primitive::Char) => CodecNode::Char(layout),
            SchemaNode::Scalar(_) => CodecNode::Scalar(layout),
            SchemaNode::Tuple(items) => self.fields(layout, &items.clone())?,
            SchemaNode::Record(fields) => {
                let items: Vec<_> = fields.iter().map(|(_, t)| *t).collect();
                self.fields(layout, &items)?
            }
            SchemaNode::Variant(arms) => {
                let arms: Vec<Vec<_>> = arms
                    .iter()
                    .map(|(_, fields)| fields.iter().map(|(_, t)| *t).collect())
                    .collect();
                self.variant(layout, &arms)?
            }
            // A core Path is its run-context identity; the runtime resolves and interns
            // its text.
            SchemaNode::Nominal { definition, .. } if definition.is_core("Path") => {
                if r.kind != LayoutKind::Path {
                    return unsupported("a Path off its layout");
                }
                CodecNode::Path(layout)
            }
            // A decoded checked opaque type is its representation, then its check.
            SchemaNode::Nominal {
                definition,
                representation,
                check: Some(check),
            } if self.checks.is_some() => {
                let representation = *representation;
                let index = self
                    .checks
                    .iter()
                    .flatten()
                    .position(|(path, _)| path == check)
                    .ok_or_else(|| {
                        PassError::Malformed(format!("{definition:?}'s check was not resolved"))
                    })?;
                CodecNode::Checked {
                    layout,
                    check: index as u32,
                    value: self.node(representation, layout),
                }
            }
            // A nominal over a scalar, box or sequence holds it as its one field.
            SchemaNode::Nominal { representation, .. } => {
                self.fields(layout, &[*representation])?
            }
            SchemaNode::Box(t) => {
                let LayoutKind::Box { payload } = r.kind else {
                    return unsupported("box off a box layout");
                };
                CodecNode::Box {
                    layout,
                    payload: self.node(*t, payload),
                }
            }
            SchemaNode::Builtin { kind, arguments } => {
                let arguments = arguments.clone();
                match kind {
                    BuiltinType::List | BuiltinType::Array => {
                        let element = if r.kind == LayoutKind::Flat {
                            r.fields
                                .first()
                                .ok_or_else(|| {
                                    PassError::Malformed(
                                        "a flat sequence without an element".into(),
                                    )
                                })?
                                .layout
                        } else {
                            match self.record(self.payload(layout)?)?.kind {
                                LayoutKind::Sequence { element }
                                | LayoutKind::Sequential { element }
                                | LayoutKind::SoaColumns { element } => element,
                                _ => {
                                    return unsupported(format!(
                                        "a {kind:?} off its sequence storage"
                                    ));
                                }
                            }
                        };
                        CodecNode::Sequence {
                            layout,
                            element: self.node(Self::argument(&arguments, 0)?, element),
                        }
                    }
                    BuiltinType::Inline | BuiltinType::Simd => {
                        let (LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. }) =
                            r.kind
                        else {
                            return unsupported(format!("{kind:?} off its layout"));
                        };
                        CodecNode::Repeat {
                            layout,
                            element: self.node(Self::argument(&arguments, 0)?, element),
                        }
                    }
                    BuiltinType::Packed => {
                        let arms = self.arms(Self::argument(&arguments, 0)?)?;
                        self.variant(layout, &arms)?
                    }
                    BuiltinType::Map | BuiltinType::Set => {
                        let container = self.record(self.payload(layout)?)?;
                        let field = |ordinal| {
                            container
                                .fields
                                .iter()
                                .find(|f| f.ordinal == ordinal)
                                .map(|f| f.layout)
                                .ok_or_else(|| PassError::Malformed("a container field".into()))
                        };
                        field(CONTAINER_INDEX)?;
                        let LayoutKind::Sequence { element } =
                            self.record(self.payload(field(CONTAINER_ENTRIES)?)?)?.kind
                        else {
                            return unsupported("container entries off a sequence");
                        };
                        if *kind == BuiltinType::Set {
                            CodecNode::Set {
                                layout,
                                element: self.node(Self::argument(&arguments, 0)?, element),
                            }
                        } else {
                            let live = self
                                .record(element)?
                                .tag
                                .as_ref()
                                .and_then(|t| t.variants.iter().find(|v| v.ordinal == 1))
                                .and_then(|v| v.fields.first())
                                .ok_or_else(|| PassError::Malformed("a map live entry".into()))?
                                .layout;
                            let pair = self.record(live)?;
                            let at = |ordinal| {
                                pair.fields
                                    .iter()
                                    .find(|f| f.ordinal == ordinal)
                                    .map(|f| f.layout)
                                    .ok_or_else(|| PassError::Malformed("a map entry field".into()))
                            };
                            let (k, v) = (at(0)?, at(1)?);
                            CodecNode::Map {
                                layout,
                                key: self.node(Self::argument(&arguments, 0)?, k),
                                value: self.node(Self::argument(&arguments, 1)?, v),
                            }
                        }
                    }
                    _ => return unsupported(format!("{kind:?}")),
                }
            }
            SchemaNode::Parameter(_) | SchemaNode::Reference(_) => {
                return Err(PassError::Malformed(
                    "a codec schema that is not concrete and canonical".into(),
                ));
            }
        })
    }
}

//! The relocatable native contract (design §1e): no unit object contains a number the
//! program assigns. Every program-assigned value a unit's code uses is read through a
//! content-named symbol that the program's glue object, regenerated at every link,
//! defines:
//!
//! - a layout's descriptor index, from the u32 cell [`descriptor_index_symbol`];
//! - a rule's failure code, from the u32 cell [`failure_code_symbol`];
//! - a codec's runtime program (whose words name descriptor indices), from the
//!   `hirsel_rt_v2_codec` record [`codec_symbol`], keyed by the index-free [`CodecKey`];
//! - a constant's static blocks ([`static_symbol`]) and a text literal's block
//!   ([`literal_symbol`]), whose immortal prefixes hold descriptor indices;
//! - a definition's C callback thunk (`emission::callback_thunk_name`).
//!
//! Glue also defines every runtime descriptor, its steps and drop wrapper, and the
//! descriptor registry. [`UnitGlue`] is what one Ready unit needs from glue, derived
//! from the unit alone, so a cached unit carries it and glue never reads a unit body.
use crate::codec::{CodecCheck, CodecNode, CodecPlan};
use crate::emission::{BOUNDED_RULE, slice_storage, symbol_name};
use crate::ir::{ConstantValue, Inst, LirUnit, Symbol, Terminator, ValueRepr};
use crate::layout_keys::LayoutKeys;
use crate::layout_types::{LayoutId, LayoutKey, LayoutKind, LayoutTable, ScalarKind};
use crate::stage::Ready;
use hirsel_core_contract::Manifest;
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

fn hex(hash: &ContentHash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// The u32 cell holding the program's descriptor index of the layout `key`.
pub fn descriptor_index_symbol(key: &LayoutKey) -> String {
    format!("hirsel.desc.{}.index", key.hex())
}

/// The hidden absolute symbol whose value is the program's descriptor index of the layout
/// `key`. A unit's `available_externally` view of a glue block names it in the block's
/// descriptor word, so a folded read of that word is a relocation, not a number.
pub fn descriptor_word_symbol(key: &LayoutKey) -> String {
    format!("hirsel.desc.{}.abs", key.hex())
}

/// The runtime descriptor (`hirsel_rt_v2_layout`) of the layout `key`.
pub fn descriptor_name(key: &LayoutKey) -> String {
    format!("hirsel.layout.{}", key.hex())
}

/// The u32 cell holding the program's failure code of `rule` (`hirsel_rt_v2_failure_begin`'s
/// `rule`). Rule ids are `[A-Z0-9-]`.
/// The u64 S-13 dispatch word, including its program-assigned package ordinal.
pub fn foreign_dispatch_symbol(definition: &DefPath) -> String {
    format!("hirsel.foreign.{}.dispatch", symbol_name(definition, None))
}

pub fn failure_code_symbol(rule: &str) -> String {
    format!("hirsel.fail.{rule}.code")
}

/// The content key of a codec plan: the plan with every layout replaced by its
/// `LayoutKey` and every check by its function's link name and FnAbi key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct CodecKey(pub ContentHash);

/// The `hirsel_rt_v2_codec` record of the codec `key`, bound to the program's registry.
pub fn codec_symbol(key: &CodecKey) -> String {
    format!("hirsel.codec.{}", hex(&key.0))
}

/// The static blocks of the constant `symbol` (CT-2); block 0's header is the symbol's
/// value plus the header offset of its payload alignment, as it is for every static
/// immortal block.
pub fn static_symbol(symbol: &Symbol) -> String {
    format!(
        "hirsel.static.{}",
        symbol_name(&symbol.definition, symbol.instance.as_ref())
    )
}

/// The static block of a text literal: `bytes` (the payload's count, capacity and
/// elements) at the payload layout `payload`.
pub fn literal_symbol(payload: &LayoutKey, bytes: &[u8]) -> String {
    let digest = ContentHash::of_parts("hirsel.native-literal.v2", [&payload.0.0[..], bytes]);
    format!("hirsel.literal.{}", hex(&digest))
}

impl CodecNode {
    /// The layouts the node names, mutably, in field order.
    pub fn layouts_mut(&mut self) -> Vec<&mut LayoutId> {
        match self {
            Self::Text {
                layout, storage, ..
            } => vec![layout, storage],
            Self::Scalar(layout)
            | Self::Char(layout)
            | Self::Path(layout)
            | Self::FlatText { layout, .. }
            | Self::Framed { layout, .. }
            | Self::Fields { layout, .. }
            | Self::Variant { layout, .. }
            | Self::Box { layout, .. }
            | Self::Sequence { layout, .. }
            | Self::Repeat { layout, .. }
            | Self::Map { layout, .. }
            | Self::Set { layout, .. }
            | Self::Checked { layout, .. } => vec![layout],
        }
    }
}

/// A codec check named independently of a unit's symbol table.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GlueCheck {
    pub definition: DefPath,
    pub instance: Option<ContentHash>,
    pub abi: crate::FnAbiId,
}

/// A unit's codec plan as glue binds it: the plan, whose checks index `checks`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GlueCodec {
    pub key: CodecKey,
    pub plan: CodecPlan,
    pub checks: Vec<GlueCheck>,
}

impl GlueCodec {
    /// The codec of `plan` in `unit`, keyed by content.
    pub fn new(
        plan: &CodecPlan,
        unit: &LirUnit<Ready>,
        table: &LayoutTable,
        keys: &LayoutKeys,
    ) -> Result<Self, String> {
        Self::of(plan, &unit.tables.symbols, table, keys)
    }

    /// The codec of `plan`, whose checks index `symbols`, keyed by content.
    pub fn of(
        plan: &CodecPlan,
        symbols: &[Symbol],
        table: &LayoutTable,
        keys: &LayoutKeys,
    ) -> Result<Self, String> {
        let checks = plan
            .checks
            .iter()
            .map(|check: &CodecCheck| {
                let symbol = symbols
                    .get(check.symbol.index())
                    .ok_or("a codec check outside the symbol table")?;
                Ok(GlueCheck {
                    definition: symbol.definition.clone(),
                    instance: symbol.instance,
                    abi: check.abi,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut erased = plan.clone();
        let mut references = Vec::new();
        for node in &mut erased.nodes {
            for layout in node.layouts_mut() {
                if layout.0 as usize >= table.layouts.len() {
                    return Err(format!("a codec node names no layout {}", layout.0));
                }
                references.push(keys.key(*layout).0);
                *layout = LayoutId(0);
            }
        }
        let mut check_parts = Vec::new();
        for (check, erased) in checks.iter().zip(&mut erased.checks) {
            let abi = keys
                .fn_abi_of(table, check.abi)
                .ok_or("a codec check of no FnAbi")?;
            check_parts.push(symbol_name(&check.definition, check.instance.as_ref()));
            check_parts.push(hex(&abi));
            erased.symbol = crate::ir::SymbolId(0);
            erased.abi = crate::FnAbiId(0);
        }
        let text = hirsel_codec::to_text(&erased);
        let key = CodecKey(ContentHash::of_parts(
            "hirsel.codec.v1",
            std::iter::once(text.as_bytes())
                .chain(references.iter().map(|k| &k.0[..]))
                .chain(check_parts.iter().map(String::as_bytes)),
        ));
        Ok(Self {
            key,
            plan: plan.clone(),
            checks,
        })
    }
}

/// What one Ready unit needs the program's glue to define (see the module).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnitGlue {
    /// The rules its failures, its operations' manifest rows and its bounded arenas
    /// (MEM-6) may fail with.
    pub failure_rules: BTreeSet<String>,
    pub codecs: Vec<GlueCodec>,
    /// Text literal blocks: payload layout and bytes.
    pub literals: BTreeSet<(LayoutId, Vec<u8>)>,
    /// The constants it reads (`ReadStatic`).
    pub statics: BTreeSet<Symbol>,
    /// The definitions (and instances) it names as C callbacks (PROG-4).
    pub callbacks: BTreeSet<(DefPath, Option<ContentHash>)>,
    /// Extern calls whose dispatch words glue binds for this program.
    pub foreign: BTreeSet<DefPath>,
    /// Non-C callable values listed in the program Entry table.
    pub entries: BTreeSet<(DefPath, Option<ContentHash>)>,
}

impl UnitGlue {
    /// What `unit` needs from glue.
    pub fn of(
        unit: &LirUnit<Ready>,
        table: &LayoutTable,
        keys: &LayoutKeys,
        manifest: &Manifest,
    ) -> Result<Self, String> {
        let mut glue = Self::default();
        glue.failure_rules
            .extend(unit.tables.failures.iter().filter_map(|f| f.rule.clone()));
        for call in &unit.tables.calls {
            if table
                .fn_abis
                .get(call.abi.0 as usize)
                .is_some_and(|abi| abi.shape == crate::abi_types::CallableShape::Foreign)
                && let crate::ir::Callee::Direct(symbol) = call.callee
            {
                glue.foreign.insert(
                    unit.tables
                        .symbols
                        .get(symbol.index())
                        .ok_or("an extern outside its symbol table")?
                        .definition
                        .clone(),
                );
            }
            if let crate::ir::Callee::Operation(op) = call.callee {
                let id = &unit.tables.operations[op.index()];
                if let Some(rule) = manifest.row(id).and_then(|r| r.failure.as_ref()) {
                    glue.failure_rules.insert(rule.0.clone());
                }
            }
        }
        // Arena construction, copy and seal exhaustion record MEM-6. A seal
        // also checks overflow when no bounded recovery is linked.
        if unit
            .terminators
            .iter()
            .any(|t| matches!(t, Terminator::Recover { .. }))
            || unit
                .instructions
                .iter()
                .any(|inst| matches!(inst, crate::ir::Inst::SealArena { .. }))
        {
            glue.failure_rules.insert(BOUNDED_RULE.to_string());
        }
        for plan in &unit.tables.codecs {
            let codec = GlueCodec::new(plan, unit, table, keys)?;
            if !glue.codecs.iter().any(|c| c.key == codec.key) {
                glue.codecs.push(codec);
            }
        }
        for constant in &unit.tables.constants {
            let bytes = match &constant.value {
                ConstantValue::String(text) => text.as_bytes(),
                ConstantValue::Bytes(bytes) => bytes.as_slice(),
                _ => continue,
            };
            let ty = unit
                .types
                .get(constant.ty.index())
                .ok_or("a constant of no type")?;
            let payload = literal_payload(table, ty.layout, ty.repr)
                .ok_or("a text constant of another type")?;
            let data = literal_data(table, payload, bytes)?;
            glue.literals.insert((payload, data));
        }
        for inst in &unit.instructions {
            match *inst {
                Inst::ReadStatic { symbol, .. } => {
                    glue.statics.insert(
                        unit.tables
                            .symbols
                            .get(symbol.index())
                            .ok_or("a static outside the symbol table")?
                            .clone(),
                    );
                }
                Inst::FunctionRef { result, symbol }
                    if unit.types[unit.values[result.index()].ty.index()].repr
                        == ValueRepr::Scalar(ScalarKind::Pointer) =>
                {
                    let symbol = unit
                        .tables
                        .symbols
                        .get(symbol.index())
                        .ok_or("a callback outside the symbol table")?;
                    glue.callbacks
                        .insert((symbol.definition.clone(), symbol.instance));
                }
                Inst::FunctionRef { symbol, .. } => {
                    let symbol = unit
                        .tables
                        .symbols
                        .get(symbol.index())
                        .ok_or("an entry outside its symbol table")?;
                    glue.entries
                        .insert((symbol.definition.clone(), symbol.instance));
                }
                _ => {}
            }
        }
        Ok(glue)
    }

    /// Adds `other`'s needs to these.
    pub fn extend(&mut self, other: &UnitGlue) {
        self.failure_rules
            .extend(other.failure_rules.iter().cloned());
        for codec in &other.codecs {
            if !self.codecs.iter().any(|c| c.key == codec.key) {
                self.codecs.push(codec.clone());
            }
        }
        self.literals.extend(other.literals.iter().cloned());
        self.statics.extend(other.statics.iter().cloned());
        self.callbacks.extend(other.callbacks.iter().cloned());
        self.foreign.extend(other.foreign.iter().cloned());
        self.entries.extend(other.entries.iter().cloned());
    }
}

/// The sequence payload a String or Bytes constant of `layout` (held as `repr`) owns: the
/// slice's element storage, or the block's payload.
pub fn literal_payload(table: &LayoutTable, layout: LayoutId, repr: ValueRepr) -> Option<LayoutId> {
    match (repr, &table.layout(layout)?.kind) {
        (ValueRepr::Aggregate, &LayoutKind::Slice { element }) => slice_storage(table, element),
        (
            ValueRepr::Block | ValueRepr::Uncounted,
            LayoutKind::Block { payload } | LayoutKind::Box { payload },
        ) => Some(*payload),
        _ => None,
    }
}

/// A text literal's payload bytes at the sequence layout `payload`: its u64 count, its u64
/// capacity (the count), padding to the first element, then `bytes`.
pub fn literal_data(
    table: &LayoutTable,
    payload: LayoutId,
    bytes: &[u8],
) -> Result<Vec<u8>, String> {
    let record = table
        .layout(payload)
        .ok_or("a literal of no payload layout")?;
    let LayoutKind::Sequence { element } = record.kind else {
        return Err("a text literal whose storage is not a sequence".into());
    };
    let element = table
        .layout(element)
        .ok_or("a sequence of no element layout")?;
    if element.size != crate::layout_types::SizeSpec::Fixed(1) {
        return Err("a text literal of elements wider than a byte".into());
    }
    let start =
        crate::layout::sequence_elements_offset(element).map_err(|e| format!("{e:?}"))? as usize;
    let mut data = vec![0u8; start];
    let count = (bytes.len() as u64).to_le_bytes();
    data[..8].copy_from_slice(&count);
    let capacity = crate::layout::SEQUENCE_CAPACITY_OFFSET as usize;
    data[capacity..capacity + 8].copy_from_slice(&count);
    data.extend_from_slice(bytes);
    Ok(data)
}

/// Every rule's failure code (`emission::FailureCodes`), from the rules some unit needs:
/// a rule's code is its index in sorted order plus one, so zero never names a rule. Only
/// glue reads the codes.
pub fn failure_codes(rules: &BTreeSet<String>) -> crate::emission::FailureCodes {
    crate::emission::FailureCodes {
        codes: rules
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                (
                    rule.clone(),
                    crate::emission::FailureCode {
                        kind: crate::emission::KIND_BUG,
                        rule: index as u32 + 1,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

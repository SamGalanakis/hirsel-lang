//! A unit as program assembly reads it (design §1e, §3 `UnitFragment`): its object and the
//! facts about it that the link, the glue object, the failure table and the launcher read,
//! so a unit whose object the object store holds needs no Ready unit.
//!
//! In the store, a fragment's program-local ids (its FnAbi, its glue's literal and codec
//! layouts and its codec checks' FnAbis) are named by content key, and loading binds them
//! to the ids the loading program gives those keys.

use crate::LayoutKeys;
use crate::abi_types::FnAbiId;
use crate::glue::{GlueCheck, GlueCodec, UnitGlue};
use crate::ir::{Callee, Inst, LirUnit, SiteRange, Symbol, Terminator, UnitOrigin};
use crate::layout_types::{LayoutId, LayoutKey, LayoutTable};
use crate::stage::{ObjectArtifact, Ready};
use hirsel_codec::{Snapshot, snapshot_struct};
use hirsel_core_contract::Manifest;
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

/// What program assembly reads from one unit besides its object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnitSummary {
    pub definition: DefPath,
    pub instance: ContentHash,
    pub origin: UnitOrigin,
    pub abi: FnAbiId,
    pub implementation_revision: ContentHash,
    /// The sites of its source and cleanup anchors, whose site codes failures report.
    pub sites: Vec<SiteRange>,
    /// It opens a bounded arena (`Terminator::Recover`).
    pub recovers: bool,
    /// It catches failures (`Terminator::Isolate`).
    pub isolates: bool,
    pub glue: UnitGlue,
    /// The functions it calls directly (`Callee::Direct` and `Callee::Bound` code).
    pub calls: Vec<Symbol>,
    /// The functions it names by `FunctionRef`.
    pub references: Vec<Symbol>,
}

/// Layout-free diagnostic facts from the exact portable unit summary. These
/// contain no native body, program-local layout/ABI IDs or acceptance authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnitObservation {
    pub definition: DefPath,
    pub instance: ContentHash,
    pub origin: UnitOrigin,
    pub implementation_revision: ContentHash,
    pub sites: Vec<SiteRange>,
    pub failure_rules: Vec<String>,
}

impl UnitSummary {
    /// The summary of a Ready unit.
    pub fn of(
        unit: &LirUnit<Ready>,
        table: &LayoutTable,
        keys: &LayoutKeys,
        manifest: &Manifest,
    ) -> Result<Self, String> {
        let header = &unit.header;
        let symbol = |id: crate::ir::SymbolId| {
            unit.tables
                .symbols
                .get(id.index())
                .cloned()
                .ok_or_else(|| format!("symbol {} outside the table", id.0))
        };
        let mut calls = Vec::new();
        for call in &unit.tables.calls {
            if let Callee::Direct(s) | Callee::Bound { code: s, .. } = call.callee {
                calls.push(symbol(s)?);
            }
        }
        let mut references = Vec::new();
        for inst in &unit.instructions {
            if let Inst::FunctionRef { symbol: s, .. } = *inst {
                references.push(symbol(s)?);
            }
        }
        Ok(Self {
            definition: header.definition.clone(),
            instance: header.instance,
            origin: header.origin,
            abi: header.abi,
            implementation_revision: header.implementation_revision,
            sites: unit
                .tables
                .source_anchors
                .iter()
                .map(|a| a.site)
                .chain(unit.tables.cleanup_anchors.iter().map(|a| a.site))
                .collect(),
            recovers: unit
                .terminators
                .iter()
                .any(|t| matches!(t, Terminator::Recover { .. })),
            isolates: unit
                .terminators
                .iter()
                .any(|t| matches!(t, Terminator::Isolate { .. })),
            glue: UnitGlue::of(unit, table, keys, manifest)?,
            calls,
            references,
        })
    }

    /// Static callable relocations are ordinary named function references too.
    /// Retain them in portable summaries, including when cached units have no bodies.
    pub fn add_static_references(&mut self, statics: &[crate::statics::StaticData]) {
        for data in statics
            .iter()
            .filter(|data| self.glue.statics.contains(&data.symbol))
        {
            for function in data.blocks.iter().flat_map(|block| &block.functions) {
                self.glue
                    .entries
                    .insert((function.symbol.definition.clone(), function.symbol.instance));
                if !self.references.contains(&function.symbol) {
                    self.references.push(function.symbol.clone());
                }
            }
        }
    }

    /// Every program-local id the summary names, mutably: its FnAbi, then per codec the
    /// plan's layouts and the checks' FnAbis. Literal layouts are walked by `literals`.
    fn for_each_id(
        &mut self,
        layout: &mut impl FnMut(&mut LayoutId),
        abi: &mut impl FnMut(&mut FnAbiId),
    ) {
        abi(&mut self.abi);
        for codec in &mut self.glue.codecs {
            for node in &mut codec.plan.nodes {
                for id in node.layouts_mut() {
                    layout(id);
                }
            }
            for check in &mut codec.checks {
                abi(&mut check.abi);
            }
        }
        let literals = std::mem::take(&mut self.glue.literals);
        self.glue.literals = literals
            .into_iter()
            .map(|(mut id, bytes)| {
                layout(&mut id);
                (id, bytes)
            })
            .collect();
    }
}

/// A unit's object and summary, with the summaries of the library and known callable
/// bodies its object copies (`linkonce_odr`), which program assembly reads as their own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnitFragment {
    pub summary: UnitSummary,
    pub templates: Vec<UnitSummary>,
    pub object: ObjectArtifact,
}

/// The magic at the start of an encoded fragment.
const MAGIC: &[u8; 8] = b"HIRFRG02";

impl UnitFragment {
    /// The fragment as the object store keeps it: the summary and the object's metadata as
    /// `hirsel_codec` text with their ids named by key, then the object's bytes.
    pub fn encode(&self, table: &LayoutTable, keys: &LayoutKeys) -> Result<Vec<u8>, String> {
        let mut summaries: Vec<UnitSummary> = std::iter::once(&self.summary)
            .chain(&self.templates)
            .cloned()
            .collect();
        let (layouts, abis) = erase_summaries(&mut summaries, table, keys)?;
        let wire = Wire {
            summaries: summaries.iter().map(WireSummary::from).collect(),
            layouts,
            abis,
            object: ObjectArtifact {
                ahead: None,
                bytes: Vec::new(),
                ..self.object.clone()
            },
        };
        let text = hirsel_codec::to_text(&wire);
        let mut out = Vec::with_capacity(16 + text.len() + self.object.bytes.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(text.len() as u64).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(&self.object.bytes);
        Ok(out)
    }

    /// The fragment `encode` wrote, its ids bound through `layout` and `abi`, which give
    /// the loading program's id of each key. The error says what does not decode or bind.
    pub fn decode(
        bytes: &[u8],
        layout: &impl Fn(&LayoutKey) -> Option<LayoutId>,
        abi: &impl Fn(&ContentHash) -> Option<FnAbiId>,
    ) -> Result<Self, String> {
        let rest = bytes
            .strip_prefix(MAGIC.as_slice())
            .ok_or("not an encoded fragment")?;
        let (length, rest) = rest.split_at_checked(8).ok_or("a truncated fragment")?;
        let length = u64::from_le_bytes(length.try_into().expect("eight bytes")) as usize;
        let (text, object) = rest
            .split_at_checked(length)
            .ok_or("a truncated fragment")?;
        let text = std::str::from_utf8(text).map_err(|e| e.to_string())?;
        let wire: Wire = hirsel_codec::from_text(text).map_err(|e| e.message)?;
        let summaries = bind_summaries(wire.summaries, &wire.layouts, &wire.abis, layout, abi)?;
        let mut summaries = summaries.into_iter();
        let summary = summaries
            .next()
            .ok_or("a fragment without its unit's summary")?;
        Ok(Self {
            summary,
            templates: summaries.collect(),
            object: ObjectArtifact {
                ahead: None,
                bytes: object.to_vec(),
                ..wire.object
            },
        })
    }
}

/// The encoded form of a fragment: its unit's summary and then its templates', with
/// erased ids, the keys they name, and the object without its bytes.
struct Wire {
    summaries: Vec<WireSummary>,
    layouts: Vec<LayoutKey>,
    abis: Vec<ContentHash>,
    object: ObjectArtifact,
}

/// `UnitSummary` with its glue's sets as sorted lists.
struct WireSummary {
    definition: DefPath,
    instance: ContentHash,
    origin: UnitOrigin,
    abi: FnAbiId,
    implementation_revision: ContentHash,
    sites: Vec<SiteRange>,
    recovers: bool,
    isolates: bool,
    failure_rules: Vec<String>,
    codecs: Vec<GlueCodec>,
    literals: Vec<(LayoutId, Vec<u8>)>,
    statics: Vec<Symbol>,
    callbacks: Vec<(DefPath, Option<ContentHash>)>,
    foreign: Vec<DefPath>,
    entries: Vec<(DefPath, Option<ContentHash>)>,
    calls: Vec<Symbol>,
    references: Vec<Symbol>,
}

impl From<&UnitSummary> for WireSummary {
    fn from(s: &UnitSummary) -> Self {
        Self {
            definition: s.definition.clone(),
            instance: s.instance,
            origin: s.origin,
            abi: s.abi,
            implementation_revision: s.implementation_revision,
            sites: s.sites.clone(),
            recovers: s.recovers,
            isolates: s.isolates,
            failure_rules: s.glue.failure_rules.iter().cloned().collect(),
            codecs: s.glue.codecs.clone(),
            literals: s.glue.literals.iter().cloned().collect(),
            statics: s.glue.statics.iter().cloned().collect(),
            callbacks: s.glue.callbacks.iter().cloned().collect(),
            foreign: s.glue.foreign.iter().cloned().collect(),
            entries: s.glue.entries.iter().cloned().collect(),
            calls: s.calls.clone(),
            references: s.references.clone(),
        }
    }
}

impl From<WireSummary> for UnitSummary {
    fn from(s: WireSummary) -> Self {
        Self {
            definition: s.definition,
            instance: s.instance,
            origin: s.origin,
            abi: s.abi,
            implementation_revision: s.implementation_revision,
            sites: s.sites,
            recovers: s.recovers,
            isolates: s.isolates,
            glue: UnitGlue {
                failure_rules: s.failure_rules.into_iter().collect::<BTreeSet<_>>(),
                codecs: s.codecs,
                literals: s.literals.into_iter().collect(),
                statics: s.statics.into_iter().collect(),
                callbacks: s.callbacks.into_iter().collect(),
                foreign: s.foreign.into_iter().collect(),
                entries: s.entries.into_iter().collect(),
            },
            calls: s.calls,
            references: s.references,
        }
    }
}

snapshot_struct!(Wire {
    summaries,
    layouts,
    abis,
    object
});
snapshot_struct!(WireSummary {
    definition,
    instance,
    origin,
    abi,
    implementation_revision,
    sites,
    recovers,
    isolates,
    failure_rules,
    codecs,
    literals,
    statics,
    callbacks,
    foreign,
    entries,
    calls,
    references
});
snapshot_struct!(GlueCodec { key, plan, checks });
snapshot_struct!(GlueCheck {
    definition,
    instance,
    abi
});

impl Snapshot for crate::glue::CodecKey {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        self.0.write(w);
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        ContentHash::read(r).map(Self)
    }
}

/// Reuse the fragment codec's exact content-named bindings for standalone
/// immutable summaries. No object bytes or fabricated object metadata is needed.
impl UnitSummary {
    pub fn encode(&self, table: &LayoutTable, keys: &LayoutKeys) -> Result<Vec<u8>, String> {
        let mut summaries = vec![self.clone()];
        let (layouts, abis) = erase_summaries(&mut summaries, table, keys)?;
        let wire = SummaryWire {
            summary: WireSummary::from(&summaries[0]),
            layouts,
            abis,
        };
        Ok(hirsel_codec::to_text(&wire).into_bytes())
    }
    /// Read only diagnostic metadata with the same summary codec. Layouts
    /// remain content names; no table binding or Value/Ready construction runs.
    pub fn observation(bytes: &[u8]) -> Result<UnitObservation, String> {
        let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let wire: SummaryWire = hirsel_codec::from_text(text).map_err(|e| e.message)?;
        let summary = wire.summary;
        Ok(UnitObservation {
            definition: summary.definition,
            instance: summary.instance,
            origin: summary.origin,
            implementation_revision: summary.implementation_revision,
            sites: summary.sites,
            failure_rules: summary.failure_rules,
        })
    }
    pub fn decode(
        bytes: &[u8],
        layout: &impl Fn(&LayoutKey) -> Option<LayoutId>,
        abi: &impl Fn(&ContentHash) -> Option<FnAbiId>,
    ) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let wire: SummaryWire = hirsel_codec::from_text(text).map_err(|e| e.message)?;
        bind_summaries(vec![wire.summary], &wire.layouts, &wire.abis, layout, abi)?
            .into_iter()
            .next()
            .ok_or_else(|| "missing native unit summary".into())
    }
}
fn erase_summaries(
    summaries: &mut [UnitSummary],
    table: &LayoutTable,
    keys: &LayoutKeys,
) -> Result<(Vec<LayoutKey>, Vec<ContentHash>), String> {
    let (mut layouts, mut abis) = (Vec::new(), Vec::new());
    let (mut layout_index, mut abi_index) = (BTreeMap::new(), BTreeMap::new());
    let (mut missing, mut missing_abi) = (None, None);
    for summary in summaries {
        summary.for_each_id(
            &mut |id| {
                if table.layout(*id).is_none() {
                    missing.get_or_insert_with(|| format!("layout {}", id.0));
                    return;
                }
                let next = layout_index.len() as u32;
                *id = LayoutId(*layout_index.entry(id.0).or_insert_with(|| {
                    layouts.push(keys.key(*id));
                    next
                }));
            },
            &mut |id| match keys.fn_abi_of(table, *id) {
                Some(key) => {
                    let next = abi_index.len() as u32;
                    *id = FnAbiId(*abi_index.entry(id.0).or_insert_with(|| {
                        abis.push(key);
                        next
                    }));
                }
                None => {
                    missing_abi.get_or_insert_with(|| format!("FnAbi {}", id.0));
                }
            },
        );
    }
    if let Some(missing) = missing.or(missing_abi) {
        return Err(format!("the summary names {missing}, outside the table"));
    }
    Ok((layouts, abis))
}
fn bind_summaries(
    encoded: Vec<WireSummary>,
    layout_keys: &[LayoutKey],
    abi_keys: &[ContentHash],
    layout: &impl Fn(&LayoutKey) -> Option<LayoutId>,
    abi: &impl Fn(&ContentHash) -> Option<FnAbiId>,
) -> Result<Vec<UnitSummary>, String> {
    let layouts = layout_keys
        .iter()
        .map(|k| layout(k).ok_or_else(|| format!("no layout has key {:?}", k.0)))
        .collect::<Result<Vec<_>, _>>()?;
    let abis = abi_keys
        .iter()
        .map(|k| abi(k).ok_or_else(|| format!("no FnAbi has key {k:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut summaries = Vec::with_capacity(encoded.len());
    for summary in encoded {
        let mut summary = UnitSummary::from(summary);
        let (mut missing, mut missing_abi) = (None, None);
        summary.for_each_id(
            &mut |id| match layouts.get(id.0 as usize) {
                Some(bound) => *id = *bound,
                None => missing = Some(format!("erased layout {}", id.0)),
            },
            &mut |id| match abis.get(id.0 as usize) {
                Some(bound) => *id = *bound,
                None => missing_abi = Some(format!("erased FnAbi {}", id.0)),
            },
        );
        if let Some(missing) = missing.or(missing_abi) {
            return Err(format!("{missing} has no binding"));
        }
        summaries.push(summary);
    }
    Ok(summaries)
}
struct SummaryWire {
    summary: WireSummary,
    layouts: Vec<LayoutKey>,
    abis: Vec<ContentHash>,
}
snapshot_struct!(SummaryWire {
    summary,
    layouts,
    abis
});

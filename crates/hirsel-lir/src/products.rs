//! Portable native products. A product carries only its reached layout/ABI
//! graph. Binding selected products is temporary and grants no stage receipt.
//!
//! Reuses layout_keys' canonical recursive component encoding and the existing
//! objects erasure walk. Unlike Zig InternPool session indices, stored edges
//! are canonical local graph indices backed by complete content keys.
use crate::abi_types::FnAbiId;
use crate::ir::LirUnit;
use crate::layout_types::{LayoutId, LayoutTable, TargetSpec};
use crate::stage::{Stage, ValidatedUnit};
use crate::{LayoutIndex, LayoutKeys};
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_syntax::ContentHash;
use std::{collections::BTreeMap, sync::Arc};

/// One reached closure, in canonical first-use order. Cyclic records use local
/// indices; no recursively nested hash encoding or program-local id escapes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutClosure {
    pub table: LayoutTable,
    pub layouts: Vec<ContentHash>,
    pub abis: Vec<ContentHash>,
}
snapshot_struct!(LayoutClosure {
    table,
    layouts,
    abis
});

/// Immutable canonical body and its direct content-named bindings. Clones share
/// the complete typed payload. Only explicit archive encoding serializes it;
/// demanded transformations clone and rebind raw data, then run independent C.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnitBody(Arc<BodyData>);
#[derive(Clone, Debug, Eq, PartialEq)]
struct BodyData {
    unit: BodyUnit,
    layouts: Vec<ContentHash>,
    abis: Vec<ContentHash>,
}
snapshot_struct!(BodyData {
    unit,
    layouts,
    abis
});
#[derive(Clone, Debug, Eq, PartialEq)]
enum BodyUnit {
    Value(LirUnit<crate::stage::Value>),
    Placed(LirUnit<crate::stage::Placed>),
    Rc(LirUnit<crate::stage::Rc>),
    Reuse(LirUnit<crate::stage::Reuse>),
    Ready(LirUnit<crate::stage::Ready>),
}
snapshot_enum!(BodyUnit { Value(unit), Placed(unit), Rc(unit), Reuse(unit), Ready(unit) });
impl BodyUnit {
    fn capture<S: Stage>(unit: LirUnit<S>) -> Self {
        use crate::stage::StageKind;
        match S::KIND {
            StageKind::Value => Self::Value(unit.into_stage()),
            StageKind::Placed => Self::Placed(unit.into_stage()),
            StageKind::Rc => Self::Rc(unit.into_stage()),
            StageKind::Reuse => Self::Reuse(unit.into_stage()),
            StageKind::Ready => Self::Ready(unit.into_stage()),
        }
    }
    fn materialize<S: Stage>(&self) -> Result<LirUnit<S>, String> {
        macro_rules! selected {
            ($unit:expr) => {{
                if $unit.header.stage != S::KIND {
                    return Err("native body product stage differs".into());
                }
                $unit.clone().into_stage()
            }};
        }
        Ok(match self {
            Self::Value(unit) => selected!(unit),
            Self::Placed(unit) => selected!(unit),
            Self::Rc(unit) => selected!(unit),
            Self::Reuse(unit) => selected!(unit),
            Self::Ready(unit) => selected!(unit),
        })
    }
}
impl hirsel_codec::Snapshot for UnitBody {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        self.0.write(w);
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(Arc::new(BodyData::read(r)?)))
    }
}
impl UnitBody {
    /// Edit raw ABI operands with copy on write. Modified products carry no
    /// native acceptance and must pass binding and independent validation.
    pub fn abi_keys_mut(&mut self) -> &mut [ContentHash] {
        &mut Arc::make_mut(&mut self.0).abis
    }
}

fn map_abi(abi: &mut crate::abi_types::FnAbi, mut layout: impl FnMut(&mut LayoutId)) {
    for p in &mut abi.parameters {
        layout(&mut p.layout);
    }
    if let Some(r) = &mut abi.result {
        layout(&mut r.layout);
    }
}

impl LayoutClosure {
    /// Capture independently checked local input rows and common rows a later
    /// stage may introduce. Additional finalizer ABIs must be explicit reads.
    pub fn capture<S: Stage>(
        unit: &ValidatedUnit<S>,
        inputs: &crate::validate::Inputs<'_>,
        additional_abis: &[FnAbiId],
    ) -> Result<(Self, UnitBody), String> {
        let table = inputs.layouts;
        let index = LayoutIndex::new(table);
        if crate::validate::UnitInputs::from(inputs).digest_with_index(unit.unit(), &index)?
            != unit.receipt().checked_inputs()
        {
            return Err("native product inputs differ from checked unit".into());
        }
        let (erased, direct_layouts, direct_abis) = crate::objects::erased(unit.unit());
        let keys = LayoutKeys::new(table);
        let mut abi_ids = direct_abis.clone();
        let mut additional = additional_abis.to_vec();
        additional.sort_by_key(|id| keys.fn_abi(&table.fn_abis[id.0 as usize]));
        for id in additional {
            if !abi_ids.contains(&id) {
                abi_ids.push(id);
            }
        }
        let closure = Self::select(table, inputs.target, &direct_layouts, &abi_ids)?;
        // Numeric aliases are not semantic identities. Canonicalize direct
        // operands by content before encoding, so equal ABI/layout rows remain
        // byte-identical after products share a temporary binding table.
        let mut body_layouts = Vec::new();
        let mut body_abis = Vec::new();
        let mut layout_positions = BTreeMap::new();
        let mut abi_positions = BTreeMap::new();
        let local_layouts: Vec<_> = direct_layouts
            .iter()
            .map(|id| {
                let key = keys.key(*id).0;
                let next = body_layouts.len() as u32;
                let position = *layout_positions.entry(key).or_insert_with(|| {
                    body_layouts.push(key);
                    next
                });
                LayoutId(position)
            })
            .collect();
        let local_abis: Vec<_> = direct_abis
            .iter()
            .map(|id| {
                let key = keys.fn_abi(&table.fn_abis[id.0 as usize]);
                let next = body_abis.len() as u32;
                let position = *abi_positions.entry(key).or_insert_with(|| {
                    body_abis.push(key);
                    next
                });
                FnAbiId(position)
            })
            .collect();
        let canonical = crate::objects::rebound(erased, &local_layouts, &local_abis)?;
        let body = UnitBody(Arc::new(BodyData {
            unit: BodyUnit::capture(canonical),
            layouts: body_layouts,
            abis: body_abis,
        }));
        Ok((closure, body))
    }

    /// Select ordinary reached layout/ABI data without inventing a unit or a
    /// validator receipt. Discovery uses this for content-named environments;
    /// a consuming Value or Ready still requires actual independent C.
    pub fn select(
        table: &LayoutTable,
        target: &TargetSpec,
        layout_roots: &[LayoutId],
        abi_ids: &[FnAbiId],
    ) -> Result<Self, String> {
        if table.target != target.digest() {
            return Err("native layout selection target differs".into());
        }
        let keys = LayoutKeys::new(table);
        let index = LayoutIndex::new(table);
        // The passes select their synthetic rows by first match. Preserve the
        // source table's selected rows ahead of same-kind source type layouts,
        // including when several closures share a temporary binding table.
        let mut roots = index.introduced().collect::<Vec<_>>();
        roots.extend_from_slice(layout_roots);
        for id in abi_ids {
            let abi = table
                .fn_abis
                .get(id.0 as usize)
                .ok_or("finalizer ABI outside table")?;
            roots.extend(abi.parameters.iter().map(|p| p.layout));
            roots.extend(abi.result.iter().map(|r| r.layout));
        }
        // Canonical first-use traversal preserves multiplicity within recursive
        // components. Equal rooted keys must not collapse a symmetric cycle.
        let mut ids = BTreeMap::new();
        let mut ordered = Vec::new();
        let mut pending = roots;
        pending.reverse();
        while let Some(id) = pending.pop() {
            if ids.contains_key(&id) {
                continue;
            }
            let record = table.layout(id).ok_or("product layout outside table")?;
            ids.insert(id, LayoutId(ordered.len() as u32));
            ordered.push(id);
            let children: Vec<_> = record
                .references()
                .into_iter()
                .chain(index.implicit(id))
                .collect();
            pending.extend(children.into_iter().rev());
        }
        let mut out = LayoutTable::new(target);
        for id in &ordered {
            let mut record = table.layout(*id).expect("reached layout").clone();
            record.for_each_reference_mut(&mut |id| *id = ids[id]);
            out.layouts.push(record);
        }
        let mut abi_rows = BTreeMap::new();
        for id in abi_ids {
            let abi = &table.fn_abis[id.0 as usize];
            abi_rows.entry(keys.fn_abi(abi)).or_insert_with(|| {
                let mut abi = abi.clone();
                map_abi(&mut abi, |id| *id = ids[id]);
                abi
            });
        }
        out.fn_abis.extend(abi_rows.values().cloned());
        let closure = Self {
            table: out,
            layouts: ordered.iter().map(|id| keys.key(*id).0).collect(),
            abis: abi_rows.keys().copied().collect(),
        };
        closure.check(target)?;
        Ok(closure)
    }

    /// Check actual rows and complete recursive keys. Descriptor fields and
    /// content integrity never substitute for independent layout validation.
    pub fn check(&self, target: &TargetSpec) -> Result<(), String> {
        if self.layouts.len() != self.table.layouts.len()
            || self.abis.len() != self.table.fn_abis.len()
            || self.abis.windows(2).any(|w| w[0] >= w[1])
        {
            return Err("noncanonical reached native layout graph".into());
        }
        crate::layout_validate::validate_table(&self.table, target)
            .map_err(|e| format!("native layout product: {e:?}"))?;
        let keys = LayoutKeys::new(&self.table);
        if keys
            .all()
            .iter()
            .map(|k| k.0)
            .ne(self.layouts.iter().copied())
            || self
                .table
                .fn_abis
                .iter()
                .map(|a| keys.fn_abi(a))
                .ne(self.abis.iter().copied())
        {
            return Err("native layout product content identity differs".into());
        }
        Ok(())
    }
}

/// One temporary table for selected products, owned by a with_ready callback.
/// The maps have this binding's lifetime, never a second publication owner.
#[derive(Clone)]
pub struct Binding {
    pub table: LayoutTable,
    layouts: BTreeMap<ContentHash, LayoutId>,
    abis: BTreeMap<ContentHash, FnAbiId>,
}
impl Binding {
    pub fn new(target: &TargetSpec, closures: &[&LayoutClosure]) -> Result<Self, String> {
        for closure in closures {
            closure.check(target)?;
        }
        let mut table = LayoutTable::new(target);
        let mut layouts = BTreeMap::new();
        let mut abis: BTreeMap<ContentHash, FnAbiId> = BTreeMap::new();
        // Merge complete SCCs, never individual equal keys within a recursive
        // component. Reuse existing graph order: dependencies settle first.
        for c in closures {
            let graph: Vec<Vec<_>> = c
                .table
                .layouts
                .iter()
                .map(|r| r.references().iter().map(|id| id.0 as usize).collect())
                .collect();
            for component in hirsel_graph::components(&graph) {
                if component
                    .iter()
                    .all(|&i| layouts.contains_key(&c.layouts[i]))
                {
                    continue;
                }
                if component
                    .iter()
                    .any(|&i| layouts.contains_key(&c.layouts[i]))
                {
                    return Err("partial recursive layout component".into());
                }
                let local: BTreeMap<_, _> = component
                    .iter()
                    .enumerate()
                    .map(|(at, &i)| (i, LayoutId((table.layouts.len() + at) as u32)))
                    .collect();
                for &i in &component {
                    layouts.entry(c.layouts[i]).or_insert(local[&i]);
                }
                for &i in &component {
                    let mut record = c.table.layouts[i].clone();
                    record.for_each_reference_mut(&mut |id| {
                        *id = local
                            .get(&(id.0 as usize))
                            .copied()
                            .unwrap_or_else(|| layouts[&c.layouts[id.0 as usize]]);
                    });
                    table.layouts.push(record);
                }
            }
            for (key, abi) in c.abis.iter().zip(&c.table.fn_abis) {
                let mut abi = abi.clone();
                map_abi(&mut abi, |id| *id = layouts[&c.layouts[id.0 as usize]]);
                match abis.get(key) {
                    Some(id) if table.fn_abis[id.0 as usize] != abi => {
                        return Err("conflicting native ABI product".into());
                    }
                    Some(_) => {}
                    None => {
                        abis.insert(*key, FnAbiId(table.fn_abis.len() as u32));
                        table.fn_abis.push(abi);
                    }
                }
            }
        }
        crate::layout_validate::validate_table(&table, target)
            .map_err(|e| format!("bound native layouts: {e:?}"))?;
        let keys = LayoutKeys::new(&table);
        if layouts.iter().any(|(key, id)| keys.key(*id).0 != *key)
            || abis
                .iter()
                .any(|(key, id)| keys.fn_abi(&table.fn_abis[id.0 as usize]) != *key)
        {
            return Err("bound native product content identities differ".into());
        }
        Ok(Self {
            table,
            layouts,
            abis,
        })
    }

    /// Resolve content identities within this binding's temporary table. These
    /// maps are owned here; no second publication/global layout map is needed.
    pub fn layout(&self, key: ContentHash) -> Option<LayoutId> {
        self.layouts.get(&key).copied()
    }
    pub fn abi(&self, key: ContentHash) -> Option<FnAbiId> {
        self.abis.get(&key).copied()
    }

    /// Clone and rebind only on demand; the returned unit has no reused seal.
    pub fn materialize<S: Stage>(&self, body: &UnitBody) -> Result<LirUnit<S>, String> {
        let layouts = body
            .0
            .layouts
            .iter()
            .map(|k| self.layouts.get(k).copied().ok_or("missing product layout"))
            .collect::<Result<Vec<_>, _>>()?;
        let abis = body
            .0
            .abis
            .iter()
            .map(|k| self.abis.get(k).copied().ok_or("missing product ABI"))
            .collect::<Result<Vec<_>, _>>()?;
        let unit = body.0.unit.materialize::<S>()?;
        crate::objects::rebound(unit, &layouts, &abis)
    }
}

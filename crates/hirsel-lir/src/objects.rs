//! The native object cache's key and store seam (design §1e, §3).
//!
//! A `UnitKey` hashes everything a unit object's bytes depend on: compiler, LIR,
//! validator, backend and ISA identity, target and opt level, the code-affecting native
//! options, the unit's instance, role, origin and linkage, its TIR digest and
//! implementation revision, the `LayoutKey`s and FnAbi keys it reaches, each callee's
//! symbol and ABI digest, its `CodecKey`s, `debug` and `measurement` (with the resolved
//! source locations under measurement), the manifest rows it uses and the finalizer
//! facts it reads. Units never contain a number the program assigns (`crate::glue`), so
//! one key names the same bytes in every program.
//!
//! An `ObjectStore` maps keys to stored unit fragments. Implementations (`none@1`,
//! `dir@1`, `sim@1`) are selected by the host; none changes program meaning, only what
//! is recompiled.
use crate::abi_types::FnAbiId;
use crate::emission::EmitInputs;
use crate::ir::{Inst, LirUnit};
use crate::layout_types::LayoutId;
use crate::stage::Ready;
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The content key of one planned unit's object.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct UnitKey(pub ContentHash);

impl UnitKey {
    /// The key's lowercase hex digits, as stores name entries.
    pub fn hex(&self) -> String {
        self.0.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Stored unit fragments by `UnitKey`. A `get` that cannot produce intact bytes is a
/// miss; a `put` that cannot store is dropped. Neither is ever an error the build sees.
pub trait ObjectStore: Send + Sync {
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>>;
    fn put(&self, key: &UnitKey, value: &[u8]);
}

/// A layout or FnAbi index a unit names.
enum TableId<'a> {
    Layout(&'a mut LayoutId),
    Abi(&'a mut FnAbiId),
}

/// Every `LayoutId` and `FnAbiId` `unit` names, mutably, in a fixed walk order: the
/// header's ABI, the types, the instructions, then the call, construct, closure, init
/// flag and codec tables. Terminators and pools name none.
fn for_each_table_id(unit: &mut LirUnit<Ready>, f: &mut impl FnMut(TableId<'_>)) {
    f(TableId::Abi(&mut unit.header.abi));
    for ty in &mut unit.types {
        f(TableId::Layout(&mut ty.layout));
    }
    for inst in &mut unit.instructions {
        match inst {
            Inst::Compare { layout, .. }
            | Inst::FlatCopy { layout, .. }
            | Inst::Hash { layout, .. }
            | Inst::Release { layout, .. }
            | Inst::Reset { layout, .. }
            | Inst::RegionReserve { layout, .. } => f(TableId::Layout(layout)),
            _ => {}
        }
    }
    let tables = &mut unit.tables;
    for call in &mut tables.calls {
        f(TableId::Abi(&mut call.abi));
    }
    for site in &mut tables.constructs {
        f(TableId::Layout(&mut site.layout));
    }
    for site in &mut tables.closures {
        if let Some(code) = &mut site.code {
            f(TableId::Abi(&mut code.abi));
        }
        f(TableId::Layout(&mut site.environment));
    }
    for flag in &mut tables.init_flags {
        if let Some(layout) = &mut flag.environment {
            f(TableId::Layout(layout));
        }
    }
    for plan in &mut tables.codecs {
        for node in &mut plan.nodes {
            for layout in node.layouts_mut() {
                f(TableId::Layout(layout));
            }
        }
        for check in &mut plan.checks {
            f(TableId::Abi(&mut check.abi));
        }
    }
}

/// `unit` with its program bindings erased: the input and layout-table digests zeroed,
/// and each `LayoutId` and `FnAbiId` renumbered by first use. Returns it with the
/// original ids in that order.
pub fn erased(unit: &LirUnit<Ready>) -> (LirUnit<Ready>, Vec<LayoutId>, Vec<FnAbiId>) {
    let mut erased = unit.clone();
    erased.header.input.digest = ContentHash([0; 32]);
    erased.header.layouts = ContentHash([0; 32]);
    let (mut layouts, mut abis) = (Vec::new(), Vec::new());
    let (mut layout_index, mut abi_index) = (BTreeMap::new(), BTreeMap::new());
    for_each_table_id(&mut erased, &mut |id| match id {
        TableId::Layout(id) => {
            let next = layout_index.len() as u32;
            let index = *layout_index.entry(id.0).or_insert_with(|| {
                layouts.push(*id);
                next
            });
            *id = LayoutId(index);
        }
        TableId::Abi(id) => {
            let next = abi_index.len() as u32;
            let index = *abi_index.entry(id.0).or_insert_with(|| {
                abis.push(*id);
                next
            });
            *id = FnAbiId(index);
        }
    });
    (erased, layouts, abis)
}

/// The `UnitKey` of `unit`'s object as `toolchain` emits it with `inputs`, `debug`
/// line tables and the small callee bodies `copies` (by their keys) copied in. Covers
/// the erased unit (`erased`), the `LayoutKey`s and FnAbi keys of the ids it names, the
/// static data it reads and the manifest rows of the operations it calls, the target,
/// the options and every program-wide fact emission reads (`EmitInputs`), the versions
/// and the unit's linkage. The error names an id outside the table.
pub fn unit_key(
    unit: &LirUnit<Ready>,
    inputs: &EmitInputs<'_>,
    toolchain: &str,
    debug: bool,
    copies: &[UnitKey],
) -> Result<UnitKey, String> {
    let (erased, layouts, abis) = erased(unit);
    let mut parts: Vec<Vec<u8>> = vec![
        hirsel_codec::to_text(&erased).into_bytes(),
        toolchain.as_bytes().to_vec(),
        format!(
            "lir {} validator {} interface {} abi {} runtime {} layout {}",
            crate::LIR_VERSION,
            crate::VALIDATOR_VERSION,
            crate::INTERFACE_VERSION,
            crate::ABI_VERSION,
            crate::RUNTIME_ABI_VERSION,
            crate::LAYOUT_VERSION,
        )
        .into_bytes(),
        inputs.target.digest().0.to_vec(),
        format!(
            "{:?} callbacks {} tracking {} fuel {} measurement {} debug {} inline {} copy {}",
            inputs.opt,
            inputs.callbacks,
            inputs.tracking,
            inputs.fuel,
            inputs.measurement,
            debug,
            inputs.inline_instances.contains(&unit.header.instance),
            inputs.available_externally.contains(&unit.header.instance),
        )
        .into_bytes(),
    ];
    for id in &layouts {
        if inputs.layouts.layout(*id).is_none() {
            return Err(format!("the unit names layout {}, outside the table", id.0));
        }
        parts.push(inputs.keys.key(*id).0.0.to_vec());
    }
    for id in &abis {
        let key = inputs
            .keys
            .fn_abi_of(inputs.layouts, *id)
            .ok_or_else(|| format!("the unit names FnAbi {}, outside the table", id.0))?;
        parts.push(key.0.to_vec());
    }
    // The static data the unit reads: glue holds the blocks, but the unit's view of them
    // carries their content.
    for inst in &unit.instructions {
        let Inst::ReadStatic { symbol, .. } = *inst else {
            continue;
        };
        let symbol = unit
            .tables
            .symbols
            .get(symbol.index())
            .ok_or("a static outside the symbol table")?;
        let data = inputs
            .statics
            .iter()
            .find(|d| d.symbol == *symbol)
            .ok_or("a static without its data")?;
        let mut data = data.clone();
        for block in &mut data.blocks {
            parts.push(inputs.keys.key(block.layout).0.0.to_vec());
            block.layout = LayoutId(0);
        }
        parts.push(hirsel_codec::to_text(&data).into_bytes());
    }
    for operation in &unit.tables.operations {
        match inputs.manifest.row(operation) {
            Some(row) => parts.push(hirsel_codec::to_text(row).into_bytes()),
            None => parts.push(b"no row".to_vec()),
        }
    }
    for copy in copies {
        parts.push(copy.0.0.to_vec());
    }
    Ok(UnitKey(ContentHash::of_parts(
        "hirsel.unit-key.v1",
        parts.iter().map(Vec::as_slice),
    )))
}

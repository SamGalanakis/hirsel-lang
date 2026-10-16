//! Optional source ownership for profiling, independent of compiler providers.
use hirsel_lir::ir::*;
use hirsel_lir::stage::Ready;
use hirsel_syntax::DefPath;
use std::collections::BTreeMap;

pub use hirsel_span::SourceFile as ProfileSource;
pub type ProfileSources = BTreeMap<DefPath, ProfileSource>;

/// Prefer the operation's explicit source/cleanup anchor. LIR operations without
/// an individual anchor retain their enclosing block's location.
pub(crate) fn instruction_site(unit: &LirUnit<Ready>, block: BlockId, inst: &Inst) -> SiteRange {
    let source = |id: SourceAnchorId| unit.tables.source_anchors[id.index()].site;
    let cleanup = |id: CleanupAnchorId| unit.tables.cleanup_anchors[id.index()].site;
    match *inst {
        Inst::Construct { construct, .. }
        | Inst::Reuse { construct, .. }
        | Inst::FlatAppend { construct, .. } => {
            source(unit.tables.constructs[construct.index()].anchor)
        }
        Inst::Closure { closure, .. } => source(unit.tables.closures[closure.index()].anchor),
        Inst::Release { anchor, .. }
        | Inst::Reset { anchor, .. }
        | Inst::FinalizeBegin { anchor, .. }
        | Inst::FinalizeEnd { anchor, .. } => cleanup(anchor),
        _ => source(unit.blocks[block.index()].anchor),
    }
}
pub(crate) fn terminator_site(unit: &LirUnit<Ready>, block: BlockId) -> SiteRange {
    let anchor = match unit.terminators[block.index()] {
        Terminator::Call { call, .. } => unit.tables.calls[call.index()].anchor,
        _ => unit.blocks[block.index()].anchor,
    };
    unit.tables.source_anchors[anchor.index()].site
}

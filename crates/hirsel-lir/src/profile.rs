//! Optional source ownership for profiling, independent of compiler providers.
use crate::ir::*;
use crate::stage::Ready;
use hirsel_syntax::DefPath;
use std::collections::BTreeMap;

/// A source owner's file and its item's anchor base: LIR sites are relative to
/// the item, so a site's file offset is `base + site`.
#[derive(Clone, Debug)]
pub struct ProfileSource {
    pub path: std::path::PathBuf,
    pub file: hirsel_span::SourceFile,
    pub base: u32,
    /// The original definition's canonical sources, including generated text.
    pub anchors: Vec<(hirsel_span::SourceFile, u32)>,
}
impl ProfileSource {
    pub const COMPILATION_DIRECTORY: &str = hirsel_span::SourceFile::COMPILATION_DIRECTORY;
    /// The package-qualified name artifacts carry (`SourceFile::logical_path`).
    pub fn logical_path(&self) -> &std::path::Path {
        self.file.logical_path()
    }
    pub fn source(&self, source: u32) -> Option<(&hirsel_span::SourceFile, u32)> {
        self.anchors
            .get(source as usize)
            .map(|(file, base)| (file, *base))
    }
    pub fn site_location(&self, site: SiteRange) -> Option<hirsel_span::LineColumn> {
        let (file, base) = self.source(site.source)?;
        file.text()
            .get(base.checked_add(site.start)? as usize..base.checked_add(site.end)? as usize)?;
        file.location(base.checked_add(site.start)?)
    }
    /// The line and column of an item-relative site offset.
    pub fn location(&self, site: u32) -> Option<hirsel_span::LineColumn> {
        self.file.location(self.base.checked_add(site)?)
    }
}
pub type ProfileSources = BTreeMap<DefPath, ProfileSource>;

/// Prefer the operation's explicit source/cleanup anchor. LIR operations without
/// an individual anchor retain their enclosing block's location.
pub fn instruction_site(unit: &LirUnit<Ready>, block: BlockId, inst: &Inst) -> SiteRange {
    let source = |id: SourceAnchorId| unit.tables.source_anchors[id.index()].site;
    let cleanup = |id: CleanupAnchorId| unit.tables.cleanup_anchors[id.index()].site;
    match *inst {
        Inst::Construct { construct, .. }
        | Inst::Reuse { construct, .. }
        | Inst::FlatAppend { construct, .. }
        | Inst::FlatClose { construct, .. } => {
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
pub fn terminator_site(unit: &LirUnit<Ready>, block: BlockId) -> SiteRange {
    let anchor = match unit.terminators[block.index()] {
        Terminator::Call { call, .. } => unit.tables.calls[call.index()].anchor,
        _ => unit.blocks[block.index()].anchor,
    };
    unit.tables.source_anchors[anchor.index()].site
}

/// Resolved source dependencies for instrumentation that embeds paths and lines.
/// Ordinary debug objects bind these only at link, never in cached unit code.
pub fn dependency_digest(sources: &ProfileSources) -> hirsel_syntax::ContentHash {
    let mut parts = Vec::new();
    for (definition, source) in sources {
        parts.push(definition.fingerprint().0.to_vec());
        for (file, base) in &source.anchors {
            parts.push(file.logical_path().to_string_lossy().as_bytes().to_vec());
            parts.push(file.text().as_bytes().to_vec());
            parts.push(base.to_le_bytes().to_vec());
        }
    }
    hirsel_syntax::ContentHash::of_parts(
        "hirsel.resolved-profile-sources.v1",
        parts.iter().map(Vec::as_slice),
    )
}

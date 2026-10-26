//! Current source inputs borrowed by frozen body workers. Signature snapshots
//! deliberately survive ordinary body edits; citation text never lives there.
use crate::*;
use hirsel_span::{FileId, Span};
use hirsel_syntax::{ContentHash, DefId, ItemSyntax};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Occurrence-aware names and exact cited-text inputs from one source revision.
/// Kept separate from a signature snapshot, which may survive body edits.
pub trait CitationSource: Sync {
    fn cited_name(&self, owner: DefId, target: DefId) -> Option<Span>;
    fn cited_text(&self, target: DefId) -> ContentHash;
    fn unique_failure(
        &self,
        _owner: DefId,
        _target: DefId,
        _claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        None
    }
    fn arm_insertion(&self, _owner: &DefPath, _site: Span) -> Option<(Span, ContentHash)> {
        None
    }
    fn record_owner(&self, _anchor: hirsel_span::Anchor) -> Option<(DefPath, u32)> {
        None
    }
}

pub(crate) struct SourceContext<'a> {
    pub unique_failures: &'a BTreeMap<DefId, Vec<hirsel_types::UniqueFailure>>,
    pub sources: &'a SourceMap,
    pub items: &'a BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    pub declarations: &'a DeclarationIndex,
    pub catalog: &'a crate::queries::catalog::Catalog,
}
impl SourceContext<'_> {
    fn file(&self, syntax: &ItemSyntax) -> Option<FileId> {
        self.sources.anchor(syntax.anchor).map(|(file, _)| file)
    }
}
impl CitationSource for SourceContext<'_> {
    fn unique_failure(
        &self,
        owner: DefId,
        target: DefId,
        claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        self.cited_name(owner, target)?;
        self.unique_failures
            .get(&target)?
            .iter()
            .find(|f| f.claim == claim)
            .cloned()
    }
    fn cited_name(&self, owner: DefId, target: DefId) -> Option<Span> {
        let file = match self.items.get(owner.path()).and_then(|v| v.first()) {
            Some(first) => self.file(first)?,
            None => {
                self.sources
                    .anchor(
                        self.declarations
                            .sites(owner.path())
                            .first()?
                            .name_span
                            .anchor,
                    )?
                    .0
            }
        };
        if let Some(items) = self.items.get(target.path()) {
            return items
                .iter()
                .find(|syntax| self.file(syntax) == Some(file))
                .map(|syntax| syntax.item.name_span);
        }
        self.declarations
            .sites(target.path())
            .iter()
            .find(|site| {
                self.sources
                    .anchor(site.name_span.anchor)
                    .is_some_and(|(f, _)| f == file)
            })
            .map(|site| site.name_span)
    }
    fn cited_text(&self, target: DefId) -> ContentHash {
        let Some(candidates) = self.items.get(target.path()) else {
            return self
                .catalog
                .cited_text(self.sources, target.path())
                .unwrap_or_else(|| {
                    ContentHash::of_parts("hirsel.read.uncited", std::iter::empty::<&[u8]>())
                });
        };
        let parts: Vec<Vec<u8>> = candidates
            .iter()
            .map(|candidate| {
                let mut part = self
                    .file(candidate)
                    .and_then(|file| self.sources.file(file))
                    .map_or(vec![], |file| {
                        file.logical_path().as_os_str().as_encoded_bytes().to_vec()
                    });
                part.push(0);
                part.extend(candidate.occurrence.to_le_bytes());
                part.extend(candidate.item.name_span.start.to_le_bytes());
                part.extend(candidate.item.name_span.end.to_le_bytes());
                part.extend(candidate.text_hash.0);
                part
            })
            .collect();
        ContentHash::of_parts("hirsel.read.cited", parts.iter().map(Vec::as_slice))
    }
}

/// Owned immutable citation inputs for admitted tasks that outlive preparation.
pub(crate) struct OwnedSourceContext {
    pub unique_failures: BTreeMap<DefId, Vec<hirsel_types::UniqueFailure>>,
    pub sources: SourceMap,
    pub items: Arc<BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>>,
    pub declarations: DeclarationIndex,
    pub catalog: crate::queries::catalog::Catalog,
    pub owners: Arc<crate::NativeRecordContext>,
    pub cited: BTreeMap<DefId, ContentHash>,
    pub arm_fixes: BTreeMap<DefPath, (Span, FileId, ContentHash)>,
}
impl OwnedSourceContext {
    fn borrowed(&self) -> SourceContext<'_> {
        SourceContext {
            unique_failures: &self.unique_failures,
            sources: &self.sources,
            items: &self.items,
            declarations: &self.declarations,
            catalog: &self.catalog,
        }
    }
}
impl CitationSource for OwnedSourceContext {
    fn unique_failure(
        &self,
        owner: DefId,
        target: DefId,
        claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        self.borrowed().unique_failure(owner, target, claim)
    }
    fn cited_name(&self, owner: DefId, target: DefId) -> Option<Span> {
        self.borrowed().cited_name(owner, target)
    }
    fn cited_text(&self, target: DefId) -> ContentHash {
        self.cited
            .get(&target)
            .copied()
            .unwrap_or_else(|| self.borrowed().cited_text(target))
    }
    fn arm_insertion(&self, owner: &DefPath, site: Span) -> Option<(Span, ContentHash)> {
        let (span, file, hash) = self.arm_fixes.get(owner)?;
        (self.sources.resolve(site).file == *file).then_some((*span, *hash))
    }
    fn record_owner(&self, anchor: hirsel_span::Anchor) -> Option<(DefPath, u32)> {
        use crate::NativeRecordSource;
        self.owners.record_owner(anchor)
    }
}

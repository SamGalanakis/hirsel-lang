//! Bodyless semantic launch binding, independent of native topology and IR.
use crate::*;
use std::collections::BTreeMap;

/// The existing source semantic closure and capability escapes. Construction
/// runs the ordinary checked source queries; no artifact or native descriptor
/// can construct this input. This conveys no native Value/Ready authority.
pub struct NativeSemanticProgram {
    program: crate::queries::SemanticProgram,
    escapes: BTreeMap<DefPath, Escape>,
    foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
}
impl NativeSemanticProgram {
    pub(crate) fn new(
        program: crate::queries::SemanticProgram,
        escapes: BTreeMap<DefPath, Escape>,
        foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
    ) -> Self {
        Self {
            program,
            escapes,
            foreign_threads,
        }
    }
    pub fn semantic_program_revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramRevision, QueryError> {
        self.program.revision(entry)
    }
    pub fn semantic_contains(&self, definition: &DefPath) -> bool {
        self.program.definitions.contains(definition)
    }
    pub fn unsafe_reach(&self, entry: &DefPath) -> Option<UnsafeReach> {
        self.program.reach(entry, &self.escapes)
    }
    pub fn home_reach(&self, root: &DefPath) -> Option<UnsafeReach> {
        let home = self
            .escapes
            .iter()
            .filter(|(path, escape)| {
                **escape == Escape::Extern
                    && self
                        .foreign_threads
                        .get(&path.0.package)
                        .copied()
                        .unwrap_or_default()
                        == hirsel_types::ForeignThreads::Home
            })
            .map(|(path, escape)| (path.clone(), *escape))
            .collect();
        self.program.reach(root, &home)
    }
}
impl QueryEngine {
    /// Capture the same semantic closure as snapshot import, retaining only
    /// checked dependency nodes and escape metadata, never source Definitions.
    pub fn native_semantic_program(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<NativeSemanticProgram, QueryError> {
        let program = self.semantic_program(roots)?;
        let escapes = self.escapes(&program)?;
        Ok(NativeSemanticProgram::new(
            program,
            escapes,
            self.foreign_threads.clone(),
        ))
    }
}

//! Charged inputs for the explicitly selected ownership diagnostic graph.
use hirsel_engine::{
    FrontBodyQueries, FrontLowerQueries, FrontNativeDefinitionInputs, FrontNativeDefinitionQueries,
    FrontNativeQueries, FrontRunQueries, runtime::MemoryReservation, store::ResultStore,
};
use hirsel_syntax::DefPath;
use std::sync::Arc;

pub(super) struct SourceInputs {
    pub definitions: Vec<DefPath>,
    pub catalogue: FrontNativeDefinitionInputs,
    pub entry: DefPath,
    pub front: Arc<FrontBodyQueries>,
    pub lower: Arc<FrontLowerQueries>,
    pub run: Arc<FrontRunQueries>,
    pub store: Arc<dyn ResultStore>,
}
pub(super) struct Admission {
    pub instances: MemoryReservation,
    pub arena: MemoryReservation,
    pub compilation: MemoryReservation,
}

/// These bytes are split out of the existing snapshot partition. Constructors
/// still bound actual retained metadata and outputs against each partition.
pub(super) struct Footprint {
    pub(super) catalogue: u64,
    pub(super) source: u64,
    pub(super) bootstrap: u64,
    pub(super) tasks: u64,
}
impl Footprint {
    pub(super) const ROUTING: u64 = 16 << 10;
    pub fn new(definitions: &[DefPath], slack: u64) -> Option<Self> {
        let bootstrap = definitions.iter().try_fold(32 << 10, |bytes: u64, path| {
            bytes.checked_add(serde_json::to_vec(path).ok()?.len() as u64 + 128)
        })?;
        Some(Self {
            catalogue: FrontNativeDefinitionQueries::input_bytes(definitions)?
                .checked_add(slack / 5)?,
            source: FrontNativeQueries::input_bytes(definitions)?.checked_add(slack / 5)?,
            bootstrap,
            tasks: slack / 5,
        })
    }
    pub fn with_catalogue_bytes(mut self, definitions: &[DefPath], bytes: u64) -> Option<Self> {
        self.catalogue = self
            .catalogue
            .checked_sub(FrontNativeDefinitionQueries::input_bytes(definitions)?)?
            .checked_add(bytes)?;
        Some(self)
    }
    pub fn bytes(&self) -> Option<u64> {
        [
            self.catalogue,
            self.source,
            self.bootstrap,
            self.tasks,
            self.tasks,
            self.tasks,
            Self::ROUTING,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
    }
}

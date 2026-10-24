//! Immutable launch facts from the admitted source semantic closure.
use super::*;
use hirsel_run_schema::{CallForm, CalleeFacts, CodeReach, ProgramRevision};
/// Metadata consumed by interpreter/H/native hosts. No mutable query engine,
/// body cache or native Value reconstruction is available through this output.
pub struct FrontRunOutput {
    pub entry: DefPath,
    pub revision: ProgramRevision,
    pub code: CodeReach,
    pub foreign: ForeignUse,
    pub callees: BTreeMap<String, CalleeFacts>,
    pub entries: BTreeMap<String, (CalleeFacts, CallForm)>,
    pub runtime: Arc<hirsel_interp::aggregate::RuntimeTypes>,
    pub checked_runtime: Arc<hirsel_interp::checked::CheckedRuntime>,
    pub(crate) _owner: Arc<dyn Send + Sync>,
}

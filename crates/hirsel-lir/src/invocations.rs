//! Independent source instance body identity, shared by invocation and storage
//! evidence. Candidate anchors cannot select their own source body inventory.
use hirsel_syntax::DefPath;

/// Body selected by the independent source instance plan. Candidate anchors
/// cannot select which source invocation inventory they are checked against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceBody {
    pub definition: DefPath,
    pub instance: hirsel_syntax::ContentHash,
    pub body: u32,
}

hirsel_codec::snapshot_struct!(ReferenceBody {
    definition,
    instance,
    body
});

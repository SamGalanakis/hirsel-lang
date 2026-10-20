//! One written header's normalization request and result, shared by the
//! signature phase and the query engine that drives it through a trait object.
use hirsel_syntax as ast;
use hirsel_types::{
    DeclarationSite, Metadata, QueryOutput, Resolution, Signature, TypeDefinition,
};

/// All borrowed input snapshots must belong to the same syntax/interner session.
pub struct Input<'a> {
    pub syntax: &'a ast::ItemSyntax,
    pub context: &'a ast::FileContext,
    pub resolution: &'a Resolution,
    pub metadata: &'a Metadata,
    /// This owner's declaration sites in source order (or empty for AST fixtures).
    pub declaration_sites: &'a [DeclarationSite],
    /// None for the published signature; a stage for a provisional one that
    /// another member of a recursive type group reads.
    pub stage: Option<hirsel_types::GroupStage>,
}
/// Private defining shape produced by the same pass as its public declaration.
#[derive(Clone, Debug)]
pub struct OwnerRepresentation {
    pub declaration: TypeDefinition,
}
pub struct NormalizedDeclaration {
    pub public: QueryOutput<Signature>,
    pub owner: Option<OwnerRepresentation>,
}

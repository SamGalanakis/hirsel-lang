//! One written header's normalization request and result, shared by the
//! signature phase and the query engine that drives it through a trait object.
use hirsel_syntax as ast;
use hirsel_types::{
    DeclarationSite, Metadata, QueryOutput, Resolution, Signature, TypeDefinition, TypeStore,
};

/// All borrowed input snapshots must belong to the same syntax/interner session.
pub struct Input<'a> {
    pub tree: &'a ast::SyntaxTree,
    pub item: ast::ItemId,
    pub context: &'a ast::FileContext,
    pub resolution: &'a Resolution,
    pub metadata: &'a Metadata,
    /// This owner's declaration sites in source order (or empty for AST fixtures).
    pub declaration_sites: &'a [DeclarationSite],
}
/// Private defining shape produced by the same pass as its public declaration.
#[derive(Clone, Debug)]
pub struct OwnerRepresentation {
    pub types: TypeStore,
    pub declaration: TypeDefinition,
}
pub struct NormalizedDeclaration {
    pub public: QueryOutput<Signature>,
    pub owner: Option<OwnerRepresentation>,
}

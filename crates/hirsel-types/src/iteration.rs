//! Signature-selected cursor execution, shared across checking and lowering.
use crate::*;
use hirsel_syntax::StmtId;

/// Published borrowed-loop association. Operations retain their own lexical
/// generic binders; specialization checks them against the actual receiver.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct LoopSourceDeclaration {
    pub open: DefPath,
    pub next: DefPath,
    pub option: DefPath,
}

/// The nominal signature which selected an implicit protocol call pair.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ProtocolAssociation {
    pub source: DefPath,
    pub fingerprint: hirsel_syntax::ContentHash,
}

/// Complete implicit open/next calls. All TypeIds belong to the checked body.
/// Source evaluation and use are fixed by H; J refines the same graph contracts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorPlan {
    pub association: Option<ProtocolAssociation>,
    pub statement: StmtId,
    pub source: ExprId,
    pub source_use: UseKind,
    pub open: DefPath,
    pub open_arguments: Vec<GenericArgument>,
    pub next: DefPath,
    pub next_arguments: Vec<GenericArgument>,
    pub cursor: TypeId,
    pub step: OptionLayout,
}
/// Declared Option constructor layout for one cursor advancement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionLayout {
    pub result: TypeId,
    pub item: TypeId,
    pub definition: DefPath,
    pub none_variant: VariantId,
    pub some_variant: VariantId,
}

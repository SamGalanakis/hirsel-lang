//! One read contract for live typed TIR and checked archived rows. Views borrow
//! their admitted owner; transforming an archive is an explicit materialization.
use crate::{CheckedTir, TirBlockRows, TirBodyHeader, TirBodyRows, TirHeader, TirMatchRows};
use hirsel_tir::*;
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub enum TirView<'a> {
    Live(&'a Definition),
    Archived(&'a CheckedTir),
}
impl<'a> TirView<'a> {
    pub fn header(self) -> Option<TirHeader> {
        match self {
            Self::Live(value) => Some(TirHeader {
                version: TIR_VERSION,
                inferred_unique: value.inferred_unique.clone(),
                path: value.path,
                signature_hash: value.signature_hash,
                entry: value.entry,
            }),
            Self::Archived(value) => value.header(),
        }
    }
    pub fn body_count(self) -> usize {
        match self {
            Self::Live(value) => value.bodies.len(),
            Self::Archived(value) => value.body_count() as usize,
        }
    }
    pub fn body(self, n: usize) -> Option<TirBodyView<'a>> {
        match self {
            Self::Live(value) => Some(TirBodyView::Live(value.bodies.get(n)?)),
            Self::Archived(value) => Some(TirBodyView::Archived(value.body(n.try_into().ok()?)?)),
        }
    }
    pub fn type_count(self) -> usize {
        match self {
            Self::Live(value) => value.types.len(),
            Self::Archived(value) => value.type_count() as usize,
        }
    }
    pub fn type_id(self, n: usize) -> Option<Cow<'a, hirsel_types::TypeId>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.types.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.type_id(n.try_into().ok()?)?)),
        }
    }
    pub fn caller_body_count(self) -> usize {
        match self {
            Self::Live(value) => value.caller_bodies.len(),
            Self::Archived(value) => value.caller_body_count() as usize,
        }
    }
    pub fn caller_body(self, n: usize) -> Option<Cow<'a, BodyId>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.caller_bodies.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.caller_body(n.try_into().ok()?)?)),
        }
    }
    pub fn protocol_association_count(self) -> usize {
        match self {
            Self::Live(value) => value.protocol_associations.len(),
            Self::Archived(value) => value.protocol_association_count() as usize,
        }
    }
    pub fn protocol_association(self, n: usize) -> Option<Cow<'a, ProtocolAssociationReceipt>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.protocol_associations.get(n)?)),
            Self::Archived(value) => {
                Some(Cow::Owned(value.protocol_association(n.try_into().ok()?)?))
            }
        }
    }
    pub fn contract_count(self) -> usize {
        match self {
            Self::Live(value) => value.contracts.len(),
            Self::Archived(value) => value.contract_count() as usize,
        }
    }
    pub fn contract(self, n: usize) -> Option<Cow<'a, (u32, BodyId)>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.contracts.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.contract(n.try_into().ok()?)?)),
        }
    }
    pub fn materialize(self) -> Option<Definition> {
        match self {
            Self::Live(value) => Some(value.clone()),
            Self::Archived(value) => value.materialize(),
        }
    }
}
#[derive(Clone, Copy)]
pub enum TirBodyView<'a> {
    Live(&'a Body),
    Archived(TirBodyRows<'a>),
}
impl<'a> TirBodyView<'a> {
    pub fn header(self) -> Option<TirBodyHeader> {
        match self {
            Self::Live(value) => Some(TirBodyHeader {
                arena_scope_grant: value.arena_scope_grant.clone(),
                parameter_scope: value.parameter_scope,
                entry: value.entry,
                result: value.result,
            }),
            Self::Archived(value) => value.header(),
        }
    }
    pub fn local_count(self) -> usize {
        match self {
            Self::Live(value) => value.locals.len(),
            Self::Archived(value) => value.local_count() as usize,
        }
    }
    pub fn local(self, n: usize) -> Option<Cow<'a, Local>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.locals.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.local(n.try_into().ok()?)?)),
        }
    }
    pub fn place_count(self) -> usize {
        match self {
            Self::Live(value) => value.places.len(),
            Self::Archived(value) => value.place_count() as usize,
        }
    }
    pub fn place(self, n: usize) -> Option<Cow<'a, Place>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.places.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.place(n.try_into().ok()?)?)),
        }
    }
    pub fn scope_count(self) -> usize {
        match self {
            Self::Live(value) => value.scopes.len(),
            Self::Archived(value) => value.scope_count() as usize,
        }
    }
    pub fn scope(self, n: usize) -> Option<Cow<'a, Scope>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.scopes.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.scope(n.try_into().ok()?)?)),
        }
    }
    pub fn block_count(self) -> usize {
        match self {
            Self::Live(value) => value.blocks.len(),
            Self::Archived(value) => value.block_count() as usize,
        }
    }
    pub fn block(self, n: usize) -> Option<TirBlockView<'a>> {
        match self {
            Self::Live(value) => Some(TirBlockView::Live(value.blocks.get(n)?)),
            Self::Archived(value) => Some(TirBlockView::Archived(value.block(n.try_into().ok()?)?)),
        }
    }
    pub fn rvalue_count(self) -> usize {
        match self {
            Self::Live(value) => value.rvalues.len(),
            Self::Archived(value) => value.rvalue_count() as usize,
        }
    }
    pub fn rvalue(self, n: usize) -> Option<Cow<'a, Rvalue>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.rvalues.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.rvalue(n.try_into().ok()?)?)),
        }
    }
    pub fn cleanup_count(self) -> usize {
        match self {
            Self::Live(value) => value.cleanups.len(),
            Self::Archived(value) => value.cleanup_count() as usize,
        }
    }
    pub fn cleanup(self, n: usize) -> Option<Cow<'a, Cleanup>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.cleanups.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.cleanup(n.try_into().ok()?)?)),
        }
    }
    pub fn cleanup_node_count(self) -> usize {
        match self {
            Self::Live(value) => value.cleanup_nodes.len(),
            Self::Archived(value) => value.cleanup_node_count() as usize,
        }
    }
    pub fn cleanup_node(self, n: usize) -> Option<Cow<'a, CleanupNode>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.cleanup_nodes.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.cleanup_node(n.try_into().ok()?)?)),
        }
    }
    pub fn parameter_count(self) -> usize {
        match self {
            Self::Live(value) => value.parameters.len(),
            Self::Archived(value) => value.parameter_count() as usize,
        }
    }
    pub fn parameter(self, n: usize) -> Option<Cow<'a, LocalId>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.parameters.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.parameter(n.try_into().ok()?)?)),
        }
    }
    pub fn borrow_cursor_count(self) -> usize {
        match self {
            Self::Live(value) => value.borrow_cursors.len(),
            Self::Archived(value) => value.borrow_cursor_count() as usize,
        }
    }
    pub fn borrow_cursor(self, n: usize) -> Option<Cow<'a, BorrowCursor>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.borrow_cursors.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.borrow_cursor(n.try_into().ok()?)?)),
        }
    }
    pub fn tail_site_count(self) -> usize {
        match self {
            Self::Live(value) => value.tail_sites.len(),
            Self::Archived(value) => value.tail_site_count() as usize,
        }
    }
    pub fn tail_site(self, n: usize) -> Option<Cow<'a, TailSite>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.tail_sites.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.tail_site(n.try_into().ok()?)?)),
        }
    }
    pub fn match_table_count(self) -> usize {
        match self {
            Self::Live(value) => value.matches.len(),
            Self::Archived(value) => value.match_table_count() as usize,
        }
    }
    pub fn match_table(self, n: usize) -> Option<TirMatchView<'a>> {
        match self {
            Self::Live(value) => Some(TirMatchView::Live(value.matches.get(n)?)),
            Self::Archived(value) => Some(TirMatchView::Archived(
                value.match_table(n.try_into().ok()?)?,
            )),
        }
    }
    pub fn scoped_parameter_count(self) -> usize {
        match self {
            Self::Live(value) => value.scoped_parameters.len(),
            Self::Archived(value) => value.scoped_parameter_count() as usize,
        }
    }
    pub fn scoped_parameter(self, n: usize) -> Option<Cow<'a, ScopedParameter>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.scoped_parameters.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.scoped_parameter(n.try_into().ok()?)?)),
        }
    }
    pub fn materialize(self) -> Option<Body> {
        match self {
            Self::Live(value) => Some(value.clone()),
            Self::Archived(value) => value.materialize(),
        }
    }
}
#[derive(Clone, Copy)]
pub enum TirBlockView<'a> {
    Live(&'a Block),
    Archived(TirBlockRows<'a>),
}
impl<'a> TirBlockView<'a> {
    pub fn instruction_count(self) -> usize {
        match self {
            Self::Live(value) => value.instructions.len(),
            Self::Archived(value) => value.instruction_count() as usize,
        }
    }
    pub fn instruction(self, n: usize) -> Option<Cow<'a, Instruction>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.instructions.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.instruction(n.try_into().ok()?)?)),
        }
    }
    pub fn terminator(self) -> Option<Cow<'a, Terminator>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(&value.terminator)),
            Self::Archived(value) => Some(Cow::Owned(value.terminator()?)),
        }
    }
    pub fn span(self) -> Option<hirsel_span::Span> {
        match self {
            Self::Live(value) => Some(value.span),
            Self::Archived(value) => value.span(),
        }
    }
    pub fn materialize(self) -> Option<Block> {
        match self {
            Self::Live(value) => Some(value.clone()),
            Self::Archived(value) => value.materialize(),
        }
    }
}
#[derive(Clone, Copy)]
pub enum TirMatchView<'a> {
    Live(&'a MatchTable),
    Archived(TirMatchRows<'a>),
}
impl<'a> TirMatchView<'a> {
    pub fn scrutinee(self) -> Option<PlaceId> {
        match self {
            Self::Live(value) => Some(value.scrutinee),
            Self::Archived(value) => value.scrutinee(),
        }
    }
    pub fn pattern_count(self) -> usize {
        match self {
            Self::Live(value) => value.patterns.len(),
            Self::Archived(value) => value.pattern_count() as usize,
        }
    }
    pub fn pattern(self, n: usize) -> Option<Cow<'a, MatchPattern>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.patterns.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.pattern(n.try_into().ok()?)?)),
        }
    }
    pub fn arm_count(self) -> usize {
        match self {
            Self::Live(value) => value.arms.len(),
            Self::Archived(value) => value.arm_count() as usize,
        }
    }
    pub fn arm(self, n: usize) -> Option<Cow<'a, MatchArm>> {
        match self {
            Self::Live(value) => Some(Cow::Borrowed(value.arms.get(n)?)),
            Self::Archived(value) => Some(Cow::Owned(value.arm(n.try_into().ok()?)?)),
        }
    }
}

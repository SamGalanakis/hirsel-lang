//! Stream the established Definition serde contract from checked rows. V's
//! byte-defined artifact identity must not require an owned body reconstruction.
use crate::{TirBlockView, TirBodyView, TirMatchView, TirView};
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeSeq, SerializeStruct},
};

struct Sequence<F> {
    len: usize,
    row: F,
}
impl<F, T> Serialize for Sequence<F>
where
    F: Fn(usize) -> Option<T>,
    T: Serialize,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len))?;
        for n in 0..self.len {
            let value =
                (self.row)(n).ok_or_else(|| S::Error::custom("unavailable checked TIR row"))?;
            sequence.serialize_element(&value)?;
        }
        sequence.end()
    }
}
macro_rules! sequence {
    ($view:expr, $count:ident, $row:ident) => {
        Sequence {
            len: $view.$count(),
            row: |n| $view.$row(n),
        }
    };
}
impl Serialize for TirView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Self::Live(value) = self {
            return value.serialize(serializer);
        }
        let header = self
            .header()
            .ok_or_else(|| S::Error::custom("unavailable checked TIR header"))?;
        let mut value = serializer.serialize_struct("Definition", 9)?;
        value.serialize_field("inferred_unique", &header.inferred_unique)?;
        value.serialize_field(
            "caller_bodies",
            &sequence!(self, caller_body_count, caller_body),
        )?;
        value.serialize_field(
            "protocol_associations",
            &sequence!(self, protocol_association_count, protocol_association),
        )?;
        value.serialize_field("types", &sequence!(self, type_count, type_id))?;
        value.serialize_field("path", &header.path)?;
        value.serialize_field("signature_hash", &header.signature_hash)?;
        value.serialize_field("bodies", &sequence!(self, body_count, body))?;
        value.serialize_field("entry", &header.entry)?;
        value.serialize_field("contracts", &sequence!(self, contract_count, contract))?;
        value.end()
    }
}
impl Serialize for TirBodyView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Self::Live(value) = self {
            return value.serialize(serializer);
        }
        let header = self
            .header()
            .ok_or_else(|| S::Error::custom("unavailable checked TIR body header"))?;
        let mut value = serializer.serialize_struct("Body", 16)?;
        value.serialize_field("arena_scope_grant", &header.arena_scope_grant)?;
        value.serialize_field("parameter_scope", &header.parameter_scope)?;
        value.serialize_field("locals", &sequence!(self, local_count, local))?;
        value.serialize_field("places", &sequence!(self, place_count, place))?;
        value.serialize_field("scopes", &sequence!(self, scope_count, scope))?;
        value.serialize_field("blocks", &sequence!(self, block_count, block))?;
        value.serialize_field("rvalues", &sequence!(self, rvalue_count, rvalue))?;
        value.serialize_field("cleanups", &sequence!(self, cleanup_count, cleanup))?;
        value.serialize_field(
            "cleanup_nodes",
            &sequence!(self, cleanup_node_count, cleanup_node),
        )?;
        value.serialize_field("entry", &header.entry)?;
        value.serialize_field("parameters", &sequence!(self, parameter_count, parameter))?;
        value.serialize_field("result", &header.result)?;
        value.serialize_field(
            "borrow_cursors",
            &sequence!(self, borrow_cursor_count, borrow_cursor),
        )?;
        value.serialize_field("tail_sites", &sequence!(self, tail_site_count, tail_site))?;
        value.serialize_field("matches", &sequence!(self, match_table_count, match_table))?;
        value.serialize_field(
            "scoped_parameters",
            &sequence!(self, scoped_parameter_count, scoped_parameter),
        )?;
        value.end()
    }
}
impl Serialize for TirBlockView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Self::Live(value) = self {
            return value.serialize(serializer);
        }
        let terminator = self
            .terminator()
            .ok_or_else(|| S::Error::custom("unavailable checked TIR terminator"))?;
        let span = self
            .span()
            .ok_or_else(|| S::Error::custom("unavailable checked TIR span"))?;
        let mut value = serializer.serialize_struct("Block", 3)?;
        value.serialize_field(
            "instructions",
            &sequence!(self, instruction_count, instruction),
        )?;
        value.serialize_field("terminator", &terminator)?;
        value.serialize_field("span", &span)?;
        value.end()
    }
}
impl Serialize for TirMatchView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Self::Live(value) = self {
            return value.serialize(serializer);
        }
        let scrutinee = self
            .scrutinee()
            .ok_or_else(|| S::Error::custom("unavailable checked TIR scrutinee"))?;
        let mut value = serializer.serialize_struct("MatchTable", 3)?;
        value.serialize_field("scrutinee", &scrutinee)?;
        value.serialize_field("patterns", &sequence!(self, pattern_count, pattern))?;
        value.serialize_field("arms", &sequence!(self, arm_count, arm))?;
        value.end()
    }
}

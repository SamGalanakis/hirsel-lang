//! Borrow-only cursors over the existing validated physical access program.
//!
//! Reuses Program::parse_access and the owning cursor's actual projections, including
//! virtual SoA rows and live keyed slots. It adds no graph/encoding, wire schema,
//! serialization checks, invocation, Depth policy, budget debit or source Take.
//! Box/framing/checked wrappers remain explicit steps instead of disappearing
//! as they do in value comparison. A checked wrapper never runs its check.
//!
//! These observations are not holder, placement or source-receipt authority.
//! A plan producer must separately prove original source correspondence,
//! complete owned scan/descriptor coverage and live journal/value custody.
mod view;
use super::{Node, Program, Scalar, pointer};
use crate::{alloc::payload, fault};
use std::marker::PhantomData;
use view::{Location, Projection, View};

/// Parsed existing program. Node ordinals are local to these exact words,
/// never durable identities or a source type/profile proof.
pub struct Access {
    program: Program,
}
impl Access {
    /// Parse compiler-emitted access metadata. As in the existing admitted
    /// readers, malformed program words are runtime invariants, not wire data.
    /// Successful parsing itself proves no source or physical correspondence.
    pub fn parse(words: &[u64]) -> Self {
        Self {
            program: Program::parse_access(words),
        }
    }

    /// Borrow an actual value without taking/incrementing any language holder.
    ///
    /// # Safety
    /// The exact program has independently validated physical/type coverage for
    /// this value. `bytes` contains its initialized root representation, and ALL
    /// projected child storage remains live, initialized and immutable for the
    /// entire lifetime of that root borrow. Actual owners/loans provide that
    /// custody, not copied pointer words or descriptive receipt images. This is
    /// not a decoder/validator for untrusted or undersized external bytes.
    pub unsafe fn borrow<'program, 'value>(
        &'program self,
        bytes: &'value [u8],
    ) -> Cursor<'program, 'value> {
        Cursor {
            program: &self.program,
            projection: Projection::at(self.program.root, bytes.as_ptr()),
            lifetime: PhantomData,
        }
    }
}

/// A typed borrowed projection; Copy/Clone acquires no metadata/storage credit.
/// The program and the original live value both outlive every derived cursor.
#[derive(Clone, Copy)]
pub struct Cursor<'program, 'value> {
    program: &'program Program,
    projection: Projection,
    lifetime: PhantomData<&'value [u8]>,
}

pub enum Value<'program, 'value> {
    Scalar {
        /// Existing access-program scalar code; raw initialized bytes preserve
        /// representation (including floating bits), without normalization.
        code: u64,
        bytes: &'value [u8],
    },
    Bytes(&'value [u8]),
    /// Run-context Path identity; this reader performs no adapter call.
    Path(u64),
    Children {
        ordinal: Option<u32>,
        count: Option<u64>,
        /// Keyed children remain in actual storage order. No canonical Set
        /// ordering or source-to-physical child association is inferred.
        set: bool,
        children: Vec<Cursor<'program, 'value>>,
    },
}

impl<'program, 'value> Cursor<'program, 'value> {
    pub fn node(&self) -> usize {
        self.projection.node
    }
    /// A virtual SoA row deliberately has no contiguous address. Its field
    /// children use their actual typed column addresses on the next read.
    pub fn address(&self) -> Option<*const u8> {
        match self.projection.location {
            Location::Address(at) => Some(at),
            Location::Row { .. } => None,
        }
    }
    fn children(
        &self,
        ordinal: Option<u32>,
        count: Option<u64>,
        set: bool,
        projections: Vec<Projection>,
    ) -> Value<'program, 'value> {
        Value::Children {
            ordinal,
            count,
            set,
            children: projections
                .into_iter()
                .map(|projection| Self {
                    projection,
                    ..*self
                })
                .collect(),
        }
    }

    /// Observe one node. Recursion is controlled by the caller's explicit
    /// cursor worklist; there is no host-stack recursion or language admission.
    pub fn read(&self) -> Value<'program, 'value> {
        // SAFETY: borrow established immutable live storage for every typed
        // projection. No read below changes a header, source value or holder.
        unsafe {
            let wrapper = match &self.program.nodes[self.projection.node] {
                Node::Checked { value, .. } => Some(Projection {
                    node: *value,
                    location: self.projection.location,
                }),
                Node::Framed(value) => Some(Projection::at(
                    *value,
                    self.address()
                        .unwrap_or_else(|| fault("framing without an address"))
                        .add(8),
                )),
                Node::Box { payload: value, .. } => Some(Projection::at(
                    *value,
                    payload(pointer(
                        self.address()
                            .unwrap_or_else(|| fault("box without an address")),
                    )),
                )),
                _ => None,
            };
            if let Some(child) = wrapper {
                return self.children(None, None, false, vec![child]);
            }
            match view::view(self.program, self.projection) {
                View::Scalar { kind, code, at } => {
                    let width = match kind {
                        Scalar::Bool => 1,
                        Scalar::Integer(width) => width,
                        Scalar::F32 | Scalar::Char => 4,
                        Scalar::F64 => 8,
                    };
                    Value::Scalar {
                        code,
                        bytes: std::slice::from_raw_parts(at, width),
                    }
                }
                View::Bytes(bytes) => Value::Bytes(bytes),
                View::Path(id) => Value::Path(id),
                View::Children {
                    ordinal,
                    count,
                    set,
                    children,
                } => self.children(ordinal, count, set, children),
            }
        }
    }
}

#[cfg(test)]
mod tests;

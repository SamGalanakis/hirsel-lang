//! Source-bound semantic payload recipes. Physical layout is deliberately absent:
//! Flat encoding and allocation elision must preserve the original operands.
//! Candidate tables are not evidence until C recomputes them from source input.
use crate::ir::SourceAnchor;
use crate::operation::NativeType;
use hirsel_codec::{snapshot_enum, snapshot_struct};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PayloadLiteral {
    Unit,
    Bool(bool),
    Integer(u128),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PayloadProducer {
    Parameter(u32),
    Capture(u32),
    Literal(PayloadLiteral),
    Construct(SourceAnchor),
    Closure(SourceAnchor),
    Call(SourceAnchor),
}

/// The original TIR place and its independently reconstructed reaching producer.
/// `capture_slot` names a closed environment capture, never a callable signature.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PayloadOperand {
    pub place: u32,
    pub ty: NativeType,
    pub producer: PayloadProducer,
    pub capture_slot: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PayloadDestination {
    Heap,
    Static,
    Arena(u32),
    Caller(Vec<u32>),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PayloadOperation {
    Allocate { destination: PayloadDestination },
    Replace { previous: u32 },
    Transfer { previous: u32 },
    Boundary,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PayloadRecipe {
    pub source: SourceAnchor,
    pub operation: PayloadOperation,
    pub inputs: Vec<PayloadOperand>,
}

/// Independent source expectation: declaration-derived type and exact recipe.
/// This is supplied separately from candidate LIR, including when no physical
/// construction survives. A producer may not populate the expectation from LIR.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SourcePayload {
    pub root_type: NativeType,
    pub recipe: PayloadRecipe,
}

snapshot_enum!(PayloadLiteral { Unit, Bool(value), Integer(value), F32(bits), F64(bits), String(value), Bytes(value) });
snapshot_enum!(PayloadProducer { Parameter(ordinal), Capture(slot), Literal(value), Construct(source), Closure(source), Call(source) });
snapshot_struct!(PayloadOperand {
    place,
    ty,
    producer,
    capture_slot
});
snapshot_enum!(PayloadDestination { Heap, Static, Arena(place), Caller(parameters) });
snapshot_enum!(PayloadOperation { Allocate { destination }, Replace { previous }, Transfer { previous }, Boundary });
snapshot_struct!(PayloadRecipe {
    source,
    operation,
    inputs
});
snapshot_struct!(SourcePayload { root_type, recipe });

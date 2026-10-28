//! Source-bound semantic payload recipes. Physical layout is deliberately absent:
//! Flat encoding and allocation elision must preserve the original operands.
//! Candidate tables are not evidence until C recomputes them from source input.
use crate::ir::SourceAnchor;
use crate::operation::NativeType;
use hirsel_codec::{snapshot_enum, snapshot_struct};

mod environment;
pub use environment::{SourceClosure, SourceClosureContext, SourceEnvironments};
mod profile;
mod receipt;
pub use profile::{
    ClosedCapture, ClosedEnvironment, ClosedStorageCase, ClosedStorageFunction,
    ClosedStorageProfile, ClosedStorageType, SourceStorageSelection,
};
pub use receipt::{
    ReceiptCapture, ReceiptContract, ReceiptGrant, ReceiptMode, ReceiptParameter, ReceiptPort,
    ReceiptProfile, ReceiptResult, ReceiptSource, ReceiptType,
};

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
    Projected {
        root: Box<PayloadProducer>,
        path: Vec<PayloadProjection>,
    },
}

/// Original logical path. Index identity follows its own reaching producer.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PayloadProjection {
    Field(u32),
    Index(Box<PayloadProducer>),
    VariantField { variant: u32, field: u32 },
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
    Arena(PayloadOperand),
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
snapshot_enum!(PayloadProducer { Parameter(ordinal), Capture(slot), Literal(value), Construct(source), Closure(source), Call(source), Projected { root, path } });
snapshot_enum!(PayloadProjection { Field(ordinal), Index(producer), VariantField { variant, field } });
snapshot_struct!(PayloadOperand {
    place,
    ty,
    producer,
    capture_slot
});
snapshot_enum!(PayloadDestination { Heap, Static, Arena(operand), Caller(parameters) });
snapshot_enum!(PayloadOperation { Allocate { destination }, Replace { previous }, Transfer { previous }, Boundary });
snapshot_struct!(PayloadRecipe {
    source,
    operation,
    inputs
});
snapshot_struct!(SourcePayload { root_type, recipe });

/// Independently expected semantic events, including allocations elided by LIR.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SourceEvent {
    pub anchor: SourceAnchor,
    pub kind: SourceEventKind,
    pub root_type: NativeType,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SourceEventKind {
    Seal,
    BorrowRoot,
    Allocate,
    Replace,
}

/// Cache observation identity, not a V/C proof. An actual declaration provider
/// returns this only after deriving its expectations from original source.
/// `Some` with empty expectations differs from an unavailable observation.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SealSourceWitness {
    /// Per-definition V/source-query identity including its reached private
    /// dependencies, validated by the actual query owner at current lookup.
    /// Never the whole-program adapt::input_identity digest.
    pub source: hirsel_syntax::ContentHash,
    pub definition: hirsel_syntax::DefPath,
    pub implementation_revision: hirsel_syntax::ContentHash,
    pub instance: hirsel_syntax::ContentHash,
    pub inventory_rule: u32,
    pub expectations: hirsel_syntax::ContentHash,
}

snapshot_struct!(SourceEvent {
    anchor,
    kind,
    root_type
});
snapshot_enum!(SourceEventKind {
    Seal,
    BorrowRoot,
    Allocate,
    Replace
});
snapshot_struct!(SealSourceWitness {
    source,
    definition,
    implementation_revision,
    instance,
    inventory_rule,
    expectations
});

impl SealSourceWitness {
    /// Canonical identity of independently reconstructed expectations. This
    /// constructor creates data only; it conveys no validated-source authority.
    pub fn from_expectations(
        source: hirsel_syntax::ContentHash,
        definition: hirsel_syntax::DefPath,
        implementation_revision: hirsel_syntax::ContentHash,
        instance: hirsel_syntax::ContentHash,
        events: &[SourceEvent],
        payloads: &[SourcePayload],
    ) -> Self {
        let events = hirsel_codec::to_text(&events.to_vec());
        let payloads = hirsel_codec::to_text(&payloads.to_vec());
        Self {
            source,
            definition,
            implementation_revision,
            instance,
            inventory_rule: 1,
            expectations: hirsel_syntax::ContentHash::of_parts(
                "hirsel.seal-source-expectations.retained-occurrences.v1",
                [events.as_bytes(), payloads.as_bytes()],
            ),
        }
    }
}

impl SealSourceWitness {
    pub fn with_environments(mut self, environments: Option<&SourceEnvironments>) -> Self {
        if let Some(environments) = environments {
            let bytes = hirsel_codec::to_text(environments);
            self.expectations = hirsel_syntax::ContentHash::of_parts(
                "hirsel.seal-source.original-environments.v1",
                [self.expectations.0.as_slice(), bytes.as_bytes()],
            );
            self.inventory_rule = 2;
        }
        self
    }
}

/// Original-source storage inventory associated through the independently
/// selected instance body. These are input data, never V/C authority supplied
/// by a candidate unit or its source-anchor table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSeal {
    pub reference: crate::invocations::ReferenceBody,
    pub witness: SealSourceWitness,
    pub events: Vec<SourceEvent>,
    pub payloads: Vec<SourcePayload>,
    pub environments: Option<SourceEnvironments>,
}
snapshot_struct!(SourceSeal {
    reference,
    witness,
    events,
    payloads
} trailing_optional environments);
impl SourceSeal {
    pub fn check(&self) -> Result<(), &'static str> {
        if self.reference.definition != self.witness.definition
            || self.reference.instance != self.witness.instance
            || self
                .events
                .iter()
                .any(|event| event.anchor.body != self.reference.body)
            || self
                .payloads
                .iter()
                .any(|payload| payload.recipe.source.body != self.reference.body)
        {
            return Err("sealed source inventory names another reference body");
        }
        if self
            .environments
            .iter()
            .flat_map(|sources| &sources.constructions)
            .any(|environment| {
                environment.check_structure().is_err()
                    || environment.original.definition != self.reference.definition
                    || environment.payload.recipe.source.body != self.reference.body
            })
        {
            return Err("sealed source environments name another original body");
        }
        if self
            .environments
            .as_ref()
            .and_then(|sources| sources.entry.as_ref())
            .is_some_and(|entry| {
                entry.check_structure().is_err()
                    || entry.original.definition != self.reference.definition
                    || entry.original.body != self.reference.body
                    || entry.instance != self.reference.instance
            })
        {
            return Err("sealed source capture entry names another original body");
        }
        if self.witness
            != SealSourceWitness::from_expectations(
                self.witness.source,
                self.reference.definition.clone(),
                self.witness.implementation_revision,
                self.reference.instance,
                &self.events,
                &self.payloads,
            )
            .with_environments(self.environments.as_ref())
        {
            return Err("sealed source inventory differs from its source observation");
        }
        Ok(())
    }
}

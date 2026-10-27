//! Original-source receipt requirements. No candidate-local layout, value or
//! instance IDs occur here. A content digest identifies a contract; independent
//! source observations, not possession of this record, establish its authority.
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_syntax::ContentHash;
mod source;
pub use source::{ReceiptPort, ReceiptProfile, ReceiptSource};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ReceiptMode {
    Borrow,
    Take,
    /// Exclusive address of a handle slot, updated on both entered call edges.
    MutSlot,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptType {
    pub source_type: ContentHash,
    pub profile: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptParameter {
    pub source: u32,
    pub ty: ReceiptType,
    pub mode: ReceiptMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptResult {
    pub ty: ReceiptType,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptCapture {
    pub ordinal: u32,
    pub ty: ReceiptType,
    pub mode: ReceiptMode,
}

/// Original source selectors, independent of candidate ABI ordinals. The
/// declaration order is the canonical order of inventory effect selectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ReceiptGrant {
    Parameter(u32),
    Capture(u32),
    CallerDestination,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptContract {
    pub schema: u32,
    pub parameters: Vec<ReceiptParameter>,
    pub result: Option<ReceiptResult>,
    pub captures: Vec<ReceiptCapture>,
    pub inventory: Vec<ReceiptGrant>,
    /// Equality-backdated semantic demand output. Never a body, V receipt,
    /// environment observation, revision or derived consuming instance key.
    pub source_demand: ContentHash,
}

impl ReceiptType {
    fn check_structure(&self) -> Result<(), &'static str> {
        if self.source_type == ContentHash([0; 32]) || self.profile == ContentHash([0; 32]) {
            return Err("receipt type lacks original type or closed profile identity");
        }
        Ok(())
    }
}

impl ReceiptContract {
    pub const SCHEMA: u32 = 1;

    /// Build canonical semantic output. This does not prove selectors, modes,
    /// root types or profiles against a source declaration.
    pub fn new(
        parameters: Vec<ReceiptParameter>,
        result: Option<ReceiptResult>,
        captures: Vec<ReceiptCapture>,
        inventory: Vec<ReceiptGrant>,
    ) -> Result<Self, &'static str> {
        let mut contract = Self {
            schema: Self::SCHEMA,
            parameters,
            result,
            captures,
            inventory,
            source_demand: ContentHash([0; 32]),
        };
        contract.check_selectors()?;
        contract.source_demand = contract.demand_content();
        Ok(contract)
    }

    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
            && self.result.is_none()
            && self.captures.is_empty()
            && self.inventory.is_empty()
    }

    fn check_selectors(&self) -> Result<(), &'static str> {
        if self.schema != Self::SCHEMA {
            return Err("unsupported receipt contract schema");
        }
        if self
            .parameters
            .windows(2)
            .any(|p| p[0].source >= p[1].source)
            || self
                .captures
                .windows(2)
                .any(|p| p[0].ordinal >= p[1].ordinal)
            || self.inventory.windows(2).any(|p| p[0] >= p[1])
        {
            return Err("receipt selectors are not unique and canonical");
        }
        for ty in self
            .parameters
            .iter()
            .map(|p| &p.ty)
            .chain(self.result.iter().map(|r| &r.ty))
            .chain(self.captures.iter().map(|c| &c.ty))
        {
            ty.check_structure()?;
        }
        Ok(())
    }

    /// Hash only finite semantic output, excluding this digest itself. Original
    /// source observations belong to the action/C witness, not this address.
    fn demand_content(&self) -> ContentHash {
        let mut bytes = hirsel_codec::Writer::default();
        use hirsel_codec::Snapshot;
        bytes.open("ReceiptDemand");
        self.schema.write(&mut bytes);
        self.parameters.write(&mut bytes);
        self.result.write(&mut bytes);
        self.captures.write(&mut bytes);
        self.inventory.write(&mut bytes);
        bytes.close();
        let (bytes, ()) = bytes.finish();
        ContentHash::of_parts("hirsel.receipt-demand.v1", [bytes.as_bytes()])
    }

    pub fn check_structure(&self) -> Result<(), &'static str> {
        self.check_selectors()?;
        if self.source_demand != self.demand_content() {
            return Err("receipt semantic demand digest disagrees with its selectors");
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, &'static str> {
        self.check_structure()?;
        Ok(hirsel_codec::to_text(self).into_bytes())
    }

    pub fn content(&self) -> Result<ContentHash, &'static str> {
        Ok(ContentHash::of_parts(
            "hirsel.receipt-contract.v1",
            [self.canonical_bytes()?.as_slice()],
        ))
    }
}

snapshot_enum!(ReceiptMode {
    Borrow,
    Take,
    MutSlot
});
snapshot_struct!(ReceiptType {
    source_type,
    profile
});
snapshot_struct!(ReceiptParameter { source, ty, mode });
snapshot_struct!(ReceiptResult { ty });
snapshot_struct!(ReceiptCapture { ordinal, ty, mode });
snapshot_enum!(ReceiptGrant { Parameter(ordinal), Capture(ordinal), CallerDestination });
snapshot_struct!(ReceiptContract {
    schema,
    parameters,
    result,
    captures,
    inventory,
    source_demand
});

#[cfg(test)]
mod tests;

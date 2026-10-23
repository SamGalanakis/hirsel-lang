//! Source-only closed storage products. These contain no candidate-local IDs,
//! layouts, ABIs, or consuming instance keys. Their hashes identify data, never
//! prove that a callable environment or original producer was independently
//! observed. C must reconstruct both products from current source queries.
use super::PayloadOperand;
use crate::invocations::ReferenceBody;
use crate::ir::SourceAnchor;
use crate::operation::NativeType;
use hirsel_codec::snapshot_struct;
use hirsel_core_contract::{OperationId, Passing};
use hirsel_syntax::ContentHash;

/// Full original type identity supplements NativeType's erased physical axes.
/// In particular NativeType::Function only retains multiplicity, so it cannot
/// key source function rows or distinguish invocation signatures by itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedStorageType {
    pub ty: NativeType,
    /// Hash of canonical original rooted type bytes, before native rewrites.
    pub source_type: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedCapture {
    pub ty: ClosedStorageType,
    pub passing: Passing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedEnvironment {
    pub captures: Vec<ClosedCapture>,
    pub sizes: Vec<u64>,
    /// Exact live capture ordinals, in declaration order.
    pub live: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedStorageCase {
    /// Original source specialization selected before deriving the consuming
    /// closed-profile key. Never a final native instance/ABI derived from this
    /// same profile, which would make its content identity cyclic.
    pub body: ReferenceBody,
    /// Actual CT5 environment recipe observation, not a signature's captures.
    pub recipe: ContentHash,
    /// None is a code-only value; Some(empty) is an actual empty environment.
    pub environment: Option<ClosedEnvironment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedStorageFunction {
    pub ty: ClosedStorageType,
    pub cases: Vec<ClosedStorageCase>,
}

/// Canonical profile output for base.key.derived(
/// "hirsel.closed-storage-profile.v1", &[canonical_bytes]). Source producer
/// selection is deliberately a separate product and cannot alter this content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedStorageProfile {
    pub schema: u32,
    pub root: ClosedStorageType,
    pub functions: Vec<ClosedStorageFunction>,
}

impl ClosedStorageProfile {
    pub const SCHEMA: u32 = 1;

    /// Normalize set-like function/case alternatives only. Capture/live-slot
    /// order is semantic and is never sorted to repair an invalid contract.
    pub fn canonical(mut self) -> Result<Self, &'static str> {
        self.functions.sort_by_key(|row| row.ty.source_type);
        for row in &mut self.functions {
            row.cases
                .sort_by_key(|case| hirsel_codec::to_text(&case.body));
        }
        self.check_structure()?;
        Ok(self)
    }

    /// Structural consistency of untrusted data only. Completeness, source
    /// reachability, resource freedom, actual recipe and private-dependency
    /// observations still require the independent current source provider.
    pub fn check_structure(&self) -> Result<(), &'static str> {
        let nonzero = |key| key != ContentHash([0; 32]);
        if self.schema != Self::SCHEMA || !nonzero(self.root.source_type) {
            return Err("unsupported or unbound closed storage profile");
        }
        let mut previous = None;
        for row in &self.functions {
            if !matches!(row.ty.ty, NativeType::Function(_))
                || !nonzero(row.ty.source_type)
                || previous.is_some_and(|key| key >= row.ty.source_type)
                || row.cases.is_empty()
            {
                return Err("closed function alternatives are not canonical and complete");
            }
            previous = Some(row.ty.source_type);
            let mut previous = None;
            for case in &row.cases {
                let key = hirsel_codec::to_text(&case.body);
                if !nonzero(case.body.instance)
                    || !nonzero(case.recipe)
                    || previous.as_ref().is_some_and(|before| before >= &key)
                {
                    return Err("closed callable case lacks a unique source recipe");
                }
                previous = Some(key);
                if let Some(environment) = &case.environment {
                    if environment.captures.iter().any(|capture| {
                        capture.passing != Passing::Take || !nonzero(capture.ty.source_type)
                    }) || environment
                        .live
                        .iter()
                        .any(|&slot| slot as usize >= environment.captures.len())
                        || environment.live.windows(2).any(|pair| pair[0] >= pair[1])
                    {
                        return Err("closed environment lacks exact owned capture ordinals");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, &'static str> {
        self.check_structure()?;
        Ok(hirsel_codec::to_text(self).into_bytes())
    }

    pub fn content(&self) -> Result<ContentHash, &'static str> {
        Ok(ContentHash::of_parts(
            "hirsel.closed-storage-profile.v1",
            [self.canonical_bytes()?.as_slice()],
        ))
    }
}

/// Independent original call-site selection. It binds the chosen profile to
/// the reviewed operation and actual original grant/root producers. This is
/// source query output, not authority conveyed by a copied candidate record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceStorageSelection {
    pub body: ReferenceBody,
    pub call: SourceAnchor,
    pub operation: OperationId,
    pub grant: PayloadOperand,
    pub root: PayloadOperand,
    pub profile: ContentHash,
}
impl SourceStorageSelection {
    pub fn check_structure(&self, profile: &ClosedStorageProfile) -> Result<(), &'static str> {
        if self.call.body != self.body.body
            || !self.operation.definition.is_core("arena.seal")
            || self.operation.declaration_digest == ContentHash([0; 32])
            || self.body.instance == ContentHash([0; 32])
            || self.root.ty != profile.root.ty
            || self.profile != profile.content()?
        {
            return Err("original source selection disagrees with closed storage profile");
        }
        // Every original source capture names its actual slot; no signature
        // or erased function type may supply a substitute capture identity.
        for operand in [&self.grant, &self.root] {
            if let super::PayloadProducer::Capture(slot) = operand.producer {
                if operand.capture_slot != Some(slot) {
                    return Err("closed source selection lacks its actual capture slot");
                }
            } else if operand.capture_slot.is_some() {
                return Err("noncapture source selection claims a capture slot");
            }
        }
        Ok(())
    }
}

snapshot_struct!(ClosedStorageType { ty, source_type });
snapshot_struct!(ClosedCapture { ty, passing });
snapshot_struct!(ClosedEnvironment {
    captures,
    sizes,
    live
});
snapshot_struct!(ClosedStorageCase {
    body,
    recipe,
    environment
});
snapshot_struct!(ClosedStorageFunction { ty, cases });
snapshot_struct!(ClosedStorageProfile {
    schema,
    root,
    functions
});
snapshot_struct!(SourceStorageSelection {
    body,
    call,
    operation,
    grant,
    root,
    profile
});

#[cfg(test)]
mod tests;

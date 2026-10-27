//! Original boundary declaration data retained independently of native IR.
//! A well-formed record is not a current source observation or a custody proof.
use super::*;
use crate::invocations::ReferenceBody;
use crate::operation::NativeType;
use crate::sealed_payload_source::ClosedStorageProfile;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ReceiptPort {
    Parameter(u32),
    Result,
    Capture(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptProfile {
    pub port: ReceiptPort,
    pub profile: ClosedStorageProfile,
}

/// The selected receipt-aware request and its original source body are
/// distinct identities. Source/profile factories derive both before native
/// restoration, never from the candidate's key, ABI or capture numbering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptSource {
    pub original: ReferenceBody,
    pub instance: ContentHash,
    pub source: ContentHash,
    pub implementation_revision: ContentHash,
    pub contract: ReceiptContract,
    pub profiles: Vec<ReceiptProfile>,
}

impl ReceiptSource {
    pub fn profile(&self, port: ReceiptPort) -> Option<&ClosedStorageProfile> {
        self.profiles
            .binary_search_by_key(&port, |row| row.port)
            .ok()
            .map(|index| &self.profiles[index].profile)
    }

    /// Check raw data only. The provider must still independently reconstruct
    /// complete original ports, demand, targets and profiles from current V.
    pub fn check_structure(&self) -> Result<(), &'static str> {
        self.contract.check_structure()?;
        if [
            self.original.instance,
            self.instance,
            self.source,
            self.implementation_revision,
        ]
        .contains(&ContentHash([0; 32]))
        {
            return Err("receipt source lacks its original observation or selected request");
        }
        if self
            .profiles
            .windows(2)
            .any(|rows| rows[0].port >= rows[1].port)
        {
            return Err("receipt source profiles are not canonical original ports");
        }
        for row in &self.profiles {
            row.profile.check_structure()?;
        }
        for (port, ty) in self
            .contract
            .parameters
            .iter()
            .map(|p| (ReceiptPort::Parameter(p.source), &p.ty))
            .chain(
                self.contract
                    .result
                    .iter()
                    .map(|r| (ReceiptPort::Result, &r.ty)),
            )
            .chain(
                self.contract
                    .captures
                    .iter()
                    .map(|c| (ReceiptPort::Capture(c.ordinal), &c.ty)),
            )
        {
            let profile = self
                .profile(port)
                .ok_or("receipt source lacks a demanded original port")?;
            if profile.root.source_type != ty.source_type || profile.content()? != ty.profile {
                return Err("receipt source type or complete profile differs from its contract");
            }
        }
        for grant in &self.contract.inventory {
            let port = match *grant {
                ReceiptGrant::Parameter(source) => ReceiptPort::Parameter(source),
                ReceiptGrant::Capture(ordinal) => ReceiptPort::Capture(ordinal),
                ReceiptGrant::CallerDestination => continue,
            };
            if !matches!(self.profile(port).map(|p| &p.root.ty),
                Some(NativeType::Named(path, arguments)) if path.is_core("Arena") && arguments.is_empty())
            {
                return Err("receipt source inventory selector is not an original Arena");
            }
        }
        Ok(())
    }
}

snapshot_enum!(ReceiptPort { Parameter(ordinal), Result, Capture(ordinal) });
snapshot_struct!(ReceiptProfile { port, profile });
snapshot_struct!(ReceiptSource {
    original,
    instance,
    source,
    implementation_revision,
    contract,
    profiles
});

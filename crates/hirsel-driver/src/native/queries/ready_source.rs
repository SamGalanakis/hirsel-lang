//! Compact source discovery metadata for a demanded Ready miss.
use hirsel_rc::instance::{InstanceId, Instances};
/// Compact callable facts captured with an instance's source discovery.
/// These are ordinary descriptor data, bound to the accepted instance's
/// discovery output and current source observations by the query owner.
/// Ready does not need the source body or an Instances registry to use them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadySourceFacts {
    origin: hirsel_layout::instance::InstanceKey,
    parameters: u32,
    noalloc: bool,
    unique: Vec<u32>,
}
impl ReadySourceFacts {
    pub(super) fn retained_metadata_bytes(&self) -> u64 {
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.path(self.origin.definition());
        weight.add(self.origin.retained_encoding_bytes());
        weight.flat_vec(&self.unique);
        weight.bytes()
    }
    pub fn for_instance(instances: &Instances, id: InstanceId) -> Self {
        let proofs = hirsel_rc::insert::Proofs::for_instance(instances, id);
        Self {
            origin: instances.view(id).key.clone(),
            parameters: instances.view(id).parameters.len() as u32,
            noalloc: proofs.noalloc,
            unique: proofs.unique,
        }
    }

    pub(super) fn origin(&self) -> &hirsel_layout::instance::InstanceKey {
        &self.origin
    }

    /// Portable metadata only; no TypeId, LayoutId or retained source body.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&(
            1u32,
            self.origin.definition(),
            self.origin.body(),
            self.origin.encoding(),
            self.parameters,
            self.noalloc,
            &self.unique,
        ))
        .map_err(|e| e.to_string())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let (schema, definition, body, encoding, parameters, noalloc, unique): (
            u32,
            hirsel_syntax::DefPath,
            u32,
            Vec<u8>,
            u32,
            bool,
            Vec<u32>,
        ) = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if schema != 1
            || unique.windows(2).any(|w| w[0] >= w[1])
            || unique.iter().any(|&p| p >= parameters)
        {
            return Err("noncanonical Ready source facts".into());
        }
        let origin =
            hirsel_layout::instance::InstanceKey::from_encoding(&definition, body, encoding)
                .map_err(|e| format!("Ready source origin: {e:?}"))?;
        Ok(Self {
            origin,
            parameters,
            noalloc,
            unique,
        })
    }
    pub(super) fn local(&self, regions: bool) -> crate::native::stages::LocalRcFacts {
        crate::native::stages::LocalRcFacts {
            guard: None,
            placement: hirsel_rc::placement::Options {
                regions,
                noalloc: self.noalloc,
            },
            proofs: hirsel_rc::insert::Proofs {
                noalloc: self.noalloc,
                unique: self.unique.clone(),
            },
        }
    }
}

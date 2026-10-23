//! Owned function fragments and linked outputs in the ordinary artifact store.
//!
//! Zig bff490a23889750de7096d9057c0a432efc929a8, src/link/Elf.zig:1664:
//! a function owns its object update. rustc 1159e78c4747b02ef996e55082b704c09b970588,
//! compiler/rustc_query_system/src/dep_graph/graph.rs:736,832: validate read
//! outputs before loading a body, and retain equal recomputation's output.
//! Changed manifests still invoke the platform linker; no image patching.
use super::queries::InstanceAddress;
use hirsel_artifact_store::{Address, Reference};
use hirsel_codec::snapshot_struct;
use hirsel_engine::runtime::ProductRef;
use hirsel_syntax::ContentHash;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CodegenKey {
    pub owner: InstanceAddress,
    pub backend: String,
    pub backend_version: ContentHash,
    pub target_features: ContentHash,
    pub profile_options: ContentHash,
    pub runtime_abi: ContentHash,
}

#[derive(Clone, Debug)]
pub struct FragmentProduct {
    pub product: ProductRef,
    pub object: Reference,
    pub summary: Reference,
}

#[derive(Clone, Debug)]
pub struct LinkKey {
    pub roots: Vec<InstanceAddress>,
    pub target: ContentHash,
    /// Exact linker implementation, options, ABI and implicit toolchain inputs.
    pub linker_options: ContentHash,
}

#[derive(Clone, Debug)]
pub struct LinkInputs {
    pub fragments: Vec<FragmentProduct>,
    pub static_manifest: Reference,
    /// Archive and foreign order is semantically significant, unlike fragments.
    pub runtime_archives: Vec<Reference>,
    pub foreign_inputs: Vec<Reference>,
    pub launch_manifest: Reference,
}

#[derive(Clone, Debug)]
pub struct LinkedProduct {
    pub product: ProductRef,
    /// A bounded chunk manifest, not an unchecked executable pathname.
    pub executable: Reference,
}

pub trait ObjectProducts {
    fn codegen(&mut self, key: &CodegenKey) -> Result<Arc<FragmentProduct>, String>;
    fn link(&mut self, key: &LinkKey, inputs: &LinkInputs)
    -> Result<Arc<LinkedProduct>, String>;
}

impl CodegenKey {
    pub fn address(&self) -> Address {
        Address(hirsel_codec::digest(
            "hirsel.codegen.address.v1",
            "Codegen",
            &CodegenAddress {
                compiler: crate::COMPILER_IDENTITY.to_vec(),
                owner: instance_bytes(&self.owner),
                backend: self.backend.clone(),
                backend_version: self.backend_version,
                target_features: self.target_features,
                profile_options: self.profile_options,
                runtime_abi: self.runtime_abi,
            },
        ))
    }
}

impl LinkKey {
    pub fn address(&self) -> Address {
        let mut roots: Vec<_> = self.roots.iter().map(instance_bytes).collect();
        roots.sort();
        roots.dedup();
        Address(hirsel_codec::digest(
            "hirsel.link.address.v1",
            "Link",
            &LinkAddress {
                compiler: crate::COMPILER_IDENTITY.to_vec(),
                roots,
                target: self.target,
                linker_options: self.linker_options,
            },
        ))
    }
}

fn instance_bytes(key: &InstanceAddress) -> Vec<u8> {
    hirsel_codec::to_text(&InstanceEncoding {
        instance: key.instance.encoding().to_vec(),
        target_abi: key.target_abi,
        policy: key.policy,
    })
    .into_bytes()
}

struct InstanceEncoding {
    instance: Vec<u8>,
    target_abi: ContentHash,
    policy: ContentHash,
}
struct CodegenAddress {
    compiler: Vec<u8>,
    owner: Vec<u8>,
    backend: String,
    backend_version: ContentHash,
    target_features: ContentHash,
    profile_options: ContentHash,
    runtime_abi: ContentHash,
}
struct LinkAddress {
    compiler: Vec<u8>,
    roots: Vec<Vec<u8>>,
    target: ContentHash,
    linker_options: ContentHash,
}
snapshot_struct!(InstanceEncoding { instance, target_abi, policy });
snapshot_struct!(CodegenAddress {
    compiler, owner, backend, backend_version, target_features, profile_options, runtime_abi
});
snapshot_struct!(LinkAddress { compiler, roots, target, linker_options });

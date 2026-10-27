//! Owned function fragments and linked outputs in the ordinary artifact store.
//!
//! Zig bff490a23889750de7096d9057c0a432efc929a8, src/link/Elf.zig:1664:
//! a function owns its object update. rustc 1159e78c4747b02ef996e55082b704c09b970588,
//! compiler/rustc_query_system/src/dep_graph/graph.rs:736,832: validate read
//! outputs before loading a body, and retain equal recomputation's output.
//! Changed manifests still invoke the platform linker; no image patching.
use super::queries::{InstanceAddress, NativeProducts, ReadyProduct};
use hirsel_codec::{Snapshot, snapshot_struct};
use hirsel_engine::runtime::ProductRef;
use hirsel_engine::store::{Action, Address, Domain, MAX_RECORD, Output, Reference};
use hirsel_engine::store::{ResultStore, VerifyStore};
use hirsel_lir::stage::ObjectArtifact;
use hirsel_syntax::ContentHash;
use std::path::Path;
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
    pub roots: Arc<[InstanceAddress]>,
    pub target: ContentHash,
    /// Exact linker implementation, options, ABI and implicit toolchain inputs.
    pub linker_options: ContentHash,
}

#[derive(Clone, Debug)]
pub struct LinkInputs {
    pub execution: Vec<Reference>,
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
    fn codegen_whole(
        &mut self,
        _key: &whole::WholeCodegenKey,
    ) -> Result<Arc<FragmentProduct>, String> {
        Err("whole-module Codegen is not registered".into())
    }
    fn link(&mut self, key: &LinkKey, inputs: &LinkInputs) -> Result<Arc<LinkedProduct>, String>;
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
snapshot_struct!(InstanceEncoding {
    instance,
    target_abi,
    policy
});
snapshot_struct!(CodegenAddress {
    compiler,
    owner,
    backend,
    backend_version,
    target_features,
    profile_options,
    runtime_abi
});
snapshot_struct!(LinkAddress {
    compiler,
    roots,
    target,
    linker_options
});

const FRAGMENT: Domain = Domain {
    kind: 0x4f000001,
    schema: 1,
};
const OBJECT: Domain = Domain {
    kind: 0x4f000002,
    schema: 1,
};
const LINKED: Domain = Domain {
    kind: 0x4f000003,
    schema: 1,
};
const LEGACY_BLOB: Domain = Domain {
    kind: 0x4f000004,
    schema: 1,
};
const BLOB: Domain = Domain {
    kind: 0x4f000004,
    schema: 2,
};
// Keep a current transport read below the unchanged 16 MiB request allowance.
const BLOB_CHUNK: usize = 1 << 20;
const CHUNK: Domain = Domain {
    kind: 0x4f000005,
    schema: 1,
};

/// The sole durable provider is shared with the source/native product graph.
/// The link closure performs the existing semantic manifest checks and actual
/// platform link. It is called only after a complete changed manifest is known.
pub struct ObjectQueries<N, L> {
    pub store: Arc<dyn ResultStore>,
    pub native: N,
    pub linker: L,
    /// EveryStage/VerifyStore harnesses recompute hits and compare bytes.
    pub verify_store: VerifyStore,
}

impl<N: NativeProducts, L> ObjectQueries<N, L>
where
    L: FnMut(&LinkKey, &LinkInputs, &[ObjectArtifact]) -> Result<Vec<u8>, String>,
{
    pub fn new(store: Arc<dyn ResultStore>, native: N, linker: L) -> Self {
        Self {
            store,
            native,
            linker,
            verify_store: VerifyStore::Off,
        }
    }

    pub fn with_verification(mut self, verify_store: VerifyStore) -> Self {
        self.verify_store = verify_store;
        self
    }
}

impl<N: NativeProducts, L> ObjectProducts for ObjectQueries<N, L>
where
    L: FnMut(&LinkKey, &LinkInputs, &[ObjectArtifact]) -> Result<Vec<u8>, String>,
{
    fn codegen_whole(
        &mut self,
        key: &whole::WholeCodegenKey,
    ) -> Result<Arc<FragmentProduct>, String> {
        whole::codegen(self, key).map(Arc::new)
    }

    fn codegen(&mut self, key: &CodegenKey) -> Result<Arc<FragmentProduct>, String> {
        let backend = match key.backend.as_str() {
            "llvm" => super::Backend::Llvm,
            "cranelift" => super::Backend::Cranelift,
            _ => return Err("unregistered native backend".into()),
        };
        // The producer checks its old reads and native authority before returning
        // a descriptor. No Value or Ready decoding is needed to decide this hit.
        let selected = self.native.codegen_units(&key.owner, &key.backend)?;
        let ready = selected.first().ok_or("Codegen omitted its owner")?;
        let mut optimizer = selected[1..]
            .iter()
            .map(|ready| ready.product)
            .collect::<Vec<_>>();
        optimizer.extend(self.native.execution_read());
        codegen_checked(
            self.store.as_ref(),
            key,
            ready,
            &optimizer,
            self.verify_store,
            || {
                self.native.with_ready(&selected, |units, inputs| {
                    if units.len() != selected.len() + ready.optimizer_reads.len() {
                        return Err("Ready materialization omitted optimizer bodies".into());
                    }
                    let owner = units[0].unit();
                    if owner.header.instance != key.owner.instance.digest()
                        || owner.header.definition != *key.owner.instance.definition()
                    {
                        return Err("Ready materialization returned another owner".into());
                    }
                    if (inputs.opt == hirsel_lir::emission::OptLevel::None
                        || backend == super::Backend::Cranelift)
                        && units.len() != 1
                    {
                        return Err("non-inlining emission received optimizer bodies".into());
                    }
                    let names: Vec<_> = units[1..]
                        .iter()
                        .map(|unit| {
                            hirsel_lir::emission::symbol_name(
                                &unit.unit().header.definition,
                                Some(&unit.unit().header.instance),
                            )
                        })
                        .collect();
                    if names.windows(2).any(|pair| pair[0] >= pair[1]) {
                        return Err("optimizer bodies are not sorted unique symbols".into());
                    }
                    let copies = units[1..]
                        .iter()
                        .map(|u| u.unit().header.instance)
                        .collect();
                    let inputs = hirsel_lir::emission::EmitInputs {
                        available_externally: &copies,
                        ..inputs.clone()
                    };
                    // Source paths/line-table binding are Link inputs; the producer
                    // binds the interval-collection option to emission_context.
                    let profile = hirsel_lir::debug::Profile {
                        debug: ready.debug,
                        sources: None,
                    };
                    let _timing =
                        hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Codegen);
                    match backend {
                        super::Backend::Llvm => {
                            hirsel_llvm::emit_object_profile(units, &inputs, profile)
                                .map_err(|e| e.to_string())
                        }
                        super::Backend::Cranelift => {
                            hirsel_cranelift::emit_object_profile(units, &inputs, profile)
                                .map_err(|e| e.to_string())
                        }
                    }
                })
            },
        )
        .map(Arc::new)
    }

    fn link(&mut self, key: &LinkKey, inputs: &LinkInputs) -> Result<Arc<LinkedProduct>, String> {
        link_checked(
            self.store.as_ref(),
            key,
            inputs,
            self.verify_store,
            &mut self.linker,
        )
        .map(Arc::new)
    }
}

#[cfg(test)]
fn codegen(
    store: &dyn ResultStore,
    key: &CodegenKey,
    ready: &ReadyProduct,
    compute: impl FnOnce() -> Result<ObjectArtifact, String>,
) -> Result<FragmentProduct, String> {
    codegen_checked(store, key, ready, &[], VerifyStore::Off, compute)
}

fn codegen_checked(
    store: &dyn ResultStore,
    key: &CodegenKey,
    ready: &ReadyProduct,
    optimizer: &[ProductRef],
    verify_store: VerifyStore,
    compute: impl FnOnce() -> Result<ObjectArtifact, String>,
) -> Result<FragmentProduct, String> {
    let address = key.address();
    let mut reads = vec![
        ready.product.record,
        ready.ready,
        ready.layout_closure,
        ready.emission_context,
        ready.unit_summary,
    ];
    reads.extend(ready.optimizer_reads.iter().map(|p| p.record));
    reads.extend(optimizer.iter().map(|p| p.record));
    // References bind domains/lengths as well as content. Action revisions are
    // deliberately absent: equal settled outputs cut off downstream work.
    fragment_checked(
        store,
        FragmentIdentity {
            address,
            owner: key.owner.instance.encoding().to_vec(),
            summary: ready.unit_summary,
        },
        &reads,
        verify_store,
        compute,
    )
}

struct FragmentIdentity {
    address: Address,
    owner: Vec<u8>,
    summary: Reference,
}

fn fragment_checked(
    store: &dyn ResultStore,
    identity: FragmentIdentity,
    reads: &[Reference],
    verify_store: VerifyStore,
    compute: impl FnOnce() -> Result<ObjectArtifact, String>,
) -> Result<FragmentProduct, String> {
    let FragmentIdentity {
        address,
        owner,
        summary,
    } = identity;
    let action = action("hirsel.codegen.action.v2", address, reads);
    let mut cached = None;
    if let Some(previous) = store.lookup_product(address).filter(|p| p.action == action) {
        let product = ProductRef {
            address,
            action,
            record: previous.reference,
        };
        if let Ok(fragment) = fragment_product(store, product)
            && fragment.summary == summary
        {
            if verify_store == VerifyStore::Off {
                return Ok(fragment);
            }
            cached = Some(fragment);
        }
    }
    let object = compute()?;
    let bytes = put_blob(store, &object.bytes)?;
    let object = put_record(
        store,
        OBJECT,
        &ObjectRecord {
            metadata: ObjectArtifact {
                bytes: Vec::new(),
                ..object
            },
            bytes: bytes.into(),
        },
    )?;
    let fragment = FragmentRecord {
        owner,
        object: object.into(),
        summary: summary.into(),
    };
    let bytes = hirsel_codec::to_text(&fragment);
    let expected = FRAGMENT
        .reference(bytes.as_bytes())
        .ok_or("native fragment exceeds record limit")?;
    if cached.is_some_and(|old| old.product.record != expected) {
        return Err("recomputed object fragment differs from cached output".into());
    }
    let record = store
        .put_content(FRAGMENT, bytes.as_bytes())
        .ok_or("native product publication failed")?;
    if record != expected {
        return Err("native fragment publication identity differs".into());
    }
    store.associate_product(address, action, record);
    Ok(FragmentProduct {
        product: ProductRef {
            address,
            action,
            record,
        },
        object,
        summary,
    })
}

#[cfg(test)]
fn link(
    store: &dyn ResultStore,
    key: &LinkKey,
    inputs: &LinkInputs,
    compute: &mut impl FnMut(&LinkKey, &LinkInputs, &[ObjectArtifact]) -> Result<Vec<u8>, String>,
) -> Result<LinkedProduct, String> {
    link_checked(store, key, inputs, VerifyStore::Off, compute)
}

fn link_checked(
    store: &dyn ResultStore,
    key: &LinkKey,
    inputs: &LinkInputs,
    verify_store: VerifyStore,
    compute: &mut impl FnMut(&LinkKey, &LinkInputs, &[ObjectArtifact]) -> Result<Vec<u8>, String>,
) -> Result<LinkedProduct, String> {
    let mut fragments = Vec::with_capacity(inputs.fragments.len());
    for fragment in &inputs.fragments {
        let record: FragmentRecord = get_record(store, FRAGMENT, fragment.product.record)?;
        if Reference::from(record.object.clone()) != fragment.object
            || Reference::from(record.summary.clone()) != fragment.summary
        {
            return Err("fragment descriptor disagrees with its product".into());
        }
        require(store, fragment.summary)?;
        let object: ObjectRecord = get_record(store, OBJECT, fragment.object)?;
        if object.metadata.target != key.target || !object.metadata.bytes.is_empty() {
            return Err("link fragment belongs to another target or embeds object bytes".into());
        }
        validate_link_input(store, object.bytes.into())?;
        fragments.push((record.owner, fragment.clone()));
    }
    fragments.sort_by(|a, b| a.0.cmp(&b.0));
    if fragments.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("duplicate fragment owner".into());
    }
    let inputs = LinkInputs {
        fragments: fragments.into_iter().map(|(_, product)| product).collect(),
        ..inputs.clone()
    };
    let mut reads: Vec<_> = inputs.fragments.iter().map(|f| f.product.record).collect();
    reads.push(inputs.static_manifest);
    reads.extend(&inputs.runtime_archives);
    reads.extend(&inputs.foreign_inputs);
    reads.push(inputs.launch_manifest);
    reads.extend(&inputs.execution);
    for read in &reads {
        require(store, *read)?;
    }
    for read in inputs.runtime_archives.iter().chain(&inputs.foreign_inputs) {
        if read.domain == BLOB {
            validate_link_input(store, *read)?;
        }
    }
    // Validate the entire fragment closure even on a linked-output hit. Missing
    // objects must not be hidden by an old successful executable association.
    let address = key.address();
    let action = action("hirsel.link.action.v1", address, &reads);
    let mut cached = None;
    if let Some(previous) = store.lookup_product(address).filter(|p| p.action == action) {
        let product = ProductRef {
            address,
            action,
            record: previous.reference,
        };
        if let Ok(linked) = linked_product(store, product) {
            if verify_store == VerifyStore::Off {
                return Ok(linked);
            }
            cached = Some(linked);
        }
    }
    let objects = inputs
        .fragments
        .iter()
        .map(|f| load_object(store, f.object))
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = compute(key, &inputs, &objects)?;
    if bytes.is_empty() {
        return Err("linker produced an empty executable".into());
    }
    let executable = put_blob(store, &bytes)?;
    let record = put_record(
        store,
        LINKED,
        &LinkedRecord {
            executable: executable.into(),
        },
    )?;
    if cached.is_some_and(|old| old.product.record != record) {
        return Err("recomputed executable differs from cached output".into());
    }
    store.associate_product(address, action, record);
    Ok(LinkedProduct {
        product: ProductRef {
            address,
            action,
            record,
        },
        executable,
    })
}

fn action(domain: &str, address: Address, reads: &[Reference]) -> Action {
    Action(hirsel_codec::digest(
        domain,
        "Reads",
        &(
            address.0,
            reads.iter().copied().map(WireRef::from).collect::<Vec<_>>(),
        ),
    ))
}

fn require(
    store: &dyn ResultStore,
    reference: Reference,
) -> Result<hirsel_engine::store::Bytes, String> {
    store
        .get_content(reference)
        .ok_or_else(|| "missing or corrupt native product content".into())
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn put_record<T: Snapshot>(
    store: &dyn ResultStore,
    domain: Domain,
    value: &T,
) -> Result<Reference, String> {
    store
        .put_content(domain, hirsel_codec::to_text(value).as_bytes())
        .ok_or_else(|| "native product publication failed".into())
}
fn get_record<T: Snapshot>(
    store: &dyn ResultStore,
    domain: Domain,
    reference: Reference,
) -> Result<T, String> {
    if reference.domain != domain {
        return Err("wrong native product domain/schema".into());
    }
    let bytes = require(store, reference)?;
    let text = std::str::from_utf8(&bytes).map_err(error)?;
    let value = hirsel_codec::from_text(text).map_err(|e| e.message)?;
    if hirsel_codec::to_text(&value) != text {
        return Err("noncanonical native product".into());
    }
    Ok(value)
}

/// Resolve a scheduler result through the same store; no output cache in clients.
pub fn linked_product(
    store: &dyn ResultStore,
    product: ProductRef,
) -> Result<LinkedProduct, String> {
    let record: LinkedRecord = get_record(store, LINKED, product.record)?;
    let executable = record.executable.into();
    validate_link_input(store, executable)?;
    Ok(LinkedProduct {
        product,
        executable,
    })
}

pub fn fragment_product(
    store: &dyn ResultStore,
    product: ProductRef,
) -> Result<FragmentProduct, String> {
    let record: FragmentRecord = get_record(store, FRAGMENT, product.record)?;
    let object = record.object.into();
    let summary = record.summary.into();
    require(store, summary)?;
    let record: ObjectRecord = get_record(store, OBJECT, object)?;
    validate_link_input(store, record.bytes.into())?;
    Ok(FragmentProduct {
        product,
        object,
        summary,
    })
}

pub fn load_object(
    store: &dyn ResultStore,
    reference: Reference,
) -> Result<ObjectArtifact, String> {
    let record: ObjectRecord = get_record(store, OBJECT, reference)?;
    if !record.metadata.bytes.is_empty() {
        return Err("object metadata embeds bytes".into());
    }
    Ok(ObjectArtifact {
        bytes: read_blob(store, record.bytes.into())?,
        ..record.metadata
    })
}

// A blob is an ordinary bounded record of checked chunk references. Large object
// and executable bytes never require raising the store's per-record limit.
fn put_blob(store: &dyn ResultStore, bytes: &[u8]) -> Result<Reference, String> {
    let chunks = bytes
        .chunks(BLOB_CHUNK)
        .map(|chunk| {
            store
                .put_content(CHUNK, chunk)
                .map(WireRef::from)
                .ok_or_else(|| "native chunk publication failed".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    put_record(
        store,
        BLOB,
        &BlobRecord {
            len: bytes.len() as u64,
            chunks,
        },
    )
}

/// Snapshot owned runtime/foreign input bytes in the same ordinary provider.
pub fn store_link_input(store: &dyn ResultStore, bytes: &[u8]) -> Result<Reference, String> {
    put_blob(store, bytes)
}

/// Snapshot a file/stream with one bounded chunk buffer, using the exact same
/// blob/chunk encoding as byte-slice inputs. Short reads do not split chunks.
pub fn store_link_input_reader(
    store: &dyn ResultStore,
    mut reader: impl std::io::Read,
) -> Result<Reference, String> {
    use std::io::Read;
    let mut chunks = Vec::new();
    let mut len = 0u64;
    let mut buffer = Vec::with_capacity(BLOB_CHUNK);
    loop {
        buffer.clear();
        reader
            .by_ref()
            .take(BLOB_CHUNK as u64)
            .read_to_end(&mut buffer)
            .map_err(error)?;
        if buffer.is_empty() {
            break;
        }
        len = len
            .checked_add(buffer.len() as u64)
            .ok_or("native blob length overflow")?;
        let reference = store
            .put_content(CHUNK, &buffer)
            .ok_or("native chunk publication failed")?;
        if CHUNK.reference(&buffer) != Some(reference) {
            return Err("native chunk content identity differs".into());
        }
        chunks.push(WireRef::from(reference));
    }
    put_record(store, BLOB, &BlobRecord { len, chunks })
}

/// Restore a frozen linker/tool input through the same atomic writer as
/// executable publication. The original mutable path is never reopened.
pub fn materialize_link_input(
    store: &dyn ResultStore,
    reference: Reference,
    destination: &Path,
    executable: bool,
) -> Result<(), String> {
    materialize_blob(store, reference, destination, executable)
}

/// Materialize a previously snapshotted input, never reopen its mutable path.
pub fn link_input_bytes(store: &dyn ResultStore, reference: Reference) -> Result<Vec<u8>, String> {
    read_blob(store, reference)
}
fn blob_record(store: &dyn ResultStore, reference: Reference) -> Result<BlobRecord, String> {
    let (domain, chunk_bytes) = if reference.domain == LEGACY_BLOB {
        (LEGACY_BLOB, MAX_RECORD)
    } else {
        (BLOB, BLOB_CHUNK)
    };
    let blob: BlobRecord = get_record(store, domain, reference)?;
    let mut total = 0u64;
    for (index, chunk) in blob.chunks.iter().enumerate() {
        let chunk: Reference = chunk.clone().into();
        if chunk.domain != CHUNK
            || chunk.len == 0
            || chunk.len > chunk_bytes as u64
            || (index + 1 != blob.chunks.len() && chunk.len != chunk_bytes as u64)
        {
            return Err("invalid native blob chunk".into());
        }
        total = total
            .checked_add(chunk.len)
            .ok_or("native blob length overflow")?;
    }
    if total != blob.len {
        return Err("native blob length mismatch".into());
    }
    Ok(blob)
}

/// Validate every chunk without retaining the complete payload in transport RAM.
pub fn validate_link_input(store: &dyn ResultStore, reference: Reference) -> Result<(), String> {
    for chunk in blob_record(store, reference)?.chunks {
        let _bytes = require(store, chunk.into())?;
    }
    Ok(())
}

fn read_blob(store: &dyn ResultStore, reference: Reference) -> Result<Vec<u8>, String> {
    let blob = blob_record(store, reference)?;
    let len = usize::try_from(blob.len).map_err(|_| "native blob exceeds host address space")?;
    let mut bytes = Vec::with_capacity(len);
    for chunk in blob.chunks {
        let chunk = require(store, chunk.into())?;
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Restore bytes atomically and check readiness before request completion.
/// Identical existing bytes retain their inode. A stale pathname never counts
/// as successful output, and no resident-specific linker/cache is involved.
pub fn materialize_executable(
    store: &dyn ResultStore,
    product: &LinkedProduct,
    destination: &Path,
) -> Result<(), String> {
    let checked = linked_product(store, product.product)?;
    if checked.executable != product.executable {
        return Err("executable descriptor mismatch".into());
    }
    materialize_blob(store, product.executable, destination, true)
}
fn materialize_blob(
    store: &dyn ResultStore,
    reference: Reference,
    destination: &Path,
    executable: bool,
) -> Result<(), String> {
    use std::io::{Read, Write};
    let blob = blob_record(store, reference)?;
    if executable && blob.chunks.is_empty() {
        return Err("empty executable output".into());
    }
    let matches = |file: &mut std::fs::File| -> Result<bool, String> {
        let mut buffer = vec![0; 64 << 10];
        for chunk in &blob.chunks {
            let chunk = require(store, chunk.clone().into())?;
            for part in chunk.chunks(buffer.len()) {
                file.read_exact(&mut buffer[..part.len()]).map_err(error)?;
                if buffer[..part.len()] != *part {
                    return Ok(false);
                }
            }
        }
        Ok(file.read(&mut buffer[..1]).map_err(error)? == 0)
    };
    let permissions = |file: &std::fs::File| -> std::io::Result<bool> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok((file.metadata()?.permissions().mode() & 0o111 != 0) == executable)
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Ok(true)
        }
    };
    if let Ok(mut file) = std::fs::File::open(destination)
        && file.metadata().map_err(error)?.is_file()
        && permissions(&file).map_err(error)?
        && matches(&mut file).unwrap_or(false)
    {
        return Ok(());
    }
    let parent = destination.parent().ok_or("executable has no parent")?;
    std::fs::create_dir_all(parent).map_err(error)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = parent.join(format!(
        ".hirsel-executable-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(error)?;
        for chunk in &blob.chunks {
            let chunk = require(store, chunk.clone().into())?;
            file.write_all(&chunk).map_err(error)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))
            .map_err(error)?;
        }
        file.sync_all().map_err(error)?;
        std::fs::rename(&temporary, destination).map_err(error)?;
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(error)?;
        let mut file = std::fs::File::open(destination).map_err(error)?;
        if !permissions(&file).map_err(error)? || !matches(&mut file).map_err(error)? {
            return Err("materialized native file failed readiness".into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[derive(Clone, Debug)]
struct WireRef {
    output: ContentHash,
    kind: u32,
    schema: u32,
    len: u64,
}
impl From<Reference> for WireRef {
    fn from(r: Reference) -> Self {
        Self {
            output: r.output.0,
            kind: r.domain.kind,
            schema: r.domain.schema,
            len: r.len,
        }
    }
}
impl From<WireRef> for Reference {
    fn from(r: WireRef) -> Self {
        Self {
            output: Output(r.output),
            domain: Domain {
                kind: r.kind,
                schema: r.schema,
            },
            len: r.len,
        }
    }
}
struct FragmentRecord {
    owner: Vec<u8>,
    object: WireRef,
    summary: WireRef,
}
struct ObjectRecord {
    metadata: ObjectArtifact,
    bytes: WireRef,
}
struct LinkedRecord {
    executable: WireRef,
}
struct BlobRecord {
    len: u64,
    chunks: Vec<WireRef>,
}
snapshot_struct!(WireRef {
    output,
    kind,
    schema,
    len
});
snapshot_struct!(FragmentRecord {
    owner,
    object,
    summary
});
snapshot_struct!(ObjectRecord { metadata, bytes });
snapshot_struct!(LinkedRecord { executable });
snapshot_struct!(BlobRecord { len, chunks });

#[cfg(test)]
#[path = "../../tests/native-validation/object_queries.rs"]
mod tests;

pub mod tasks;
pub mod whole;
pub use tasks::{
    CODEGEN_KIND, LINK_KIND, LinkDependencies, ObjectTaskFactory, ObjectTaskRequest,
    ObjectTaskSnapshot,
};

#[cfg(test)]
mod transport_tests {
    use super::*;
    use hirsel_engine::store::DirStore;

    #[test]
    fn streamed_inputs_preserve_chunk_identity_and_refuse_incomplete_content() {
        let directory = tempfile::tempdir().unwrap();
        let store = DirStore::new(directory.path().join("products"));
        let bytes = vec![0x5a; MAX_RECORD + 1];
        // Deliberately short reads must produce the same canonical chunks.
        struct Short<'a>(&'a [u8]);
        impl std::io::Read for Short<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let len = output.len().min(self.0.len()).min(17);
                output[..len].copy_from_slice(&self.0[..len]);
                self.0 = &self.0[len..];
                Ok(len)
            }
        }
        let reference = store_link_input(&store, &bytes).unwrap();
        assert_eq!(
            store_link_input_reader(&store, Short(&bytes)).unwrap(),
            reference
        );
        assert_eq!(link_input_bytes(&store, reference).unwrap(), bytes);
        let mut record: BlobRecord = get_record(&store, BLOB, reference).unwrap();
        record.len += 1;
        let invalid = put_record(&store, BLOB, &record).unwrap();
        assert!(link_input_bytes(&store, invalid).is_err());
        record.len -= 1;
        record.chunks[0].kind = BLOB.kind;
        let invalid = put_record(&store, BLOB, &record).unwrap();
        assert!(link_input_bytes(&store, invalid).is_err());
        let record: BlobRecord = get_record(&store, BLOB, reference).unwrap();
        let chunk: Reference = record.chunks[0].clone().into();
        let path = hirsel_artifact_store::Store::new(store.root().into()).output_path(chunk);
        std::fs::remove_file(path).unwrap();
        let reopened = DirStore::new(store.root().into());
        assert!(link_input_bytes(&reopened, reference).is_err());
        assert!(
            materialize_link_input(
                &reopened,
                reference,
                &directory.path().join("missing"),
                true
            )
            .is_err()
        );
        assert!(!directory.path().join("missing").exists());
    }

    #[test]
    fn materialization_restores_bytes_modes_and_durable_inputs_atomically() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let store = DirStore::new(directory.path().join("products"));
        let reference = store_link_input_reader(&store, b"frozen linker input".as_slice()).unwrap();
        let path = directory.path().join("output/file");
        materialize_link_input(&store, reference, &path, true).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        materialize_link_input(&store, reference, &path, true).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        materialize_link_input(&store, reference, &path, true).unwrap();
        assert_ne!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
            0
        );
        std::fs::write(&path, b"stale bytes").unwrap();
        let reopened = DirStore::new(store.root().into());
        materialize_link_input(&reopened, reference, &path, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"frozen linker input");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
            0
        );
        std::fs::remove_file(&path).unwrap();
        materialize_link_input(&reopened, reference, &path, true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"frozen linker input");
        let empty = store_link_input(&store, b"").unwrap();
        assert!(materialize_link_input(&store, empty, &path, true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"frozen linker input");
    }
}

pub mod capture;
pub mod platform;
pub mod thin;

pub mod runtime;

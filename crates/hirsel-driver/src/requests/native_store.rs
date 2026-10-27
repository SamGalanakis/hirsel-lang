//! Native products retain their host-selected cache independently of front results.
use hirsel_engine::{ToolError, runtime::ProcessBudget, store::*};
use std::{path::Path, sync::Arc};

pub(super) fn capture(
    source: Arc<dyn ResultStore>,
    request: &super::NativeRequest,
    purpose: super::native_platform::NativePurpose,
    budget: &Arc<ProcessBudget>,
    transport: u64,
    scratch: &Path,
    disk: u64,
) -> Result<Arc<dyn ResultStore>, ToolError> {
    let partition = hirsel_lir::objects::partition(
        &request.target.triple,
        request.backend.name(),
        &format!("{:?}", purpose.opt(request)).to_lowercase(),
    );
    let directory = hirsel_lir::objects::DirObjects::default_location(&partition)
        .map(|store| store.root().to_owned());
    let selected = std::env::var_os("HIRSEL_OBJECTS");
    let default = if directory.is_some() {
        "dir@1"
    } else {
        "none@1"
    };
    let name = selected
        .as_ref()
        .map(|value| value.to_string_lossy())
        .unwrap_or_else(|| default.into());
    let cache: Arc<dyn ResultStore> = match name.as_ref() {
        "none@1" => Arc::new(NoneStore),
        "sim@1" => Arc::new(SimStore::default()),
        "dir@1" => Arc::new(
            DirStore::new(directory.ok_or_else(|| {
                ToolError::Runtime(
                    "dir@1: no cache directory; set an absolute XDG_CACHE_HOME or HOME".into(),
                )
            })?)
            .with_cap(hirsel_lir::objects::DEFAULT_CAP),
        ),
        other => {
            return Err(ToolError::Runtime(format!(
                "HIRSEL_OBJECTS: unknown object store `{other}`"
            )));
        }
    };
    let publication = cache.publication_scope();
    let reads = cache.read_scope();
    let backend = Arc::new(NativeStore {
        source,
        cache,
        _publication: publication,
        _reads: reads,
    });
    let allowance = budget.reserve(transport).map_err(|error| {
        ToolError::Runtime(format!(
            "native object transport exceeds the shared process budget: {error}",
        ))
    })?;
    let store =
        RequestStore::with_scratch_and_verifier_transport(backend, allowance, scratch, disk)
            .map_err(ToolError::Io)?;
    Ok(Arc::new(store))
}
struct NativeStore {
    source: Arc<dyn ResultStore>,
    cache: Arc<dyn ResultStore>,
    _publication: Option<Box<dyn Send + Sync>>,
    _reads: Option<Box<dyn Send + Sync>>,
}
impl ResultStore for NativeStore {
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.cache.local_product_store()
    }
    fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
        self.cache.read_scope()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.cache.verifier()
    }
    fn enabled(&self) -> bool {
        self.cache.enabled()
    }
    fn query_cache_enabled(&self) -> bool {
        self.cache.query_cache_enabled()
    }
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.cache.get(key)
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.cache.put(key, value);
    }
    fn lookup_product(&self, address: Address) -> Option<Association> {
        self.cache.lookup_product(address)
    }
    fn associate_product(&self, address: Address, action: Action, reference: Reference) {
        self.cache.associate_product(address, action, reference);
    }
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        if let Some(bytes) = self.cache.get_content(reference) {
            return Some(bytes);
        }
        let bytes = self.source.get_content(reference)?;
        // A retained native product must not depend on a previous request's
        // transient source transport. Copy only the content it actually reads.
        self.cache.put_content(reference.domain, &bytes);
        Some(bytes)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        self.cache.put_content(domain, bytes)
    }
    fn name(&self) -> &'static str {
        self.cache.name()
    }
}

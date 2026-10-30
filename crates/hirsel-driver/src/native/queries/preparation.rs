//! Per-definition native views over settled ordinary source products. Tasks
//! demand one source descriptor, without assembling a source/native snapshot.
//! Target/ABI/toolchain/policy reads and C/Ready admission stay in consumers.
use hirsel_engine::{
    FrontNativeDefinitionOutput, FrontNativeDefinitionQueries, NativeDefinitionProduct,
    NativeMetadataWeight, NativeOriginalProduct, runtime::*, store::*,
};
use hirsel_rc::instance::{Requirement, SourceDeclaration};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    task::Poll,
};

pub const PREPARATION_KIND: u32 = 0x4e00_0050;
const OUTPUT: Domain = Domain {
    kind: 0x4e00_0051,
    schema: 1,
};

/// Declaration, lexical closure contracts and type/layout requirements for one
/// source owner. The original front owner remains charged in full, even when a
/// consumer selects only one small view. No mutable native memo lives here.
pub struct NativePreparedSource {
    product: ProductRef,
    source_product: ProductRef,
    front: FrontNativeDefinitionOutput,
    declaration: Option<SourceDeclaration>,
    anchors: Vec<hirsel_span::Anchor>,
    requirements: Vec<Requirement>,
    _allowance: MemoryReservation,
}
impl NativePreparedSource {
    pub fn product(&self) -> ProductRef {
        self.product.clone()
    }
    pub fn source_product(&self) -> ProductRef {
        self.source_product.clone()
    }
    pub fn definition(&self) -> &NativeDefinitionProduct {
        &self.front.product.source
    }
    pub fn original(&self) -> Option<&Arc<NativeOriginalProduct>> {
        self.front.original.as_ref()
    }
    pub fn declaration(&self) -> Option<&SourceDeclaration> {
        self.declaration.as_ref()
    }
    pub fn anchors(&self) -> &[hirsel_span::Anchor] {
        &self.anchors
    }
    pub fn requirements(&self) -> &[Requirement] {
        &self.requirements
    }
    pub fn retained_metadata_bytes(&self) -> u64 {
        self._allowance.bytes()
    }
}

struct Entry {
    path: DefPath,
    source: QueryId,
    query: QueryId,
    completed: OnceLock<Arc<NativePreparedSource>>,
}
/// The namespace contains dispatch slots only. Each demanded output owns its
/// copied view charge independently of the registry and outstanding tasks.
pub struct NativePreparationQueries {
    source: Arc<FrontNativeDefinitionQueries>,
    entries: BTreeMap<ContentHash, Entry>,
    store: Arc<dyn ResultStore>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    allowance: Mutex<MemoryReservation>,
    _owner: MemoryReservation,
}
impl NativePreparationQueries {
    pub fn new(
        source: Arc<FrontNativeDefinitionQueries>,
        definitions: &[DefPath],
        store: Arc<dyn ResultStore>,
        weight: MemoryWeight,
        mut allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<Self>, String> {
        if weight.retained < size_of::<Task>() as u64 {
            return Err("native preparation continuation exceeds admission".into());
        }
        let mut metadata = NativeMetadataWeight::default();
        metadata.add(size_of::<Self>() + 2 * size_of::<usize>());
        metadata.tree::<ContentHash, Entry>(definitions.len());
        for path in definitions {
            metadata.path(path);
        }
        let owner = allowance
            .split(metadata.bytes())
            .ok_or("native preparation namespace exceeds admission")?;
        let mut entries = BTreeMap::new();
        for path in definitions {
            let dependency = source
                .query(path)
                .ok_or("native preparation source is outside its namespace")?;
            let query = query(dependency);
            if entries
                .insert(
                    query.address.0,
                    Entry {
                        path: path.clone(),
                        source: dependency,
                        query,
                        completed: OnceLock::new(),
                    },
                )
                .is_some()
            {
                return Err("duplicate native preparation source".into());
            }
        }
        Ok(Arc::new(Self {
            source,
            entries,
            store,
            weight,
            errors,
            allowance: Mutex::new(allowance),
            _owner: owner,
        }))
    }
    pub fn input_bytes(&self) -> u64 {
        self._owner.bytes()
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        let id = query(self.source.query(path)?);
        self.entries
            .get(&id.address.0)
            .filter(|entry| entry.path == *path)
            .map(|entry| entry.query)
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    /// Only this producer's exact completion opens its typed views. Source
    /// receipts and ordinary content associations supply no native authority.
    pub fn outcome(&self, product: ProductRef) -> Option<Arc<NativePreparedSource>> {
        let output = self.entries.get(&product.address.0)?.completed.get()?;
        if product != output.product {
            return None;
        }
        let bytes = self.store.get_content(
            product
                .archive(ArchiveAdmission::Retained)
                .ok()?
                .reference(),
        )?;
        (OUTPUT.reference(&bytes)
            == Some(
                product
                    .archive(ArchiveAdmission::Retained)
                    .ok()?
                    .reference(),
            ))
        .then(|| output.clone())
    }
    fn prepare(&self, entry: &Entry, source_product: ProductRef) -> Result<ProductRef, Reference> {
        if let Some(output) = entry.completed.get() {
            return if output.source_product == source_product
                && self.outcome(output.product.clone()).is_some()
            {
                Ok(output.product.clone())
            } else {
                Err(self.errors.protocol)
            };
        }
        let front = self
            .source
            .outcome(source_product.clone())
            .ok_or(self.errors.protocol)?;
        if front.product.source.definition != entry.path {
            return Err(self.errors.protocol);
        }
        let mut buffer = [0u8; 1024];
        let len = {
            let mut writer = std::io::Cursor::new(&mut buffer[..]);
            serde_json::to_writer(
                &mut writer,
                &(
                    source_product.address.0,
                    source_product.action.0,
                    source_product
                        .archive(ArchiveAdmission::Retained)
                        .map_err(|_| self.errors.resource)?
                        .reference(),
                ),
            )
            .map_err(|_| self.errors.resource)?;
            writer.position() as usize
        };
        let bytes = &buffer[..len];
        if bytes.len() as u64 > self.weight.scratch {
            return Err(self.errors.resource);
        }
        let allowance = self
            .allowance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .split(copied_bytes(&front.product))
            .ok_or(self.errors.resource)?;
        let declaration =
            super::source::source_declaration(&front.product).map_err(|_| self.errors.protocol)?;
        let anchors = front.product.contracts.as_ref().map_or_else(
            || vec![front.product.source.source.syntax.anchor],
            |contracts| contracts.source_anchors.clone(),
        );
        let requirements = front
            .product
            .requirements
            .edges
            .iter()
            .map(crate::native::adapt::requirement)
            .collect::<Vec<_>>();
        let record = self
            .store
            .put_content(OUTPUT, bytes)
            .filter(|reference| OUTPUT.reference(bytes) == Some(*reference))
            .ok_or(self.errors.resource)?;
        let product = ProductRef::import_archive(
            entry.query.address,
            Action(ContentHash::of_parts(
                "hirsel.native.preparation-action.v1",
                [
                    entry.query.address.0.0.as_slice(),
                    record.output.0.0.as_slice(),
                ],
            )),
            self.store
                .admit_product_archive(record, bytes.to_vec().into())
                .ok_or(self.errors.resource)?,
        );
        entry
            .completed
            .set(Arc::new(NativePreparedSource {
                product: product.clone(),
                source_product,
                front,
                declaration,
                anchors,
                requirements,
                _allowance: allowance,
            }))
            .map_err(|_| self.errors.protocol)?;
        self.store
            .associate_product(product.address, product.action, record);
        Ok(product)
    }
}
fn copied_bytes(source: &hirsel_engine::NativeDiscoveryProduct) -> u64 {
    fn vector<T>(weight: &mut NativeMetadataWeight, len: usize) {
        weight.add(len.saturating_mul(size_of::<T>()));
    }
    // These projections clone vectors or collect exact-size slice iterators.
    // Reserve their requested layouts before allocating any copied view.
    let mut weight = NativeMetadataWeight::default();
    weight.add(size_of::<NativePreparedSource>() + 2 * size_of::<usize>());
    if let Some(contracts) = &source.contracts {
        weight.add(
            size_of::<hirsel_layout::instance::InstanceDeclaration>() + 2 * size_of::<usize>(),
        );
        weight.path(&source.source.definition);
        vector::<hirsel_types::TypeId>(&mut weight, contracts.entry_parameters.len());
        vector::<hirsel_tir::BodyId>(&mut weight, contracts.caller_bodies.len());
        vector::<Vec<hirsel_lir::abi_types::ParameterPath>>(
            &mut weight,
            contracts.scoped_parameters.len(),
        );
        for parameters in &contracts.scoped_parameters {
            vector::<hirsel_lir::abi_types::ParameterPath>(&mut weight, parameters.len());
            for (_, fields) in parameters {
                vector::<u32>(&mut weight, fields.len());
            }
        }
        vector::<hirsel_span::Anchor>(&mut weight, contracts.source_anchors.len());
    } else {
        vector::<hirsel_span::Anchor>(&mut weight, 1);
    }
    vector::<Requirement>(&mut weight, source.requirements.edges.len());
    for edge in &source.requirements.edges {
        weight.path(&edge.owner);
        weight.path(&edge.callee);
        vector::<hirsel_types::GenericArgument>(&mut weight, edge.arguments.len());
    }
    weight.bytes()
}
fn query(source: QueryId) -> QueryId {
    QueryId {
        kind: PREPARATION_KIND,
        address: Address(ContentHash::of_parts(
            "hirsel.native.preparation-address.v1",
            [
                source.kind.to_le_bytes().as_slice(),
                source.address.0.0.as_slice(),
            ],
        )),
    }
}
struct Factory(Arc<NativePreparationQueries>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if self
            .0
            .entries
            .get(&query.address.0)
            .is_none_or(|entry| entry.query != query)
        {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task {
            owner: self.0.clone(),
            query,
            terminal: None,
        }))
    }
}
struct Task {
    owner: Arc<NativePreparationQueries>,
    query: QueryId,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        self.owner.weight
    }
    fn continuation_bytes(&self) -> u64 {
        size_of::<Self>() as u64
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let result = if let Some(result) = self.terminal.clone() {
            result
        } else {
            let entry = &self.owner.entries[&self.query.address.0];
            match cx.read(entry.source) {
                Poll::Pending => return QueryPoll::Waiting(vec![entry.source]),
                Poll::Ready(Err(error)) => Err(error),
                Poll::Ready(Ok(source)) => self.owner.prepare(entry, source),
            }
        };
        self.terminal = Some(result.clone());
        match result {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}

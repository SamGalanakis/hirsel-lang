//! Native Link continuations with explicit root, fragment, native and byte reads.
use super::{products::SettledProducts, root_tasks::NativeRootTasks, roots::RootProducts, *};
use crate::native::object_queries::{
    CodegenKey, LinkDependencies, LinkInputs, LinkKey, ObjectProducts, ObjectQueries,
    ObjectTaskRequest, ObjectTaskSnapshot,
};
use hirsel_artifact_store::{Action, Address, Domain};
use hirsel_engine::{runtime::*, store::ResultStore};
use std::{collections::BTreeMap, task::Poll};

pub const LINK_INPUT_KIND: u32 = 0x4e00_0006;
pub const LINK_MANIFEST_KIND: u32 = 0x4e00_0007;
const INPUT: Domain = Domain {
    kind: LINK_INPUT_KIND,
    schema: 1,
};
const MANIFEST: Domain = Domain {
    kind: LINK_MANIFEST_KIND,
    schema: 1,
};

/// Frozen host platform inputs remain owned by the executor. The native side
/// supplies only the settled descriptor scope; this interface conveys no proof.
pub trait NativeLinkExecutor: Send + Sync {
    fn link(&self, native: &mut RootProducts, input: LinkExecution<'_>) -> Result<Vec<u8>, String>;
}
pub struct LinkExecution<'a> {
    pub key: &'a LinkKey,
    pub inputs: &'a LinkInputs,
    pub root: &'a Arc<NativeRootProduct>,
    pub ready: &'a [Arc<ReadyProduct>],
    pub objects: &'a [hirsel_lir::stage::ObjectArtifact],
}
/// Captured during input admission, never discovered by QueryFactory::create.
pub struct NativeLinkPlan {
    pub key: LinkKey,
    pub fragments: Vec<CodegenKey>,
    pub runtime_archives: Vec<Reference>,
    pub foreign_inputs: Vec<Reference>,
    pub executor: Arc<dyn NativeLinkExecutor>,
    pub weight: MemoryWeight,
}
struct Plan {
    input: NativeLinkPlan,
    manifest: QueryId,
    static_input: QueryId,
    runtime: Vec<QueryId>,
    foreign: Vec<QueryId>,
    launch_input: QueryId,
    ready: Vec<QueryId>,
}
#[derive(Clone, Copy)]
enum Leaf {
    Static,
    Launch,
    Fixed(Reference),
}
enum Work {
    Manifest(Arc<Plan>),
    Input(Leaf),
}
struct Request {
    query: QueryId,
    work: Work,
}
type Id = (u32, ContentHash);
fn id(query: QueryId) -> Id {
    (query.kind, query.address.0)
}
fn address(kind: u32, bytes: &[u8]) -> QueryId {
    QueryId {
        kind,
        address: Address(ContentHash::of_parts(
            "hirsel.native.link-input-address.v1",
            [kind.to_le_bytes().as_slice(), bytes],
        )),
    }
}
fn product(query: QueryId, record: Reference) -> ProductRef {
    ProductRef {
        address: query.address,
        action: Action(ContentHash::of_parts(
            "hirsel.native.link-input-action.v1",
            [record.output.0.0.as_slice()],
        )),
        record,
    }
}

/// Immutable request plans plus the root producer's completed ordinary output.
/// Native/C checks remain in the selected task-local producers.
pub struct LinkTasks {
    root: Arc<NativeRootTasks>,
    plans: BTreeMap<ContentHash, Arc<Plan>>,
    requests: BTreeMap<Id, Arc<Request>>,
    leaf_weight: MemoryWeight,
}
impl LinkTasks {
    pub fn new(
        root: Arc<NativeRootTasks>,
        inputs: Vec<NativeLinkPlan>,
        leaf_weight: MemoryWeight,
    ) -> Result<Arc<Self>, String> {
        let mut owners = root
            .native
            .snapshot
            .owners
            .values()
            .cloned()
            .collect::<Vec<_>>();
        owners.sort_by(InstanceAddress::canonical_cmp);
        let root_query = root.query_id();
        let static_input = address(
            LINK_INPUT_KIND,
            &serde_json::to_vec(&(root_query.address.0, 0u32)).map_err(|e| e.to_string())?,
        );
        let launch_input = address(
            LINK_INPUT_KIND,
            &serde_json::to_vec(&(root_query.address.0, 1u32)).map_err(|e| e.to_string())?,
        );
        let mut requests = BTreeMap::new();
        requests.insert(
            id(static_input),
            Arc::new(Request {
                query: static_input,
                work: Work::Input(Leaf::Static),
            }),
        );
        requests.insert(
            id(launch_input),
            Arc::new(Request {
                query: launch_input,
                work: Work::Input(Leaf::Launch),
            }),
        );
        let mut plans = BTreeMap::new();
        for input in inputs {
            let mut selected = input.key.roots.clone();
            selected.sort_by(InstanceAddress::canonical_cmp);
            if selected != owners
                || input.key.target != root.native.snapshot.options.target.digest()
                || input.fragments.len() != owners.len()
                || input
                    .fragments
                    .iter()
                    .zip(&owners)
                    .any(|(fragment, owner)| &fragment.owner != owner)
                || input.fragments.first().is_none_or(|first| {
                    !matches!(first.backend.as_str(), "llvm" | "cranelift")
                        || input.fragments.iter().any(|f| f.backend != first.backend)
                })
            {
                return Err(
                    "native Link plan differs from the complete selected instance/backend closure"
                        .into(),
                );
            }
            let retained = std::mem::size_of::<ObjectTaskRequest>()
                + owners
                    .iter()
                    .map(|k| k.instance.encoding().len() + std::mem::size_of::<InstanceAddress>())
                    .sum::<usize>();
            if retained as u64 > input.weight.retained {
                return Err("Link continuation exceeds declared retained allowance".into());
            }
            let mut leaves = |references: &[Reference]| -> Result<Vec<QueryId>, String> {
                references
                    .iter()
                    .map(|reference| {
                        if root.native.snapshot.store.get_content(*reference).is_none() {
                            return Err("missing admitted Link input".into());
                        }
                        let query = address(
                            LINK_INPUT_KIND,
                            &serde_json::to_vec(reference).map_err(|e| e.to_string())?,
                        );
                        requests.entry(id(query)).or_insert_with(|| {
                            Arc::new(Request {
                                query,
                                work: Work::Input(Leaf::Fixed(*reference)),
                            })
                        });
                        Ok(query)
                    })
                    .collect()
            };
            let runtime = leaves(&input.runtime_archives)?;
            let foreign = leaves(&input.foreign_inputs)?;
            let manifest = address(LINK_MANIFEST_KIND, &input.key.address().0.0);
            let ready = owners.iter().map(InstanceAddress::ready_query_id).collect();
            let plan = Arc::new(Plan {
                input,
                manifest,
                static_input,
                runtime,
                foreign,
                launch_input,
                ready,
            });
            if plans
                .insert(plan.input.key.address().0, plan.clone())
                .is_some()
            {
                return Err("duplicate native Link task".into());
            }
            requests.insert(
                id(manifest),
                Arc::new(Request {
                    query: manifest,
                    work: Work::Manifest(plan),
                }),
            );
        }
        let maximum_reads =
            owners.len() * std::mem::size_of::<QueryId>() + std::mem::size_of::<LeafTask>();
        if maximum_reads as u64 > leaf_weight.retained {
            return Err("Link manifest continuation exceeds declared retained allowance".into());
        }
        Ok(Arc::new(Self {
            root,
            plans,
            requests,
            leaf_weight,
        }))
    }
    fn store(&self) -> &dyn ResultStore {
        self.root.native.snapshot.store.as_ref()
    }
    fn failure(&self, message: &str) -> Reference {
        self.root.native.failure(message)
    }
    fn publish(&self, query: QueryId, domain: Domain, bytes: &[u8]) -> Result<ProductRef, String> {
        let expected = domain
            .reference(bytes)
            .ok_or("Link task metadata exceeds record bound")?;
        let record = self
            .store()
            .put_content(domain, bytes)
            .ok_or("Link task metadata publication failed")?;
        if record != expected {
            return Err("Link task metadata identity differs".into());
        }
        let product = product(query, record);
        self.store()
            .associate_product(query.address, product.action, record);
        Ok(product)
    }
    fn current(&self, product: ProductRef) -> Result<(), String> {
        let association = self
            .store()
            .lookup_product(product.address)
            .ok_or("missing Link task association")?;
        if association.action != product.action || association.reference != product.record {
            return Err("Link task output changed".into());
        }
        Ok(())
    }
    fn manifest(
        &self,
        manifest: ProductRef,
    ) -> Result<(Arc<Plan>, ProductRef, Arc<NativeRootProduct>), String> {
        self.current(manifest)?;
        if manifest.record.domain != MANIFEST {
            return Err("wrong native Link manifest domain".into());
        }
        let bytes = self
            .store()
            .get_content(manifest.record)
            .ok_or("missing native Link manifest")?;
        let (link, address, action, record): (ContentHash, ContentHash, ContentHash, Reference) =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let plan = self
            .plans
            .get(&link)
            .ok_or("unregistered native Link manifest")?;
        if plan.manifest.address != manifest.address {
            return Err("native Link manifest address differs".into());
        }
        let root_ref = ProductRef {
            address: Address(address),
            action: Action(action),
            record,
        };
        let root = self.root.output(root_ref)?;
        Ok((plan.clone(), root_ref, root))
    }
}

pub struct LinkInputFactory {
    tasks: Arc<LinkTasks>,
}
impl LinkInputFactory {
    pub fn new(tasks: Arc<LinkTasks>) -> Self {
        Self { tasks }
    }
}
impl QueryFactory for LinkInputFactory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let request = self
            .tasks
            .requests
            .get(&id(query))
            .ok_or_else(|| self.tasks.failure("unregistered Link input task"))?
            .clone();
        Ok(Box::new(LeafTask {
            tasks: self.tasks.clone(),
            request,
            terminal: None,
        }))
    }
}
struct LeafTask {
    tasks: Arc<LinkTasks>,
    request: Arc<Request>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl LeafTask {
    fn compute(&self, cx: &mut dyn QueryContext) -> Result<ProductRef, QueryPoll> {
        let root = match &self.request.work {
            Work::Input(Leaf::Fixed(_)) => None,
            _ => match cx.read(self.tasks.root.query_id()) {
                Poll::Pending => return Err(QueryPoll::Waiting(vec![self.tasks.root.query_id()])),
                Poll::Ready(Err(error)) => return Err(QueryPoll::Failed(error)),
                Poll::Ready(Ok(product)) => Some((
                    product,
                    self.tasks
                        .root
                        .output(product)
                        .map_err(|e| QueryPoll::Failed(self.tasks.failure(&e)))?,
                )),
            },
        };
        let result = match &self.request.work {
            Work::Manifest(plan) => {
                let (root, _) = root.unwrap();
                let bytes = serde_json::to_vec(&(
                    plan.input.key.address().0,
                    root.address.0,
                    root.action.0,
                    root.record,
                ))
                .map_err(|e| QueryPoll::Failed(self.tasks.failure(&e.to_string())))?;
                self.tasks.publish(self.request.query, MANIFEST, &bytes)
            }
            Work::Input(leaf) => {
                let reference = match leaf {
                    Leaf::Fixed(reference) => *reference,
                    Leaf::Static => root.as_ref().unwrap().1.static_manifest,
                    Leaf::Launch => root.as_ref().unwrap().1.launch_manifest,
                };
                if self.tasks.store().get_content(reference).is_none() {
                    return Err(QueryPoll::Failed(
                        self.tasks.failure("missing settled Link input bytes"),
                    ));
                }
                let bytes = serde_json::to_vec(&reference)
                    .map_err(|e| QueryPoll::Failed(self.tasks.failure(&e.to_string())))?;
                self.tasks.publish(self.request.query, INPUT, &bytes)
            }
        };
        result.map_err(|e| QueryPoll::Failed(self.tasks.failure(&e)))
    }
}
impl QueryTask for LeafTask {
    fn id(&self) -> QueryId {
        self.request.query
    }
    fn weight(&self) -> MemoryWeight {
        self.tasks.leaf_weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return match result {
                Ok(p) => QueryPoll::Complete(p),
                Err(e) => QueryPoll::Failed(e),
            };
        }
        match self.compute(cx) {
            Ok(product) => {
                self.terminal = Some(Ok(product));
                QueryPoll::Complete(product)
            }
            Err(QueryPoll::Failed(error)) => {
                self.terminal = Some(Err(error));
                QueryPoll::Failed(error)
            }
            Err(waiting) => waiting,
        }
    }
}
impl ObjectTaskSnapshot for LinkTasks {
    fn store(&self) -> &dyn ResultStore {
        self.store()
    }
    fn request(&self, query: QueryId) -> Result<ObjectTaskRequest, Reference> {
        let plan = self
            .plans
            .get(&query.address.0)
            .ok_or_else(|| self.failure("unregistered native Link task"))?;
        if query != plan.input.key.query_id() {
            return Err(self.failure("native Link query kind differs"));
        }
        Ok(ObjectTaskRequest::Link {
            key: plan.input.key.clone(),
            manifest: plan.manifest,
            weight: plan.input.weight,
        })
    }
    fn ready_descriptor(&self, _: ProductRef) -> Result<ReadyProduct, Reference> {
        Err(self.failure("Link snapshot has no Codegen descriptor lookup"))
    }
    fn optimizer_query(&self, _: ProductRef) -> Result<QueryId, Reference> {
        Err(self.failure("Link snapshot has no optimizer producer"))
    }
    fn link_dependencies(&self, manifest: ProductRef) -> Result<LinkDependencies, Reference> {
        let (plan, _, _) = self.manifest(manifest).map_err(|e| self.failure(&e))?;
        Ok(LinkDependencies {
            fragments: plan
                .input
                .fragments
                .iter()
                .map(CodegenKey::query_id)
                .collect(),
            native_reads: plan.ready.clone(),
            static_manifest: plan.static_input,
            runtime_archives: plan.runtime.clone(),
            foreign_inputs: plan.foreign.clone(),
            launch_manifest: plan.launch_input,
        })
    }
    fn link_input(&self, product: ProductRef) -> Result<Reference, Reference> {
        let read = || -> Result<Reference, String> {
            self.current(product)?;
            if product.record.domain != INPUT {
                return Err("wrong Link input descriptor domain".into());
            }
            let bytes = self
                .store()
                .get_content(product.record)
                .ok_or("missing Link input descriptor")?;
            let reference = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if self.store().get_content(reference).is_none() {
                return Err("missing Link input content".into());
            }
            Ok(reference)
        };
        read().map_err(|e| self.failure(&e))
    }
    fn products(&self, settled: &[ProductRef]) -> Result<Box<dyn ObjectProducts>, Reference> {
        let compute = || -> Result<Box<dyn ObjectProducts>, String> {
            let manifest = *settled.first().ok_or("missing settled Link manifest")?;
            let (plan, root_ref, root) = self.manifest(manifest)?;
            let mut expected = plan
                .input
                .fragments
                .iter()
                .map(CodegenKey::query_id)
                .collect::<Vec<_>>();
            expected.push(plan.static_input);
            expected.extend(&plan.runtime);
            expected.extend(&plan.foreign);
            expected.push(plan.launch_input);
            expected.extend(&plan.ready);
            if settled.len() != expected.len() + 1
                || settled[1..]
                    .iter()
                    .zip(&expected)
                    .any(|(p, q)| p.address != q.address)
            {
                return Err("Link producer received a different settled dependency scope".into());
            }
            let reads = &settled[settled.len() - plan.ready.len()..];
            let native = SettledProducts::new(self.root.native.snapshot.clone(), reads)?;
            let (mut link, _) = self.root.products(root_ref, reads)?;
            let readies = reads
                .iter()
                .map(|read| super::ready_product(self.store(), *read).map(Arc::new))
                .collect::<Result<Vec<_>, _>>()?;
            let snapshot = self.root.native.snapshot.store.clone();
            Ok(Box::new(ObjectQueries::new(
                snapshot,
                native,
                move |key: &LinkKey,
                      inputs: &LinkInputs,
                      objects: &[hirsel_lir::stage::ObjectArtifact]| {
                    if key.address() != plan.input.key.address() {
                        return Err("native Link executor key changed".into());
                    }
                    plan.input.executor.link(
                        &mut link,
                        LinkExecution {
                            key,
                            inputs,
                            root: &root,
                            ready: &readies,
                            objects,
                        },
                    )
                },
            )))
        };
        compute().map_err(|e| self.failure(&e))
    }
    fn failure(&self, message: &str) -> Reference {
        self.failure(message)
    }
}

//! Descriptor-only root publication and linker assembly over actual C outputs.
#[path = "cpu.rs"]
mod cpu;

use super::{
    instance::STATICS_FINALIZERS_DOMAIN,
    local::{FinalizerRows, StaticRows, UnitEmissionContext, read_statics},
    products::{DemandProducts, NativeSnapshot},
    ready::UNIT_SUMMARY_DOMAIN,
    *,
};
use crate::native::root_source::NativeLinkSource;
use hirsel_engine::store::{Action, Domain};
use hirsel_lir::{fragment::UnitSummary, products::Binding, validate::NativeEnvironment};
use std::collections::{BTreeMap, BTreeSet};

const ROOT_RECORD: Domain = Domain {
    kind: ROOT_KIND,
    schema: 1,
};
const STATIC_MANIFEST: Domain = Domain {
    kind: 0x4e00_0019,
    schema: 1,
};
const LAUNCH_MANIFEST: Domain = Domain {
    kind: 0x4e00_001a,
    schema: 4,
};

/// Explicit platform linker inputs for a selected root. The object query owns
/// their frozen archive/foreign/tool identities in its complete action.
pub struct RootLinkInputs<'a> {
    pub entry: &'a hirsel_syntax::DefPath,
    pub backend: crate::native::Backend,
    pub objects: &'a [hirsel_lir::stage::ObjectArtifact],
    pub toolchain: &'a crate::native::Toolchain,
    pub directory: &'a std::path::Path,
}

/// Selected inputs for body-free executable planning with a frozen linker.
pub struct RootExecutableInputs<'a> {
    pub entry: &'a hirsel_syntax::DefPath,
    pub backend: crate::native::Backend,
    pub objects: &'a [hirsel_lir::stage::ObjectArtifact],
    pub directory: &'a std::path::Path,
    pub requested: Option<&'a hirsel_run_config::ValidatedRun>,
    pub standalone: bool,
    pub platform: crate::native::ExecutableLinkInputs<'a>,
}

/// Executable planning with a fully captured runtime/foreign file namespace.
pub struct RootCapturedExecutableInputs<'a> {
    pub entry: &'a hirsel_syntax::DefPath,
    pub backend: crate::native::Backend,
    pub objects: &'a [hirsel_lir::stage::ObjectArtifact],
    pub directory: &'a std::path::Path,
    pub requested: Option<&'a hirsel_run_config::ValidatedRun>,
    pub standalone: bool,
    pub platform: crate::native::CapturedExecutableLinkInputs<'a>,
}

/// Executable planning from a kit snapshot selected before codegen.
pub struct RootFrozenExecutableInputs<'a> {
    pub entry: &'a hirsel_syntax::DefPath,
    pub backend: crate::native::Backend,
    pub objects: &'a [hirsel_lir::stage::ObjectArtifact],
    pub directory: &'a std::path::Path,
    pub requested: Option<&'a hirsel_run_config::ValidatedRun>,
    pub standalone: bool,
    pub platform: crate::native::FrozenExecutableLinkInputs<'a>,
}

/// Task-local root view. Source metadata and completed native inputs are owned
/// immutable snapshots; construction runs no instance/Ready/dependency query.
pub struct RootProducts {
    inner: DemandProducts,
    source: Arc<NativeLinkSource>,
    key: NativeRootKey,
    selected: Option<Arc<NativeRootProduct>>,
}
impl RootProducts {
    pub fn new(
        snapshot: Arc<NativeSnapshot>,
        source: Arc<NativeLinkSource>,
        key: NativeRootKey,
    ) -> Result<Self, String> {
        if key.roots.windows(2).any(|w| w[0] >= w[1])
            || key.target_abi != snapshot.options.target.digest()
            || source.input() != &snapshot.emission.input
            || snapshot
                .owners
                .values()
                .any(|owner| owner.policy != key.policy)
            || key
                .roots
                .iter()
                .any(|path| !source.semantic().semantic_contains(path))
        {
            return Err("native root differs from the admitted source/request snapshot".into());
        }
        if crate::native::foreign::dispatch(&source.view()) != snapshot.emission.foreign {
            return Err("native root foreign dispatch differs from codegen inputs".into());
        }
        Ok(Self {
            inner: DemandProducts::new(snapshot),
            source,
            key,
            selected: None,
        })
    }
    /// Restore only an actual root continuation's completed descriptor scope.
    /// Ordinary result-store bytes confer no native acceptance; every supplied
    /// Ready dependency is independently re-admitted through this request's C.
    pub(super) fn settled(
        snapshot: Arc<NativeSnapshot>,
        source: Arc<NativeLinkSource>,
        key: NativeRootKey,
        root: Arc<NativeRootProduct>,
        reads: &[ProductRef],
    ) -> Result<Self, String> {
        let mut view = Self::new(snapshot.clone(), source, key)?;
        view.selected = Some(root);
        view.admitted_root(view.selected.as_ref().unwrap())?;
        for read in reads {
            let owner = snapshot
                .owners
                .get(&read.address.0)
                .ok_or("unknown settled link Ready")?;
            let current = view
                .inner
                .local
                .queries
                .lookup(
                    owner,
                    read.action,
                    &NativeEnvironment {
                        operations: &snapshot
                            .instances
                            .operations(owner, &snapshot.environment())?,
                        ..snapshot.environment()
                    },
                )
                .ok_or("settled link Ready lost current C admission")?;
            if current.product != *read
                || view.inner.selected.insert(read.address.0, *read).is_some()
            {
                return Err("duplicate or changed settled link Ready".into());
            }
        }
        Ok(view)
    }

    /// Complete the ordinary object query's Link miss: emit program glue from
    /// selected metadata, then invoke the existing semantic linker/launcher.
    /// No Value or Ready body is opened by either assembly or glue emission.
    pub fn link(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        inputs: RootLinkInputs<'_>,
    ) -> Result<Vec<u8>, String> {
        self.link_with(
            root,
            products,
            inputs.backend,
            inputs.objects,
            |program, objects| {
                crate::native::link_objects(
                    program,
                    inputs.entry,
                    objects,
                    inputs.toolchain,
                    inputs.directory,
                )
                .map_err(|e| e.to_string())
            },
        )
    }
    /// Assemble only the admitted root's portable metadata and glue, then emit
    /// the existing launcher and shared kit invocation. The caller captures the
    /// plan with its selected frozen linker before publishing an executable.
    pub fn plan_executable(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        inputs: RootExecutableInputs<'_>,
    ) -> Result<crate::native::ExecutableLinkPlan, String> {
        self.with_link_objects(
            root,
            products,
            inputs.backend,
            inputs.objects,
            |program, objects| {
                crate::native::plan_executable_link(
                    program,
                    inputs.entry,
                    objects,
                    inputs.directory,
                    inputs.requested,
                    inputs.standalone,
                    inputs.platform,
                )
                .map_err(|error| error.to_string())
            },
        )
    }

    /// Use the exact admitted root/Ready metadata with a previously snapshotted
    /// kit. No live kit manifest or leaf is read during planning or capture.
    pub fn plan_frozen_executable(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        inputs: RootFrozenExecutableInputs<'_>,
    ) -> Result<crate::native::ExecutableLinkPlan, String> {
        self.with_link_objects(
            root,
            products,
            inputs.backend,
            inputs.objects,
            |program, objects| {
                crate::native::plan_frozen_executable_link(
                    program,
                    inputs.entry,
                    objects,
                    inputs.directory,
                    inputs.requested,
                    inputs.standalone,
                    inputs.platform,
                )
                .map_err(|error| error.to_string())
            },
        )
    }

    /// The admitted root supplies launcher/glue metadata; the captured platform
    /// supplies every runtime/foreign virtual name, byte and build identity.
    pub fn plan_captured_executable(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        inputs: RootCapturedExecutableInputs<'_>,
    ) -> Result<crate::native::object_queries::platform::CapturedPlatformPlan, String> {
        self.with_link_objects(
            root,
            products,
            inputs.backend,
            inputs.objects,
            |program, objects| {
                crate::native::plan_captured_executable_link(
                    program,
                    inputs.entry,
                    objects,
                    inputs.directory,
                    inputs.requested,
                    inputs.standalone,
                    inputs.platform,
                )
                .map_err(|error| error.to_string())
            },
        )
    }

    /// Captured package-test link from this root's exact bound entry table.
    pub fn plan_captured_tests(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        inputs: RootCapturedExecutableInputs<'_>,
    ) -> Result<crate::native::object_queries::platform::CapturedPlatformPlan, String> {
        let tests = self.test_entries(root)?;
        self.with_link_objects(
            root,
            products,
            inputs.backend,
            inputs.objects,
            |program, objects| {
                crate::native::link::plan_captured_test_link(
                    program,
                    &tests,
                    objects,
                    inputs.directory,
                    inputs.platform,
                )
                .map_err(|error| error.to_string())
            },
        )
    }

    fn link_with(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        backend: crate::native::Backend,
        objects: &[hirsel_lir::stage::ObjectArtifact],
        link: impl FnOnce(
            &crate::native::ReadyProgram,
            &[hirsel_lir::stage::ObjectArtifact],
        ) -> Result<std::path::PathBuf, String>,
    ) -> Result<Vec<u8>, String> {
        self.with_link_objects(root, products, backend, objects, |program, objects| {
            let path = link(program, objects)?;
            std::fs::read(path).map_err(|e| e.to_string())
        })
    }

    /// Assemble existing program glue and optional current source debug data
    /// around already emitted fragments. No Ready body is restored here.
    pub fn backend_objects(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        backend: crate::native::Backend,
        objects: &[hirsel_lir::stage::ObjectArtifact],
    ) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, String> {
        self.with_link_objects(root, products, backend, objects, |_, objects| {
            Ok(objects.to_vec())
        })
    }

    fn with_link_objects<R>(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        backend: crate::native::Backend,
        objects: &[hirsel_lir::stage::ObjectArtifact],
        use_objects: impl FnOnce(
            &crate::native::ReadyProgram,
            &[hirsel_lir::stage::ObjectArtifact],
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        let sources = self
            .inner
            .snapshot
            .emission
            .debug
            .then(|| self.source.profiles());
        let opt = self.inner.snapshot.emission.opt;
        self.with_program_context(root, products, Some(objects), |program| {
            crate::native::check_runtime_calls(program).map_err(|e| e.to_string())?;
            let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &program.table,
                &program.finalizers,
            );
            let mut objects = objects.to_vec();
            cpu::emit(program, backend, &mut objects, opt)?;
            objects.push(
                crate::native::emit_glue(program, backend, &registry).map_err(|e| e.to_string())?,
            );
            if let Some(sources) = &sources {
                objects.push(
                    crate::native::link::debug_artifact(program, &objects, sources)
                        .map_err(|error| error.to_string())?,
                );
            }
            use_objects(program, &objects)
        })
    }

    /// A new task-local linker view of already selected outputs. Cloning this
    /// bounded descriptor scope executes no dependencies or transformations.
    pub fn link_view(&self) -> Result<Self, String> {
        let root = self
            .selected
            .as_ref()
            .ok_or("native root has not been selected")?;
        self.admitted_root(root)?;
        let mut inner = DemandProducts::new(self.inner.snapshot.clone());
        inner.selected = self.inner.selected.clone();
        Ok(Self {
            inner,
            source: self.source.clone(),
            key: self.key.clone(),
            selected: self.selected.clone(),
        })
    }

    /// Sole root-owned debug projection, available only after this exact
    /// current root has been admitted. Callers never supply a second map.
    pub fn profile_sources(
        &self,
        root: &Arc<NativeRootProduct>,
    ) -> Result<Arc<crate::native::ProfileSources>, String> {
        self.admitted_root(root)?;
        Ok(self.source.profiles())
    }
    /// Host launch input selected by this admitted root, never an independent
    /// record supplied to the linker after its action has been chosen.
    pub fn requested_run(
        &self,
        root: &Arc<NativeRootProduct>,
    ) -> Result<Option<(hirsel_syntax::DefPath, Arc<hirsel_run_config::ValidatedRun>)>, String>
    {
        self.admitted_root(root)?;
        Ok(self.source.requested_run())
    }
    pub fn test_entries(
        &self,
        root: &Arc<NativeRootProduct>,
    ) -> Result<Vec<(hirsel_syntax::DefPath, u64)>, String> {
        self.admitted_root(root)?;
        Ok(self.source.test_entries().to_vec())
    }
    /// Diagnostic tables for this exact admitted root and its explicit Ready
    /// dependency scope. Only portable summaries are opened; no ReadyProgram,
    /// layout binding, native body or source-engine query is constructed.
    pub fn observation_metadata(
        &mut self,
        root: &Arc<NativeRootProduct>,
    ) -> Result<crate::requests::NativeObservation, String> {
        self.admitted_root(root)?;
        let snapshot = &self.inner.snapshot;
        let environment = snapshot.environment();
        let mut observations = Vec::with_capacity(root.instances.len());
        for owner in &root.instances {
            let operations = snapshot.instances.operations(owner, &environment)?;
            let scoped = NativeEnvironment {
                operations: &operations,
                ..environment
            };
            let selected = self
                .inner
                .selected
                .get(&owner.ready_query_id().address.0)
                .ok_or("observation metadata has an unread Ready dependency")?;
            let ready = self
                .inner
                .local
                .queries
                .lookup(owner, selected.action, &scoped)
                .ok_or("observation Ready dependency lost current C admission")?;
            if ready.product != *selected || ready.unit_summary.domain != UNIT_SUMMARY_DOMAIN {
                return Err("observation metadata differs from selected Ready".into());
            }
            let bytes = snapshot
                .store
                .get_content(ready.unit_summary)
                .ok_or("missing observation unit summary")?;
            let observation = hirsel_lir::fragment::UnitSummary::observation(&bytes)?;
            if observation.definition != *owner.instance.definition()
                || observation.instance != owner.instance.digest()
                || (observation.origin == hirsel_lir::ir::UnitOrigin::Source
                    && !self
                        .source
                        .semantic()
                        .semantic_contains(&observation.definition))
            {
                return Err("observation summary differs from admitted owner/source".into());
            }
            observations.push(observation);
        }
        Ok(crate::requests::NativeObservation {
            failure_rules: crate::native::emit::failure_rules_from(
                observations
                    .iter()
                    .flat_map(|unit| unit.failure_rules.iter().map(String::as_str)),
            ),
            sites: crate::native::observe::site_codes_from(
                observations.iter().map(|unit| {
                    (
                        &unit.definition,
                        &unit.implementation_revision,
                        unit.sites.as_slice(),
                    )
                }),
                Clone::clone,
            ),
            anchors: self.source.source_anchors().clone(),
        })
    }

    fn launch_bytes(&self) -> Result<Vec<u8>, String> {
        let snapshot = &self.inner.snapshot;
        serde_json::to_vec(&(
            self.source.manifest_bytes()?,
            &snapshot.options.launch,
            snapshot.emission.fuel,
            snapshot.emission.tracking,
            snapshot.context,
        ))
        .map_err(|e| e.to_string())
    }

    fn put(&self, domain: Domain, bytes: &[u8]) -> Result<Reference, String> {
        let expected = domain
            .reference(bytes)
            .ok_or("native root record exceeds limit")?;
        let actual = self
            .inner
            .snapshot
            .store
            .put_content(domain, bytes)
            .ok_or("native root publication failed")?;
        if actual != expected {
            return Err("native root content identity differs".into());
        }
        Ok(actual)
    }
    fn admitted_root(&self, root: &NativeRootProduct) -> Result<(), String> {
        let selected = self
            .selected
            .as_ref()
            .ok_or("native root has not been selected")?;
        if selected.product != root.product
            || selected.instances != root.instances
            || selected.static_manifest != root.static_manifest
            || selected.launch_manifest != root.launch_manifest
            || selected.statics_finalizers != root.statics_finalizers
        {
            return Err("link root differs from selected native output".into());
        }
        if LAUNCH_MANIFEST.reference(&self.launch_bytes()?) != Some(root.launch_manifest) {
            return Err("root source projection differs from admitted launch input".into());
        }
        let store = self.inner.snapshot.store.as_ref();
        let current = store
            .lookup_product(root.product.address)
            .ok_or("missing native root association")?;
        if current.action != root.product.action || current.reference != root.product.record {
            return Err("native root is no longer settled".into());
        }
        for reference in [
            root.product.record,
            root.static_manifest,
            root.launch_manifest,
            root.statics_finalizers,
        ] {
            if store.get_content(reference).is_none() {
                return Err("missing native root content".into());
            }
        }
        Ok(())
    }
}
impl NativeProducts for RootProducts {
    fn instance(&mut self, key: &InstanceAddress) -> Result<Arc<InstanceProduct>, String> {
        self.inner.instance(key)
    }
    fn arena_summary(&mut self, key: &ComponentKey) -> Result<Arc<ArenaSummary>, String> {
        self.inner.arena_summary(key)
    }
    fn ready(&mut self, key: &InstanceAddress) -> Result<Arc<ReadyProduct>, String> {
        self.inner.ready(key)
    }
    fn codegen_units(
        &mut self,
        key: &InstanceAddress,
        backend: &str,
    ) -> Result<Vec<Arc<ReadyProduct>>, String> {
        self.inner.codegen_units(key, backend)
    }
    fn with_ready<R>(
        &mut self,
        products: &[Arc<ReadyProduct>],
        use_units: impl FnOnce(
            &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        self.inner.with_ready(products, use_units)
    }
}
impl NativeRoots for RootProducts {
    fn root(&mut self, key: &NativeRootKey) -> Result<Arc<NativeRootProduct>, String> {
        if key != &self.key {
            return Err("unregistered native root selection".into());
        }
        let snapshot = &self.inner.snapshot;
        let mut instances = snapshot.owners.values().cloned().collect::<Vec<_>>();
        instances.sort_by(InstanceAddress::canonical_cmp);
        if instances.is_empty() {
            return Err("empty native root instance closure".into());
        }
        let mut reads = Vec::new();
        let mut statics = Vec::new();
        for key in &instances {
            let (instance, _) = snapshot.instances.admitted(key, &snapshot.environment())?;
            reads.push(instance.product.product.record);
            // Static data/finalizer selection is a dependency even if a changed
            // static leaves the Ready unit and unit object byte-for-byte equal.
            if snapshot
                .store
                .get_content(instance.product.statics_finalizers)
                .is_none()
            {
                return Err("missing root static/finalizer input".into());
            }
            statics.push(instance.product.statics_finalizers);
        }
        let static_manifest = self.put(
            STATIC_MANIFEST,
            &serde_json::to_vec(&statics).map_err(|e| e.to_string())?,
        )?;
        let launch_manifest = self.put(LAUNCH_MANIFEST, &self.launch_bytes()?)?;
        let address = key.query_id()?.address;
        let action_bytes = serde_json::to_vec(&(
            crate::COMPILER_IDENTITY.as_slice(),
            &reads,
            static_manifest,
            launch_manifest,
        ))
        .map_err(|e| e.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.root-action.v1",
            [action_bytes.as_slice()],
        ));
        let members = instances
            .iter()
            .map(|key| {
                (
                    key.instance.definition(),
                    key.instance.body(),
                    key.instance.encoding(),
                    key.target_abi,
                    key.policy,
                )
            })
            .collect::<Vec<_>>();
        let record = self.put(
            ROOT_RECORD,
            &serde_json::to_vec(&(members, static_manifest, launch_manifest))
                .map_err(|e| e.to_string())?,
        )?;
        snapshot.store.associate_product(address, action, record);
        let product = Arc::new(NativeRootProduct {
            product: ProductRef {
                address,
                action,
                record,
            },
            instances,
            static_manifest,
            launch_manifest,
            statics_finalizers: static_manifest,
        });
        self.selected = Some(product.clone());
        self.admitted_root(&product)?;
        Ok(product)
    }

    fn with_link_context<R>(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        use_context: impl FnOnce(&crate::native::ReadyProgram) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_program_context(root, products, None, use_context)
    }
}

impl RootProducts {
    fn with_program_context<R>(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        cpu_objects: Option<&[hirsel_lir::stage::ObjectArtifact]>,
        use_context: impl FnOnce(&crate::native::ReadyProgram) -> Result<R, String>,
    ) -> Result<R, String> {
        self.admitted_root(root)?;
        let snapshot = self.inner.snapshot.clone();
        if products.len() != root.instances.len() || !snapshot.emission.inline_instances.is_empty()
        {
            return Err("link selection or optimizer dependency closure is incomplete".into());
        }
        let mut selected = BTreeMap::new();
        for product in products {
            if !product.optimizer_reads.is_empty()
                || self.inner.selected.get(&product.product.address.0) != Some(&product.product)
                || selected
                    .insert(product.product.address.0, product)
                    .is_some()
            {
                return Err("unread, duplicate or unsupported optimized link dependency".into());
            }
        }
        let environment = snapshot.environment();
        let mut captured = Vec::new();
        let mut linkage = Vec::new();
        let mut contexts = Vec::new();
        for key in &root.instances {
            let product = *selected
                .get(&key.address(READY_KIND).0)
                .ok_or("missing selected link Ready")?;
            let (base, _) = snapshot.instances.admitted(key, &environment)?;
            let operations = snapshot.instances.operations(key, &environment)?;
            let scoped = NativeEnvironment {
                operations: &operations,
                ..environment
            };
            let checked = self
                .inner
                .local
                .queries
                .linkage(key, product.product.action, &scoped)
                .ok_or("link dependency lost C linkage admission")?;
            let closure = self
                .inner
                .local
                .queries
                .assembly_layouts(key, product, &scoped)?;
            let bytes = snapshot
                .store
                .get_content(product.emission_context)
                .ok_or("missing link emission context")?;
            let context: UnitEmissionContext =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if context.request != snapshot.context
                || context.statics_finalizers != base.product.statics_finalizers
            {
                return Err("link emission context differs from admitted instance".into());
            }
            contexts.push(context);
            captured.push((product, closure));
            linkage.push(checked);
        }
        hirsel_lir::validate::check_arena_scoped_links(&linkage)?;
        let binding = Binding::new(
            environment.target,
            &captured.iter().map(|(_, c)| c).collect::<Vec<_>>(),
        )?;
        let keys = hirsel_lir::LayoutKeys::new(&binding.table);
        let mut summaries = Vec::new();
        let mut semantic_owners = BTreeMap::new();
        let mut functions = BTreeMap::new();
        for (((product, _), checked), key) in captured.iter().zip(&linkage).zip(&root.instances) {
            if product.unit_summary.domain != UNIT_SUMMARY_DOMAIN {
                return Err("wrong link summary domain".into());
            }
            let bytes = snapshot
                .store
                .get_content(product.unit_summary)
                .ok_or("missing link summary")?;
            let summary = UnitSummary::decode(&bytes, &|key| binding.layout(key.0), &|key| {
                binding.abi(*key)
            })?;
            if summary.definition != *key.instance.definition()
                || summary.instance != key.instance.digest()
                || keys.fn_abi_of(&binding.table, summary.abi) != Some(checked.abi())
            {
                return Err("link summary differs from C-admitted owner or ABI".into());
            }
            let source_owner = match summary.origin {
                hirsel_lir::ir::UnitOrigin::Source => Some(summary.definition.clone()),
                hirsel_lir::ir::UnitOrigin::Library => None,
            };
            if let Some(previous) =
                semantic_owners.insert(summary.definition.clone(), source_owner.clone())
                && previous != source_owner
            {
                return Err("mixed native semantic owner origins".into());
            }
            if self.source.functions().get(key.instance.definition()) == Some(&key.instance.body())
                && functions
                    .insert(summary.definition.clone(), summary.instance)
                    .is_some()
            {
                return Err("duplicate nongeneric native entry".into());
            }
            summaries.push(summary);
        }
        let mut statics = BTreeMap::new();
        let mut finalizers = BTreeMap::new();
        for context in contexts {
            for data in read_statics(
                snapshot.store.as_ref(),
                context.statics_finalizers,
                &binding,
            )? {
                let name = hirsel_codec::to_text(&data.symbol);
                if let Some(previous) = statics.insert(name, data.clone())
                    && previous != data
                {
                    return Err("conflicting linked static data".into());
                }
            }
            if context.statics_finalizers.domain != STATICS_FINALIZERS_DOMAIN {
                return Err("wrong link finalizer domain".into());
            }
            let bytes = snapshot
                .store
                .get_content(context.statics_finalizers)
                .ok_or("missing link finalizers")?;
            let (_, rows): (StaticRows, FinalizerRows) =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if rows.windows(2).any(|w| w[0] >= w[1]) {
                return Err("noncanonical linked finalizers".into());
            }
            for (layout, definition, instance, abi, library) in rows {
                let checked = linkage
                    .iter()
                    .find(|c| c.definition() == &definition && c.instance() == instance)
                    .ok_or("linked finalizer has no C-admitted instance")?;
                let summary = summaries
                    .iter()
                    .find(|s| s.definition == definition && s.instance == instance)
                    .ok_or("missing linked finalizer summary")?;
                if checked.abi() != abi
                    || (summary.origin == hirsel_lir::ir::UnitOrigin::Library) != library
                {
                    return Err("linked finalizer ABI or origin differs".into());
                }
                let layout = binding
                    .layout(layout)
                    .ok_or("missing linked finalizer layout")?;
                if let Some(previous) = finalizers.insert(layout, instance)
                    && previous != instance
                {
                    return Err("conflicting linked finalizer instances".into());
                }
            }
        }
        // Root metadata retains the complete independently admitted inventory.
        // Body-free assembly never reconstructs a Value or trusts unit anchors
        // to select source identities; selectors are rebound by full ABI content.
        let mut operations = self.source.operations().clone();
        operations.pretransfer_adapters = super::instance_operations::bind_pretransfer(
            environment.operations,
            &binding.table,
            environment.target,
        )?;
        operations.invocation_bodies.clear();
        for owner in &root.instances {
            let definition = owner.instance.definition();
            let instance = owner.instance.digest();
            if let Some(body) = environment.operations.invocation_body(definition, instance) {
                operations
                    .invocation_bodies
                    .push(hirsel_lir::invocations::ReferenceBody {
                        definition: definition.clone(),
                        instance,
                        body,
                    });
                if let Some(rows) = environment.operations.invocations(definition) {
                    operations
                        .invocations
                        .insert(definition.clone(), rows.to_vec());
                }
            }
        }
        let cpu_units = if let Some(objects) = cpu_objects {
            cpu::materialize(
                self,
                root,
                &selected,
                &summaries,
                &binding,
                &operations,
                objects,
            )?
        } else {
            Vec::new()
        };
        with_link_assembly_units(
            &snapshot,
            &self.source,
            LinkAssembly {
                operations,
                semantic_owners,
                summaries,
                functions,
                finalizers,
                table: binding.table,
                keys,
                statics: statics.into_values().collect(),
            },
            cpu_units,
            use_context,
        )
    }
}

/// Shared body-free linker projection. Both ordinary and ownership consumers
/// supply only summaries/layouts derived from their own exact current C scope.
pub(super) struct LinkAssembly {
    pub operations: hirsel_layout::operation::OperationRegistry,
    pub semantic_owners: BTreeMap<hirsel_syntax::DefPath, Option<hirsel_syntax::DefPath>>,
    pub summaries: Vec<UnitSummary>,
    pub functions: BTreeMap<hirsel_syntax::DefPath, ContentHash>,
    pub finalizers: BTreeMap<hirsel_lir::LayoutId, ContentHash>,
    pub table: hirsel_lir::LayoutTable,
    pub keys: hirsel_lir::LayoutKeys,
    pub statics: Vec<hirsel_lir::statics::StaticData>,
}
pub(super) fn with_link_assembly<R>(
    snapshot: &NativeSnapshot,
    source: &NativeLinkSource,
    assembly: LinkAssembly,
    use_context: impl FnOnce(&crate::native::ReadyProgram) -> Result<R, String>,
) -> Result<R, String> {
    with_link_assembly_units(snapshot, source, assembly, Vec::new(), use_context)
}

fn with_link_assembly_units<R>(
    snapshot: &NativeSnapshot,
    source: &NativeLinkSource,
    assembly: LinkAssembly,
    units: Vec<hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>>,
    use_context: impl FnOnce(&crate::native::ReadyProgram) -> Result<R, String>,
) -> Result<R, String> {
    let LinkAssembly {
        operations,
        semantic_owners,
        summaries,
        functions,
        finalizers,
        table,
        keys,
        statics,
    } = assembly;
    let callbacks = summaries
        .iter()
        .flat_map(|s| s.glue.callbacks.iter().map(|(path, _)| path.clone()))
        .collect();
    let entries = summaries
        .iter()
        .flat_map(|s| {
            s.glue.entries.iter().map(|(path, instance)| {
                (
                    hirsel_lir::emission::function_thunk_name(path, instance.as_ref()),
                    path.to_string(),
                )
            })
        })
        .collect();
    let mut program = crate::native::ReadyProgram {
        semantic_owners,
        semantic_units: None,
        validation: snapshot.options.validation,
        operations,
        launch: snapshot.options.launch.clone(),
        input: snapshot.emission.input.clone(),
        target: snapshot.options.target.clone(),
        table,
        keys,
        manifest: snapshot.manifest.clone(),
        ahead_environments: hirsel_lir::ahead::environments(
            &units.iter().map(|u| u.unit().clone()).collect::<Vec<_>>(),
        ),
        units,
        summaries,
        inline_instances: BTreeSet::new(),
        callbacks,
        entries,
        callee_facts: None,
        finalizers,
        functions,
        launches: source.launches(),
        revisions: BTreeMap::new(),
        exit_reports: BTreeMap::new(),
        statics,
        discharged: snapshot.options.discharge.clone(),
        foreign_links: vec![],
        data_abi: None,
        foreign_unreplayable: false,
        foreign_dispatch: BTreeMap::new(),
        escaping: BTreeSet::new(),
        fuel: snapshot.emission.fuel,
        failure_table: None,
    };
    source.bind(&mut program).map_err(|e| e.to_string())?;
    use_context(&program)
}

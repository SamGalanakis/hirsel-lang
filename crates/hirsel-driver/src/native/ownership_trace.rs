//! Explicit Ready-local ownership diagnostics. This staged API refuses every
//! unsupported unit before emitting any object; ordinary emission is unchanged.
//! Earlier rewriting-pass recipes remain a separate qualification obligation.
use super::{Backend, NativeError, OptLevel, ReadyProgram, Sites, StageName};
use hirsel_lir::emission::{EmitInputs, site_code};
use hirsel_lir::ir::{ConstructorKind, Inst};
use hirsel_lir::ownership_trace::native::LeafPlan;
use hirsel_lir::stage::ObjectArtifact;
use hirsel_run_schema::ownership_trace::native::{DropMember, LocalCalls, Metadata, Units};
use hirsel_syntax::DefPath;
use std::collections::{BTreeMap, BTreeSet};

/// Objects and immutable row expectations derived from the checked program,
/// independently of any runtime log. This is the conservative leaf adapter.
pub struct LeafObjects {
    objects: Vec<ObjectArtifact>,
    units: Units,
    program: [u8; 32],
    entry: DefPath,
    calls: LocalCalls,
}
fn name(path: &DefPath) -> String {
    format!(
        "{}@{}::{}",
        path.0.package.name,
        path.0.package.version,
        path.0
            .modules
            .iter()
            .chain(std::iter::once(&path.0.name))
            .cloned()
            .collect::<Vec<_>>()
            .join(".")
    )
}

/// Emit explicitly selected diagnostic objects and their separately checked
/// static expectations. Missing Ready bodies and unsupported transport fail
/// closed; ordinary cached objects cannot fill an unobserved diagnostic unit.
pub fn emit_leaf(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
    backend: Backend,
    opt: OptLevel,
) -> Result<LeafObjects, NativeError> {
    fn failed(message: impl Into<String>) -> NativeError {
        NativeError::new(StageName::Emission, message)
    }
    if program.units.len() != program.summaries.len() {
        return Err(failed(
            "ownership emission requires all checked Ready bodies",
        ));
    }
    let selected: Vec<_> = program.units.iter().collect();
    let plans = selected
        .iter()
        .map(|unit| {
            LeafPlan::prepare(unit, &program.table)
                .map(|plan| (unit.unit().digest(), plan.plan().clone()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(failed)?;
    let empty = BTreeSet::new();
    let inputs = EmitInputs {
        ahead: None,
        continuation: None,
        alternate: false,
        index: None,
        verify_backend: program.validation == super::Validation::EveryStage,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: &empty,
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt,
        measurement: false,
        fuel: program.fuel,
    };
    emit_relocated(
        program,
        entry,
        &selected,
        &inputs,
        hirsel_lir::ownership_trace::native::Request::Relocated(&plans),
        sites,
        backend,
    )
}

/// Complete checked diagnostic closure with the original transported plans.
/// The source-bound linker projection owns the semantic/runtime metadata.
pub(crate) fn emit_relocated(
    program: &ReadyProgram,
    entry: &DefPath,
    selected: &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
    inputs: &EmitInputs<'_>,
    request: hirsel_lir::ownership_trace::native::Request<'_>,
    sites: &Sites<'_>,
    backend: Backend,
) -> Result<LeafObjects, NativeError> {
    fn failed(message: impl Into<String>) -> NativeError {
        NativeError::new(StageName::Emission, message)
    }
    let (plans, resources) = match request {
        hirsel_lir::ownership_trace::native::Request::Relocated(plans) => (plans, false),
        hirsel_lir::ownership_trace::native::Request::Resources(plans) => (plans, true),
        hirsel_lir::ownership_trace::native::Request::Leaf => {
            return Err(failed("original plans required for relocated emission"));
        }
    };
    let prepared = request.prepare(selected, inputs.layouts).map_err(failed)?;
    if selected.len() != program.summaries.len() {
        return Err(failed(
            "ownership emission requires the complete checked closure",
        ));
    }
    let revision = program
        .revision(entry)
        .ok_or_else(|| failed("ownership emission has no checked semantic entry binding"))?
        .digest();
    let call_sites =
        hirsel_lir::ownership_trace::native::local_call_sites(selected).map_err(failed)?;
    // Runtime frames include implicit finalizers as well as ordinary calls.
    // Source classification cannot remove a physical target from decoding.
    let calls = hirsel_lir::ownership_trace::native::physical_calls(selected).map_err(failed)?;
    let mut units = BTreeMap::new();
    for unit in selected {
        let original = plans
            .get(&unit.unit().digest())
            .ok_or_else(|| failed("missing original ownership plan"))?;
        let plan = prepared
            .get(&unit.unit().header.instance)
            .ok_or_else(|| failed("missing checked ownership emission plan"))?;
        let body = unit.unit();
        let mut types = BTreeMap::new();
        for inst in &body.instructions {
            if let Inst::Construct {
                result, construct, ..
            } = inst
            {
                let symbol = match body.tables.constructs[construct.index()].constructor {
                    ConstructorKind::Record(symbol)
                    | ConstructorKind::Variant {
                        definition: symbol, ..
                    } => Some(symbol),
                    _ => None,
                };
                if let Some(symbol) = symbol {
                    types.insert(
                        body.types[body.values[result.index()].ty.index()].layout,
                        name(&body.tables.symbols[symbol.index()].definition),
                    );
                }
            }
        }
        let mut markers: BTreeMap<u32, Metadata> = BTreeMap::new();
        for marker in plan.markers() {
            let definition = program
                .semantic_owners
                .get(&body.header.definition)
                .and_then(Option::as_ref)
                .unwrap_or(&body.header.definition);
            let implicit = plan.implicit_parameter_cleanup_source(body, marker.id);
            let projected = implicit.map_or(marker.site, |(site, _)| site);
            // Library bodies have checked recipe/parameter ordinals, not a
            // spelled HIR file. Their independently planned runtime-bundle
            // binding must remain explicit rather than borrowing caller spans.
            let mut source = if body.header.origin == hirsel_lir::ir::UnitOrigin::Library
                && matches!(
                    program.semantic_owners.get(&body.header.definition),
                    Some(None)
                ) {
                hirsel_run_schema::Site {
                    definition: name(definition),
                    file: format!("<native-library:{}>", name(&body.header.definition)),
                    line: 0,
                    column: 0,
                }
            } else {
                let failure = sites.failure("ownership", definition, projected)?;
                sites
                    .failure_site(&failure)
                    .ok_or_else(|| failed("ownership marker has no source projection"))?
            };
            source.definition = match implicit {
                Some((_, parameter)) => format!(
                    "{} [implicit parameter {parameter} cleanup]",
                    name(definition)
                ),
                None => name(definition),
            };
            let location = |at: hirsel_lir::ownership_trace::native::NativeLocation| {
                (u64::from(at.kind) << 32) | u64::from(at.index)
            };
            let projected = |at| -> Result<_, NativeError> {
                Ok(hirsel_lir::ownership_trace::native::projected_location(
                    body,
                    inputs.layouts,
                    at,
                )
                .map_err(failed)?
                .map(|path| {
                    hirsel_run_schema::ownership_trace::native::LocationPath {
                        root: location(path.root),
                        fields: path.fields.into_iter().map(|part| match part {
                            hirsel_lir::ownership_trace::native::NativeProjection::Field(field) =>
                                hirsel_run_schema::ownership_trace::Projection::Field { field },
                            hirsel_lir::ownership_trace::native::NativeProjection::VariantField { variant, field } =>
                                hirsel_run_schema::ownership_trace::Projection::VariantField { variant, field },
                        }).collect(),
                    }
                }))
            };
            let metadata = Metadata {
                action: marker.kind,
                optional_carrier: plan.optional_carrier(inputs.layouts, marker.id),
                empty_environment: plan
                    .empty_environment(body, inputs.layouts, marker.id)
                    .map_err(failed)?,
                callee: if matches!(marker.kind, 3 | 5 | 6) {
                    let row = original
                        .observations
                        .iter()
                        .find(|row| row.marker.id == marker.id)
                        .ok_or_else(|| failed("Alias lost original call binding"))?;
                    let hirsel_lir::ownership_trace::Point::CallEntered(block) = row.point else {
                        return Err(failed("Alias is not an entered call"));
                    };
                    let target = call_sites
                        .get(&body.digest().0)
                        .and_then(|sites| sites.get(&block))
                        .copied();
                    if target.is_none()
                        && !plan
                            .empty_environment(body, inputs.layouts, marker.id)
                            .map_err(failed)?
                    {
                        return Err(failed("Alias has no exact checked callee"));
                    }
                    target
                } else {
                    None
                },
                cleanup: if marker.kind == 4 {
                    Some(
                        plan.cleanup_captures(body, inputs.layouts, marker.id)
                            .map_err(failed)?
                            .1
                            .into_iter()
                            .map(|capture| (capture.field, capture.layout.0))
                            .collect(),
                    )
                } else {
                    None
                },
                cleanup_paths: if marker.kind == 4 {
                    plan.cleanup_captures(body, inputs.layouts, marker.id)
                        .map_err(failed)?
                        .1
                        .into_iter()
                        .map(|capture| (capture.field, capture.path))
                        .collect()
                } else {
                    BTreeMap::new()
                },
                additional_drops: Vec::new(),
                token: location(marker.token),
                owner: location(marker.owner),
                token_path: projected(marker.token)?,
                owner_path: projected(marker.owner)?,
                site: site_code(
                    &body.header.definition,
                    &body.header.implementation_revision,
                    marker.site,
                ),
                layout: marker.layout.0,
                source,
                value_type: types
                    .get(&marker.layout)
                    .cloned()
                    .unwrap_or_else(|| format!("layout#{}", marker.layout.0)),
            };
            if let Some(prior) = markers.get_mut(&marker.id.0) {
                if prior.action != 2
                    || metadata.action != 2
                    || prior.token != prior.owner
                    || metadata.token != metadata.owner
                    || prior.token_path != prior.owner_path
                    || metadata.token_path != metadata.owner_path
                    || prior.site != metadata.site
                    || prior.source != metadata.source
                {
                    return Err(failed("ownership marker has inconsistent compound members"));
                }
                prior.additional_drops.push(DropMember {
                    location: metadata.token,
                    path: metadata.token_path,
                    layout: metadata.layout,
                    value_type: metadata.value_type,
                });
            } else {
                markers.insert(marker.id.0, metadata);
            }
        }
        if units.insert(plan.plan().unit.0, markers).is_some() {
            return Err(failed(
                "ownership emission contains duplicate unit identities",
            ));
        }
    }
    super::check_runtime_calls(program)?;
    let object = match backend {
        Backend::Llvm if resources => {
            hirsel_llvm::emit_object_resources_ownership(selected, inputs, plans)
                .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?
        }
        Backend::Cranelift if resources => {
            hirsel_cranelift::emit_object_resources_ownership(selected, inputs, plans)
                .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?
        }
        Backend::Llvm => hirsel_llvm::emit_object_relocated_ownership(selected, inputs, plans)
            .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?,
        Backend::Cranelift => {
            hirsel_cranelift::emit_object_relocated_ownership(selected, inputs, plans)
                .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?
        }
    };
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    let glue = super::emit_glue(program, backend, &registry)?;
    Ok(LeafObjects {
        objects: vec![object, glue],
        // Bulk construction packs the immutable marker trees. No insert or
        // removal occurs after this point, so their retained node bound is
        // the bulk builder's shape rather than arbitrary editing occupancy.
        units: units
            .into_iter()
            .map(|(unit, markers)| (unit, markers.into_iter().collect()))
            .collect(),
        program: revision,
        entry: entry.clone(),
        calls,
    })
}

/// The native language outcome remains available even if the diagnostic log
/// is missing, truncated, overflowed or inconsistent with checked metadata.
pub struct LeafExecution {
    pub execution: super::Launched,
    pub program: [u8; 32],
    pub trace: Result<hirsel_run_schema::ownership_trace::Trace, String>,
}
// Rust alloc bulk_push fills nodes with eleven keys before opening the
// next subtree; fix_right_border_of_plentiful only redistributes keys. Thus
// level counts are floor(n/12)+1, floor(n/144)+1, ... through the root.
// rust-lang/rust ea137335b, alloc/collections/btree/append.rs:8-68,
// map.rs:2557-2572 and fix.rs:102-118. Nodes use the same conservative field
// alignment bound as NativeMetadataWeight::tree, including internal edges.
// Only the freshly bulk-collected, private immutable marker maps use this.
fn packed_marker_bytes(w: &mut hirsel_engine::NativeMetadataWeight, len: usize) {
    use std::alloc::Layout;
    if len == 0 {
        return;
    }
    let fields = [
        Layout::new::<usize>(),
        Layout::new::<u16>(),
        Layout::new::<u16>(),
        Layout::new::<[u32; 11]>(),
        Layout::new::<[Metadata; 11]>(),
        Layout::new::<[usize; 12]>(),
    ];
    let align = fields.iter().map(Layout::align).max().unwrap();
    let bytes = fields.iter().fold(align - 1, |n, field| {
        n.saturating_add(field.size())
            .saturating_add(field.align() - 1)
    });
    let mut keys = len;
    let mut nodes = 0usize;
    loop {
        keys /= 12;
        nodes = nodes.saturating_add(keys.saturating_add(1));
        if keys == 0 {
            break;
        }
    }
    w.add(nodes.saturating_mul(bytes));
}

impl LeafObjects {
    pub(crate) fn retained_bytes(&self) -> u64 {
        use hirsel_engine::NativeMetadataWeight as Weight;
        let mut w = Weight::default();
        w.add(std::mem::size_of::<Self>());
        w.path(&self.entry);
        w.vec(&self.objects, |w, object| {
            let ObjectArtifact {
                ahead,
                target: _,
                format: _,
                bytes,
                exports,
                imports,
                relocations,
                abi: _,
                reproducibility,
                debug,
            } = object;
            w.flat_vec(bytes);
            let mut symbol = |w: &mut Weight, symbol: &hirsel_lir::stage::ObjectSymbol| {
                w.add(symbol.name.capacity());
                if let Some(path) = &symbol.definition {
                    w.path(path);
                }
            };
            w.vec(exports, &mut symbol);
            w.vec(imports, &mut symbol);
            w.flat_vec(relocations);
            w.add(reproducibility.toolchain.capacity());
            w.vec(debug, |w, row| {
                w.add(row.symbol.capacity());
                w.vec(&row.items, |w, path| w.path(path));
                w.flat_vec(&row.sources);
                w.flat_vec(&row.owner_kinds);
                w.flat_vec(&row.ranges);
            });
            if let Some(ahead) = ahead {
                w.flat_vec(&ahead.frames);
                w.flat_vec(&ahead.bounds);
            }
        });
        w.map(&self.units, |w, _, markers| {
            packed_marker_bytes(w, markers.len());
            for marker in markers.values() {
                w.add(marker.value_type.capacity());
                w.add(marker.source.definition.capacity());
                w.add(marker.source.file.capacity());
                for path in [&marker.token_path, &marker.owner_path]
                    .into_iter()
                    .flatten()
                {
                    w.flat_vec(&path.fields);
                }
                if let Some(fields) = &marker.cleanup {
                    w.flat_vec(fields);
                }
                // These immutable maps are built once from the checked plan.
                // An empty map has never allocated a node; unlike a mutable
                // tree, it cannot retain a formerly populated empty root.
                if !marker.cleanup_paths.is_empty() {
                    w.map(&marker.cleanup_paths, |w, _, path| w.flat_vec(path));
                }
                w.vec(&marker.additional_drops, |w, member| {
                    w.add(member.value_type.capacity());
                    if let Some(path) = &member.path {
                        w.flat_vec(&path.fields);
                    }
                });
            }
        });
        w.map(&self.calls, |w, _, targets| {
            // Constructed once by collect; empty sets never allocated nodes.
            if !targets.is_empty() {
                w.tree::<[u8; 32], ()>(targets.len())
            }
        });
        w.bytes()
    }
    pub fn objects(&self) -> &[ObjectArtifact] {
        &self.objects
    }
    pub fn metadata(&self) -> &Units {
        &self.units
    }
    pub fn program(&self) -> [u8; 32] {
        self.program
    }

    /// Link through the selected runtime product and decode only after its
    /// process is quiescent. Private objects/metadata prevent replacing the
    /// checked producer with unobserved objects at this consumer seam.
    pub fn launch(
        &self,
        program: &ReadyProgram,
        toolchain: &super::Toolchain,
        dir: &std::path::Path,
        event_limit: usize,
    ) -> Result<LeafExecution, NativeError> {
        self.launch_selected(
            program,
            toolchain,
            dir,
            event_limit,
            None,
            super::Tape::Live {
                record: None,
                revocations: None,
            },
        )
    }
    pub(crate) fn launch_selected(
        &self,
        program: &ReadyProgram,
        toolchain: &super::Toolchain,
        dir: &std::path::Path,
        event_limit: usize,
        run: Option<&hirsel_run_config::ValidatedRun>,
        tape: super::Tape<'_>,
    ) -> Result<LeafExecution, NativeError> {
        if run.is_some_and(|run| run.budgets().is_some_and(|budgets| budgets.fuel.is_some()))
            && !program.fuel
        {
            return Err(NativeError::new(
                StageName::Launch,
                "the selected ownership executable has no fuel checks",
            ));
        }
        if run.is_some_and(|run| run.record().inputs != program.launch) {
            return Err(NativeError::new(
                StageName::Launch,
                "ownership launch inputs differ from the compiled request",
            ));
        }
        if program
            .revision(&self.entry)
            .map(|revision| revision.digest())
            != Some(self.program)
        {
            return Err(NativeError::new(
                StageName::Link,
                "ownership objects belong to another semantic program",
            ));
        }
        if !toolchain.runtime_build.ownership_trace {
            return Err(NativeError::new(
                StageName::Link,
                "ownership objects require the explicitly selected runtime product",
            ));
        }
        std::fs::create_dir_all(dir)
            .map_err(|error| NativeError::new(StageName::Link, error.to_string()))?;
        let runnable =
            super::link_runnable(program, &self.entry, &self.objects, toolchain, dir, run)?;
        let sidecar = dir.join("ownership.rows.json");
        let execution = super::link::launch_ownership(
            &runnable,
            &program.target,
            toolchain,
            dir,
            &sidecar,
            run,
            tape,
        )?;
        let trace = (|| -> Result<_, String> {
            use std::io::Read;
            let file = std::fs::File::open(&sidecar)
                .map_err(|error| format!("ownership sidecar: {error}"))?;
            // Bound a corrupt external log before serde allocates its rows.
            let log = serde_json::from_reader(std::io::BufReader::new(file).take(32 * 1024 * 1024))
                .map_err(|error| format!("ownership sidecar: {error}"))?;
            hirsel_run_schema::ownership_trace::native::decode_local_calls(
                &log,
                self.program,
                &self.units,
                &self.calls,
                event_limit,
            )
            .map_err(|error| error.to_string())
        })();
        Ok(LeafExecution {
            execution,
            program: self.program,
            trace,
        })
    }
}

/// `hirsel run --native --trace-ownership`: select the source-owned diagnostic
/// graph and execute its original Value-to-Ready plans with the opt-in runtime.
/// Unsupported ownership transport refuses before any executable runs.
pub fn run_path(
    path: &std::path::Path,
    backend: Backend,
    verify: hirsel_engine::Verify,
    event_limit: usize,
) -> Result<Result<LeafExecution, NativeError>, crate::ToolError> {
    run_path_with_launch(
        path,
        crate::HostLaunch::empty(),
        backend,
        verify,
        event_limit,
    )
}

/// Admit the same grants or replay record as ordinary native execution before
/// launching the source-owned diagnostic objects. No trace grants authority.
pub fn run_path_with_launch(
    path: &std::path::Path,
    launch: crate::HostLaunch<'_>,
    backend: Backend,
    verify: hirsel_engine::Verify,
    event_limit: usize,
) -> Result<Result<LeafExecution, NativeError>, crate::ToolError> {
    hirsel_engine::with_compiler_stack(|| {
        use crate::requests::{
            CompileOperation, CompileResult, CompilerRequests, OwnershipRequest,
        };
        if matches!(launch, crate::HostLaunch::Callee { .. }) {
            return Err(crate::ToolError::Runtime(
                "launch: a runner host runs its callees interpreted".into(),
            ));
        }
        let selection = crate::launch_binding::LaunchSelection::read_for_compilation(launch)?;
        let (inputs, fuel) = selection.compiler_inputs()?;
        let definition = selection.entry().cloned();
        let compiler = crate::COMPILER.with_verify(verify).with_owned_engines();
        let config = compiler.configuration();
        let selector = CompilerRequests::for_host(compiler.clone(), config.clone());
        let limits = selector.input_limits();
        let attempts = selector.capture_attempts();
        let mut host = crate::resident::ObservedSession::new(
            path.to_owned(),
            compiler,
            config,
            selector,
            limits,
            attempts,
        );
        let target = super::TargetSpec::x86_64_unknown_linux_gnu();
        let output = host.request(CompileOperation::NativeOwnership(OwnershipRequest {
            definition,
            target: target.clone(),
            validation: super::Validation::EveryStage,
            launch: inputs,
            fuel,
        }))?;
        let CompileResult::Ownership(execution) = &output.result else {
            return Err(crate::ToolError::Runtime(
                "ownership request returned another operation".into(),
            ));
        };
        let execution = execution.clone();
        drop(output);
        drop(host);
        Ok((|| {
            let objects = execution
                .emit(backend)
                .map_err(|error| NativeError::new(StageName::Emission, error))?;
            let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(super::runtime_cache::default_directory);
            let toolchain = super::Toolchain::in_tree_with_runtime(
                &cache,
                &[&target],
                super::RuntimeBuild {
                    ownership_trace: true,
                    ..Default::default()
                },
            )?;
            let scratch = super::executable::Scratch::new()?;
            execution
                .launch_with_selection(
                    &objects,
                    &toolchain,
                    &scratch.0,
                    event_limit,
                    launch,
                    selection,
                )
                .map_err(|error| NativeError::new(StageName::Link, error))
        })())
    })
}

//! Explicit Ready-local ownership diagnostics. This staged API refuses every
//! unsupported unit before emitting any object; ordinary emission is unchanged.
//! Earlier rewriting-pass recipes remain a separate qualification obligation.
use super::{Backend, NativeError, OptLevel, ReadyProgram, Sites, StageName};
use hirsel_lir::emission::{EmitInputs, site_code};
use hirsel_lir::ir::{BlockId, ConstructorKind, Inst};
use hirsel_lir::ownership_trace::{Moment, native::LeafPlan};
use hirsel_lir::stage::ObjectArtifact;
use hirsel_run_schema::ownership_trace::native::{LocalCalls, Metadata, Units};
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
    if program.units.len() != program.summaries.len() || program.unit_objects.is_some() {
        return Err(failed(
            "ownership emission requires all checked Ready bodies",
        ));
    }
    if !program.ahead_units.is_empty() {
        return Err(failed(
            "ownership emission does not yet cover alternate task bodies",
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
    emit_relocated(program, entry, &selected, &inputs, &plans, sites, backend)
}

/// Complete checked diagnostic closure with the original transported plans.
/// The source-bound linker projection owns the semantic/runtime metadata.
pub(crate) fn emit_relocated(
    program: &ReadyProgram,
    entry: &DefPath,
    selected: &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
    inputs: &EmitInputs<'_>,
    plans: &BTreeMap<hirsel_syntax::ContentHash, hirsel_lir::ownership_trace::Plan>,
    sites: &Sites<'_>,
    backend: Backend,
) -> Result<LeafObjects, NativeError> {
    fn failed(message: impl Into<String>) -> NativeError {
        NativeError::new(StageName::Emission, message)
    }
    hirsel_lir::ownership_trace::native::Request::Relocated(plans)
        .prepare(selected, inputs.layouts)
        .map_err(failed)?;
    if selected.len() != program.summaries.len() || !program.ahead_units.is_empty() {
        return Err(failed(
            "ownership emission requires the complete checked closure",
        ));
    }
    let revision = program
        .revision(entry)
        .ok_or_else(|| failed("ownership emission has no checked semantic entry binding"))?
        .digest();
    let calls = hirsel_lir::ownership_trace::native::local_calls(selected)
        .map_err(|message| NativeError::new(StageName::Emission, message))?;
    let mut units = BTreeMap::new();
    for unit in selected {
        let original = plans
            .get(&unit.unit().digest())
            .ok_or_else(|| failed("missing original ownership plan"))?;
        let plan = LeafPlan::from_relocated(unit, inputs.layouts, original).map_err(|message| {
            NativeError {
                definition: Some(unit.unit().header.definition.clone()),
                ..NativeError::new(StageName::Emission, message)
            }
        })?;
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
        let mut markers = BTreeMap::new();
        for (block, info) in body.blocks.iter().enumerate() {
            for index in 0..info.instructions.len {
                for moment in [Moment::Before, Moment::After] {
                    for marker in plan.at(BlockId(block as u32), index, moment) {
                        let definition = program
                            .semantic_owners
                            .get(&body.header.definition)
                            .and_then(Option::as_ref)
                            .unwrap_or(&body.header.definition);
                        let failure = sites.failure("ownership", definition, marker.site)?;
                        let mut source = sites
                            .failure_site(&failure)
                            .ok_or_else(|| failed("ownership marker has no source projection"))?;
                        source.definition = name(definition);
                        let location = |at: hirsel_lir::ownership_trace::native::NativeLocation| {
                            (u64::from(at.kind) << 32) | u64::from(at.index)
                        };
                        markers.insert(
                            marker.id.0,
                            Metadata {
                                action: marker.kind,
                                token: location(marker.token),
                                owner: location(marker.owner),
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
                            },
                        );
                    }
                }
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
        units,
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
            w.map(markers, |w, _, marker| {
                w.add(marker.value_type.capacity());
                w.add(marker.source.definition.capacity());
                w.add(marker.source.file.capacity());
            })
        });
        w.map(&self.calls, |w, _, targets| {
            w.tree::<[u8; 32], ()>(targets.len())
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
            super::link_runnable(program, &self.entry, &self.objects, toolchain, dir, None)?;
        let sidecar = dir.join("ownership.rows.json");
        let execution =
            super::link::launch_ownership(&runnable, &program.target, toolchain, dir, &sidecar)?;
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

/// `hirsel run --native --trace-ownership`: build the checked source, emit its
/// diagnostic objects on `backend` and run them with the selected ownership
/// runtime product and no grants. Programs outside the leaf adapter refuse
/// before anything runs; ordinary native objects never stand in for them.
pub fn run_path(
    path: &std::path::Path,
    backend: Backend,
    verify: hirsel_engine::Verify,
    event_limit: usize,
) -> Result<Result<LeafExecution, NativeError>, crate::ToolError> {
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = crate::COMPILER
            .with_verify(verify)
            .with_owned_engines()
            .load_program(path)?;
        loaded.check_for_execution()?;
        let input =
            loaded
                .engine
                .export_native_input()
                .map_err(|error| crate::ToolError::Query {
                    error,
                    phase: loaded.engine.unavailable,
                    sources: loaded.engine.sources.clone(),
                })?;
        Ok((|| {
            let checked = loaded.engine.import_native_input(input).map_err(|error| {
                NativeError::new(
                    StageName::Manifest,
                    format!("the native snapshot: {error:?}"),
                )
            })?;
            let target = super::TargetSpec::x86_64_unknown_linux_gnu();
            let ready = super::build(&checked, &super::Options::new(target.clone()))?;
            let sites = Sites::new(checked.snapshot());
            let objects = emit_leaf(&ready, &loaded.entry, &sites, backend, OptLevel::None)?;
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
            objects.launch(&ready, &toolchain, &scratch.0, event_limit)
        })())
    })
}

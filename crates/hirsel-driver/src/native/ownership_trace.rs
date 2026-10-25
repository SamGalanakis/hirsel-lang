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
    let failed = |message| NativeError::new(StageName::Emission, message);
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
    let revision = program
        .revision(entry)
        .ok_or_else(|| failed("ownership emission has no checked semantic entry binding"))?
        .digest();
    let selected: Vec<_> = program.units.iter().collect();
    let calls = hirsel_lir::ownership_trace::native::local_calls(&selected)
        .map_err(|message| NativeError::new(StageName::Emission, message))?;
    let mut units = BTreeMap::new();
    for unit in &program.units {
        let plan = LeafPlan::prepare(unit, &program.table).map_err(|message| NativeError {
            definition: Some(unit.unit().header.definition.clone()),
            ..NativeError::new(StageName::Emission, message)
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
    let empty = BTreeSet::new();
    let inputs = EmitInputs {
        ahead: None,
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
    let object = match backend {
        Backend::Llvm => hirsel_llvm::emit_object_ownership(&selected, &inputs)
            .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?,
        Backend::Cranelift => hirsel_cranelift::emit_object_ownership(&selected, &inputs)
            .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?,
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

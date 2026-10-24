//! P7: the Ready program as owned objects for its target. The emitter gets immutable
//! inputs only. Unit objects name every number the program assigns by content
//! (`hirsel_lir::glue`); the program's glue object, emitted here at every link, binds
//! them: failure codes from the rules the program's failure edges start, descriptor
//! indices from its registry.
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_lir::emission::{EmitInputs, FailureCodes, GlueCallback, GlueInputs, OptLevel};
use hirsel_lir::glue::UnitGlue;
use hirsel_lir::stage::ObjectArtifact;
use std::collections::BTreeSet;

/// The code generator that lowers Ready LIR to objects. Cranelift builds fast debug
/// executables; the Cranelift parity gate holds it to LLVM's observations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Backend {
    #[default]
    Llvm,
    Cranelift,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Llvm => "llvm",
            Self::Cranelift => "cranelift",
        }
    }
}

pub use hirsel_lir::emission::KIND_BUG;

/// The rules a program can fail with, in order: a rule's runtime code is its
/// index here plus one, so zero never names a rule.
pub fn failure_rules(program: &ReadyProgram) -> Vec<String> {
    failure_rules_from(
        program
            .summaries
            .iter()
            .flat_map(|summary| summary.glue.failure_rules.iter().map(String::as_str)),
    )
}

pub(crate) fn failure_rules_from<'a>(rules: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    rules
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub fn failure_codes(program: &ReadyProgram) -> FailureCodes {
    hirsel_lir::glue::failure_codes(&failure_rules(program).into_iter().collect())
}

/// The glue object of `program` for `backend` (`hirsel_lir::glue`): what its units
/// import by content, bound to the program's numbers.
pub fn emit_glue(
    program: &ReadyProgram,
    backend: Backend,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
) -> Result<ObjectArtifact, NativeError> {
    let failed = |definition: Option<hirsel_syntax::DefPath>, message: String| NativeError {
        definition,
        ..NativeError::new(StageName::Emission, message)
    };
    let mut glue = UnitGlue::default();
    for summary in &program.summaries {
        glue.extend(&summary.glue);
    }
    let failures = failure_codes(program);
    let mut callbacks = Vec::with_capacity(glue.callbacks.len());
    for (definition, instance) in &glue.callbacks {
        let unit = program
            .summaries
            .iter()
            .find(|s| s.definition == *definition && instance.is_none_or(|i| s.instance == i))
            .ok_or_else(|| {
                failed(
                    Some(definition.clone()),
                    "a C callback of no unit".to_string(),
                )
            })?;
        callbacks.push(GlueCallback {
            definition: definition.clone(),
            instance: Some(unit.instance),
            abi: unit.abi,
        });
    }
    let inputs = GlueInputs {
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        registry: registry.clone(),
        finalizers: &program.finalizers,
        glue: &glue,
        failures: &failures,
        statics: &program.statics,
        callbacks: &callbacks,
        foreign: &program.foreign_dispatch,
        aliasing_unstated: !program.callbacks.is_empty(),
    };
    match backend {
        Backend::Llvm => hirsel_llvm::emit_glue(&inputs).map_err(|e| failed(None, e.to_string())),
        Backend::Cranelift => {
            hirsel_cranelift::emit_glue(&inputs).map_err(|e| failed(None, e.to_string()))
        }
    }
}

/// Emit the units `select` keeps into one object. Units left out are imports
/// of this object, resolved by the objects the other partitions emit.
pub fn emit(
    program: &ReadyProgram,
    opt: OptLevel,
    select: impl Fn(usize) -> bool,
) -> Result<ObjectArtifact, NativeError> {
    emit_registered(
        program,
        Backend::Llvm,
        opt,
        select,
        &BTreeSet::new(),
        false,
        None,
    )
}

/// The optimized LLVM IR of the whole program, as `emit` would lower it into one
/// object: the text codegen controls inspect.
pub fn emit_ir(program: &ReadyProgram, opt: OptLevel) -> Result<String, NativeError> {
    let units: Vec<_> = program.units.iter().collect();
    let inputs = EmitInputs {
        ahead: None,
        alternate: false,
        index: None,
        verify_backend: program.validation == super::stages::Validation::EveryStage,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: &BTreeSet::new(),
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt,
        measurement: false,
        fuel: program.fuel,
    };
    hirsel_llvm::emit_optimized_ir(&units, &inputs)
        .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))
}

/// The Cranelift IR of the whole program as one object, before compilation: the text a
/// Cranelift parity failure is read from.
pub fn emit_clif(program: &ReadyProgram) -> Result<String, NativeError> {
    check_runtime_calls(program)?;
    let units: Vec<_> = program.units.iter().collect();
    let inputs = EmitInputs {
        ahead: None,
        alternate: false,
        index: None,
        verify_backend: program.validation == super::stages::Validation::EveryStage,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: &BTreeSet::new(),
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt: OptLevel::None,
        measurement: false,
        fuel: program.fuel,
    };
    hirsel_cranelift::emit_clif(&units, &inputs)
        .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))
}

pub(crate) fn emit_registered(
    program: &ReadyProgram,
    backend: Backend,
    opt: OptLevel,
    select: impl Fn(usize) -> bool,
    copies: &BTreeSet<hirsel_syntax::ContentHash>,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<ObjectArtifact, NativeError> {
    let units: Vec<_> = program
        .units
        .iter()
        .enumerate()
        .filter(|(index, _)| select(*index))
        .map(|(_, unit)| unit)
        .collect();
    let ahead = super::ahead::plan(program);
    let inputs = EmitInputs {
        ahead: Some(&ahead),
        alternate: false,
        index: None,
        verify_backend: program.validation == super::stages::Validation::EveryStage,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: copies,
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt,
        measurement,
        fuel: program.fuel,
    };
    // A debug build records intervals; the link's debug object resolves them.
    let profile = hirsel_lir::debug::Profile {
        debug: sources.is_some(),
        sources,
    };
    let emitted = match backend {
        Backend::Llvm => hirsel_llvm::emit_object_profile(&units, &inputs, profile).map_err(|e| {
            let definition = match &e {
                hirsel_llvm::CodegenError::Receipt { definition, .. }
                | hirsel_llvm::CodegenError::Invalid { definition, .. }
                | hirsel_llvm::CodegenError::Unsupported { definition, .. } => {
                    Some(definition.clone())
                }
                _ => None,
            };
            (definition, e.to_string())
        }),
        Backend::Cranelift => hirsel_cranelift::emit_object_profile(&units, &inputs, profile)
            .map_err(|e| (e.definition().cloned(), e.to_string())),
    };
    emitted.map_err(|(definition, message)| NativeError {
        definition,
        ..NativeError::new(StageName::Emission, message)
    })
}

/// Every call of a `RuntimePrimitive` operation must have exactly the
/// physical signature its runtime export declares
/// (`hirsel_rt_operations::HIRSEL_ABI_EXPORTS`): emission calls each at the
/// Hirsel ABI of its declaration, so an export with another shape would be
/// called with the wrong arguments.
pub fn check_runtime_calls(program: &ReadyProgram) -> Result<(), NativeError> {
    for unit in &program.units {
        check_unit_runtime_calls(unit.unit(), &program.table, &program.manifest)?;
    }
    Ok(())
}

/// `check_runtime_calls` for one unit.
pub(crate) fn check_unit_runtime_calls(
    unit: &hirsel_lir::ir::LirUnit<hirsel_lir::stage::Ready>,
    table: &hirsel_lir::layout_types::LayoutTable,
    manifest: &hirsel_core_contract::Manifest,
) -> Result<(), NativeError> {
    use hirsel_core_contract::{Disposition, Engine};
    use hirsel_lir::abi_types::PhysicalType;
    use hirsel_lir::ir::Callee;
    use hirsel_lir::layout_types::SizeSpec;
    use hirsel_rt_operations::{HIRSEL_ABI_EXPORTS, Physical};
    {
        for site in &unit.tables.calls {
            let Callee::Operation(op) = site.callee else {
                continue;
            };
            let id = &unit.tables.operations[op.index()];
            let Some(Disposition::RuntimePrimitive(template)) = manifest
                .row(id)
                .map(|row| row.dispositions.get(Engine::Native))
            else {
                continue;
            };
            let mismatch = |what: String| NativeError {
                definition: Some(unit.header.definition.clone()),
                ..NativeError::new(
                    StageName::Emission,
                    format!("runtime primitive `{template}`: {what}"),
                )
            };
            let symbol = hirsel_lir::abi::resolve_call_symbol(template, unit, site)
                .ok_or_else(|| mismatch("the template names no symbol for this call".into()))?;
            let export = HIRSEL_ABI_EXPORTS
                .iter()
                .find(|e| e.symbol == symbol)
                .ok_or_else(|| mismatch(format!("the runtime exports no `{symbol}`")))?;
            let abi = &table.fn_abis[site.abi.0 as usize];
            let result = abi
                .result
                .as_ref()
                .and_then(|r| table.layout(r.layout))
                .and_then(|l| match l.size {
                    SizeSpec::Fixed(size) => Some((size, l.align)),
                    SizeSpec::Dynamic(_) => None,
                });
            let slots: Option<Vec<Physical>> = abi
                .parameters
                .iter()
                .flat_map(|p| &p.slots)
                .map(|s| match s.ty {
                    PhysicalType::I8 => Some(Physical::I8),
                    PhysicalType::I16 => Some(Physical::I16),
                    PhysicalType::I32 => Some(Physical::I32),
                    PhysicalType::I64 => Some(Physical::I64),
                    PhysicalType::F32 => Some(Physical::F32),
                    PhysicalType::F64 => Some(Physical::F64),
                    PhysicalType::Pointer => Some(Physical::Pointer),
                    _ => None,
                })
                .collect();
            if !abi.context
                || abi.environment
                || abi.caller_site != export.caller_site
                || abi.destination != export.destination
                || result != export.result
                || slots.as_deref() != Some(export.parameters)
            {
                return Err(mismatch(format!(
                    "the call's ABI (destination {}, result {result:?}, slots {slots:?}) \
                     is not `{symbol}`'s export ({export:?})",
                    abi.destination
                )));
            }
        }
    }
    Ok(())
}

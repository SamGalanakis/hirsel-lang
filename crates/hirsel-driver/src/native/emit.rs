//! P7: the Ready program as one owned object for its target. The emitter gets
//! immutable inputs only; failure codes are assigned here, from the rules the
//! program's failure edges start.
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_lir::emission::{EmitInputs, FailureCode, FailureCodes, OptLevel};
use hirsel_lir::stage::ObjectArtifact;
use std::collections::BTreeSet;

/// The failure category code of every language failure (`kind` of
/// `hirsel_rt_v2_failure_begin`): M1b has one category, FAIL-1's `bug`.
pub const KIND_BUG: u32 = 1;

/// The rules a program can fail with, in order: a rule's runtime code is its
/// index here plus one, so zero never names a rule.
pub fn failure_rules(program: &ReadyProgram) -> Vec<String> {
    let mut rules: BTreeSet<&str> = program
        .units
        .iter()
        .flat_map(|u| u.unit().tables.failures.iter())
        .filter_map(|f| f.rule.as_deref())
        .collect();
    for unit in &program.units {
        for call in &unit.unit().tables.calls {
            if let hirsel_lir::ir::Callee::Operation(op) = call.callee {
                let id = &unit.unit().tables.operations[op.index()];
                if let Some(rule) = program.manifest.row(id).and_then(|r| r.failure.as_ref()) {
                    rules.insert(&rule.0);
                }
            }
        }
    }
    // A bounded arena's exhaustion (MEM-6) starts at any arena construction or copy; only
    // `arena.bounded`'s recovery opens one.
    if program.units.iter().any(|u| {
        u.unit()
            .terminators
            .iter()
            .any(|t| matches!(t, hirsel_lir::ir::Terminator::Recover { .. }))
    }) {
        rules.insert(hirsel_lir::emission::BOUNDED_RULE);
    }
    rules.into_iter().map(str::to_string).collect()
}

pub fn failure_codes(program: &ReadyProgram) -> FailureCodes {
    FailureCodes {
        codes: failure_rules(program)
            .into_iter()
            .enumerate()
            .map(|(index, rule)| {
                (
                    rule,
                    FailureCode {
                        kind: KIND_BUG,
                        rule: index as u32 + 1,
                    },
                )
            })
            .collect(),
    }
}

/// Emit the units `select` keeps into one object. Units left out are imports
/// of this object, resolved by the objects the other partitions emit.
pub fn emit(
    program: &ReadyProgram,
    opt: OptLevel,
    select: impl Fn(usize) -> bool,
) -> Result<ObjectArtifact, NativeError> {
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    emit_registered(
        program,
        opt,
        select,
        &BTreeSet::new(),
        &registry,
        false,
        None,
    )
}

/// The optimized LLVM IR of the whole program, as `emit` would lower it into one
/// object: the text codegen controls inspect.
pub fn emit_ir(program: &ReadyProgram, opt: OptLevel) -> Result<String, NativeError> {
    let failures = failure_codes(program);
    let units: Vec<_> = program.units.iter().collect();
    let inputs = EmitInputs {
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        manifest: &program.manifest,
        failures: &failures,
        statics: &program.statics,
        finalizers: &program.finalizers,
        inline_instances: &program.inline_instances,
        available_externally: &BTreeSet::new(),
        callbacks: &program.callbacks,
        opt,
        registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
            &program.table,
            &program.finalizers,
        ),
        measurement: false,
    };
    hirsel_llvm::emit_optimized_ir(&units, &inputs)
        .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))
}

pub(crate) fn emit_registered(
    program: &ReadyProgram,
    opt: OptLevel,
    select: impl Fn(usize) -> bool,
    copies: &BTreeSet<hirsel_syntax::ContentHash>,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
    measurement: bool,
    sources: Option<&hirsel_llvm::ProfileSources>,
) -> Result<ObjectArtifact, NativeError> {
    let failures = failure_codes(program);
    let units: Vec<_> = program
        .units
        .iter()
        .enumerate()
        .filter(|(index, _)| select(*index))
        .map(|(_, unit)| unit)
        .collect();
    let inputs = EmitInputs {
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        manifest: &program.manifest,
        failures: &failures,
        statics: &program.statics,
        finalizers: &program.finalizers,
        inline_instances: &program.inline_instances,
        available_externally: copies,
        callbacks: &program.callbacks,
        opt,
        registry: registry.clone(),
        measurement,
    };
    hirsel_llvm::emit_object_profile(&units, &inputs, sources).map_err(|e| {
        let definition = match &e {
            hirsel_llvm::CodegenError::Receipt { definition, .. }
            | hirsel_llvm::CodegenError::Invalid { definition, .. }
            | hirsel_llvm::CodegenError::Unsupported { definition, .. }
            | hirsel_llvm::CodegenError::UnknownRule { definition, .. } => Some(definition.clone()),
            _ => None,
        };
        NativeError {
            definition,
            ..NativeError::new(StageName::Emission, e.to_string())
        }
    })
}

/// Every call of a `RuntimePrimitive` operation must have exactly the
/// physical signature its runtime export declares
/// (`hirsel_rt::operations::HIRSEL_ABI_EXPORTS`): emission calls each at the
/// Hirsel ABI of its declaration, so an export with another shape would be
/// called with the wrong arguments.
pub fn check_runtime_calls(program: &ReadyProgram) -> Result<(), NativeError> {
    use hirsel_core_contract::{Disposition, Engine};
    use hirsel_lir::abi_types::PhysicalType;
    use hirsel_lir::ir::Callee;
    use hirsel_lir::layout_types::SizeSpec;
    use hirsel_rt::operations::{HIRSEL_ABI_EXPORTS, Physical};
    for unit in &program.units {
        let unit = unit.unit();
        for site in &unit.tables.calls {
            let Callee::Operation(op) = site.callee else {
                continue;
            };
            let id = &unit.tables.operations[op.index()];
            let Some(Disposition::RuntimePrimitive(template)) = program
                .manifest
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
            let abi = &program.table.fn_abis[site.abi.0 as usize];
            let result = abi
                .result
                .as_ref()
                .and_then(|r| program.table.layout(r.layout))
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

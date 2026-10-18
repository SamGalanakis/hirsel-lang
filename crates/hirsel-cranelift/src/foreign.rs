//! Platform C calls, outside Hirsel's status protocol (PROG-4).
use cranelift_codegen::{
    Context,
    ir::{self, AbiParam, InstBuilder},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};
use hirsel_lir::abi_types::{CallableShape, FnAbi, ParameterPassing, ScalarExtension};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{CConvention, LayoutKind, ScalarKind};
use hirsel_lir::runtime_abi::RuntimeEntry;
use hirsel_lir::stage::ObjectSymbolKind;
use hirsel_syntax::DefPath;

use crate::CodegenError;
use crate::function::{FnEmitter, TRAP};
use crate::module::{Emitted, Emitter, Hidden, PTR, Sig, scalar_type};

fn aarch64_narrow(kind: ScalarKind, convention: CConvention) -> Option<bool> {
    if convention != CConvention::Aapcs64 {
        return None;
    }
    if kind == ScalarKind::Bool {
        return Some(false);
    }
    crate::scalar::integer(kind).and_then(|(width, signed)| (width < 64).then_some(signed))
}

fn extended(ty: ir::Type, extension: ScalarExtension, convention: CConvention) -> AbiParam {
    let param = AbiParam::new(ty);
    // AAPCS64 leaves upper bits of narrow C arguments/results unspecified.
    if convention == CConvention::Aapcs64 {
        return param;
    }
    match extension {
        ScalarExtension::None => param,
        ScalarExtension::Sign => param.sext(),
        ScalarExtension::Zero => param.uext(),
    }
}

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn foreign_call(
        &mut self,
        call: CallId,
        abi: &FnAbi,
        normal: Option<EdgeId>,
    ) -> Emitted {
        let unit = self.unit;
        let site = &unit.tables.calls[call.index()];
        let Callee::Direct(symbol) = site.callee else {
            return self.invalid("an extern called through a value");
        };
        let Some(symbol) = unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a callee outside the symbol table");
        };
        let name = &symbol.definition.0.name;
        let arguments = self.pool(site.arguments, &unit.pools.call_arguments)?;
        let mut sig = ir::Signature::new(self.em.call_conv);
        let mut args = Vec::new();
        let mut column_returns = Vec::new();
        for parameter in &abi.parameters {
            let Some(argument) = arguments.iter().find(|a| a.parameter == parameter.source) else {
                return self.invalid("a call without an argument for a parameter");
            };
            match (
                parameter.passing,
                argument.value,
                &self.layout(parameter.layout)?.kind,
            ) {
                (_, _, LayoutKind::Handle) => {}
                (
                    ParameterPassing::ByValue,
                    ArgumentValue::Value(value),
                    LayoutKind::Scalar(scalar),
                ) => {
                    let value = self.get1(value)?;
                    let convention = self.em.inputs.target.c_convention;
                    if let Some(signed) = aarch64_narrow(*scalar, convention) {
                        sig.params.push(AbiParam::new(ir::types::I64));
                        let value = if signed {
                            self.b.ins().sextend(ir::types::I64, value)
                        } else {
                            self.b.ins().uextend(ir::types::I64, value)
                        };
                        args.push(value);
                    } else {
                        sig.params.push(extended(
                            scalar_type(*scalar),
                            parameter.extension,
                            convention,
                        ));
                        args.push(value);
                    }
                }
                (
                    ParameterPassing::ByValue,
                    ArgumentValue::Value(value),
                    LayoutKind::Slice { .. },
                ) => {
                    let (data, length) = self.slice_bytes(value)?;
                    sig.params
                        .extend([AbiParam::new(PTR), AbiParam::new(ir::types::I64)]);
                    args.extend([data, length]);
                }
                (
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::PointerLength,
                    ArgumentValue::Value(value),
                    _,
                ) => {
                    let place = self.stack(parameter.layout)?;
                    let shape = self.value_shape(value)?;
                    let v = self.get(value)?;
                    self.store(shape, v, place)?;
                    sig.params.push(AbiParam::new(PTR));
                    args.push(place.ptr);
                }
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { address, .. }, _) => {
                    sig.params.push(AbiParam::new(PTR));
                    let place = self.place(address)?;
                    let passed = self.gather(place)?;
                    if place.columns.is_some() {
                        column_returns.push((place, passed));
                    }
                    args.push(passed.ptr);
                }
                _ => return self.invalid("an extern argument outside the C convention"),
            }
        }
        if let Some(result) = &abi.result {
            let LayoutKind::Scalar(scalar) = self.layout(result.layout)?.kind else {
                return self.invalid("an extern result that is not a scalar");
            };
            sig.returns.push(extended(
                if aarch64_narrow(scalar, self.em.inputs.target.c_convention).is_some() {
                    ir::types::I64
                } else {
                    scalar_type(scalar)
                },
                hirsel_lir::abi::extension_of(scalar),
                self.em.inputs.target.c_convention,
            ));
        }
        self.em.manifest_import(
            name,
            ObjectSymbolKind::Foreign,
            Some(&symbol.definition),
            Some(abi),
        );
        let function = self.em.import_function(name, &sig)?;
        self.rt(RuntimeEntry::ForeignEnter, &[self.context])?;
        let returned = self.call(function, &args);
        for (destination, temporary) in column_returns {
            let shape = self.layout_shape(destination.layout)?;
            let value = self.load(shape, temporary);
            self.scatter(destination, shape, value)?;
        }
        if let Some(value) = site.result {
            let Some(&returned) = returned.first() else {
                return self.invalid("a call result without an extern result");
            };
            let kind = self.scalar_kind(value)?;
            let returned = if aarch64_narrow(kind, self.em.inputs.target.c_convention).is_some() {
                self.b.ins().ireduce(scalar_type(kind), returned)
            } else {
                returned
            };
            self.define1(value, returned)?;
        }
        let Some(normal) = normal else {
            return self.invalid("an extern call without a normal edge");
        };
        self.jump(normal)
    }
}

impl Emitter<'_> {
    /// `R callback(void *ctx, ...)`, obtaining the run context from the active extern call.
    pub(crate) fn define_callback(
        &mut self,
        name: &str,
        target: FuncId,
        sig: &Sig<'_>,
        linkage: Linkage,
        definition: &DefPath,
    ) -> Emitted {
        let unsupported = |what: &str| CodegenError::Unsupported {
            definition: definition.clone(),
            what: format!("a C callback {what}"),
        };
        let abi = sig.abi;
        if abi.shape != CallableShape::Definition || abi.environment {
            return Err(unsupported("of a definition with an environment"));
        }
        let Some((context_parameter, rest)) = abi.parameters.split_first() else {
            return Err(unsupported("without a context parameter"));
        };
        if context_parameter.passing != ParameterPassing::ExclusiveAddress {
            return Err(unsupported("whose context is not a `mut` address"));
        }
        let mut c_sig = ir::Signature::new(self.call_conv);
        c_sig.params.push(AbiParam::new(PTR));
        for (index, parameter) in rest.iter().enumerate() {
            if parameter.passing != ParameterPassing::ByValue || parameter.slots.len() != 1 {
                return Err(unsupported("parameter that is not one scalar"));
            }
            let kind = self
                .inputs
                .layouts
                .layout(parameter.layout)
                .map(|l| &l.kind);
            let narrow = matches!(kind, Some(LayoutKind::Scalar(kind)) if aarch64_narrow(*kind, self.inputs.target.c_convention).is_some());
            c_sig.params.push(extended(
                if narrow {
                    ir::types::I64
                } else {
                    sig.ir.params[sig.first[index + 1]].value_type
                },
                parameter.extension,
                self.inputs.target.c_convention,
            ));
        }
        if let Some(kind) = sig.value {
            c_sig.returns.push(extended(
                if aarch64_narrow(kind, self.inputs.target.c_convention).is_some() {
                    ir::types::I64
                } else {
                    scalar_type(kind)
                },
                hirsel_lir::abi::extension_of(kind),
                self.inputs.target.c_convention,
            ));
        }
        let id = self.declare_function(name, linkage, &c_sig)?;
        let enter = self.runtime(RuntimeEntry::CallbackContext)?;
        let failed = self.runtime(RuntimeEntry::CallbackFailed)?;
        let mut ctx = Context::new();
        ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), c_sig);
        let mut bc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut bc);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let params = b.block_params(entry).to_vec();
            let enter = self.module.declare_func_in_func(enter, b.func);
            let call = b.ins().call(enter, &[]);
            let run = b.inst_results(call)[0];
            let mut args = Vec::new();
            for hidden in &sig.hidden {
                args.push(match hidden {
                    Hidden::Context => run,
                    Hidden::Destination => b.ins().iconst(PTR, 0),
                    Hidden::Result => {
                        if !abi
                            .result
                            .as_ref()
                            .and_then(|r| self.inputs.layouts.layout(r.layout))
                            .is_some_and(|l| l.kind == LayoutKind::Unit)
                        {
                            return Err(unsupported("result that is not a scalar"));
                        }
                        let slot = b.create_sized_stack_slot(ir::StackSlotData::new(
                            ir::StackSlotKind::ExplicitSlot,
                            1,
                            0,
                        ));
                        b.ins().stack_addr(PTR, slot, 0)
                    }
                    Hidden::Environment => return Err(unsupported("with an environment")),
                });
            }
            args.push(params[0]);
            for (index, _) in rest.iter().enumerate() {
                let raw = params[index + 1];
                let ty = sig.ir.params[sig.first[index + 1]].value_type;
                let value = if b.func.dfg.value_type(raw) != ty {
                    b.ins().ireduce(ty, raw)
                } else {
                    raw
                };
                args.push(value);
            }
            let callee = self.module.declare_func_in_func(target, b.func);
            let call = b.ins().call(callee, &args);
            let results = b.inst_results(call).to_vec();
            let bad = b.create_block();
            let normal = b.create_block();
            b.ins().brif(results[0], bad, &[], normal, &[]);
            b.switch_to_block(bad);
            let failed = self.module.declare_func_in_func(failed, b.func);
            b.ins().call(failed, &[run]);
            b.ins().trap(TRAP);
            b.switch_to_block(normal);
            if let Some(kind) = sig.value {
                let word = results[1];
                let ty = scalar_type(kind);
                let v = match kind {
                    ScalarKind::F64 => b.ins().bitcast(ty, ir::MemFlagsData::new(), word),
                    ScalarKind::F32 => {
                        let word = b.ins().ireduce(ir::types::I32, word);
                        b.ins().bitcast(ty, ir::MemFlagsData::new(), word)
                    }
                    _ if ty.bits() < 64 => b.ins().ireduce(ty, word),
                    _ => word,
                };
                let v = if let Some(signed) = aarch64_narrow(kind, self.inputs.target.c_convention)
                {
                    if signed {
                        b.ins().sextend(ir::types::I64, v)
                    } else {
                        b.ins().uextend(ir::types::I64, v)
                    }
                } else {
                    v
                };
                b.ins().return_(&[v]);
            } else {
                b.ins().return_(&[]);
            }
            b.seal_all_blocks();
            b.finalize(self.module.target_config());
        }
        self.define(name, id, &mut ctx)
    }
}

//! One object: the ISA, the declarations every unit and helper needs, the function bodies,
//! and the manifest the finished object is checked against.

use std::collections::BTreeMap;

use cranelift_codegen::ir::{self, AbiParam, InstBuilder, types};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::{Context, ir::Function};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use hirsel_lir::abi::abi_digest;
use hirsel_lir::abi_types::{CallableShape, FnAbi, PhysicalType, ScalarExtension};
use hirsel_lir::emission::{
    EmitInputs, Manifested, OptLevel, artifact, check_receipt, function_thunk_name,
    register_result, symbol_name,
};
use hirsel_lir::ir::UnitOrigin;
use hirsel_lir::layout_types::{LayoutId, LayoutTable, ScalarKind, TargetSpec};
use hirsel_lir::object::ObjectFile;
use hirsel_lir::runtime_abi::{CName, CType, RuntimeEntry};
use hirsel_lir::stage::{ObjectSymbol, ObjectSymbolKind, Ready, ValidatedUnit};
use hirsel_syntax::DefPath;

use crate::CodegenError;

pub(crate) type Emitted<T = ()> = Result<T, CodegenError>;

/// The pointer type of both supported targets.
pub(crate) const PTR: ir::Type = types::I64;

/// The Cranelift type of a physical ABI slot.
pub(crate) fn physical(ty: PhysicalType) -> ir::Type {
    match ty {
        PhysicalType::I8 => types::I8,
        PhysicalType::I16 => types::I16,
        PhysicalType::I32 => types::I32,
        PhysicalType::I64 => types::I64,
        PhysicalType::I128 => types::I128,
        PhysicalType::F32 => types::F32,
        PhysicalType::F64 => types::F64,
        PhysicalType::Pointer => PTR,
    }
}

/// The Cranelift type of a scalar in SSA form: Bool is a byte holding 0 or 1.
pub(crate) fn scalar_type(kind: ScalarKind) -> ir::Type {
    match kind {
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => types::I8,
        ScalarKind::U16 | ScalarKind::I16 => types::I16,
        ScalarKind::U32 | ScalarKind::I32 => types::I32,
        ScalarKind::U64 | ScalarKind::I64 | ScalarKind::Pointer => types::I64,
        ScalarKind::U128 | ScalarKind::I128 => types::I128,
        ScalarKind::F32 => types::F32,
        ScalarKind::F64 => types::F64,
    }
}

/// A hidden parameter of a Hirsel ABI v1 entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hidden {
    Context,
    Environment,
    Destination,
    Result,
}

/// The Cranelift signature of one `FnAbi`: hidden slots in ABI order, then each declared
/// parameter's physical slots, returning a zero-extended `u8` status, and with a register
/// result (`register_result`) the value word after it instead of a result pointer.
#[derive(Clone)]
pub(crate) struct Sig<'a> {
    pub(crate) abi: &'a FnAbi,
    pub(crate) ir: ir::Signature,
    pub(crate) hidden: Vec<Hidden>,
    /// Per declared parameter, the index of its first Cranelift parameter.
    pub(crate) first: Vec<usize>,
    /// The plain scalar result returned in the value word.
    pub(crate) value: Option<ScalarKind>,
}

impl<'a> Sig<'a> {
    pub(crate) fn hidden_index(&self, which: Hidden) -> Option<usize> {
        self.hidden.iter().position(|h| *h == which)
    }
}

/// `abi` lowered with `value` as its register result (none for a runtime primitive, which
/// keeps its result pointer).
pub(crate) fn signature<'a>(
    call_conv: CallConv,
    abi: &'a FnAbi,
    value: Option<ScalarKind>,
) -> Result<Sig<'a>, String> {
    if !abi.context {
        return Err("a Hirsel entry without a run context".into());
    }
    let mut ir = ir::Signature::new(call_conv);
    let mut hidden = vec![Hidden::Context];
    if abi.environment {
        hidden.push(Hidden::Environment);
    }
    if abi.destination {
        hidden.push(Hidden::Destination);
    }
    if abi.result.is_some() && value.is_none() {
        hidden.push(Hidden::Result);
    }
    ir.params.extend(hidden.iter().map(|_| AbiParam::new(PTR)));
    let mut first = Vec::new();
    for parameter in &abi.parameters {
        first.push(ir.params.len());
        for slot in &parameter.slots {
            let param = AbiParam::new(physical(slot.ty));
            ir.params
                .push(match (parameter.extension, parameter.slots.len()) {
                    (ScalarExtension::Sign, 1) => param.sext(),
                    (ScalarExtension::Zero, 1) => param.uext(),
                    _ => param,
                });
        }
    }
    ir.returns.push(AbiParam::new(types::I8).uext());
    if value.is_some() {
        ir.returns.push(AbiParam::new(types::I64));
    }
    Ok(Sig {
        abi,
        ir,
        hidden,
        first,
        value,
    })
}

/// The Cranelift type of a C type of `hirsel_rt_v2.h`, or None for void.
fn c_type(ty: CType) -> Option<(ir::Type, bool)> {
    if ty.pointers > 0 {
        return Some((PTR, false));
    }
    match ty.name {
        CName::Void => None,
        CName::U8 => Some((types::I8, true)),
        CName::U32 => Some((types::I32, true)),
        CName::U64 => Some((types::I64, false)),
        _ => Some((PTR, false)),
    }
}

/// A helper function a body asked for, defined once the units are.
pub(crate) enum Helper {
    /// `hirsel.compare.<layout>[.eq]`.
    Compare { layout: LayoutId, equality: bool },
    /// `hirsel.hash.<layout>`.
    Hash { layout: LayoutId },
    /// `hirsel.drop.<layout>`, calling the drop body `body`.
    Drop { body: FuncId },
}

/// The object being emitted: its Cranelift module, the declarations made so far and the
/// manifest the finished object must match.
pub(crate) struct Emitter<'a> {
    pub(crate) module: ObjectModule,
    pub(crate) inputs: &'a EmitInputs<'a>,
    pub(crate) call_conv: CallConv,
    pub(crate) manifested: Manifested,
    pub(crate) functions: BTreeMap<String, FuncId>,
    pub(crate) data: BTreeMap<String, DataId>,
    /// Helper functions declared and still to be defined.
    pub(crate) pending: Vec<(FuncId, Helper)>,
    /// A counter for anonymous constants (static masks).
    pub(crate) anonymous: u64,
    /// With `Some`, each function's Cranelift IR as it is defined (`crate::emit_clif`).
    pub(crate) clif: Option<String>,
}

fn isa(target: &TargetSpec, opt: OptLevel) -> Emitted<OwnedTargetIsa> {
    let refuse = |message: String| CodegenError::Target {
        triple: target.triple.clone(),
        message,
    };
    let mut flags = settings::builder();
    let level = if opt == OptLevel::None {
        "none"
    } else {
        "speed"
    };
    for (name, value) in [
        ("opt_level", level),
        ("is_pic", "true"),
        ("enable_verifier", "true"),
        ("preserve_frame_pointers", "true"),
        // i128 parameters and results in register pairs, as LLVM passes them (SysV, AAPCS64).
        ("enable_llvm_abi_extensions", "true"),
    ] {
        flags
            .set(name, value)
            .map_err(|e| refuse(format!("{name}={value}: {e}")))?;
    }
    let builder = cranelift_codegen::isa::lookup_by_name(&target.triple)
        .map_err(|e| refuse(e.to_string()))?;
    builder
        .finish(settings::Flags::new(flags))
        .map_err(|e| refuse(e.to_string()))
}

/// Emits `units` as one object (`crate::emit_object`); with `clif`, also the text of
/// every function's Cranelift IR.
pub(crate) fn emit(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    clif: bool,
) -> Emitted<(hirsel_lir::stage::ObjectArtifact, Option<String>)> {
    for unit in units {
        check_receipt(unit, inputs).map_err(|problem| CodegenError::Receipt {
            definition: unit.unit().header.definition.clone(),
            problem,
        })?;
    }
    if !inputs.registry.validates(inputs.layouts, inputs.finalizers) {
        return Err(CodegenError::Manifest {
            problem: "foreign or noncanonical descriptor registry".into(),
        });
    }
    let isa = isa(inputs.target, inputs.opt)?;
    let call_conv = isa.default_call_conv();
    let builder = ObjectBuilder::new(isa, "hirsel", cranelift_module::default_libcall_names())
        .map_err(|e| CodegenError::Target {
            triple: inputs.target.triple.clone(),
            message: e.to_string(),
        })?;
    let mut emitter = Emitter {
        module: ObjectModule::new(builder),
        inputs,
        call_conv,
        manifested: Manifested::default(),
        functions: BTreeMap::new(),
        data: BTreeMap::new(),
        pending: Vec::new(),
        anonymous: 0,
        clif: clif.then(String::new),
    };
    let emitted: Vec<&ValidatedUnit<Ready>> = units
        .iter()
        .copied()
        .filter(|u| {
            !inputs
                .available_externally
                .contains(&u.unit().header.instance)
        })
        .collect();
    // Declare every unit and thunk first, so calls between them resolve in this object.
    let mut bodies = Vec::new();
    for unit in &emitted {
        let unit = unit.unit();
        let header = &unit.header;
        if inputs.callbacks.contains(&header.definition) {
            return Err(CodegenError::Unsupported {
                definition: header.definition.clone(),
                what: "a C callback thunk (unsafe ffi)".into(),
            });
        }
        let abi = emitter.fn_abi(header.abi, &header.definition)?;
        let name = symbol_name(&header.definition, Some(&header.instance));
        if emitter.functions.contains_key(&name) {
            return Err(CodegenError::DuplicateSymbol { name });
        }
        let sig = emitter.entry_signature(abi, &header.definition)?;
        let library = header.origin == UnitOrigin::Library
            || inputs.inline_instances.contains(&header.instance);
        let linkage = if library {
            Linkage::Preemptible
        } else {
            Linkage::Export
        };
        let id = emitter.declare_function(&name, linkage, &sig.ir)?;
        let thunk = (abi.shape == CallableShape::Definition)
            .then(|| function_thunk_name(&header.definition, Some(&header.instance)));
        let record = |name: &String| ObjectSymbol {
            name: name.clone(),
            kind: ObjectSymbolKind::Function,
            definition: Some(header.definition.clone()),
            abi: Some(abi_digest(abi)),
        };
        if library {
            emitter.manifested.library.insert(name.clone());
            if let Some(thunk) = &thunk {
                emitter.manifested.library.insert(thunk.clone());
            }
        } else {
            emitter
                .manifested
                .exports
                .insert(name.clone(), record(&name));
            if let Some(thunk) = &thunk {
                emitter
                    .manifested
                    .exports
                    .insert(thunk.clone(), record(thunk));
            }
        }
        if let Some(thunk) = thunk {
            let thunk_sig = thunk_signature(&sig.ir);
            let thunk_id = emitter.declare_function(&thunk, linkage, &thunk_sig)?;
            emitter.define_thunk(&thunk, thunk_id, id, &thunk_sig, &sig.ir)?;
        }
        bodies.push((unit, id, sig));
    }
    let mut context = Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    for (unit, id, sig) in bodies {
        let name = symbol_name(&unit.header.definition, Some(&unit.header.instance));
        context.func = Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        {
            let builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            crate::function::FnEmitter::new(&mut emitter, unit, builder, sig).emit()?;
        }
        emitter.define(&name, id, &mut context)?;
    }
    // The registry names every descriptor of the program, and a descriptor its finalizer's
    // drop wrapper.
    if let Some(first) = emitted.first() {
        let definition = &first.unit().header.definition;
        emitter.registry_table(definition)?;
        emitter.drain_helpers(definition)?;
    }
    emitter.manifested.settle_imports();
    let Emitter {
        module,
        manifested,
        clif,
        ..
    } = emitter;
    let bytes = module.finish().emit().map_err(|e| CodegenError::Object {
        message: e.to_string(),
    })?;
    let file = ObjectFile::read(inputs.target, bytes)
        .map_err(|message| CodegenError::Object { message })?;
    artifact(file, manifested, units, inputs, crate::TOOLCHAIN)
        .map(|artifact| (artifact, clif))
        .map_err(|problem| CodegenError::Manifest { problem })
}

/// A thunk takes the environment after the context and otherwise the entry's parameters.
fn thunk_signature(entry: &ir::Signature) -> ir::Signature {
    let mut sig = entry.clone();
    sig.params.insert(1, AbiParam::new(PTR));
    sig
}

impl<'a> Emitter<'a> {
    pub(crate) fn fn_abi(
        &self,
        id: hirsel_lir::FnAbiId,
        definition: &DefPath,
    ) -> Emitted<&'a FnAbi> {
        self.inputs
            .layouts
            .fn_abis
            .get(id.0 as usize)
            .ok_or_else(|| CodegenError::Invalid {
                definition: definition.clone(),
                what: format!("no FnAbi {}", id.0),
            })
    }

    pub(crate) fn layouts(&self) -> &'a LayoutTable {
        self.inputs.layouts
    }

    /// The signature of a Hirsel entry of `abi`: a plain scalar result returns in a register.
    pub(crate) fn entry_signature(&self, abi: &'a FnAbi, definition: &DefPath) -> Emitted<Sig<'a>> {
        signature(
            self.call_conv,
            abi,
            register_result(self.inputs.layouts, abi),
        )
        .map_err(|what| CodegenError::Unsupported {
            definition: definition.clone(),
            what,
        })
    }

    /// The signature of a runtime primitive of `abi`: every result through its pointer.
    pub(crate) fn runtime_signature(
        &self,
        abi: &'a FnAbi,
        definition: &DefPath,
    ) -> Emitted<Sig<'a>> {
        signature(self.call_conv, abi, None).map_err(|what| CodegenError::Unsupported {
            definition: definition.clone(),
            what,
        })
    }

    /// Declares `name`, or returns its earlier declaration (which must have the same
    /// signature; a defining linkage overrides an import).
    pub(crate) fn declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        sig: &ir::Signature,
    ) -> Emitted<FuncId> {
        let id = self
            .module
            .declare_function(name, linkage, sig)
            .map_err(|e| CodegenError::Cranelift {
                name: name.to_owned(),
                message: e.to_string(),
            })?;
        self.functions.insert(name.to_owned(), id);
        Ok(id)
    }

    /// An imported function, unless this object defines it.
    pub(crate) fn import_function(&mut self, name: &str, sig: &ir::Signature) -> Emitted<FuncId> {
        match self.functions.get(name) {
            Some(&id) => Ok(id),
            None => self.declare_function(name, Linkage::Import, sig),
        }
    }

    /// The runtime entry `entry`, imported with its C declaration.
    pub(crate) fn runtime(&mut self, entry: RuntimeEntry) -> Emitted<FuncId> {
        let declaration = entry.declaration();
        let mut sig = ir::Signature::new(self.call_conv);
        for parameter in declaration.parameters {
            let (ty, extend) = c_type(parameter.ty).expect("no void parameter");
            sig.params.push(if extend {
                AbiParam::new(ty).uext()
            } else {
                AbiParam::new(ty)
            });
        }
        if let Some((ty, extend)) = c_type(declaration.returns) {
            sig.returns.push(if extend {
                AbiParam::new(ty).uext()
            } else {
                AbiParam::new(ty)
            });
        }
        self.manifested
            .import(declaration.symbol, ObjectSymbolKind::Runtime, None);
        self.import_function(declaration.symbol, &sig)
    }

    pub(crate) fn define(&mut self, name: &str, id: FuncId, context: &mut Context) -> Emitted {
        if let Some(text) = &mut self.clif {
            text.push_str(&format!("; {name}\n{}\n", context.func.display()));
        }
        self.module
            .define_function(id, context)
            .map_err(|e| CodegenError::Cranelift {
                name: name.to_owned(),
                message: match e {
                    cranelift_module::ModuleError::Compilation(error) => {
                        cranelift_codegen::print_errors::pretty_error(&context.func, error)
                    }
                    other => other.to_string(),
                },
            })?;
        self.module.clear_context(context);
        Ok(())
    }

    /// The thunk `name`: forwards every argument but the environment to `target`.
    fn define_thunk(
        &mut self,
        name: &str,
        id: FuncId,
        target: FuncId,
        sig: &ir::Signature,
        target_sig: &ir::Signature,
    ) -> Emitted {
        let mut context = Context::new();
        context.func = Function::with_name_signature(ir::UserFuncName::default(), sig.clone());
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let params = b.block_params(entry).to_vec();
            let mut args = vec![params[0]];
            args.extend_from_slice(&params[2..]);
            debug_assert_eq!(args.len(), target_sig.params.len());
            let callee = self.module.declare_func_in_func(target, b.func);
            let call = b.ins().call(callee, &args);
            let results = b.inst_results(call).to_vec();
            b.ins().return_(&results);
            b.seal_all_blocks();
            b.finalize(self.module.target_config());
        }
        self.define(name, id, &mut context)
    }

    /// A private helper function `name` of `sig`, queued for definition if new.
    pub(crate) fn helper(
        &mut self,
        name: &str,
        sig: &ir::Signature,
        helper: impl FnOnce(FuncId) -> Helper,
    ) -> Emitted<FuncId> {
        if let Some(&id) = self.functions.get(name) {
            return Ok(id);
        }
        let id = self.declare_function(name, Linkage::Local, sig)?;
        let helper = helper(id);
        self.pending.push((id, helper));
        Ok(id)
    }

    pub(crate) fn define_helper(
        &mut self,
        id: FuncId,
        helper: Helper,
        context: &mut Context,
        builder_context: &mut FunctionBuilderContext,
        definition: &DefPath,
    ) -> Emitted {
        let name = self
            .module
            .declarations()
            .get_function_decl(id)
            .linkage_name(id)
            .into_owned();
        let sig = self
            .module
            .declarations()
            .get_function_decl(id)
            .signature
            .clone();
        context.func = Function::with_name_signature(ir::UserFuncName::default(), sig);
        {
            let b = FunctionBuilder::new(&mut context.func, builder_context);
            match helper {
                Helper::Compare { layout, equality } => {
                    crate::compare::define_comparator(self, b, layout, equality, definition)?
                }
                Helper::Hash { layout } => {
                    crate::compare::define_hasher(self, b, layout, definition)?
                }
                Helper::Drop { body } => self.define_drop_wrapper(b, body),
            }
        }
        self.define(&name, id, context)
    }

    /// The data object `name`, declared with `linkage` if new.
    pub(crate) fn declare_data(&mut self, name: &str, linkage: Linkage) -> Emitted<(DataId, bool)> {
        if let Some(&id) = self.data.get(name) {
            return Ok((id, false));
        }
        let id = self
            .module
            .declare_data(name, linkage, false, false)
            .map_err(|e| CodegenError::Cranelift {
                name: name.to_owned(),
                message: e.to_string(),
            })?;
        self.data.insert(name.to_owned(), id);
        Ok((id, true))
    }

    pub(crate) fn define_data(
        &mut self,
        name: &str,
        id: DataId,
        description: &DataDescription,
    ) -> Emitted {
        self.module
            .define_data(id, description)
            .map_err(|e| CodegenError::Cranelift {
                name: name.to_owned(),
                message: e.to_string(),
            })
    }

    /// A fresh private constant of `bytes`, aligned to `align`.
    pub(crate) fn constant(&mut self, prefix: &str, bytes: Vec<u8>, align: u64) -> Emitted<DataId> {
        self.anonymous += 1;
        let name = format!("{prefix}.{}", self.anonymous);
        let (id, _) = self.declare_data(&name, Linkage::Local)?;
        let mut description = DataDescription::new();
        description.define(bytes.into_boxed_slice());
        description.set_align(align);
        self.define_data(&name, id, &description)?;
        Ok(id)
    }

    /// Records a function import for the manifest.
    pub(crate) fn manifest_import(
        &mut self,
        name: &str,
        kind: ObjectSymbolKind,
        definition: Option<&DefPath>,
        abi: Option<&FnAbi>,
    ) {
        self.manifested.import_at(name, kind, definition, abi);
    }
}

/// One argument of the launcher call `launch_object` emits.
#[derive(Clone, Debug)]
pub enum LaunchArgument {
    /// The entry adapter's address.
    Entry,
    /// An integer of `bits` bits.
    Int { bits: u8, value: u64 },
    /// The address of a private constant holding these bytes.
    Bytes(Vec<u8>),
    /// The address of the external data symbol.
    Symbol(String),
}

/// PROG-3's exit report of an entry's returned `Err`: the entry result's descriptor index,
/// the error payload's offset in it and the diagnostic view nodes, `root` the payload's.
#[derive(Clone, Debug)]
pub struct ExitReport {
    pub result_index: u32,
    pub offset: u64,
    pub root: usize,
    pub nodes: Vec<ReportNode>,
}

/// One view node as the runtime reads it: `{kind, a, b, c, text, text length, fields, field
/// count}`, each field `{offset, node, label, label length}`.
#[derive(Clone, Debug, Default)]
pub struct ReportNode {
    pub kind: u64,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub text: Vec<u8>,
    pub fields: Vec<(u64, usize, Vec<u8>)>,
}

/// The launcher object: `main`, which ignores SIGPIPE and returns what `launcher(arguments)`
/// returns, and the private entry adapter `hirsel.entry(ctx, dest, out, roots)` that calls the
/// source entry `entry(ctx, dest, out, roots + 8 * w...)` for each root word index `w`. When the
/// entry returns normally with the `Err` tag (1) at `out`, the adapter first calls
/// `hirsel_rt_v2_exit_report(ctx, out, result index, offset, root node)`.
pub fn launch_object(
    target: &TargetSpec,
    entry: &str,
    root_words: &[u64],
    report: &ExitReport,
    launcher: &str,
    arguments: &[LaunchArgument],
) -> Result<Vec<u8>, CodegenError> {
    let isa = isa(target, OptLevel::None)?;
    let call_conv = isa.default_call_conv();
    let builder = ObjectBuilder::new(
        isa,
        "hirsel-main",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| CodegenError::Target {
        triple: target.triple.clone(),
        message: e.to_string(),
    })?;
    let mut module = ObjectModule::new(builder);
    let refuse = |name: &str| {
        let name = name.to_owned();
        move |e: cranelift_module::ModuleError| CodegenError::Cranelift {
            name,
            message: e.to_string(),
        }
    };
    let mut entry_sig = ir::Signature::new(call_conv);
    entry_sig.params.extend(std::iter::repeat_n(
        AbiParam::new(PTR),
        3 + root_words.len(),
    ));
    entry_sig.returns.push(AbiParam::new(types::I8).uext());
    let entry_id = module
        .declare_function(entry, Linkage::Import, &entry_sig)
        .map_err(refuse(entry))?;
    let mut adapter_sig = ir::Signature::new(call_conv);
    adapter_sig
        .params
        .extend(std::iter::repeat_n(AbiParam::new(PTR), 4));
    adapter_sig.returns.push(AbiParam::new(types::I8).uext());
    let adapter_id = module
        .declare_function("hirsel.entry", Linkage::Local, &adapter_sig)
        .map_err(refuse("hirsel.entry"))?;
    let root_node = report_nodes(&mut module, report)?;
    let mut report_sig = ir::Signature::new(call_conv);
    report_sig.params.extend([
        AbiParam::new(PTR),
        AbiParam::new(PTR),
        AbiParam::new(types::I32),
        AbiParam::new(types::I64),
        AbiParam::new(PTR),
    ]);
    let report_id = module
        .declare_function("hirsel_rt_v2_exit_report", Linkage::Import, &report_sig)
        .map_err(refuse("hirsel_rt_v2_exit_report"))?;
    let mut context = Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    context.func = Function::with_name_signature(ir::UserFuncName::default(), adapter_sig);
    {
        let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        let params = b.block_params(block).to_vec();
        let mut args = params[..3].to_vec();
        for &word in root_words {
            args.push(b.ins().iadd_imm_s(params[3], (word * 8) as i64));
        }
        let callee = module.declare_func_in_func(entry_id, b.func);
        let call = b.ins().call(callee, &args);
        let status = b.inst_results(call)[0];
        let (returned, reported, done) = (b.create_block(), b.create_block(), b.create_block());
        b.ins().brif(status, done, &[], returned, &[]);
        b.switch_to_block(returned);
        let flags = ir::MemFlagsData::new();
        let tag = b.ins().load(types::I32, flags, params[2], 0);
        let error = b.ins().icmp_imm_s(ir::condcodes::IntCC::Equal, tag, 1);
        b.ins().brif(error, reported, &[], done, &[]);
        b.switch_to_block(reported);
        let index = b.ins().iconst(types::I32, i64::from(report.result_index));
        let offset = b.ins().iconst(types::I64, report.offset as i64);
        let node = module.declare_data_in_func(root_node, b.func);
        let node = b.ins().symbol_value(PTR, node);
        let callee = module.declare_func_in_func(report_id, b.func);
        b.ins()
            .call(callee, &[params[0], params[2], index, offset, node]);
        b.ins().jump(done, &[]);
        b.switch_to_block(done);
        b.ins().return_(&[status]);
        b.seal_all_blocks();
        b.finalize(module.target_config());
    }
    module
        .define_function(adapter_id, &mut context)
        .map_err(refuse("hirsel.entry"))?;
    module.clear_context(&mut context);
    // The launcher: its parameters are the arguments' types; it returns an i32.
    let mut launcher_sig = ir::Signature::new(call_conv);
    for argument in arguments {
        launcher_sig.params.push(match argument {
            LaunchArgument::Int { bits: 8, .. } => AbiParam::new(types::I8).uext(),
            LaunchArgument::Int { bits: 32, .. } => AbiParam::new(types::I32).uext(),
            LaunchArgument::Int { .. } => AbiParam::new(types::I64),
            _ => AbiParam::new(PTR),
        });
    }
    launcher_sig.returns.push(AbiParam::new(types::I32));
    let launcher_id = module
        .declare_function(launcher, Linkage::Import, &launcher_sig)
        .map_err(refuse(launcher))?;
    let mut signal_sig = ir::Signature::new(call_conv);
    signal_sig.params.push(AbiParam::new(types::I32));
    signal_sig.params.push(AbiParam::new(PTR));
    signal_sig.returns.push(AbiParam::new(PTR));
    let signal_id = module
        .declare_function("signal", Linkage::Import, &signal_sig)
        .map_err(refuse("signal"))?;
    let mut data = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        match argument {
            LaunchArgument::Bytes(bytes) => {
                let name = format!("hirsel.launch.{index}");
                let id = module
                    .declare_data(&name, Linkage::Local, false, false)
                    .map_err(refuse(&name))?;
                let mut description = DataDescription::new();
                description.define(bytes.clone().into_boxed_slice());
                description.set_align(8);
                module
                    .define_data(id, &description)
                    .map_err(refuse(&name))?;
                data.push(Some(id));
            }
            LaunchArgument::Symbol(name) => {
                let id = module
                    .declare_data(name, Linkage::Import, false, false)
                    .map_err(refuse(name))?;
                data.push(Some(id));
            }
            _ => data.push(None),
        }
    }
    let mut main_sig = ir::Signature::new(call_conv);
    main_sig.returns.push(AbiParam::new(types::I32));
    let main_id = module
        .declare_function("main", Linkage::Export, &main_sig)
        .map_err(refuse("main"))?;
    context.func = Function::with_name_signature(ir::UserFuncName::default(), main_sig);
    {
        let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let block = b.create_block();
        b.switch_to_block(block);
        let signal = module.declare_func_in_func(signal_id, b.func);
        let sigpipe = b.ins().iconst(types::I32, 13);
        let ignore = b.ins().iconst(PTR, 1);
        b.ins().call(signal, &[sigpipe, ignore]);
        let mut args = Vec::new();
        for (argument, data) in arguments.iter().zip(&data) {
            args.push(match (argument, data) {
                (LaunchArgument::Entry, _) => {
                    let adapter = module.declare_func_in_func(adapter_id, b.func);
                    b.ins().func_addr(PTR, adapter)
                }
                (LaunchArgument::Int { bits, value }, _) => {
                    let ty = match bits {
                        8 => types::I8,
                        32 => types::I32,
                        _ => types::I64,
                    };
                    b.ins().iconst(ty, *value as i64)
                }
                (_, Some(id)) => {
                    let gv = module.declare_data_in_func(*id, b.func);
                    b.ins().symbol_value(PTR, gv)
                }
                (_, None) => unreachable!("data arguments have their data"),
            });
        }
        let launch = module.declare_func_in_func(launcher_id, b.func);
        let call = b.ins().call(launch, &args);
        let code = b.inst_results(call)[0];
        b.ins().return_(&[code]);
        b.seal_all_blocks();
        b.finalize(module.target_config());
    }
    module
        .define_function(main_id, &mut context)
        .map_err(refuse("main"))?;
    module.finish().emit().map_err(|e| CodegenError::Object {
        message: e.to_string(),
    })
}

/// The exit report's view nodes as private constants; the root node's id.
fn report_nodes(module: &mut ObjectModule, report: &ExitReport) -> Emitted<DataId> {
    let refuse = |name: String| {
        move |e: cranelift_module::ModuleError| CodegenError::Cranelift {
            name,
            message: e.to_string(),
        }
    };
    let declare = |module: &mut ObjectModule, name: String| {
        module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(refuse(name))
    };
    let mut nodes = Vec::with_capacity(report.nodes.len());
    for i in 0..report.nodes.len() {
        nodes.push(declare(module, format!("hirsel.report.node.{i}"))?);
    }
    let Some(&root) = nodes.get(report.root) else {
        return Err(CodegenError::Manifest {
            problem: "an exit report root outside its nodes".into(),
        });
    };
    // Bytes, at least one so every constant has an address of its own.
    let bytes = |module: &mut ObjectModule, name: String, text: &[u8]| -> Emitted<DataId> {
        let id = declare(module, name.clone())?;
        let mut description = DataDescription::new();
        let mut image = text.to_vec();
        if image.is_empty() {
            image.push(0);
        }
        description.define(image.into_boxed_slice());
        module.define_data(id, &description).map_err(refuse(name))?;
        Ok(id)
    };
    for (i, node) in report.nodes.iter().enumerate() {
        let text = bytes(module, format!("hirsel.report.text.{i}"), &node.text)?;
        let mut description = DataDescription::new();
        let mut image = vec![0u8; (node.fields.len() * 32).max(8)];
        let mut targets = Vec::new();
        for (j, (offset, child, label)) in node.fields.iter().enumerate() {
            let Some(&child) = nodes.get(*child) else {
                return Err(CodegenError::Manifest {
                    problem: "an exit report field outside its nodes".into(),
                });
            };
            let label_id = bytes(module, format!("hirsel.report.label.{i}.{j}"), label)?;
            image[j * 32..j * 32 + 8].copy_from_slice(&offset.to_le_bytes());
            image[j * 32 + 24..j * 32 + 32].copy_from_slice(&(label.len() as u64).to_le_bytes());
            targets.push((j * 32 + 8, child));
            targets.push((j * 32 + 16, label_id));
        }
        description.define(image.into_boxed_slice());
        description.set_align(8);
        for (at, target) in targets {
            let gv = module.declare_data_in_data(target, &mut description);
            description.write_data_addr(at as u32, gv, 0);
        }
        let name = format!("hirsel.report.fields.{i}");
        let fields = declare(module, name.clone())?;
        module
            .define_data(fields, &description)
            .map_err(refuse(name))?;
        let mut image = Vec::with_capacity(64);
        for word in [
            node.kind,
            node.a,
            node.b,
            node.c,
            0,
            node.text.len() as u64,
            0,
        ] {
            image.extend(word.to_le_bytes());
        }
        image.extend((node.fields.len() as u64).to_le_bytes());
        let mut description = DataDescription::new();
        description.define(image.into_boxed_slice());
        description.set_align(8);
        for (at, target) in [(32, text), (48, fields)] {
            let gv = module.declare_data_in_data(target, &mut description);
            description.write_data_addr(at, gv, 0);
        }
        module
            .define_data(nodes[i], &description)
            .map_err(refuse(format!("hirsel.report.node.{i}")))?;
    }
    Ok(root)
}

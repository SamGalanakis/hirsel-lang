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
    EmitInputs, GlueInputs, Manifested, OptLevel, artifact, function_thunk_name, register_result,
    symbol_name,
};
use hirsel_lir::ir::UnitOrigin;
use hirsel_lir::layout_types::{LayoutId, LayoutTable, ScalarKind, TargetSpec};
use hirsel_lir::object::ObjectFile;
use hirsel_lir::runtime_abi::{CName, CType, RuntimeEntry};
use hirsel_lir::stage::{ObjectArtifact, ObjectSymbol, ObjectSymbolKind, Ready, ValidatedUnit};
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

/// A hidden parameter of a Hirsel ABI v3 entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hidden {
    Context,
    Environment,
    CallerSite,
    OperationSite,
    Destination,
    Result,
}

/// The Cranelift signature of one `FnAbi`: hidden slots in ABI order, then each declared
/// parameter's physical slots, returning a zero-extended `u8` status, and with a register
/// result (`register_result`) the value word before it instead of a result pointer.
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
    if abi.caller_site {
        hidden.extend([Hidden::CallerSite, Hidden::OperationSite]);
    }
    if abi.destination {
        hidden.push(Hidden::Destination);
    }
    if abi.result.is_some() && value.is_none() {
        hidden.push(Hidden::Result);
    }
    ir.params.extend(hidden.iter().map(|h| {
        AbiParam::new(match h {
            Hidden::CallerSite | Hidden::OperationSite => types::I64,
            _ => PTR,
        })
    }));
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
    let receipts = hirsel_lir::abi::receipt_slots(abi, ir.params.len())?;
    ir.params
        .extend(std::iter::repeat_n(AbiParam::new(PTR), receipts.len()));
    if value.is_some() {
        ir.returns.push(AbiParam::new(types::I64));
    }
    ir.returns.push(AbiParam::new(types::I8).uext());
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
    /// `hirsel.compare.<layout key>[.eq]`.
    Compare {
        layout: LayoutId,
        equality: bool,
    },
    /// `hirsel.hash.<layout key>`.
    Hash {
        layout: LayoutId,
    },
    /// `hirsel.drop.<layout>`, calling the drop body `body`.
    Drop {
        body: FuncId,
        caller_site: bool,
    },
    DropResume {
        body: FuncId,
        caller_site: bool,
    },
    /// `hirsel.check.<symbol>`, a codec check's `hirsel_rt_v2_check_thunk` (SER-1).
    Check(CheckThunk),
}

/// How a check thunk calls its check: the check `body`, how it takes the borrowed value
/// and how it returns the verdict.
pub(crate) struct CheckThunk {
    pub(crate) body: FuncId,
    /// Each by-value slot's offset in the value and type; None passes the value's address.
    pub(crate) slots: Option<Vec<(u64, ir::Type)>>,
    /// The verdict is the register value word, not a result pointer's byte.
    pub(crate) register: bool,
}

/// What one object is emitted from: the target, the layout table and its keys, and either
/// a unit object's `EmitInputs` or the program glue's `GlueInputs`.
#[derive(Clone, Copy)]
pub(crate) struct Inputs<'a> {
    pub(crate) target: &'a TargetSpec,
    pub(crate) layouts: &'a LayoutTable,
    pub(crate) keys: &'a hirsel_lir::LayoutKeys,
    pub(crate) opt: OptLevel,
    pub(crate) unit: Option<&'a EmitInputs<'a>>,
    pub(crate) glue: Option<&'a GlueInputs<'a>>,
}

impl<'a> Inputs<'a> {
    /// The inputs of a unit object.
    pub(crate) fn units(inputs: &'a EmitInputs<'a>) -> Self {
        Self {
            target: inputs.target,
            layouts: inputs.layouts,
            keys: inputs.keys,
            opt: inputs.opt,
            unit: Some(inputs),
            glue: None,
        }
    }
    /// The inputs of the program's glue object, which is compiled without optimization.
    pub(crate) fn program(glue: &'a GlueInputs<'a>) -> Self {
        Self {
            target: glue.target,
            layouts: glue.layouts,
            keys: glue.keys,
            opt: OptLevel::None,
            unit: None,
            glue: Some(glue),
        }
    }
    /// A unit object's inputs; only unit emission reads them.
    pub(crate) fn unit(&self) -> &'a EmitInputs<'a> {
        self.unit.expect("unit inputs in unit emission")
    }
    /// The glue object's inputs; only glue emission reads them.
    pub(crate) fn glue(&self) -> &'a GlueInputs<'a> {
        self.glue.expect("glue inputs in glue emission")
    }
}

/// The object being emitted: its Cranelift module, the declarations made so far and the
/// manifest the finished object must match.
pub(crate) struct Emitter<'a> {
    pub(crate) module: ObjectModule,
    pub(crate) inputs: Inputs<'a>,
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
    /// Whether functions get source locations and debug intervals (`debug::Profile`).
    pub(crate) debug: bool,
    debug_functions: Vec<hirsel_lir::debug::DebugFunction>,
}

pub(crate) fn isa(target: &TargetSpec, opt: OptLevel) -> Emitted<OwnedTargetIsa> {
    isa_with_verifier(target, opt, hirsel_lir::emission::validate_stages())
}

fn isa_with_verifier(target: &TargetSpec, opt: OptLevel, verify: bool) -> Emitted<OwnedTargetIsa> {
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
        // Cranelift's own IR verifier checks this backend, not the program: debug
        // builds, which every test runs, and every-stage validation keep it; the
        // shipped compiler skips it, as cg_clif does (src/lib.rs:257).
        (
            "enable_verifier",
            if cfg!(debug_assertions) || verify {
                "true"
            } else {
                "false"
            },
        ),
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
    profile: hirsel_lir::debug::Profile<'_>,
    ownership: Option<hirsel_lir::ownership_trace::native::Request<'_>>,
) -> Emitted<(
    hirsel_lir::stage::ObjectArtifact,
    Option<String>,
    BTreeMap<String, u64>,
)> {
    let ownership_plans = if let Some(ownership) = ownership {
        if inputs.alternate || !inputs.available_externally.is_empty() {
            return Err(CodegenError::Object {
                message: "ownership tracing with alternate/template code is not implemented".into(),
            });
        }
        Some(
            ownership
                .prepare(units, inputs.layouts)
                .map_err(|message| CodegenError::Object { message })?,
        )
    } else {
        None
    };
    let owned;
    let index = match inputs.index.filter(|index| index.matches(inputs.layouts)) {
        Some(index) => index,
        None => {
            owned = hirsel_lir::LayoutIndex::new(inputs.layouts);
            &owned
        }
    };
    let inputs = &EmitInputs {
        index: Some(index),
        ..inputs.clone()
    };
    for unit in units {
        hirsel_lir::emission::check_receipt(unit, inputs).map_err(|problem| {
            CodegenError::Receipt {
                definition: unit.unit().header.definition.clone(),
                problem,
            }
        })?;
    }
    let mut emitter = Emitter::new(Inputs::units(inputs), clif)?;
    emitter.debug = profile.debug;
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
        let abi = emitter.fn_abi(header.abi, &header.definition)?;
        let name = if inputs.alternate {
            hirsel_lir::ahead::alternate_name(&header.definition, header.instance)
        } else {
            symbol_name(&header.definition, Some(&header.instance))
        };
        if emitter.functions.contains_key(&name) {
            return Err(CodegenError::DuplicateSymbol { name });
        }
        let sig = emitter.entry_signature(abi, &header.definition)?;
        let library = !inputs.alternate
            && (header.origin == UnitOrigin::Library
                || inputs.inline_instances.contains(&header.instance));
        let linkage = if library {
            Linkage::Preemptible
        } else {
            Linkage::Export
        };
        let id = emitter.declare_function(&name, linkage, &sig.ir)?;
        let thunk = (!inputs.alternate && abi.shape == CallableShape::Definition)
            .then(|| function_thunk_name(&header.definition, Some(&header.instance)));
        let record = |name: &String| ObjectSymbol {
            name: name.clone(),
            kind: ObjectSymbolKind::Function,
            definition: Some(header.definition.clone()),
            abi: Some(abi_digest(abi)),
        };
        // A template's own object retains weak definitions; callers import them.
        // The owner participates in the same link manifest as an ordinary source unit.
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
        if let Some(thunk) = thunk {
            let thunk_sig = thunk_signature(&sig.ir, abi.caller_site);
            let thunk_id = emitter.declare_function(&thunk, linkage, &thunk_sig)?;
            emitter.define_thunk(&thunk, thunk_id, id, &thunk_sig, &sig.ir)?;
        }
        bodies.push((unit, id, sig));
    }
    // Ahead code also carries each resumable plan's state-0 alternate: a save
    // continues in the ordinary dispatcher, which reissues the boundary.
    let mut resumed = Vec::new();
    if inputs.alternate
        && let Some(plans) = inputs.continuation
    {
        for unit in &emitted {
            let unit = unit.unit();
            let header = &unit.header;
            let Some(plan) = plans.get(header.instance).filter(|p| {
                p.representation == hirsel_lir::continuation::Representation::Resumable
            }) else {
                continue;
            };
            let abi = emitter.fn_abi(header.abi, &header.definition)?;
            let mut sig = emitter.entry_signature(abi, &header.definition)?;
            sig.ir.params.push(ir::AbiParam::new(PTR));
            let name =
                hirsel_lir::ahead::alternate_resume_name(&header.definition, header.instance);
            if emitter.functions.contains_key(&name) {
                return Err(CodegenError::DuplicateSymbol { name });
            }
            let id = emitter.declare_function(&name, Linkage::Export, &sig.ir)?;
            emitter.manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name: name.clone(),
                    kind: ObjectSymbolKind::Function,
                    definition: Some(header.definition.clone()),
                    abi: None,
                },
            );
            resumed.push((unit, plan, name, id, sig));
        }
    }
    let depth_leaf =
        hirsel_lir::invocations::room::depth_leaf_units(units.iter().map(|u| u.unit()));
    let counter_units =
        hirsel_lir::invocations::room::counter_units(units.iter().map(|u| u.unit()));
    let mut frames = BTreeMap::new();
    let mut context = Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    for (unit, id, sig) in bodies {
        let name = if inputs.alternate {
            hirsel_lir::ahead::alternate_name(&unit.header.definition, unit.header.instance)
        } else {
            symbol_name(&unit.header.definition, Some(&unit.header.instance))
        };
        context.func = Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        {
            let builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let mut function = crate::function::FnEmitter::new(&mut emitter, unit, builder, sig);
            function.room = hirsel_lir::invocations::room::RoomChecks::of(unit, &depth_leaf);
            // Resumable alternates retain the runtime admission's mode token.
            if !inputs.alternate {
                function.counters =
                    hirsel_lir::invocations::room::RoomChecks::of(unit, &counter_units);
            }
            function.ownership = ownership_plans
                .as_ref()
                .and_then(|plans| plans.get(&unit.header.instance));
            if inputs.measurement {
                function.measured = profile.sources.and_then(|s| s.get(&unit.header.definition));
            }
            function.emit()?;
        }
        emitter.compile(&name, id, &mut context)?;
        if inputs.alternate
            && let Some(extent) = context
                .compiled_code()
                .and_then(|c| c.buffer.frame_layout())
                .and_then(|f| f.frame_to_fp_offset.checked_add(16))
        {
            frames.insert(name.clone(), u64::from(extent));
        }
        if emitter.debug {
            emitter.debug_functions.push(crate::dwarf::intervals(
                &name,
                unit,
                context.compiled_code().expect("defined function"),
            ));
        }
        emitter.module.clear_context(&mut context);
    }
    for (unit, plan, name, id, sig) in resumed {
        context.func = Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        {
            let builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let mut function = crate::function::FnEmitter::new(&mut emitter, unit, builder, sig);
            function.room = hirsel_lir::invocations::room::RoomChecks::of(unit, &depth_leaf);

            function.resume = Some(crate::resume::Resume {
                plan,
                state: 0,
                frame: None,
                resuming: None,
            });
            function.emit()?;
        }
        emitter.compile(&name, id, &mut context)?;
        if let Some(extent) = context
            .compiled_code()
            .and_then(|c| c.buffer.frame_layout())
            .and_then(|f| f.frame_to_fp_offset.checked_add(16))
        {
            frames.insert(name.clone(), u64::from(extent));
        }
        emitter.module.clear_context(&mut context);
    }
    if let Some(first) = emitted.first() {
        emitter.drain_helpers(&first.unit().header.definition)?;
    }
    emitter.manifested.settle_imports();
    let Emitter {
        module,
        manifested,
        clif,
        debug_functions,
        ..
    } = emitter;
    let product = module.finish();
    let bytes = crate::object::finish(product)?;
    let file = ObjectFile::read(inputs.target, bytes)
        .map_err(|message| CodegenError::Object { message })?;
    artifact(file, manifested, inputs, crate::TOOLCHAIN)
        .map(|mut artifact| {
            if ownership.is_some() {
                artifact.reproducibility.options = hirsel_syntax::ContentHash::of_parts(
                    "hirsel.native-ownership-leaf-options.v1",
                    [
                        artifact.reproducibility.options.0.as_slice(),
                        hirsel_lir::ownership_trace::native::ABI_VERSION
                            .to_le_bytes()
                            .as_slice(),
                    ],
                );
            }
            if let Some(fingerprint) = ownership.and_then(|request| request.fingerprint()) {
                artifact.reproducibility.options = hirsel_syntax::ContentHash::of_parts(
                    "hirsel.native-ownership-relocated-options.v1",
                    [
                        artifact.reproducibility.options.0.as_slice(),
                        fingerprint.0.as_slice(),
                    ],
                );
            }

            (
                ObjectArtifact {
                    debug: debug_functions,
                    ..artifact
                },
                clif,
                frames,
            )
        })
        .map_err(|problem| CodegenError::Manifest { problem })
}

/// A thunk takes the environment after the context and otherwise the entry's parameters.
fn thunk_signature(entry: &ir::Signature, caller_site: bool) -> ir::Signature {
    let mut sig = entry.clone();
    sig.params.insert(1, AbiParam::new(PTR));
    if !caller_site {
        sig.params
            .splice(2..2, [AbiParam::new(types::I64), AbiParam::new(types::I64)]);
    }
    sig
}

/// Hirsel units and glue bind in this link output. Images must keep their
/// symbols non-preemptible (`local:` units and `-Bsymbolic`), as shipped links do.
/// Runtime, foreign and backend-introduced toolchain imports keep GOT indirection.
pub(crate) fn colocate_hirsel_symbols(
    module: &ObjectModule,
    manifested: &Manifested,
    function: &mut ir::Function,
) {
    for (_, reference) in function.stencil.dfg.ext_funcs.iter_mut() {
        let ir::ExternalName::User(user) = reference.name else {
            continue;
        };
        let user = &function.params.user_named_funcs()[user];
        if user.namespace != 0 {
            continue;
        }
        let declaration = module
            .declarations()
            .get_function_decl(FuncId::from_u32(user.index));
        let Some(name) = declaration.name.as_deref() else {
            continue;
        };
        let hirsel = manifested
            .imports
            .get(name)
            .or_else(|| manifested.exports.get(name))
            .is_some_and(|symbol| symbol.kind == ObjectSymbolKind::Function)
            || manifested.library.contains(name);
        if hirsel {
            reference.colocated = true;
        }
    }
}

impl<'a> Emitter<'a> {
    pub(crate) fn new(inputs: Inputs<'a>, clif: bool) -> Emitted<Self> {
        let verify = inputs
            .unit
            .map_or_else(hirsel_lir::emission::validate_stages, |i| i.verify_backend);
        let isa = isa_with_verifier(inputs.target, inputs.opt, verify)?;
        let call_conv = isa.default_call_conv();
        let builder = ObjectBuilder::new(isa, "hirsel", cranelift_module::default_libcall_names())
            .map_err(|e| CodegenError::Target {
                triple: inputs.target.triple.clone(),
                message: e.to_string(),
            })?;
        Ok(Self {
            module: ObjectModule::new(builder),
            inputs,
            call_conv,
            manifested: Manifested::default(),
            functions: BTreeMap::new(),
            data: BTreeMap::new(),
            pending: Vec::new(),
            anonymous: 0,
            clif: clif.then(String::new),
            debug: false,
            debug_functions: Vec::new(),
        })
    }

    pub(crate) fn fn_abi(
        &self,
        id: hirsel_lir::FnAbiId,
        definition: &DefPath,
    ) -> Emitted<&'a FnAbi> {
        let abi = self
            .inputs
            .layouts
            .fn_abis
            .get(id.0 as usize)
            .ok_or_else(|| CodegenError::Invalid {
                definition: definition.clone(),
                what: format!("no FnAbi {}", id.0),
            })?;
        hirsel_lir::abi::check_implemented_transport(abi).map_err(|what| {
            CodegenError::Invalid {
                definition: definition.clone(),
                what: what.into(),
            }
        })?;
        Ok(abi)
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
        self.compile(name, id, context)?;
        self.module.clear_context(context);
        Ok(())
    }

    pub(crate) fn compile(&mut self, name: &str, id: FuncId, context: &mut Context) -> Emitted {
        colocate_hirsel_symbols(&self.module, &self.manifested, &mut context.func);
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
            let inserted = sig.params.len() - target_sig.params.len();
            args.extend_from_slice(&params[1 + inserted..]);
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
                Helper::Drop { body, caller_site } => {
                    self.define_drop_wrapper(b, body, caller_site)
                }
                Helper::DropResume { body, caller_site } => {
                    self.define_drop_resume_wrapper(b, body, caller_site)
                }
                Helper::Check(thunk) => self.define_check_wrapper(b, thunk),
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

    /// The private read-only constant of `bytes` aligned to `align`, named by its content
    /// (`hirsel_lir::statics::constant_data_name`): one per distinct table.
    pub(crate) fn constant_data(&mut self, align: u32, bytes: &[u8]) -> Emitted<DataId> {
        let name = hirsel_lir::statics::constant_data_name(align, bytes);
        let (id, fresh) = self.declare_data(&name, Linkage::Local)?;
        if fresh {
            let mut description = DataDescription::new();
            description.define(bytes.to_vec().into_boxed_slice());
            description.set_align(u64::from(align));
            self.define_data(&name, id, &description)?;
        }
        Ok(id)
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

/// One field of the program descriptor (`hirsel_rt_v2_program_descriptor`) a launch object
/// emits, in order; the caller lays the fields out at their natural alignment.
#[derive(Clone, Debug)]
pub enum DescriptorField {
    U32(u32),
    U64(u64),
    /// The entry adapter's address.
    Entry,
    /// The address of the external data symbol.
    Symbol(String),
    /// The address of a private constant holding these bytes.
    Bytes(Vec<u8>),
    /// These bytes, in place.
    Inline(Vec<u8>),
    /// The address of a private table of `{code, path, path length}` rows (ABI 30): each
    /// imported function's address and a private constant of its path.
    Entries(Vec<(String, Vec<u8>)>),
}

/// PROG-3's exit report of an entry's returned `Err`: the entry result's descriptor index,
/// the error payload's offset in it and the diagnostic view nodes, `root` the payload's.
#[derive(Clone, Debug)]
pub struct ExitReport {
    pub result_index: u32,
    pub is_result: bool,
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

/// The launcher object: `main_symbol(argc, argv)`, which ignores SIGPIPE and returns
/// `hirsel_rt_v2_main(&hirsel.program, argc, argv)`, the private program descriptor of `descriptor`'s
/// fields, and the private entry adapter `hirsel.entry(ctx, dest, out, roots)` that calls the
/// source entry `entry(ctx, dest, out, roots + 8 * w...)` for each root word index `w`. When the
/// entry returns normally with the `Err` tag (1) at `out`, the adapter first calls
/// `hirsel_rt_v2_exit_report(ctx, out, result index, offset, root node)`.
pub fn named_launch_object(
    main_symbol: &str,
    target: &TargetSpec,
    entry: &str,
    caller_site: bool,
    root_words: &[u64],
    report: &ExitReport,
    descriptor: &[DescriptorField],
) -> Result<Vec<u8>, CodegenError> {
    launch_object(
        main_symbol,
        target,
        entry,
        caller_site,
        root_words,
        report,
        descriptor,
        false,
    )
}

/// The existing runner decoder as an independent callee replay root: it takes
/// only its context and publishes the canonical reply through RunnerReply.
pub fn named_callee_launch_object(
    main_symbol: &str,
    target: &TargetSpec,
    entry: &str,
    caller_site: bool,
    root_words: &[u64],
    report: &ExitReport,
    descriptor: &[DescriptorField],
) -> Result<Vec<u8>, CodegenError> {
    launch_object(
        main_symbol,
        target,
        entry,
        caller_site,
        root_words,
        report,
        descriptor,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn launch_object(
    main_symbol: &str,
    target: &TargetSpec,
    entry: &str,
    caller_site: bool,
    root_words: &[u64],
    report: &ExitReport,
    descriptor: &[DescriptorField],
    callee_entry: bool,
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
        if callee_entry {
            1
        } else {
            3 + root_words.len() + if caller_site { 2 } else { 0 }
        },
    ));
    entry_sig.returns.push(AbiParam::new(types::I8).uext());
    let entry_id = module
        .declare_function(entry, Linkage::Import, &entry_sig)
        .map_err(refuse(entry))?;
    let mut admission_sig = ir::Signature::new(call_conv);
    admission_sig.params.push(AbiParam::new(PTR));
    admission_sig.returns.push(AbiParam::new(types::I8).uext());
    let admission_id = module
        .declare_function("hirsel_rt_v2_depth_enter", Linkage::Import, &admission_sig)
        .map_err(refuse("hirsel_rt_v2_depth_enter"))?;
    let mut retirement_sig = ir::Signature::new(call_conv);
    retirement_sig.params.push(AbiParam::new(PTR));
    let retirement_id = module
        .declare_function("hirsel_rt_v2_depth_leave", Linkage::Import, &retirement_sig)
        .map_err(refuse("hirsel_rt_v2_depth_leave"))?;
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
        let admit = module.declare_func_in_func(admission_id, b.func);
        b.ins().call(admit, &[params[0]]);
        let mut args = vec![params[0]];
        if !callee_entry && caller_site {
            let zero = b.ins().iconst(types::I64, 0);
            args.extend([zero, zero]);
        }
        if !callee_entry {
            args.extend_from_slice(&params[1..3]);
            for &word in root_words {
                args.push(b.ins().iadd_imm_s(params[3], (word * 8) as i64));
            }
        }
        let callee = module.declare_func_in_func(entry_id, b.func);
        // Units and glue bind in this executable/image, independently of partition.
        b.func.dfg.ext_funcs[callee].colocated = true;
        let call = b.ins().call(callee, &args);
        let status = b.inst_results(call)[0];
        if callee_entry {
            let zero = b.ins().iconst(types::I8, 0);
            b.ins().store(ir::MemFlagsData::new(), zero, params[2], 0);
        }
        let (returned, reported, done) = (b.create_block(), b.create_block(), b.create_block());
        b.ins().brif(status, done, &[], returned, &[]);
        b.switch_to_block(returned);
        let flags = ir::MemFlagsData::new();
        if report.is_result {
            let tag = b.ins().load(types::I32, flags, params[2], 0);
            let error = b.ins().icmp_imm_s(ir::condcodes::IntCC::Equal, tag, 1);
            b.ins().brif(error, reported, &[], done, &[]);
        } else {
            b.ins().jump(done, &[]);
        }
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
        let retire = module.declare_func_in_func(retirement_id, b.func);
        b.ins().call(retire, &[params[0]]);
        b.ins().return_(&[status]);
        b.seal_all_blocks();
        b.finalize(module.target_config());
    }
    module
        .define_function(adapter_id, &mut context)
        .map_err(refuse("hirsel.entry"))?;
    module.clear_context(&mut context);
    // The program descriptor: scalars and inline bytes in place, addresses relocated.
    let mut bytes = Vec::new();
    let mut description = DataDescription::new();
    for (ordinal, field) in descriptor.iter().enumerate() {
        let at = bytes.len() as u32;
        match field {
            DescriptorField::U32(value) => bytes.extend(value.to_le_bytes()),
            DescriptorField::U64(value) => bytes.extend(value.to_le_bytes()),
            DescriptorField::Inline(inline) => bytes.extend(inline),
            DescriptorField::Entry => {
                let adapter = module.declare_func_in_data(adapter_id, &mut description);
                description.write_function_addr(at, adapter);
                bytes.extend([0; 8]);
            }
            DescriptorField::Symbol(name) => {
                let id = module
                    .declare_data(name, Linkage::Import, false, false)
                    .map_err(refuse(name))?;
                let gv = module.declare_data_in_data(id, &mut description);
                description.write_data_addr(at, gv, 0);
                bytes.extend([0; 8]);
            }
            DescriptorField::Bytes(data) => {
                let name = format!("hirsel.program.{ordinal}");
                let id = module
                    .declare_data(&name, Linkage::Local, false, false)
                    .map_err(refuse(&name))?;
                let mut constant = DataDescription::new();
                constant.define(data.clone().into_boxed_slice());
                constant.set_align(8);
                module.define_data(id, &constant).map_err(refuse(&name))?;
                let gv = module.declare_data_in_data(id, &mut description);
                description.write_data_addr(at, gv, 0);
                bytes.extend([0; 8]);
            }
            DescriptorField::Entries(rows) => {
                let mut table = DataDescription::new();
                let mut words = Vec::with_capacity(rows.len() * 24);
                for (index, (function, path)) in rows.iter().enumerate() {
                    let row = (index * 24) as u32;
                    // A table row takes the address; it never calls through
                    // this declaration. Retain an existing entry/report ABI.
                    let code = match module.declarations().get_name(function) {
                        Some(cranelift_module::FuncOrDataId::Func(id)) => id,
                        _ => module
                            .declare_function(
                                function,
                                Linkage::Import,
                                &ir::Signature::new(call_conv),
                            )
                            .map_err(refuse(function))?,
                    };
                    let code = module.declare_func_in_data(code, &mut table);
                    table.write_function_addr(row, code);
                    let name = format!("hirsel.program.{ordinal}.{index}");
                    let id = module
                        .declare_data(&name, Linkage::Local, false, false)
                        .map_err(refuse(&name))?;
                    let mut constant = DataDescription::new();
                    constant.define(path.clone().into_boxed_slice());
                    module.define_data(id, &constant).map_err(refuse(&name))?;
                    let gv = module.declare_data_in_data(id, &mut table);
                    table.write_data_addr(row + 8, gv, 0);
                    words.extend([0; 16]);
                    words.extend((path.len() as u64).to_le_bytes());
                }
                let name = format!("hirsel.program.{ordinal}");
                let id = module
                    .declare_data(&name, Linkage::Local, false, false)
                    .map_err(refuse(&name))?;
                table.define(words.into_boxed_slice());
                table.set_align(8);
                module.define_data(id, &table).map_err(refuse(&name))?;
                let gv = module.declare_data_in_data(id, &mut description);
                description.write_data_addr(at, gv, 0);
                bytes.extend([0; 8]);
            }
        }
    }
    description.define(bytes.into_boxed_slice());
    description.set_align(8);
    let program_id = module
        .declare_data("hirsel.program", Linkage::Local, false, false)
        .map_err(refuse("hirsel.program"))?;
    module
        .define_data(program_id, &description)
        .map_err(refuse("hirsel.program"))?;
    let mut launcher_sig = ir::Signature::new(call_conv);
    launcher_sig.params.extend([
        AbiParam::new(PTR),
        AbiParam::new(types::I32),
        AbiParam::new(PTR),
    ]);
    launcher_sig.returns.push(AbiParam::new(types::I32));
    let launcher_id = module
        .declare_function("hirsel_rt_v2_main", Linkage::Import, &launcher_sig)
        .map_err(refuse("hirsel_rt_v2_main"))?;
    let mut signal_sig = ir::Signature::new(call_conv);
    signal_sig.params.push(AbiParam::new(types::I32));
    signal_sig.params.push(AbiParam::new(PTR));
    signal_sig.returns.push(AbiParam::new(PTR));
    let signal_id = module
        .declare_function("signal", Linkage::Import, &signal_sig)
        .map_err(refuse("signal"))?;
    let mut main_sig = ir::Signature::new(call_conv);
    main_sig
        .params
        .extend([AbiParam::new(types::I32), AbiParam::new(PTR)]);
    main_sig.returns.push(AbiParam::new(types::I32));
    let main_id = module
        .declare_function(main_symbol, Linkage::Export, &main_sig)
        .map_err(refuse("main"))?;
    context.func = Function::with_name_signature(ir::UserFuncName::default(), main_sig);
    {
        let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        let (argc, argv) = (b.block_params(block)[0], b.block_params(block)[1]);
        let signal = module.declare_func_in_func(signal_id, b.func);
        let sigpipe = b.ins().iconst(types::I32, 13);
        let ignore = b.ins().iconst(PTR, 1);
        b.ins().call(signal, &[sigpipe, ignore]);
        let program = module.declare_data_in_func(program_id, b.func);
        let program = b.ins().symbol_value(PTR, program);
        let launcher = module.declare_func_in_func(launcher_id, b.func);
        let call = b.ins().call(launcher, &[program, argc, argv]);
        let code = b.inst_results(call)[0];
        b.ins().return_(&[code]);
        b.seal_all_blocks();
        b.finalize(module.target_config());
    }
    module
        .define_function(main_id, &mut context)
        .map_err(refuse("main"))?;
    crate::object::finish(module.finish())
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

#[cfg(test)]
mod receipt_tests;

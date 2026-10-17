//! The emission pipeline: validated Ready units to one owned object.
//!
//! `emit_object` checks every unit's receipt against the immutable inputs, cross-checks
//! LLVM's DataLayout against `TargetSpec`, builds one module, runs the verifier before and
//! after the optimization pipeline, generates the object, reads its symbol interface back
//! from the ELF bytes and compares it with the manifest the emitter declared. Every LLVM
//! handle and every borrowed LIR table is dropped before the `ObjectArtifact` is returned.
//!
//! `compile_ir` is the toolchain smoke path: textual IR through the same verifier,
//! optimization and object stages.

use std::collections::{BTreeMap, BTreeSet};

use hirsel_core_contract::{Disposition, Engine, Manifest, Passing};
use hirsel_lir::abi::abi_digest;
use hirsel_lir::abi_types::{
    CallableShape, FnAbi, ParameterPassing, PhysicalType, ScalarExtension,
};
use hirsel_lir::ir::*;
use hirsel_lir::layout::{
    SEQUENCE_CAPACITY_OFFSET, align_up, payload_offset, sequence_elements_offset,
    sequence_slice_start, sequential_elements_offset,
};
use hirsel_lir::layout_types::{
    DropStep, Endian, FieldLayout, FinalizerRef, LayoutId, LayoutKind, LayoutRecord, LayoutTable,
    ObjectFormat, ScalarKind, ScanStep, SizeSpec, SizeTerm, TagForm, TargetSpec,
};
use hirsel_lir::rt_program::{self, StepKind};
use hirsel_lir::runtime_abi::{
    CName, COUNT_IMMORTAL, COUNT_OFFSET, COUNT_UNIQUE, CType, DomainTag, LAYOUT_DESCRIPTOR_VERSION,
    LAYOUT_MAGIC, RuntimeEntry, small_pool,
};
use hirsel_lir::stage::{
    DebugSource, InputIdentity, ObjectArtifact, ObjectSymbol, ObjectSymbolKind, Ready, Relocation,
    Reproducibility, StageKind, ValidatedUnit,
};
use hirsel_syntax::{ContentHash, DefPath};

use crate::compare::{self, CompareError, ORDER_DEPTH, ORDER_EQUAL, ORDER_GREATER, ORDER_LESS};
use crate::context::{
    Access, Attribute, AttributeSlot, BasicBlock, BinOp, Builder, Cast, Context, Function,
    IntPredicate, Module, TargetMachine, Type, Value, loaded_version,
};
use crate::glue;
use crate::object::ObjectFile;
use crate::scalar;
use crate::{CodegenError, PINNED_LLVM, VerifyStage};

mod bindings;
mod blocks;
mod callables;
mod calls;
mod constants;
mod construction;
mod descriptors;
mod flat;
mod flow;
mod instructions;
mod ownership;
mod pair;
mod places;
mod sequence_state;
mod signature;
mod values;

pub use pair::register_result;
pub(crate) use pair::{PairField, PairWord, REGISTER_PAIR, pair_type};
use pair::{decode_word, encode_word, scalar_shape, value_word};
use signature::{Hidden, Signature};

/// A word of the prefix an immortal block (static, region owner or arena block) carries
/// before its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrefixWord {
    Capacity,
    Align,
    Count,
    Descriptor,
}

/// The immortal prefix's words in address order, ending at the payload: the extent
/// `{ capacity, align }` (`hirsel_rt_v2_extent`), then the counted header
/// `{ count, descriptor }`. Static blocks, region owners and arena blocks are laid out
/// from here and [`PrefixWord::bytes`].
pub(crate) const IMMORTAL_PREFIX: [PrefixWord; 4] = [
    PrefixWord::Capacity,
    PrefixWord::Align,
    PrefixWord::Count,
    PrefixWord::Descriptor,
];

/// Bytes of the immortal prefix; the payload lies at the first multiple of its physical
/// alignment at or past them.
pub(crate) const IMMORTAL_PREFIX_BYTES: u64 = PrefixWord::Capacity.back();

impl PrefixWord {
    pub(crate) const fn bytes(self) -> u64 {
        match self {
            PrefixWord::Capacity | PrefixWord::Align => 8,
            PrefixWord::Count | PrefixWord::Descriptor => 4,
        }
    }

    /// Bytes from this word's start to the end of the prefix.
    pub(crate) const fn back(self) -> u64 {
        let (mut i, mut found, mut bytes) = (0, false, 0);
        while i < IMMORTAL_PREFIX.len() {
            found |= IMMORTAL_PREFIX[i] as u8 == self as u8;
            if found {
                bytes += IMMORTAL_PREFIX[i].bytes();
            }
            i += 1;
        }
        bytes
    }
}

/// The optimization pipeline run between the two verifier passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptLevel {
    /// `default<O0>`, with no code generation optimization.
    None,
    /// `default<O3>`.
    Speed,
}

impl OptLevel {
    fn pipeline(self) -> &'static str {
        match self {
            OptLevel::None => "default<O0>",
            OptLevel::Speed => "default<O3>",
        }
    }
}

/// The runtime failure codes of one spec rule: `hirsel_rt_v2_failure_begin`'s `kind` and
/// `rule` (the failure category and the rule's index), assigned by the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FailureCode {
    pub kind: u32,
    pub rule: u32,
}

/// Rule ID (`SEM-1`) to runtime failure codes. Every rule a failure edge starts must have one;
/// emission never invents a code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailureCodes {
    pub codes: BTreeMap<String, FailureCode>,
}

/// The immutable inputs of one emission. The emitter receives no provider, query engine or
/// linker, and keeps no reference to any input after it returns.
#[derive(Clone, Debug)]
pub struct EmitInputs<'a> {
    /// The frozen front-end input every unit was built from.
    pub input: &'a InputIdentity,
    pub target: &'a TargetSpec,
    /// The table every unit's receipt names.
    pub layouts: &'a LayoutTable,
    /// The operation manifest every unit's receipt names.
    pub manifest: &'a Manifest,
    pub failures: &'a FailureCodes,
    /// The bytes of each static a `ReadStatic` names.
    pub statics: &'a [hirsel_lir::statics::StaticData],
    /// Per layout with a user finalizer, the instance digest of its drop body's unit.
    pub finalizers: &'a BTreeMap<LayoutId, ContentHash>,
    /// Source specializations and known callable bodies replicated by the driver.
    pub inline_instances: &'a BTreeSet<ContentHash>,
    /// Small source units another object of the partition implements, emitted here as
    /// `available_externally` bodies so calls into them inline across objects.
    pub available_externally: &'a BTreeSet<ContentHash>,
    pub opt: OptLevel,
    pub registry: hirsel_lir::descriptor_registry::DescriptorRegistry,
    /// Instrument inline count edges. Set only when linking a measurement runtime archive.
    pub measurement: bool,
}

/// The pinned toolchain, as the object's reproducibility record names it.
fn toolchain() -> String {
    format!("llvm {PINNED_LLVM}")
}

/// C library symbols for byte comparisons and those LLVM's lowering may introduce for
/// `llvm.fma` on a target without fused multiply-add, `frem` and memory intrinsics, and the 128-bit
/// division and integer-to-float conversion routines every C toolchain's support
/// library (libgcc, compiler-rt) provides.
pub const TOOLCHAIN_IMPORTS: [&str; 17] = [
    "__divti3",
    "__floattidf",
    "__floattisf",
    "__floatuntidf",
    "__floatuntisf",
    "__modti3",
    "__udivti3",
    "__umodti3",
    "bcmp",
    "fma",
    "fmaf",
    "fmod",
    "fmodf",
    "memcmp",
    "memcpy",
    "memmove",
    "memset",
];

/// The link name of a definition (and specialized instance). Injective and C-identifier safe:
/// `_H1`, then the package name, package version, modules and name, each as its escaped
/// length and escaped text, then `I` and the instance digest's first 16 hex digits when an
/// instance is named, then `E`. Escaping keeps `[A-Za-z0-9]`, doubles `_`, and writes any
/// other byte as `_` and two lowercase hex digits.
pub fn symbol_name(definition: &DefPath, instance: Option<&ContentHash>) -> String {
    fn escaped(out: &mut String, text: &str) {
        let mut piece = String::new();
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => piece.push(byte as char),
                b'_' => piece.push_str("__"),
                _ => piece.push_str(&format!("_{byte:02x}")),
            }
        }
        out.push_str(&piece.len().to_string());
        out.push_str(&piece);
    }
    let path = &definition.0;
    let mut name = String::from("_H1");
    escaped(&mut name, &path.package.name);
    escaped(&mut name, &path.package.version);
    for module in &path.modules {
        escaped(&mut name, module);
    }
    escaped(&mut name, &path.name);
    if let Some(instance) = instance {
        name.push('I');
        for byte in &instance.0[..8] {
            name.push_str(&format!("{byte:02x}"));
        }
    }
    name.push('E');
    name
}

/// The link name of a definition instance's function-reference thunk: the entry an indirect
/// call reaches through a callable made from the definition, taking (and ignoring) an
/// environment after the context. It is `symbol_name` with `T` after the `_H1` prefix; a
/// `symbol_name` segment always starts with a digit there, so no thunk name is a unit's name
/// and thunk names are injective as `symbol_name` is.
pub fn function_thunk_name(definition: &DefPath, instance: Option<&ContentHash>) -> String {
    let name = symbol_name(definition, instance);
    format!("_H1T{}", &name[3..])
}

/// A digest names the layout's one address identity across every object.
fn descriptor_name(layout: &LayoutRecord) -> String {
    let digest = hirsel_lir::snapshot::digest("runtime-layout-descriptor", layout);
    let hex: String = digest.0.iter().map(|b| format!("{b:02x}")).collect();
    format!("hirsel.layout.{hex}")
}

/// The `site` code a failure passes to `hirsel_rt_v2_failure_begin`: the first eight bytes of
/// a digest of the definition, its implementation revision and the anchor's byte range. The
/// observation layer maps it back by recomputing it for the input's anchors.
pub fn site_code(definition: &DefPath, revision: &ContentHash, site: SiteRange) -> u64 {
    let digest = ContentHash::of_parts(
        "hirsel.native-site.v1",
        [
            definition.fingerprint().0.as_slice(),
            revision.0.as_slice(),
            &site.start.to_le_bytes(),
            &site.end.to_le_bytes(),
        ],
    );
    u64::from_le_bytes(digest.0[..8].try_into().expect("eight bytes")).max(1)
}

/// Checks that the loaded libLLVM is the pin and that LLVM's DataLayout for `target` agrees
/// with every fact `TargetSpec` pins: the layout string, byte order, pointer width and each
/// scalar's size and ABI alignment. LLVM's answer is a cross-check, never adopted.
pub fn check_target(target: &TargetSpec) -> Result<(), CodegenError> {
    check_loaded()?;
    machine(target, false).map(drop)
}

fn check_loaded() -> Result<(), CodegenError> {
    let loaded = loaded_version();
    if loaded != PINNED_LLVM {
        return Err(CodegenError::LoadedLlvm { loaded });
    }
    Ok(())
}

fn machine(target: &TargetSpec, optimize: bool) -> Result<TargetMachine, CodegenError> {
    let machine =
        TargetMachine::new(target, optimize).map_err(|message| CodegenError::TargetMachine {
            triple: target.triple.clone(),
            message,
        })?;
    let facts = machine.data_layout();
    let mismatch = |what: &str, expected: String, actual: String| CodegenError::DataLayout {
        triple: target.triple.clone(),
        what: what.to_owned(),
        expected,
        actual,
    };
    if facts.text != target.llvm_data_layout {
        return Err(mismatch(
            "layout string",
            target.llvm_data_layout.clone(),
            facts.text,
        ));
    }
    let little = target.endian == Endian::Little;
    if facts.little_endian != little {
        return Err(mismatch(
            "byte order",
            format!("little endian {little}"),
            format!("little endian {}", facts.little_endian),
        ));
    }
    if facts.pointer_bytes != target.pointer_bytes {
        return Err(mismatch(
            "pointer bytes",
            target.pointer_bytes.to_string(),
            facts.pointer_bytes.to_string(),
        ));
    }
    let kinds = [
        ScalarKind::I8,
        ScalarKind::I16,
        ScalarKind::I32,
        ScalarKind::I64,
        ScalarKind::I128,
        ScalarKind::F32,
        ScalarKind::F64,
        ScalarKind::Pointer,
    ];
    for (kind, (size, align)) in kinds.iter().zip(facts.scalars) {
        let expected = (target.scalar_size(*kind), target.scalar_align(*kind));
        if (size, align) != expected {
            return Err(mismatch(
                &format!("{kind:?} size and alignment"),
                format!("{expected:?}"),
                format!("{:?}", (size, align)),
            ));
        }
    }
    Ok(machine)
}

/// Compiles textual LLVM IR to an object for `target`: the toolchain smoke path.
///
/// The IR may leave the triple and DataLayout unset; if it sets them they must be the
/// target's. The verifier runs before and after the optimization pipeline. Every LLVM handle
/// is dropped before the object is returned, so it owns all of its bytes.
pub fn compile_ir(
    ir: &str,
    target: &TargetSpec,
    opt: OptLevel,
) -> Result<ObjectFile, CodegenError> {
    check_loaded()?;
    let bytes = {
        let machine = machine(target, opt != OptLevel::None)?;
        let context = Context::new();
        let module = Module::parse(&context, "hirsel", ir)
            .map_err(|message| CodegenError::Parse { message })?;
        let triple = module.triple();
        if !triple.is_empty() && triple != target.triple {
            return Err(CodegenError::Triple {
                expected: target.triple.clone(),
                actual: triple,
            });
        }
        let layout = module.data_layout();
        if !layout.is_empty() && layout != target.llvm_data_layout {
            return Err(CodegenError::DataLayout {
                triple: target.triple.clone(),
                what: "module layout string".into(),
                expected: target.llvm_data_layout.clone(),
                actual: layout,
            });
        }
        module.set_target(&machine);
        optimize_and_emit(&module, &machine, opt)?
    };
    ObjectFile::read(target, bytes)
}

fn optimize_and_emit(
    module: &Module<'_>,
    machine: &TargetMachine,
    opt: OptLevel,
) -> Result<Vec<u8>, CodegenError> {
    optimize(module, machine, opt)?;
    module
        .emit_object(machine)
        .map_err(|message| CodegenError::Emit { message })
}

fn optimize(
    module: &Module<'_>,
    machine: &TargetMachine,
    opt: OptLevel,
) -> Result<(), CodegenError> {
    module.verify().map_err(|message| CodegenError::Verify {
        stage: VerifyStage::BeforeOptimization,
        message,
    })?;
    module
        .run_passes(opt.pipeline(), machine)
        .map_err(|message| CodegenError::Passes { message })?;
    crate::runtime::expand_fast_paths(module);
    module
        .run_passes("always-inline", machine)
        .map_err(|message| CodegenError::Passes { message })?;
    module.verify().map_err(|message| CodegenError::Verify {
        stage: VerifyStage::AfterOptimization,
        message,
    })
}

/// Checks a unit's receipt against the inputs emission was given. A receipt binds the unit,
/// the layout table, the target, the manifest, the validator and the front-end input; any
/// other value means the unit was validated against something else.
fn check_receipt(unit: &ValidatedUnit<Ready>, inputs: &EmitInputs<'_>) -> Result<(), CodegenError> {
    let raw = unit.receipt().raw();
    let header = &unit.unit().header;
    let reject = |problem: &str| CodegenError::Receipt {
        definition: header.definition.clone(),
        problem: problem.to_owned(),
    };
    if raw.stage != StageKind::Ready || header.stage != StageKind::Ready {
        return Err(reject("the receipt is not for the Ready stage"));
    }
    if raw.unit != unit.unit().digest() {
        return Err(reject("the receipt names another unit"));
    }
    if raw.lir_version != hirsel_lir::LIR_VERSION || header.lir_version != hirsel_lir::LIR_VERSION {
        return Err(reject("the unit is of another LIR version"));
    }
    if raw.validator_version != hirsel_lir::VALIDATOR_VERSION {
        return Err(reject("the receipt is from another validator version"));
    }
    let target = inputs.target.digest();
    if raw.target != target || header.target != target || inputs.layouts.target != target {
        return Err(reject("the receipt names another target"));
    }
    let layouts = inputs.layouts.digest();
    if raw.layouts != layouts || header.layouts != layouts {
        return Err(reject("the receipt names another layout table"));
    }
    if raw.manifest != hirsel_lir::snapshot::digest("manifest", inputs.manifest) {
        return Err(reject("the receipt names another operation manifest"));
    }
    if raw.input != *inputs.input || header.input != *inputs.input {
        return Err(reject("the receipt names another front-end input"));
    }
    if inputs.input.interface_version != hirsel_lir::INTERFACE_VERSION {
        return Err(reject("the input is of another interface version"));
    }
    Ok(())
}

/// The symbols the emitter declared: what the object must export and may import.
#[derive(Default)]
struct Manifested {
    exports: BTreeMap<String, ObjectSymbol>,
    imports: BTreeMap<String, ObjectSymbol>,
    /// The `linkonce_odr` library units the object defines (LIR schema revision 7).
    library: BTreeSet<String>,
    /// Shared runtime descriptor and step globals emitted by this object.
    descriptors: BTreeSet<String>,
}

/// Emits validated Ready units as one relocatable object for `inputs.target`.
pub fn emit_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    emit_object_profile(units, inputs, None)
}

/// Emit optional DWARF line tables and, with measurement, source-site call scopes.
pub fn emit_object_profile(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    sources: Option<&crate::ProfileSources>,
) -> Result<ObjectArtifact, CodegenError> {
    let (bytes, manifested) = lower(units, inputs, sources, |module, machine| {
        optimize_and_emit(module, machine, inputs.opt)
    })?;
    let file = ObjectFile::read(inputs.target, bytes)?;
    let mut artifact = artifact(file, manifested, units, inputs)?;
    if sources.is_some() {
        artifact.reproducibility.options = ContentHash::of_parts(
            "hirsel.profile-lines-options.v1",
            [artifact.reproducibility.options.0.as_slice()],
        );
    }
    Ok(artifact)
}

/// The verified, optimized LLVM IR `emit_object` would emit as an object: the text
/// codegen controls inspect.
pub fn emit_optimized_ir(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    lower(units, inputs, None, |module, machine| {
        optimize(module, machine, inputs.opt)?;
        Ok(module.text())
    })
    .map(|(text, _)| text)
}

/// The verified module text before optimization, for IR-level controls.
pub fn emit_ir(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    lower(units, inputs, None, |module, _| Ok(module.text())).map(|(text, _)| text)
}

/// Lowers `units` into one module for the target and hands it to `finish`.
fn lower<T>(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    sources: Option<&crate::ProfileSources>,
    finish: impl FnOnce(&Module<'_>, &TargetMachine) -> Result<T, CodegenError>,
) -> Result<(T, Manifested), CodegenError> {
    check_loaded()?;
    for unit in units {
        check_receipt(unit, inputs)?;
    }
    if !inputs.registry.validates(inputs.layouts, inputs.finalizers) {
        return Err(CodegenError::Manifest {
            problem: "foreign or noncanonical descriptor registry".into(),
        });
    }
    let mut manifested = Manifested::default();
    let finished = {
        let machine = machine(inputs.target, inputs.opt != OptLevel::None)?;
        let context = Context::new();
        let module = Module::new(&context, "hirsel");
        module.set_target(&machine);
        let debug =
            sources.map(|_| crate::context::DebugInfo::new(&module, inputs.opt != OptLevel::None));
        let mut functions = Vec::new();
        for unit in units {
            let unit = unit.unit();
            let abi = fn_abi(inputs.layouts, unit.header.abi, &unit.header.definition)?;
            let name = symbol_name(&unit.header.definition, Some(&unit.header.instance));
            if module.defines(&name)
                || manifested.exports.contains_key(&name)
                || manifested.library.contains(&name)
            {
                return Err(CodegenError::DuplicateSymbol { name });
            }
            let external = Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?;
            let signature = if unit.header.origin == UnitOrigin::Library {
                Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?
                    .with_roots(&module, inputs.layouts)
            } else {
                Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?
            };
            let body_name = if signature.roots.is_empty() {
                name.clone()
            } else {
                format!("{name}{ROOTS_SUFFIX}")
            };
            let function = signature.declare(&module, &body_name, &unit.header.definition, true)?;
            let entry = if signature.roots.is_empty() {
                function
            } else {
                let entry = external.declare(&module, &name, &unit.header.definition, true)?;
                define_root_adapter(
                    &module,
                    entry,
                    function,
                    &signature,
                    &external,
                    inputs.layouts,
                );
                module.make_linkonce_odr(entry);
                module.add_attribute(entry, AttributeSlot::Function, Attribute::InlineHint);
                manifested.library.insert(body_name);
                entry
            };
            // A small source unit another object implements and exports: here only an
            // inlinable body. Calls the optimizer keeps stay imports of that export.
            if inputs.available_externally.contains(&unit.header.instance) {
                module.make_available_externally(function);
                functions.push((unit, function, signature));
                continue;
            }
            // A library unit is in every object whose units call it, `linkonce_odr` and
            // `inlinehint`, so a partition never changes whether it inlines. It is no
            // export of the object: the copy may be gone once every call is inlined.
            if unit.header.origin == UnitOrigin::Library
                || inputs.inline_instances.contains(&unit.header.instance)
            {
                module.make_linkonce_odr(function);
                let attribute = if unit.header.definition.is_core("list.push")
                    || unit.header.definition.is_core("$native.list_push_spare")
                {
                    Attribute::AlwaysInline
                } else {
                    Attribute::InlineHint
                };
                module.add_attribute(function, AttributeSlot::Function, attribute);
                if unit.header.definition.is_core("$native.list_grow") {
                    module.add_attribute(function, AttributeSlot::Function, Attribute::Cold);
                    module.add_attribute(function, AttributeSlot::Function, Attribute::NoInline);
                }
                manifested.library.insert(name);
                if abi.shape == CallableShape::Definition {
                    // A library operation can be a FunctionRef (integer Order
                    // dictionaries). Its callable thunk has the same linkage
                    // as its body and is present in every calling object.
                    let name =
                        function_thunk_name(&unit.header.definition, Some(&unit.header.instance));
                    let thunk = define_function_thunk(&module, &name, entry, &external);
                    module.make_linkonce_odr(thunk);
                    module.add_attribute(thunk, AttributeSlot::Function, Attribute::InlineHint);
                    manifested.library.insert(name);
                }
                functions.push((unit, function, signature));
                continue;
            }
            manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name,
                    kind: ObjectSymbolKind::Function,
                    definition: Some(unit.header.definition.clone()),
                    abi: Some(abi_digest(abi)),
                },
            );
            if abi.shape == CallableShape::Definition {
                let thunk =
                    function_thunk_name(&unit.header.definition, Some(&unit.header.instance));
                define_function_thunk(&module, &thunk, function, &signature);
                manifested.exports.insert(
                    thunk.clone(),
                    ObjectSymbol {
                        name: thunk,
                        kind: ObjectSymbolKind::Function,
                        definition: Some(unit.header.definition.clone()),
                        abi: Some(abi_digest(abi)),
                    },
                );
            }
            functions.push((unit, function, signature));
        }
        for (unit, function, signature) in functions {
            let mut emitter =
                FnEmitter::new(&module, inputs, unit, function, signature, &mut manifested);
            if let (Some(debug), Some(source)) = (
                debug.as_ref(),
                sources.and_then(|s| s.get(&unit.header.definition)),
            ) {
                let site =
                    unit.tables.source_anchors[unit.blocks[unit.entry.index()].anchor.index()].site;
                let (line, _) = source
                    .location(site.start)
                    .map(|p| (p.line, p.column))
                    .unwrap_or((1, 1));
                let scope = debug.function(
                    function,
                    &source.path.to_string_lossy(),
                    &unit.header.definition.to_string(),
                    line,
                );
                emitter.profile = Some((debug, scope, source));
                debug.location(&emitter.prologue, scope, line, 1);
                debug.location(&emitter.builder, scope, line, 1);
            }
            emitter.registry_table()?;
            emitter.emit()?;
        }
        // An import the module also defines is internal to the object.
        manifested.imports.retain(|name, _| {
            !manifested.exports.contains_key(name) && !manifested.library.contains(name)
        });
        if let Some(debug) = &debug {
            debug.finalize();
        }
        finish(&module, &machine)?
    };
    Ok((finished, manifested))
}

/// Compares the object's symbol interface with the manifest and builds the artifact.
fn artifact(
    file: ObjectFile,
    manifested: Manifested,
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    let declared: Vec<&String> = manifested.exports.keys().collect();
    let exported: Vec<&String> = file
        .exports
        .iter()
        .filter(|name| {
            !manifested.library.contains(*name) && !manifested.descriptors.contains(*name)
        })
        .collect();
    if exported != declared {
        return Err(CodegenError::Manifest {
            problem: format!(
                "the object exports {:?}, the manifest {:?}",
                file.exports, declared
            ),
        });
    }
    let mut imports = Vec::new();
    for name in &file.imports {
        let symbol = match manifested.imports.get(name) {
            Some(symbol) => symbol.clone(),
            None if TOOLCHAIN_IMPORTS.contains(&name.as_str()) => ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: None,
                abi: None,
            },
            None => {
                return Err(CodegenError::Manifest {
                    problem: format!("the object imports `{name}`, which the manifest does not"),
                });
            }
        };
        imports.push(symbol);
    }
    let relocations = file
        .relocations
        .iter()
        .map(|r| Relocation {
            offset: r.offset,
            symbol: file
                .imports
                .iter()
                .position(|name| *name == r.symbol)
                .expect("a relocation's import is in the import list") as u32,
            kind: r.kind,
            addend: r.addend,
        })
        .collect();
    let exports: Vec<ObjectSymbol> = manifested.exports.into_values().collect();
    let abi = ContentHash::of_parts(
        "hirsel.object-abi.v1",
        exports
            .iter()
            .filter_map(|s| s.abi.as_ref())
            .map(|h| h.0.as_slice()),
    );
    let mut options = format!(
        "{:?} {} {} {}",
        inputs.opt,
        inputs.target.triple,
        inputs.target.cpu,
        inputs.target.features.join(",")
    );
    if inputs.measurement {
        options.push_str(" measurement");
    }
    let mut debug: Vec<DebugSource> = units
        .iter()
        .map(|u| DebugSource {
            definition: u.unit().header.definition.clone(),
            implementation_revision: u.unit().header.implementation_revision,
        })
        .collect();
    debug.sort_by(|a, b| a.definition.cmp(&b.definition));
    debug.dedup();
    Ok(ObjectArtifact {
        input: inputs.input.clone(),
        target: inputs.target.digest(),
        format: ObjectFormat::Elf,
        bytes: file.bytes,
        exports,
        imports,
        relocations,
        debug,
        abi,
        layouts: inputs.layouts.digest(),
        reproducibility: Reproducibility {
            toolchain: toolchain(),
            options: ContentHash::of_parts("hirsel.native-options.v1", [options.as_bytes()]),
            lir_version: hirsel_lir::LIR_VERSION,
            runtime_abi_version: hirsel_lir::RUNTIME_ABI_VERSION,
        },
    })
}

/// Defines `name`, the function-reference thunk of the definition entry `target`: the
/// callable ABI of the same declared type (an environment after the context), forwarding
/// every other argument and what the entry returns.
fn define_function_thunk<'m>(
    module: &'m Module<'m>,
    name: &str,
    target: Function<'m>,
    signature: &Signature<'m, '_>,
) -> Function<'m> {
    let count = signature.types.len();
    let mut types = signature.types.clone();
    types.insert(1, module.ptr_type());
    let ty = module.function_type(signature.returns, &types);
    let thunk = module
        .function(name, ty)
        .expect("a thunk name is never declared with another type");
    if signature.returns_status() {
        module.add_attribute(thunk, AttributeSlot::Return, Attribute::ZeroExt);
    }
    let builder = Builder::new(module);
    builder.position(module.append_block(thunk));
    let mut arguments = vec![module.parameter(thunk, 0)];
    arguments.extend((1..count as u32).map(|i| module.parameter(thunk, i + 1)));
    let status = builder.call(target, &arguments);
    signature.attribute_call(module, status);
    builder.ret(status);
    thunk
}

/// The public address ABI is kept at function-reference and external boundaries.
/// Direct library calls enter the root-value body instead.
fn define_root_adapter<'m>(
    module: &'m Module<'m>,
    entry: Function<'m>,
    target: Function<'m>,
    signature: &Signature<'m, '_>,
    external: &Signature<'m, '_>,
    layouts: &LayoutTable,
) {
    let builder = Builder::new(module);
    builder.position(module.append_block(entry));
    let original_arity = signature
        .companions
        .iter()
        .flatten()
        .copied()
        .min()
        .unwrap_or(signature.types.len() as u32);
    let mut args: Vec<_> = (0..original_arity)
        .map(|i| module.parameter(entry, i))
        .collect();
    for &i in &signature.roots {
        let first = signature.first[i] as usize;
        args[first] = builder.load(module.ptr_type(), args[first], 8);
    }
    for (field, &i) in signature.roots.iter().enumerate() {
        if signature.companions[field].is_some() {
            let pointer = args[signature.first[i] as usize];
            let data = builder.offset(
                pointer,
                module.const_int(
                    module.int_type(64),
                    u128::from({
                        let LayoutKind::Block { payload } = layouts
                            .layout(signature.abi.parameters[i].layout)
                            .expect("root layout")
                            .kind
                        else {
                            unreachable!("sequence block")
                        };
                        payload_offset(layouts.layout(payload).expect("sequence payload").align)
                            .expect("validated alignment")
                    }),
                ),
            );
            let length = builder.load_access(module.int_type(64), data, 8, Access::Length);
            // Not deferred: the body stores the count it sets, as the external caller reads it.
            args.extend([
                length,
                module.const_zero(module.int_type(1)),
                module.const_zero(module.int_type(1)),
            ]);
        }
    }
    let returned = builder.call(target, &args);
    signature.attribute_call(module, returned);
    for (field, &i) in signature.roots.iter().enumerate() {
        builder.store(
            signature.root_pointer(&builder, returned, field),
            module.parameter(entry, signature.first[i]),
            8,
        );
    }
    let status = signature.status(&builder, returned);
    let value = signature.result(&builder, returned);
    builder.ret(external.returned(&builder, module.const_zero(external.returns), status, value));
}

fn fn_abi<'a>(
    layouts: &'a LayoutTable,
    id: hirsel_lir::FnAbiId,
    definition: &DefPath,
) -> Result<&'a FnAbi, CodegenError> {
    layouts
        .fn_abis
        .get(id.0 as usize)
        .ok_or_else(|| CodegenError::Invalid {
            definition: definition.clone(),
            what: format!("no FnAbi {}", id.0),
        })
}

/// Bytes of a `hirsel_rt_v2_region` record (`HIRSEL_RT_V2_ASSERT_SIZE` in the header).
const REGION_RECORD_BYTES: u64 = 48;

/// The name suffix of a library body's private root-value entry. It changes with that
/// entry's signature, so a `linkonce_odr` body of an older signature never merges
/// with a caller of this one.
const ROOTS_SUFFIX: &str = ".roots8";

/// The private descriptor of the empty environment a function reference carries.

/// How a value is held in SSA form: unit, one scalar, or the bytes of a fixed-size inline
/// value (aggregates, variants, callables, handles), moved whole and read through memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Unit,
    Scalar(ScalarKind),
    /// Keep code and environment as pointers through SSA, without byte packing.
    Callable,
    /// An LLVM array of the widest integer that divides both `size` and `align`, so an
    /// aligned pointer or word field stays one store when SROA splits the value.
    Bytes {
        size: u64,
        align: u32,
    },
}

/// One field's FlatFormatV0 bytes in a flat construction: a fixed field's value, or an
/// encoded source's `length` bytes at `bytes`, written behind a u64 length prefix.
#[derive(Clone, Copy)]
enum FlatPart<'m> {
    Fixed {
        value: ValueId,
        size: u64,
    },
    Encoded {
        bytes: Value<'m>,
        length: Value<'m>,
    },
    Sequence {
        source: Place<'m>,
        encoding: LayoutId,
    },
    /// A flat list builder's payload (LIR schema revision 22): its count and
    /// used bytes are the list's count and content, copied whole.
    Built {
        data: Value<'m>,
        first: u64,
    },
    /// A byte field an open element built in place (LIR schema revision 24):
    /// its length word and `length` bytes are already written.
    InPlace {
        length: Value<'m>,
    },
}

/// A prepared address: its pointer, layout and the alignment it is known to have.
#[derive(Clone, Copy, Debug)]
struct Place<'m> {
    ptr: Value<'m>,
    layout: LayoutId,
    align: u32,
    /// The counted block (its header) whose payload holds this place, if any.
    block: Option<Value<'m>>,
}

pub(crate) fn low_bit(n: u64) -> u32 {
    if n == 0 {
        u32::MAX
    } else {
        1u32 << n.trailing_zeros().min(31)
    }
}

/// The counted sequence payload a slice of `element` owns: the table derives it with the
/// slice, whose owner word is that block's header address.
pub(crate) fn slice_storage(layouts: &LayoutTable, element: LayoutId) -> Option<LayoutId> {
    layouts
        .layouts
        .iter()
        .position(|l| l.kind == LayoutKind::Sequence { element })
        .map(|i| LayoutId(i as u32))
}

/// The u32 variant ordinal at `offset` bytes from `base` (aligned to `align`), as an `i32`.
/// Every tag read in this crate reaches it, the comparator's included.
pub(crate) fn load_tag<'m>(
    builder: &Builder<'m>,
    base: Value<'m>,
    align: u32,
    offset: u64,
    form: TagForm,
    unaligned: bool,
) -> Value<'m> {
    let module = builder.module();
    let i64_ty = module.int_type(64);
    let i32_ty = module.int_type(32);
    let at = if offset == 0 {
        base
    } else {
        builder.offset(base, module.const_int(i64_ty, u128::from(offset)))
    };
    let align = |width: u32| {
        if unaligned {
            1
        } else {
            align.min(low_bit(offset)).min(width)
        }
    };
    match form {
        TagForm::Ordinal => builder.load(i32_ty, at, align(4)),
        TagForm::LiveBit { bit } => {
            let word = builder.load(i64_ty, at, align(8));
            let set = builder.binary(BinOp::And, word, module.const_int(i64_ty, 1u128 << bit));
            let live = builder.icmp(IntPredicate::Ne, set, module.const_zero(i64_ty));
            builder.select(
                live,
                module.const_int(i32_ty, 1),
                module.const_int(i32_ty, 0),
            )
        }
        // A null-pointer niche: the dataful variant's block reference word is non-null, the
        // fieldless variant's null.
        TagForm::Niche { nullary, dataful } => {
            let ptr_ty = module.ptr_type();
            let word = builder.load(ptr_ty, at, align(8));
            let null = builder.icmp(IntPredicate::Eq, word, module.const_zero(ptr_ty));
            builder.select(
                null,
                module.const_int(i32_ty, u128::from(nullary)),
                module.const_int(i32_ty, u128::from(dataful)),
            )
        }
    }
}

/// Companion storage is private and promotable just like the root itself.
/// `unique` is a must fact: false asks for a real count test, never claims sharedness.
#[derive(Clone, Copy)]
struct SequenceState<'m> {
    unique: Value<'m>,
    length: Value<'m>,
    /// The slot holds a live block whose count word may lag `length`: a
    /// `SeqSetLen` here, or a private root call or entry, deferred its store.
    /// A flush stores it (only where it differs, so a static block is never
    /// written) before anything reads the count from memory.
    dirty: Value<'m>,
}

/// Emits one Ready unit into its declared function.
struct FnEmitter<'m, 'a> {
    module: &'m Module<'m>,
    inputs: &'a EmitInputs<'a>,
    unit: &'a LirUnit<Ready>,
    function: Function<'m>,
    signature: Signature<'m, 'a>,
    manifested: &'a mut Manifested,
    builder: Builder<'m>,
    profile: Option<(
        &'a crate::context::DebugInfo<'m>,
        crate::context::DebugScope,
        &'a crate::ProfileSource,
    )>,
    /// Positioned at the end of the prologue, which holds every stack slot.
    prologue: Builder<'m>,
    prologue_block: BasicBlock<'m>,
    context: Value<'m>,
    values: Vec<Option<Value<'m>>>,
    /// Storage version names: never operands, so they have no LLVM value.
    versions: BTreeSet<ValueId>,
    slots: Vec<Option<Place<'m>>>,
    sequence_states: Vec<Option<SequenceState<'m>>>,
    read_roots: Vec<Option<SlotId>>,
    preserved_lengths: Vec<Option<Value<'m>>>,
    /// Slots whose count store can be deferred (`SequenceState::dirty`): a
    /// sequence state's slot that a `SeqSetLen`, a private root call or the
    /// entry (a root parameter's companions) can leave ahead of its block.
    deferred: Vec<bool>,
    /// Deferred slots this unit reads whole without taking them (a borrow or a
    /// bitwise read): such a view, whose length is not preserved, may read the
    /// slot's block from memory.
    viewed: Vec<bool>,
    /// Slots of root parameters that hand their length back to the caller.
    companion_slots: Vec<bool>,
    /// A root parameter slot's `defer` companion: false when its caller keeps the
    /// count in the block, so a `SeqSetLen` stores it and the return flushes.
    defer_params: Vec<Option<Value<'m>>>,
    fresh_detaches: BTreeSet<ValueId>,
    /// The preserved length of the view an address is projected from, for its
    /// first step: an index into it bounds by that length, not the count word.
    known_count: Option<Value<'m>>,
    /// The instruction being emitted: its block and index.
    at: (BlockId, usize),
    /// A builder's release whose block a flat construction may have taken
    /// over, and whether it did (`adopt_builder`).
    adopted: BTreeMap<ValueId, Value<'m>>,
    addresses: Vec<Option<Place<'m>>>,
    /// Per address of a variable flat element, where its sequence's elements begin
    /// (`FlatNext` offsets count from there; LIR schema revision 7).
    flat_contents: Vec<Option<Value<'m>>>,
    /// Set by the last variable flat element step `project` took.
    flat_content: Option<Value<'m>>,
    blocks: Vec<BasicBlock<'m>>,
    /// Per block, a phi per parameter (None for version joins).
    phis: Vec<Vec<Option<Value<'m>>>>,
    failures: BTreeMap<FailureId, BasicBlock<'m>>,
    /// The environment block a closure body was entered with.
    environment: Option<Value<'m>>,
    /// Per compiler-local region instance, its `hirsel_rt_v2_region` record in the frame.
    regions: Vec<Value<'m>>,
    /// A register-result entry's one return block and its status and word phis.
    register_return: Option<(BasicBlock<'m>, Value<'m>, Value<'m>)>,
}

type Emitted<T = ()> = Result<T, CodegenError>;

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_lir::stage::InputIdentity;
    use hirsel_syntax::{ItemPath, PackageId};

    fn definition(name: &str) -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "fixture".into(),
                version: "0.1".into(),
            },
            modules: vec!["m_n".into()],
            name: name.into(),
        })
    }

    /// The immortal prefix is the runtime's extent and then its counted header, word by
    /// word (`hirsel_rt_v2.h`): the header's `HEADER_SIZE` bytes end at the prefix, with the
    /// count at `COUNT_OFFSET` and the descriptor at `LAYOUT_OFFSET`, and the extent's
    /// capacity, then its alignment, lie `IMMORTAL_CAPACITY_BACK` bytes before the header.
    /// A change to `IMMORTAL_PREFIX` or `PrefixWord::bytes` fails here, not in a
    /// capacity query that misreads a static block.
    #[test]
    fn the_immortal_prefix_is_the_runtime_extent_then_header() {
        use hirsel_lir::runtime_abi::{HEADER_SIZE, IMMORTAL_CAPACITY_BACK, LAYOUT_OFFSET};
        let at = |word: PrefixWord| IMMORTAL_PREFIX_BYTES - word.back();
        let header = IMMORTAL_PREFIX_BYTES - HEADER_SIZE;
        assert_eq!(at(PrefixWord::Count), header + COUNT_OFFSET);
        assert_eq!(at(PrefixWord::Descriptor), header + LAYOUT_OFFSET);
        assert_eq!(
            PrefixWord::Count.bytes() + PrefixWord::Descriptor.bytes(),
            HEADER_SIZE
        );
        assert_eq!(header - at(PrefixWord::Capacity), IMMORTAL_CAPACITY_BACK);
        assert_eq!(
            at(PrefixWord::Align),
            at(PrefixWord::Capacity) + PrefixWord::Capacity.bytes()
        );
        assert_eq!(at(PrefixWord::Capacity), 0);
        assert_eq!(IMMORTAL_PREFIX_BYTES, 24);
    }

    /// Since `.roots5`, a returned root's pointer word is the pointer itself, for sequence
    /// roots too, whose must-unique fact has a word of its own. Under `.roots4` the
    /// sequence pointer carried that fact in its low bits, and every loop over a
    /// root call or inlined body masked and re-tagged it (`ptrtoint`, `and -4`,
    /// `inttoptr`) per iteration; a headerless payload's low bits are kept the same way.
    #[test]
    fn root_returns_carry_pointers_untagged() {
        use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbiKey};
        let context = Context::new();
        let module = Module::new(&context, "root-pointers");
        let abi = FnAbi {
            key: FnAbiKey {
                callable: ContentHash::of_bytes(b"root"),
                abi_version: 1,
            },
            shape: CallableShape::Definition,
            multiplicity: CallMultiplicity::Many,
            context: true,
            environment: false,
            destination: false,
            result: None,
            result_arena: None,
            diverges: false,
            parameters: vec![],
            version: 1,
        };
        let (ptr, i64_ty, i1) = (module.ptr_type(), module.int_type(64), module.int_type(1));
        for sequence in [false, true] {
            let mut signature = Signature::runtime(&module, &abi, &definition("root")).unwrap();
            signature.roots = vec![0];
            signature.sequence_roots = vec![sequence];
            if sequence {
                signature.uniques = vec![Some(2)];
                signature.status_field = 3;
                signature.returns = module.struct_type(&[ptr, i64_ty, i1, i64_ty]);
            } else {
                signature.uniques = vec![None];
                signature.returns = module.struct_type(&[ptr, i64_ty]);
            }
            let builder = Builder::new(&module);
            let result_ty = module.struct_type(&[ptr, i1, module.int_type(8)]);
            let function = module
                .function(
                    &format!("roots{}", u8::from(sequence)),
                    module.function_type(result_ty, &[ptr, i1, i64_ty]),
                )
                .unwrap();
            builder.position(module.append_block(function));
            let mut returned = module.const_zero(signature.returns);
            returned = builder.insert(returned, module.parameter(function, 0), 0);
            returned = builder.insert(
                returned,
                module.parameter(function, 2),
                signature.status_field,
            );
            let unique = if sequence {
                returned = builder.insert(returned, module.parameter(function, 1), 2);
                signature.root_unique(&builder, returned, 0)
            } else {
                module.const_zero(i1)
            };
            let pointer = signature.root_pointer(&builder, returned, 0);
            let status = signature.status(&builder, returned);
            let mut result = builder.insert(module.const_zero(result_ty), pointer, 0);
            result = builder.insert(result, unique, 1);
            builder.ret(builder.insert(result, status, 2));
        }
        module.verify().unwrap();
        let text = module.text();
        for cast in ["ptrtoint", "inttoptr", " and ", " or ", " lshr "] {
            assert!(!text.contains(cast), "{cast} in {text}");
        }
    }

    #[test]
    fn link_names_are_injective_c_identifiers() {
        let a = symbol_name(&definition("f"), None);
        // `fixture`, `0.1` (`.` escaped), `m_n` (`_` doubled), `f`.
        assert_eq!(a, "_H17fixture50_2e14m__n1fE");
        let instance = ContentHash::of_bytes(b"i");
        let b = symbol_name(&definition("f"), Some(&instance));
        assert!(b.starts_with("_H17fixture") && b.contains('I') && b.ends_with('E'));
        assert_ne!(a, b);
        // `a_b` in one segment and `a`, `b` in two never collide.
        let mut split = definition("b");
        split.0.modules = vec!["a".into()];
        let mut joined = definition("a_b");
        joined.0.modules = Vec::new();
        assert_ne!(symbol_name(&split, None), symbol_name(&joined, None));
        // Function-reference thunks: a marker no unit name has, and injective as unit names
        // are, so no thunk collides with a unit or another thunk.
        let names = [
            (definition("f"), None),
            (definition("f"), Some(instance)),
            (split.clone(), None),
            (joined.clone(), None),
            (definition("T"), None),
        ];
        let mut all = BTreeSet::new();
        for (path, instance) in &names {
            let unit = symbol_name(path, instance.as_ref());
            let thunk = function_thunk_name(path, instance.as_ref());
            assert!(thunk.starts_with("_H1T") && !unit.starts_with("_H1T"));
            assert!(all.insert(unit) && all.insert(thunk));
        }
        assert_eq!(
            function_thunk_name(&definition("f"), None),
            "_H1T7fixture50_2e14m__n1fE"
        );
        let unicode = symbol_name(&definition("é"), None);
        assert!(
            unicode
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "{unicode}"
        );
    }

    fn manifested(exports: &[&str], imports: &[&str]) -> Manifested {
        let symbol = |name: &&str| {
            (
                name.to_string(),
                ObjectSymbol {
                    name: name.to_string(),
                    kind: ObjectSymbolKind::Function,
                    definition: None,
                    abi: None,
                },
            )
        };
        Manifested {
            exports: exports.iter().map(symbol).collect(),
            imports: imports.iter().map(symbol).collect(),
            library: BTreeSet::new(),
            descriptors: BTreeSet::new(),
        }
    }

    fn file(exports: &[&str], imports: &[&str]) -> ObjectFile {
        ObjectFile {
            bytes: Vec::new(),
            exports: exports.iter().map(|s| s.to_string()).collect(),
            imports: imports.iter().map(|s| s.to_string()).collect(),
            relocations: Vec::new(),
        }
    }

    fn check(file: ObjectFile, manifested: Manifested) -> Result<ObjectArtifact, CodegenError> {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let layouts = LayoutTable::new(&target);
        let manifest = Manifest::skeleton([]);
        let failures = FailureCodes::default();
        let input = InputIdentity {
            interface_version: hirsel_lir::INTERFACE_VERSION,
            digest: ContentHash::of_bytes(b"input"),
        };
        let finalizers = BTreeMap::new();
        let inputs = EmitInputs {
            input: &input,
            target: &target,
            layouts: &layouts,
            manifest: &manifest,
            failures: &failures,
            statics: &[],
            finalizers: &finalizers,
            inline_instances: &std::collections::BTreeSet::new(),
            available_externally: &std::collections::BTreeSet::new(),
            registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &layouts,
                &finalizers,
            ),
            opt: OptLevel::None,
            measurement: false,
        };
        artifact(file, manifested, &[], &inputs)
    }

    #[test]
    fn the_object_interface_must_be_the_manifest() {
        check(
            file(
                &["f"],
                &[
                    "g",
                    "memcpy",
                    "__floattidf",
                    "__floattisf",
                    "__floatuntidf",
                    "__floatuntisf",
                ],
            ),
            manifested(&["f"], &["g", "h"]),
        )
        .expect("declared and toolchain imports are accepted; a declared import may be unused");
        for (file, manifested) in [
            (file(&["f", "x"], &[]), manifested(&["f"], &[])),
            (file(&[], &[]), manifested(&["f"], &[])),
            (file(&["f"], &["surprise"]), manifested(&["f"], &[])),
        ] {
            match check(file, manifested) {
                Err(CodegenError::Manifest { .. }) => {}
                other => panic!("expected a manifest mismatch, got {other:?}"),
            }
        }
    }
}

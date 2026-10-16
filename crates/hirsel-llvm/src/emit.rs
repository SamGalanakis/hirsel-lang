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

/// Undefined symbols LLVM's own lowering may introduce: C library functions for `llvm.fma` on
/// a target without fused multiply-add, `frem` and memory intrinsics, and the 128-bit
/// division and integer-to-float conversion routines every C toolchain's support
/// library (libgcc, compiler-rt) provides.
pub const TOOLCHAIN_IMPORTS: [&str; 15] = [
    "__divti3",
    "__floattidf",
    "__floattisf",
    "__floatuntidf",
    "__floatuntisf",
    "__modti3",
    "__udivti3",
    "__umodti3",
    "fma",
    "fmaf",
    "fmod",
    "fmodf",
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
                let attribute = if unit.header.definition == core_library_path("list.push")
                    || unit.header.definition == core_library_path("$native.list_push_spare")
                {
                    Attribute::AlwaysInline
                } else {
                    Attribute::InlineHint
                };
                module.add_attribute(function, AttributeSlot::Function, attribute);
                if unit.header.definition == core_library_path("$native.list_grow") {
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
                    &crate::path(&unit.header.definition),
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
            args.extend([length, module.const_zero(module.int_type(1))]);
        }
    }
    let returned = builder.call(target, &args);
    signature.attribute_call(module, returned);
    for (field, &i) in signature.roots.iter().enumerate() {
        let pointer = signature.root_pointer(&builder, returned, field);
        builder.store(pointer, module.parameter(entry, signature.first[i]), 8);
        if signature.companions[field].is_some() {
            // The body hands back a length its block may lag: store it where it
            // differs (never into a static block, whose count it then equals).
            let LayoutKind::Block { payload } = layouts
                .layout(signature.abi.parameters[i].layout)
                .expect("root layout")
                .kind
            else {
                unreachable!("sequence block")
            };
            let data = builder.offset(
                pointer,
                module.const_int(
                    module.int_type(64),
                    u128::from(
                        payload_offset(layouts.layout(payload).expect("sequence payload").align)
                            .expect("validated alignment"),
                    ),
                ),
            );
            let length = builder.extract(returned, field as u32 * 2 + 1);
            let count = builder.load_access(module.int_type(64), data, 8, Access::Length);
            let differs = builder.icmp(IntPredicate::Ne, count, length);
            let write = module.append_block(entry);
            let done = module.append_block(entry);
            builder.cond_br(differs, write, done);
            builder.position(write);
            builder.store_access(length, data, 8, Access::Length);
            builder.br(done);
            builder.position(done);
        }
    }
    let status = signature.status(&builder, returned);
    let value = signature.result(&builder, returned);
    builder.ret(external.returned(&builder, module.const_zero(external.returns), status, value));
}

fn core_library_path(name: &str) -> DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().expect("core declaration name");
    DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
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
const ROOTS_SUFFIX: &str = ".roots7";

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

/// An LLVM parameter of a Hirsel ABI v1 entry.
#[derive(Clone, Copy, Debug)]
enum Hidden {
    Context,
    Environment,
    Destination,
    Result,
}

/// The LLVM signature of one `FnAbi`: hidden slots in ABI order, then each declared
/// parameter's physical slots, returning a zero-extended `u8` status. A Hirsel entry whose
/// result is a plain scalar (`register_result`) has no result pointer and returns
/// `{ i8 status, value word }` instead, in registers on x86-64 and AArch64.
struct Signature<'m, 'a> {
    abi: &'a FnAbi,
    ty: Type<'m>,
    /// The LLVM parameter types, hidden slots first.
    types: Vec<Type<'m>>,
    hidden: Vec<Hidden>,
    /// Per declared parameter, the index of its first LLVM parameter.
    first: Vec<u32>,
    extensions: Vec<(u32, Attribute)>,
    /// Internal library roots travel by value and return on both status outcomes.
    roots: Vec<usize>,
    sequence_roots: Vec<bool>,
    /// Private sequence inputs: full length and must-unique.
    companions: Vec<Option<u32>>,
    /// Per root, the returned `i1` field holding its must-unique fact (sequence roots).
    uniques: Vec<Option<u32>>,
    /// The returned field holding the status.
    status_field: u32,
    /// The plain scalar result returned in a register word instead of through a
    /// result pointer, and the returned field holding it.
    value: Option<(ScalarKind, u32)>,
    returns: Type<'m>,
}

/// The scalar a value of this layout kind is held as in SSA form, if it is one.
fn scalar_shape(kind: &LayoutKind) -> Option<ScalarKind> {
    match kind {
        LayoutKind::Scalar(kind) => Some(*kind),
        LayoutKind::Path => Some(ScalarKind::U64),
        LayoutKind::Block { .. }
        | LayoutKind::Box { .. }
        | LayoutKind::Header
        | LayoutKind::Arena => Some(ScalarKind::Pointer),
        _ => None,
    }
}

/// The scalar a Hirsel entry of `abi` returns in a register word, when its result is a
/// plain scalar of at most 64 bits. Every other result keeps its result pointer.
///
/// Register result invariant (ABI v1 lowering): such an entry returns the two-field
/// struct `{ i8 status, word }`, where the word is `ptr` for a pointer result and `i64`
/// otherwise. On success the word holds the value's bits in its low bits (Bool as one
/// byte, floats by bit pattern); emitted code zero-extends them and never assumes a
/// caller or callee did. On failure the word is zero and meaningless. Both targets
/// return the two fields in registers (RAX/AL and RDX, W0 and X1), which is also the C
/// struct `{ uint8_t status; uint64_t value; }` (or `void *value`). Definitions,
/// function-reference thunks, closure and iterator bodies follow it alike, so a direct,
/// a private and an indirect call agree; runtime primitives keep their result pointer.
pub fn register_result(layouts: &LayoutTable, abi: &FnAbi) -> Option<ScalarKind> {
    let result = abi.result.as_ref()?;
    let kind = scalar_shape(&layouts.layout(result.layout)?.kind)?;
    (!matches!(kind, ScalarKind::U128 | ScalarKind::I128)).then_some(kind)
}

/// A field of the register pair a plain scalar result returns as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairField {
    Status,
    Value,
}

/// The register pair's fields in order: `{ i8 status, word }` (`register_result`).
/// Emission's return type and the fields it reads and writes, and the C typedefs every
/// harness uses (`c_entry`), all come from here and [`PairField::word`].
pub(crate) const REGISTER_PAIR: [PairField; 2] = [PairField::Status, PairField::Value];

/// What a register pair field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairWord {
    /// An unsigned integer of this many bits.
    Int(u32),
    Pointer,
}

impl PairField {
    /// The field's index in the returned struct.
    pub(crate) fn index(self) -> u32 {
        REGISTER_PAIR
            .iter()
            .position(|&field| field == self)
            .expect("every pair field is in the pair") as u32
    }

    /// The field's member name in the C typedef.
    pub(crate) fn name(self) -> &'static str {
        match self {
            PairField::Status => "status",
            PairField::Value => "value",
        }
    }

    /// The word this field holds for a register result of `kind`: the u8 status, and a
    /// pointer for a reference and 64 bits otherwise.
    pub(crate) fn word(self, kind: ScalarKind) -> PairWord {
        match self {
            PairField::Status => PairWord::Int(8),
            PairField::Value if kind == ScalarKind::Pointer => PairWord::Pointer,
            PairField::Value => PairWord::Int(64),
        }
    }
}

fn pair_word<'m>(module: &'m Module<'m>, word: PairWord) -> Type<'m> {
    match word {
        PairWord::Int(bits) => module.int_type(bits),
        PairWord::Pointer => module.ptr_type(),
    }
}

/// The struct a Hirsel entry with a register result of `kind` returns.
pub(crate) fn pair_type<'m>(module: &'m Module<'m>, kind: ScalarKind) -> Type<'m> {
    module.struct_type(&REGISTER_PAIR.map(|field| pair_word(module, field.word(kind))))
}

/// The register word carrying a returned scalar of `kind`.
fn value_word<'m>(module: &'m Module<'m>, kind: ScalarKind) -> Type<'m> {
    pair_word(module, PairField::Value.word(kind))
}

/// `value` (an SSA scalar of `kind`) as a word of type `word`, zero-extended.
fn encode_word<'m>(
    builder: &Builder<'m>,
    kind: ScalarKind,
    value: Value<'m>,
    word: Type<'m>,
) -> Value<'m> {
    let module = builder.module();
    let i64_ty = module.int_type(64);
    let bits = match kind {
        ScalarKind::Pointer if word == module.ptr_type() => return value,
        ScalarKind::Pointer => return builder.cast(Cast::PtrToInt, value, word),
        ScalarKind::F32 => builder.cast(Cast::Bits, value, module.int_type(32)),
        ScalarKind::F64 => return builder.cast(Cast::Bits, value, i64_ty),
        ScalarKind::U64 | ScalarKind::I64 => return value,
        _ => value,
    };
    builder.cast(Cast::ZExt, bits, i64_ty)
}

/// The scalar of `kind` a returned `word` of type `ty` carries in its low bits.
fn decode_word<'m>(
    builder: &Builder<'m>,
    kind: ScalarKind,
    word: Value<'m>,
    ty: Type<'m>,
) -> Value<'m> {
    let module = builder.module();
    match kind {
        ScalarKind::Pointer if ty == module.ptr_type() => word,
        ScalarKind::Pointer => builder.cast(Cast::IntToPtr, word, module.ptr_type()),
        ScalarKind::Bool => {
            // A Bool is one byte, as in memory: any nonzero byte is true.
            let byte = builder.cast(Cast::Trunc, word, module.int_type(8));
            builder.icmp(
                IntPredicate::Ne,
                byte,
                module.const_int(module.int_type(8), 0),
            )
        }
        ScalarKind::F32 => {
            let bits = builder.cast(Cast::Trunc, word, module.int_type(32));
            builder.cast(Cast::Bits, bits, module.f32_type())
        }
        ScalarKind::F64 => builder.cast(Cast::Bits, word, module.f64_type()),
        ScalarKind::U64 | ScalarKind::I64 => word,
        _ => {
            let width = scalar::integer(kind).expect("an integer kind").0;
            builder.cast(Cast::Trunc, word, module.int_type(width))
        }
    }
}

fn physical<'m>(module: &'m Module<'m>, ty: PhysicalType) -> Type<'m> {
    match ty {
        PhysicalType::I8 => module.int_type(8),
        PhysicalType::I16 => module.int_type(16),
        PhysicalType::I32 => module.int_type(32),
        PhysicalType::I64 => module.int_type(64),
        PhysicalType::I128 => module.int_type(128),
        PhysicalType::F32 => module.f32_type(),
        PhysicalType::F64 => module.f64_type(),
        PhysicalType::Pointer => module.ptr_type(),
    }
}

impl<'m, 'a> Signature<'m, 'a> {
    /// The signature of a Hirsel entry: a plain scalar result returns in registers.
    fn of(
        module: &'m Module<'m>,
        layouts: &LayoutTable,
        abi: &'a FnAbi,
        definition: &DefPath,
    ) -> Result<Self, CodegenError> {
        Self::with_result(module, abi, definition, register_result(layouts, abi))
    }

    /// The signature of a runtime primitive, whose every result is written through its
    /// result pointer.
    fn runtime(
        module: &'m Module<'m>,
        abi: &'a FnAbi,
        definition: &DefPath,
    ) -> Result<Self, CodegenError> {
        Self::with_result(module, abi, definition, None)
    }

    fn with_result(
        module: &'m Module<'m>,
        abi: &'a FnAbi,
        definition: &DefPath,
        value: Option<ScalarKind>,
    ) -> Result<Self, CodegenError> {
        if !abi.context {
            return Err(CodegenError::Unsupported {
                definition: definition.clone(),
                what: "a Hirsel entry without a run context".into(),
            });
        }
        let ptr = module.ptr_type();
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
        let mut types = vec![ptr; hidden.len()];
        let mut first = Vec::new();
        let mut extensions = Vec::new();
        for parameter in &abi.parameters {
            first.push(types.len() as u32);
            let attribute = match parameter.extension {
                ScalarExtension::None => None,
                ScalarExtension::Sign => Some(Attribute::SignExt),
                ScalarExtension::Zero => Some(Attribute::ZeroExt),
            };
            if let (Some(attribute), [_]) = (attribute, parameter.slots.as_slice()) {
                extensions.push((types.len() as u32, attribute));
            }
            types.extend(parameter.slots.iter().map(|s| physical(module, s.ty)));
        }
        let returns = match value {
            Some(kind) => pair_type(module, kind),
            None => module.int_type(8),
        };
        let ty = module.function_type(returns, &types);
        Ok(Self {
            abi,
            ty,
            types,
            hidden,
            first,
            extensions,
            roots: Vec::new(),
            sequence_roots: Vec::new(),
            companions: Vec::new(),
            uniques: Vec::new(),
            status_field: if value.is_some() {
                PairField::Status.index()
            } else {
                1
            },
            value: value.map(|kind| (kind, PairField::Value.index())),
            returns,
        })
    }

    fn with_roots(mut self, module: &'m Module<'m>, layouts: &LayoutTable) -> Self {
        self.roots = self
            .abi
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                (p.passing == ParameterPassing::ExclusiveAddress
                    && matches!(
                        layouts.layout(p.layout).map(|l| &l.kind),
                        Some(LayoutKind::Block { .. } | LayoutKind::Box { .. })
                    ))
                .then_some(i)
            })
            .collect();
        self.sequence_roots = self
            .roots
            .iter()
            .map(|&i| {
                match layouts
                    .layout(self.abi.parameters[i].layout)
                    .map(|l| &l.kind)
                {
                    Some(LayoutKind::Block { payload }) => matches!(
                        layouts.layout(*payload).map(|l| &l.kind),
                        Some(LayoutKind::Sequence { .. })
                    ),
                    _ => false,
                }
            })
            .collect();
        self.companions = self
            .sequence_roots
            .iter()
            .map(|&sequence| {
                sequence.then(|| {
                    let first = self.types.len() as u32;
                    self.types.extend([module.int_type(64), module.int_type(1)]);
                    first
                })
            })
            .collect();
        if !self.roots.is_empty() {
            // Every root's first word is its pointer, typed `ptr` and never
            // tagged, so a loop that calls or inlines a root body carries the
            // pointer as itself: no mask or re-tag per iteration. Other roots can
            // be headerless regional payloads and return status in the second
            // word. A sequence root's second word is its full u64 length, and its
            // must-unique fact has a word of its own; its capacity is the block's
            // own header word (runtime ABI 5), so nothing caches it. When the first
            // root is a sequence, status is a last word of its own: every return
            // site supplies a constant there, so after inlining the caller's
            // status branch threads to the right continuation and the facts it
            // carries fold. That one-root return takes four words (memory on
            // x86-64 and AArch64); only uninlined calls pay for it.
            let mut fields = Vec::new();
            for _ in &self.roots {
                fields.extend([module.ptr_type(), module.int_type(64)]);
            }
            let mut extra = |ty: Type<'m>| {
                fields.push(ty);
                fields.len() as u32 - 1
            };
            let sequences = self.sequence_roots.clone();
            self.uniques = sequences
                .iter()
                .map(|&sequence| sequence.then(|| extra(module.int_type(1))))
                .collect();
            self.status_field = 1;
            if sequences[0] {
                self.status_field = extra(module.int_type(64));
            }
            // A register result is one more word, after every root's.
            if let Some((_, field)) = &mut self.value {
                *field = extra(module.int_type(64));
            }
            self.returns = module.struct_type(&fields);
            self.ty = module.function_type(self.returns, &self.types);
        }
        self
    }

    fn root_pointer(&self, builder: &Builder<'m>, returned: Value<'m>, field: usize) -> Value<'m> {
        builder.extract(returned, field as u32 * 2)
    }

    /// A returned sequence root's must-unique fact.
    fn root_unique(&self, builder: &Builder<'m>, returned: Value<'m>, field: usize) -> Value<'m> {
        builder.extract(
            returned,
            self.uniques[field].expect("a sequence root's must-unique word"),
        )
    }

    /// The `u8` status a call of this signature returned.
    fn status(&self, builder: &Builder<'m>, returned: Value<'m>) -> Value<'m> {
        let module = builder.module();
        if !self.roots.is_empty() {
            let word = builder.extract(returned, self.status_field);
            return builder.cast(Cast::Trunc, word, module.int_type(8));
        }
        if self.value.is_some() {
            return builder.extract(returned, self.status_field);
        }
        returned
    }

    /// The register result a successful call of this signature returned.
    fn result(&self, builder: &Builder<'m>, returned: Value<'m>) -> Option<Value<'m>> {
        let (kind, field) = self.value?;
        let ty = if self.roots.is_empty() {
            value_word(builder.module(), kind)
        } else {
            builder.module().int_type(64)
        };
        Some(decode_word(
            builder,
            kind,
            builder.extract(returned, field),
            ty,
        ))
    }

    /// Whether the entry returns a bare status byte.
    fn returns_status(&self) -> bool {
        self.roots.is_empty() && self.value.is_none()
    }

    /// What an entry of this signature returns: the `u8` `status` and, with a register
    /// result, `value` (a zero word when absent, as on failure). Root words are already
    /// in `returned`, which is otherwise zero.
    fn returned(
        &self,
        builder: &Builder<'m>,
        mut returned: Value<'m>,
        status: Value<'m>,
        value: Option<Value<'m>>,
    ) -> Value<'m> {
        let module = builder.module();
        if self.returns_status() {
            return status;
        }
        if self.roots.is_empty() {
            returned = builder.insert(returned, status, self.status_field);
        }
        if let (Some((kind, field)), Some(value)) = (self.value, value) {
            let word = if self.roots.is_empty() {
                value_word(module, kind)
            } else {
                module.int_type(64)
            };
            returned = builder.insert(returned, encode_word(builder, kind, value, word), field);
        }
        returned
    }

    fn hidden_index(&self, which: fn(&Hidden) -> bool) -> Option<u32> {
        self.hidden.iter().position(which).map(|i| i as u32)
    }

    /// The function `name` with this signature. Emitted code provides argument and status
    /// extension but never assumes it, since an AAPCS64 C caller or callee does not extend
    /// narrow integers: only a definition's status return is marked extended (it returns a
    /// constant), and parameters are extended at call sites (`attribute_call`).
    fn declare(
        &self,
        module: &'m Module<'m>,
        name: &str,
        definition: &DefPath,
        defining: bool,
    ) -> Result<Function<'m>, CodegenError> {
        let function = module
            .function(name, self.ty)
            .map_err(|what| CodegenError::Invalid {
                definition: definition.clone(),
                what,
            })?;
        module.add_attribute(function, AttributeSlot::Function, Attribute::NoUnwind);
        if defining && self.returns_status() {
            module.add_attribute(function, AttributeSlot::Return, Attribute::ZeroExt);
        }
        Ok(function)
    }

    /// The caller extends each narrow integer argument as the `FnAbi` says.
    fn attribute_call(&self, module: &'m Module<'m>, call: Value<'m>) {
        // Hirsel failure crosses this ABI as status and cleanup, never unwinding.
        // In particular, this is also true when the code pointer is erased.
        module.add_call_attribute(call, AttributeSlot::Function, Attribute::NoUnwind);
        for &(index, attribute) in &self.extensions {
            module.add_call_attribute(call, AttributeSlot::Parameter(index), attribute);
        }
    }
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
    /// Deferred slots this unit borrows whole: a view whose length is not
    /// preserved may read such a slot's block from memory.
    viewed: Vec<bool>,
    /// Slots of root parameters that hand their length back to the caller.
    companion_slots: Vec<bool>,
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

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn new(
        module: &'m Module<'m>,
        inputs: &'a EmitInputs<'a>,
        unit: &'a LirUnit<Ready>,
        function: Function<'m>,
        signature: Signature<'m, 'a>,
        manifested: &'a mut Manifested,
    ) -> Self {
        let prologue_block = module.append_block(function);
        let prologue = Builder::new(module);
        prologue.position(prologue_block);
        let context = module.parameter(function, 0);
        let environment = signature
            .hidden_index(|h| matches!(h, Hidden::Environment))
            .map(|index| module.parameter(function, index));
        Self {
            module,
            inputs,
            unit,
            function,
            signature,
            manifested,
            builder: Builder::new(module),
            profile: None,
            prologue,
            prologue_block,
            context,
            values: vec![None; unit.values.len()],
            versions: BTreeSet::new(),
            slots: vec![None; unit.slots.len()],
            sequence_states: vec![None; unit.slots.len()],
            read_roots: vec![None; unit.values.len()],
            preserved_lengths: vec![None; unit.values.len()],
            deferred: vec![false; unit.slots.len()],
            viewed: vec![false; unit.slots.len()],
            companion_slots: vec![false; unit.slots.len()],
            fresh_detaches: BTreeSet::new(),
            known_count: None,
            at: (BlockId(0), 0),
            adopted: BTreeMap::new(),
            addresses: vec![None; unit.addresses.len()],
            flat_contents: vec![None; unit.addresses.len()],
            flat_content: None,
            blocks: Vec::new(),
            phis: Vec::new(),
            failures: BTreeMap::new(),
            environment,
            regions: Vec::new(),
            register_return: None,
        }
    }

    fn definition(&self) -> &DefPath {
        &self.unit.header.definition
    }

    fn invalid<T>(&self, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Invalid {
            definition: self.definition().clone(),
            what: what.into(),
        })
    }

    fn unsupported<T>(&self, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Unsupported {
            definition: self.definition().clone(),
            what: what.into(),
        })
    }

    fn layout(&self, id: LayoutId) -> Emitted<&'a LayoutRecord> {
        match self.inputs.layouts.layout(id) {
            Some(record) => Ok(record),
            None => self.invalid(format!("no layout {}", id.0)),
        }
    }

    /// The bytes a value of `id` occupies in storage: a Packed value's
    /// largest variant (`SizeSpec::storage`).
    fn fixed_size(&self, id: LayoutId) -> Emitted<u64> {
        match self.layout(id)?.size.storage() {
            Some(size) => Ok(size),
            None => self.unsupported("a value of dynamic extent"),
        }
    }

    fn lir_type(&self, id: LirTypeId) -> Emitted<&'a LirType> {
        match self.unit.types.get(id.index()) {
            Some(ty) => Ok(ty),
            None => self.invalid(format!("no type {}", id.0)),
        }
    }

    fn value_type(&self, value: ValueId) -> Emitted<&'a LirType> {
        match self.unit.values.get(value.index()) {
            Some(info) => self.lir_type(info.ty),
            None => self.invalid(format!("no value {}", value.0)),
        }
    }

    fn shape(&self, ty: &LirType) -> Emitted<Shape> {
        if matches!(ty.repr, ValueRepr::Token | ValueRepr::RegionMark) {
            return Ok(Shape::Scalar(ScalarKind::Pointer));
        }
        self.layout_shape(ty.layout)
    }

    fn layout_shape(&self, layout: LayoutId) -> Emitted<Shape> {
        let record = self.layout(layout)?;
        if let Some(kind) = scalar_shape(&record.kind) {
            return Ok(Shape::Scalar(kind));
        }
        Ok(match record.kind {
            LayoutKind::Unit => Shape::Unit,
            LayoutKind::Callable { .. } => Shape::Callable,
            _ => Shape::Bytes {
                size: self.fixed_size(layout)?,
                align: record.align,
            },
        })
    }

    fn value_shape(&self, value: ValueId) -> Emitted<Shape> {
        self.shape(self.value_type(value)?)
    }

    fn llvm_type(&self, shape: Shape) -> Type<'m> {
        match shape {
            Shape::Unit => self.module.struct_type(&[]),
            Shape::Scalar(kind) => scalar::scalar_type(&self.builder, kind),
            Shape::Callable => self.module.struct_type(&[self.module.ptr_type(); 2]),
            Shape::Bytes { size, align } => {
                let unit = [8, 4, 2, 1]
                    .into_iter()
                    .find(|&unit| u64::from(align) % unit == 0 && size % unit == 0)
                    .unwrap_or(1);
                self.module
                    .array_type(self.module.int_type((unit * 8) as u32), size / unit)
            }
        }
    }

    fn load(&self, shape: Shape, place: Place<'m>) -> Value<'m> {
        let b = &self.builder;
        let loaded = |ty| {
            // `block` proves that the place lies in a payload, past the counted
            // header. Unknown incoming addresses stay unclassified; they need
            // not inherit a source-level alias promise from their scalar type.
            if place.block.is_some() {
                b.load_access(ty, place.ptr, place.align, Access::Value)
            } else {
                b.load(ty, place.ptr, place.align)
            }
        };
        match shape {
            Shape::Unit => self.module.const_zero(self.llvm_type(Shape::Unit)),
            Shape::Scalar(ScalarKind::Bool) => {
                let byte = loaded(self.module.int_type(8));
                let zero = self.module.const_int(self.module.int_type(8), 0);
                b.icmp(IntPredicate::Ne, byte, zero)
            }
            Shape::Scalar(_) => loaded(self.llvm_type(shape)),
            _ => b.load(self.llvm_type(shape), place.ptr, place.align),
        }
    }

    fn store(&self, shape: Shape, value: Value<'m>, place: Place<'m>) {
        let b = &self.builder;
        let stored = |value| {
            if place.block.is_some() {
                b.store_access(value, place.ptr, place.align, Access::Value);
            } else {
                b.store(value, place.ptr, place.align);
            }
        };
        match shape {
            Shape::Unit => {}
            Shape::Scalar(ScalarKind::Bool) => {
                stored(b.cast(Cast::ZExt, value, self.module.int_type(8)));
            }
            Shape::Scalar(_) => stored(value),
            _ => b.store(value, place.ptr, place.align),
        }
    }

    /// A stack slot in the prologue for a value of `layout`.
    fn stack(&self, layout: LayoutId) -> Emitted<Place<'m>> {
        let size = self.fixed_size(layout)?;
        let align = self.layout(layout)?.align;
        let ty = self.module.array_type(self.module.int_type(8), size);
        Ok(Place {
            ptr: self.prologue.alloca(ty, align),
            layout,
            align,
            block: None,
        })
    }

    fn get(&self, value: ValueId) -> Emitted<Value<'m>> {
        match self.values.get(value.index()).copied().flatten() {
            Some(v) => Ok(v),
            None if self.versions.contains(&value) => {
                self.invalid(format!("storage version {} used as an operand", value.0))
            }
            None => self.invalid(format!("value {} used before its definition", value.0)),
        }
    }

    /// Define a scalar operation's flag, present exactly when the operation reports one.
    fn define_flag(&mut self, flag: Option<ValueId>, llvm: Option<Value<'m>>) -> Emitted {
        match (flag, llvm) {
            (Some(flag), Some(llvm)) => self.define(flag, llvm),
            (None, None) => Ok(()),
            _ => self.invalid("a scalar flag the operation does not report"),
        }
    }

    fn define(&mut self, value: ValueId, llvm: Value<'m>) -> Emitted {
        match self.values.get_mut(value.index()) {
            Some(slot @ None) => {
                *slot = Some(llvm);
                Ok(())
            }
            Some(Some(_)) => self.invalid(format!("value {} defined twice", value.0)),
            None => self.invalid(format!("no value {}", value.0)),
        }
    }

    fn const_u64(&self, n: u64) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(64), u128::from(n))
    }

    fn pool<T: Copy>(&self, range: PoolRange<T>, pool: &'a [T]) -> Emitted<&'a [T]> {
        match range.get(pool) {
            Some(items) => Ok(items),
            None => self.invalid("a pool range outside its pool"),
        }
    }

    fn edge(&self, edge: EdgeId) -> Emitted<&'a Edge> {
        match self.unit.tables.edges.get(edge.index()) {
            Some(e) => Ok(e),
            None => self.invalid(format!("no edge {}", edge.0)),
        }
    }

    fn failure_edge(&self, failure: FailureId) -> Emitted<&'a FailureEdge> {
        match self.unit.tables.failures.get(failure.index()) {
            Some(f) => Ok(f),
            None => self.invalid(format!("no failure edge {}", failure.0)),
        }
    }

    fn runtime(&mut self, entry: RuntimeEntry) -> Emitted<Function<'m>> {
        let declaration = entry.declaration();
        let c_type = |ty: CType| -> Type<'m> {
            if ty.pointers > 0 {
                return self.module.ptr_type();
            }
            match ty.name {
                CName::Void => self.module.void_type(),
                CName::U8 => self.module.int_type(8),
                CName::U32 => self.module.int_type(32),
                CName::U64 => self.module.int_type(64),
                _ => self.module.ptr_type(),
            }
        };
        let parameters: Vec<_> = declaration
            .parameters
            .iter()
            .map(|p| c_type(p.ty))
            .collect();
        let ty = self
            .module
            .function_type(c_type(declaration.returns), &parameters);
        let function = match self.module.function(declaration.symbol, ty) {
            Ok(function) => function,
            Err(what) => return self.invalid(what),
        };
        crate::runtime::substrate(self.module, function, entry);
        self.manifested
            .imports
            .entry(declaration.symbol.to_owned())
            .or_insert_with(|| ObjectSymbol {
                name: declaration.symbol.to_owned(),
                kind: ObjectSymbolKind::Runtime,
                definition: None,
                abi: None,
            });
        Ok(function)
    }

    // The function.

    fn emit(mut self) -> Emitted {
        self.find_versions()?;
        self.fresh_detaches = self
            .unit
            .instructions
            .iter()
            .filter_map(|inst| match inst {
                Inst::UniqueCopy { result, .. } => Some(*result),
                _ => None,
            })
            .collect();
        for inst in &self.unit.instructions {
            if let Inst::Dup { value } | Inst::Recast { value, .. } = *inst {
                self.fresh_detaches.remove(&value);
            }
        }
        for address in &self.unit.addresses {
            if let AddressBase::Value(value) = address.base {
                self.fresh_detaches.remove(&value);
            }
        }
        for field in &self.unit.pools.fields {
            self.fresh_detaches.remove(&field.value);
        }
        for value in &self.unit.pools.values {
            self.fresh_detaches.remove(value);
        }
        for argument in &self.unit.pools.call_arguments {
            if let ArgumentValue::Value(value) = argument.value {
                self.fresh_detaches.remove(&value);
            }
        }
        let order = self.reverse_postorder()?;
        for _ in &self.unit.blocks {
            self.blocks.push(self.module.append_block(self.function));
        }
        self.create_phis()?;
        self.allocate_slots()?;
        self.bind_parameters()?;
        self.allocate_sequence_states()?;
        for b in order {
            self.emit_block(BlockId(b as u32))?;
        }
        let entry = self.blocks[self.unit.entry.index()];
        // The prologue ends by storing the length of each root parameter that
        // arrives without a sequence state: its caller may have deferred it.
        self.builder.position(self.prologue_block);
        self.settle_unstated_roots()?;
        self.builder.br(entry);
        Ok(())
    }

    /// Storage version names: stores', replaces' and slot-backed parameters' versions, and
    /// every block parameter an edge passes one to.
    fn find_versions(&mut self) -> Emitted {
        let unit = self.unit;
        for inst in &unit.instructions {
            match *inst {
                Inst::Store { version, .. }
                | Inst::SeqReserve { version, .. }
                | Inst::SeqSetLen { version, .. }
                | Inst::SeqAppend { version, .. }
                | Inst::FlatAppend { version, .. }
                | Inst::FlatField { version, .. }
                | Inst::FlatClose { version, .. } => {
                    self.versions.insert(version);
                }
                Inst::Replace {
                    version,
                    displaced_version,
                    ..
                } => {
                    self.versions.insert(version);
                    self.versions.insert(displaced_version);
                }
                _ => {}
            }
        }
        // A `mut` argument's write-back version names the root slot after the call.
        for call in &unit.tables.calls {
            for argument in self.pool(call.arguments, &unit.pools.call_arguments)? {
                if let ArgumentValue::Address { version, .. } = argument.value {
                    self.versions.insert(version);
                }
            }
        }
        for parameter in &self.signature.abi.parameters {
            let backed = unit
                .slots
                .iter()
                .any(|s| s.kind == SlotKind::Parameter(parameter.source));
            if backed && let Some(&value) = unit.parameters.get(parameter.source as usize) {
                self.versions.insert(value);
            }
        }
        loop {
            let mut grew = false;
            for edge in &unit.tables.edges {
                let arguments = self.pool(edge.arguments, &unit.pools.values)?;
                let Some(block) = unit.blocks.get(edge.target.index()) else {
                    return self.invalid("an edge to a missing block");
                };
                let parameters = self.pool(block.parameters, &unit.pools.values)?;
                for (a, p) in arguments.iter().zip(parameters) {
                    if self.versions.contains(a) && self.versions.insert(*p) {
                        grew = true;
                    }
                }
            }
            if !grew {
                return Ok(());
            }
        }
    }

    /// Successor blocks of a block over normal and failure edges.
    fn successors(&self, b: usize) -> Emitted<Vec<usize>> {
        let unit = self.unit;
        let block = &unit.blocks[b];
        let mut edges = Vec::new();
        let mut failures = Vec::new();
        for inst in self.pool(block.instructions, &unit.instructions)? {
            inst.visit(&mut |r| {
                if let Ref::Failure(f) = r {
                    failures.push(f);
                }
            });
        }
        let Some(terminator) = unit.terminators.get(b) else {
            return self.invalid("a block without a terminator");
        };
        let mut arms = None;
        terminator.visit(&mut |r| match r {
            Ref::Edge(e) => edges.push(e),
            Ref::Failure(f) => failures.push(f),
            Ref::SwitchArms(range) => arms = Some(range),
            _ => {}
        });
        if let Some(range) = arms {
            edges.extend(
                self.pool(range, &unit.pools.switch_arms)?
                    .iter()
                    .map(|a| a.edge),
            );
        }
        for f in failures {
            edges.push(self.failure_edge(f)?.cleanup);
        }
        edges
            .into_iter()
            .map(|e| self.edge(e).map(|e| e.target.index()))
            .collect()
    }

    /// Blocks in reverse postorder from the entry, so each definition is emitted before the
    /// uses it dominates. Every block is reachable (the validator checks it).
    fn reverse_postorder(&self) -> Emitted<Vec<usize>> {
        let count = self.unit.blocks.len();
        if self.unit.entry.index() >= count {
            return self.invalid("no entry block");
        }
        let mut seen = vec![false; count];
        let mut post = Vec::new();
        let mut stack = vec![(
            self.unit.entry.index(),
            self.successors(self.unit.entry.index())?,
            0,
        )];
        seen[self.unit.entry.index()] = true;
        while let Some((b, succ, next)) = stack.last_mut() {
            if let Some(&s) = succ.get(*next) {
                *next += 1;
                if s >= count {
                    return self.invalid("an edge to a missing block");
                }
                if !seen[s] {
                    seen[s] = true;
                    let succ = self.successors(s)?;
                    stack.push((s, succ, 0));
                }
            } else {
                post.push(*b);
                stack.pop();
            }
        }
        if post.len() != count {
            return self.invalid("an unreachable block");
        }
        post.reverse();
        Ok(post)
    }

    fn create_phis(&mut self) -> Emitted {
        let unit = self.unit;
        for (b, block) in unit.blocks.iter().enumerate() {
            self.builder.position(self.blocks[b]);
            let mut phis = Vec::new();
            for &p in self.pool(block.parameters, &unit.pools.values)? {
                if self.versions.contains(&p) {
                    phis.push(None);
                    continue;
                }
                let ty = self.llvm_type(self.value_shape(p)?);
                let phi = self.builder.phi(ty);
                self.define(p, phi)?;
                phis.push(Some(phi));
            }
            self.phis.push(phis);
        }
        Ok(())
    }

    fn allocate_slots(&mut self) -> Emitted {
        let unit = self.unit;
        for (s, slot) in unit.slots.iter().enumerate() {
            match slot.kind {
                // Bound to their environment fields at entry.
                SlotKind::Parameter(_) | SlotKind::Capture(_) => continue,
                SlotKind::Local
                | SlotKind::Temporary
                | SlotKind::ReturnSlot
                | SlotKind::LoopCarried(_)
                | SlotKind::Displaced => {}
            }
            let layout = self.lir_type(slot.ty)?.layout;
            self.slots[s] = Some(self.stack(layout)?);
        }
        let record = self
            .module
            .array_type(self.module.int_type(8), REGION_RECORD_BYTES);
        for _ in &unit.tables.regions {
            self.regions.push(self.prologue.alloca(record, 8));
        }
        Ok(())
    }

    /// Only a whole, private root has companion state. Captured addresses and
    /// externally passed roots retain ordinary memory semantics. Restrict the
    /// state to sequences of pointer-free, silently destroyed elements, so an
    /// element release/finalizer cannot change the root's count or length.
    fn whole_slot(&self, address: AddressId) -> Option<SlotId> {
        let info = self.unit.addresses.get(address.index())?;
        if info.projection.len != 0 {
            return None;
        }
        match info.base {
            AddressBase::Slot(slot) => Some(slot),
            AddressBase::Address(base) => self.whole_slot(base),
            AddressBase::Value(_) => None,
        }
    }

    fn root_call(&self, call: &CallSite) -> bool {
        if let Callee::Direct(symbol) = call.callee {
            let symbol = &self.unit.tables.symbols[symbol.index()];
            let name = symbol_name(&symbol.definition, symbol.instance.as_ref());
            self.manifested
                .library
                .contains(&format!("{name}{ROOTS_SUFFIX}"))
        } else {
            false
        }
    }

    fn allocate_sequence_states(&mut self) -> Emitted {
        // Mutable captures can name storage behind an erased environment.
        if !self.unit.tables.closures.is_empty() {
            return Ok(());
        }
        for (i, slot) in self.unit.slots.iter().enumerate() {
            if matches!(slot.kind, SlotKind::Capture(_)) {
                continue;
            }
            if let SlotKind::Parameter(source) = slot.kind
                && !self
                    .signature
                    .roots
                    .iter()
                    .any(|&i| self.signature.abi.parameters[i].source == source)
            {
                continue;
            }
            let ty = self.lir_type(slot.ty)?;
            if ty.repr != ValueRepr::Block {
                continue;
            }
            let layout = ty.layout;
            let Some(element) = self.sequence_element(layout)? else {
                continue;
            };
            let record = self.layout(element)?;
            if record.finalizer.is_some() || !record.scan.is_empty() || !record.drop.is_empty() {
                continue;
            }
            let escaped = self.unit.tables.calls.iter().any(|call| {
                !self.root_call(call) && call.arguments.get(&self.unit.pools.call_arguments).is_some_and(|args| {
                    args.iter().any(|arg| matches!(arg.value, ArgumentValue::Address { address, .. } if self.whole_slot(address) == Some(SlotId(i as u32))))
                })
            });
            if escaped {
                continue;
            }
            let state = SequenceState {
                unique: self.prologue.alloca(self.module.int_type(1), 1),
                length: self.prologue.alloca(self.module.int_type(64), 8),
                dirty: self.prologue.alloca(self.module.int_type(1), 1),
            };
            self.prologue.store(
                self.module.const_zero(self.module.int_type(1)),
                state.unique,
                1,
            );
            self.prologue.store(self.const_u64(0), state.length, 8);
            self.prologue.store(
                self.module.const_zero(self.module.int_type(1)),
                state.dirty,
                1,
            );
            if let SlotKind::Parameter(source) = slot.kind {
                let field = self
                    .signature
                    .roots
                    .iter()
                    .position(|&p| self.signature.abi.parameters[p].source == source)
                    .expect("private parameter");
                let first = self.signature.companions[field].expect("sequence inputs");
                self.builder
                    .store(self.module.parameter(self.function, first), state.length, 8);
                self.builder.store(
                    self.module.parameter(self.function, first + 1),
                    state.unique,
                    1,
                );
                // The caller hands over its length, which its block may lag.
                self.builder.store(
                    self.module.const_int(self.module.int_type(1), 1),
                    state.dirty,
                    1,
                );
                self.companion_slots[i] = true;
                self.deferred[i] = true;
            }
            self.sequence_states[i] = Some(state);
        }
        let unit = self.unit;
        for inst in &unit.instructions {
            match *inst {
                Inst::SeqSetLen { address, .. } => {
                    if let Some(slot) = self.whole_slot(address) {
                        self.deferred[slot.index()] = true;
                    }
                }
                Inst::Read {
                    address,
                    mode: ReadMode::Borrow,
                    ..
                } => {
                    if let Some(slot) = self.whole_slot(address) {
                        self.viewed[slot.index()] = true;
                    }
                }
                _ => {}
            }
        }
        for call in &unit.tables.calls {
            if !self.root_call(call) {
                continue;
            }
            for argument in self.pool(call.arguments, &unit.pools.call_arguments)? {
                if let ArgumentValue::Address { address, .. } = argument.value
                    && let Some(slot) = self.whole_slot(address)
                {
                    self.deferred[slot.index()] = true;
                }
            }
        }
        for (i, state) in self.sequence_states.iter().enumerate() {
            self.deferred[i] &= state.is_some();
            self.viewed[i] &= self.deferred[i];
        }
        Ok(())
    }

    /// Stores `length` into the count word of the sequence block `pointer` (at
    /// `layout`) where the two differ: never into a static or shared block,
    /// whose count a deferred length always equals.
    fn store_length_if_differs(
        &self,
        pointer: Value<'m>,
        layout: LayoutId,
        length: Value<'m>,
    ) -> Emitted {
        let Some(payload) = self.layout_payload(layout)? else {
            return self.invalid("a length of a place that holds no block");
        };
        let data = self.payload_of(pointer, payload)?;
        let count = self
            .builder
            .load_access(self.module.int_type(64), data.ptr, 8, Access::Length);
        let differs = self.builder.icmp(IntPredicate::Ne, count, length);
        let write = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(differs, write, done);
        self.builder.position(write);
        self.builder
            .store_access(length, data.ptr, 8, Access::Length);
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }

    /// Leaving the entry: each deferred slot's block stores its length, but a root
    /// parameter's, whose length its caller receives. A function that can hand a
    /// view out (through a result, a destination, an environment or a parameter
    /// holding a reference) stores its borrowed root parameters' too.
    fn flush_on_return(&self) -> Emitted {
        let abi = &self.signature.abi;
        let plain = |layout: LayoutId| {
            self.inputs
                .layouts
                .layout(layout)
                .is_some_and(|l| l.scan.is_empty() && l.drop.is_empty() && l.finalizer.is_none())
        };
        let isolated = !abi.environment
            && !abi.destination
            && abi.result.as_ref().is_none_or(|r| plain(r.layout))
            && abi
                .parameters
                .iter()
                .enumerate()
                .all(|(i, p)| self.signature.roots.contains(&i) || plain(p.layout));
        for slot in 0..self.deferred.len() {
            if !self.companion_slots[slot] || (self.viewed[slot] && !isolated) {
                self.flush_length(slot)?;
            }
        }
        Ok(())
    }

    /// Brings the count word of deferred slot `slot` up to its length, if dirty.
    fn flush_length(&self, slot: usize) -> Emitted {
        let (Some(state), Some(Some(place))) =
            (self.sequence_states[slot], self.slots.get(slot).copied())
        else {
            return Ok(());
        };
        if !self.deferred[slot] {
            return Ok(());
        }
        let i1 = self.module.int_type(1);
        let dirty = self.builder.load(i1, state.dirty, 1);
        let flush = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(dirty, flush, done);
        self.builder.position(flush);
        let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
        let length = self.builder.load(self.module.int_type(64), state.length, 8);
        self.store_length_if_differs(pointer, place.layout, length)?;
        self.builder.br(done);
        self.builder.position(done);
        self.builder
            .store(self.module.const_zero(i1), state.dirty, 1);
        Ok(())
    }

    /// Flushes every deferred slot (but the root parameters, unless `companions`).
    fn flush_lengths(&self, companions: bool) -> Emitted {
        for slot in 0..self.deferred.len() {
            if companions || !self.companion_slots[slot] {
                self.flush_length(slot)?;
            }
        }
        Ok(())
    }

    /// Before a count read from memory through a view of unknown origin: flushes
    /// each borrowed deferred slot whose block has the sequence payload `payload`.
    fn flush_viewed(&self, payload: LayoutId) -> Emitted {
        for slot in 0..self.deferred.len() {
            if !self.viewed[slot] {
                continue;
            }
            let Some(Some(place)) = self.slots.get(slot).copied() else {
                continue;
            };
            if self.layout_payload(place.layout)? == Some(payload) {
                self.flush_length(slot)?;
            }
        }
        Ok(())
    }

    /// Sets slot `slot`'s count word as current: its block's count is its length.
    fn settled(&self, slot: usize) {
        if let Some(state) = self.sequence_states[slot] {
            self.builder.store(
                self.module.const_zero(self.module.int_type(1)),
                state.dirty,
                1,
            );
        }
    }

    /// A root parameter without a sequence state keeps its count in its block:
    /// store the length its caller handed over, which the block may lag.
    fn settle_unstated_roots(&mut self) -> Emitted {
        for (field, &i) in self.signature.roots.iter().enumerate() {
            let Some(first) = self.signature.companions[field] else {
                continue;
            };
            let source = self.signature.abi.parameters[i].source;
            let stated = self
                .unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(source))
                .is_some_and(|s| self.sequence_states[s].is_some());
            if stated {
                continue;
            }
            let pointer = self
                .module
                .parameter(self.function, self.signature.first[i]);
            let length = self.module.parameter(self.function, first);
            self.store_length_if_differs(pointer, self.signature.abi.parameters[i].layout, length)?;
        }
        Ok(())
    }

    fn sequence_length(&self, pointer: Value<'m>, layout: LayoutId) -> Emitted<Value<'m>> {
        let Some(payload) = self.layout_payload(layout)? else {
            return self.invalid("sequence without payload");
        };
        let data = self.payload_of(pointer, payload)?;
        Ok(self
            .builder
            .load_access(self.module.int_type(64), data.ptr, 8, Access::Length))
    }

    fn sequence_state_at(&self, place: Place<'m>) -> Option<SequenceState<'m>> {
        self.slots
            .iter()
            .position(|slot| slot.is_some_and(|s| s.ptr == place.ptr))
            .and_then(|i| self.sequence_states[i])
    }

    fn invalidate_uniqueness(&self) {
        for state in self.sequence_states.iter().flatten() {
            self.builder.store(
                self.module.const_zero(self.module.int_type(1)),
                state.unique,
                1,
            );
        }
    }

    fn store_sequence_state(&self, address: AddressId, value: ValueId) -> Emitted {
        let Some(slot) = self.whole_slot(address) else {
            return Ok(());
        };
        let Some(state) = self.sequence_states[slot.index()] else {
            return Ok(());
        };
        let length = match self.preserved_lengths[value.index()] {
            Some(length) => length,
            None => self.sequence_length(self.get(value)?, self.value_type(value)?.layout)?,
        };
        self.builder.store(length, state.length, 8);
        // A detach result goes straight into its new owning root; if another use
        // can increase its count, retain the actual test instead of a must fact.
        let fresh = self.fresh_detaches.contains(&value);
        self.builder.store(
            self.module
                .const_int(self.module.int_type(1), u128::from(fresh)),
            state.unique,
            1,
        );
        Ok(())
    }

    /// Binds each capture slot to its field of the environment the body was entered with.
    /// The environment is B's `environment()` record of the captures in ordinal order, each
    /// field at the layout of its capture slot (a borrowed capture's view, a `mut` capture's
    /// address), placed as an aggregate places fields, behind the counted block header.
    fn bind_captures(&mut self) -> Emitted {
        let unit = self.unit;
        let mut captures: Vec<(u32, usize, LayoutId)> = Vec::new();
        for (s, slot) in unit.slots.iter().enumerate() {
            if let SlotKind::Capture(i) = slot.kind {
                captures.push((i, s, self.lir_type(slot.ty)?.layout));
            }
        }
        if captures.is_empty() {
            return Ok(());
        }
        captures.sort_by_key(|c| c.0);
        if captures.iter().enumerate().any(|(n, c)| c.0 as usize != n)
            || captures.len() != unit.captures.len()
        {
            return self.invalid("capture slots that are not one per environment field");
        }
        let Some(environment) = self.environment else {
            return self.invalid("capture slots in a unit without an environment");
        };
        let mut offset = 0u64;
        let mut align = 1u32;
        let mut fields = Vec::new();
        for &(_, s, layout) in &captures {
            let field_align = self.layout(layout)?.align;
            offset = match align_up(offset, u64::from(field_align)) {
                Ok(offset) => offset,
                Err(_) => return self.invalid("an environment beyond u64"),
            };
            fields.push((s, offset, layout));
            offset += self.fixed_size(layout)?;
            align = align.max(field_align);
        }
        let start = match payload_offset(align) {
            Ok(start) => start,
            Err(_) => return self.invalid("an environment payload offset beyond u64"),
        };
        let payload = Place {
            ptr: self.builder.offset(environment, self.const_u64(start)),
            layout: LayoutId(u32::MAX),
            align,
            block: Some(environment),
        };
        for (s, offset, layout) in fields {
            let ptr = self.builder.offset(payload.ptr, self.const_u64(offset));
            let align = align.min(low_bit(offset)).min(self.layout(layout)?.align);
            self.slots[s] = Some(Place {
                ptr,
                layout,
                align,
                block: payload.block,
            });
        }
        Ok(())
    }

    /// Binds each declared parameter: its value, or its slot's storage.
    fn bind_parameters(&mut self) -> Emitted {
        let unit = self.unit;
        let abi = self.signature.abi;
        if unit.parameters.len() != abi.parameters.len() {
            return self.invalid("parameter count differs from the FnAbi");
        }
        self.builder.position(self.prologue_block);
        for (i, parameter) in abi.parameters.iter().enumerate() {
            let value = unit.parameters[i];
            let slot = unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(parameter.source));
            let first = self.signature.first[i];
            let layout = parameter.layout;
            let align = self.layout(layout)?.align;
            if self.signature.roots.contains(&i) {
                let Some(s) = slot else {
                    return self.invalid("a root parameter without storage");
                };
                let place = self.stack(layout)?;
                self.store(
                    Shape::Scalar(ScalarKind::Pointer),
                    self.module.parameter(self.function, first),
                    place,
                );
                self.slots[s] = Some(place);
                continue;
            }
            match parameter.passing {
                ParameterPassing::ByValue => {
                    let args: Vec<_> = (0..parameter.slots.len() as u32)
                        .map(|k| self.module.parameter(self.function, first + k))
                        .collect();
                    let assembled = self.assemble(layout, &args)?;
                    match slot {
                        Some(s) => {
                            let place = self.stack(layout)?;
                            self.store(self.layout_shape(layout)?, assembled, place);
                            self.slots[s] = Some(place);
                        }
                        None => self.define(value, assembled)?,
                    }
                }
                ParameterPassing::ReadOnlyAddress
                | ParameterPassing::TransferAddress
                | ParameterPassing::ExclusiveAddress => {
                    let place = Place {
                        ptr: self.module.parameter(self.function, first),
                        layout,
                        align,
                        block: None,
                    };
                    match slot {
                        Some(s) => self.slots[s] = Some(place),
                        None if parameter.convention == Passing::Mut => {
                            return self.invalid("a mut parameter without its slot");
                        }
                        None => {
                            let loaded = self.load(self.layout_shape(layout)?, place);
                            self.define(value, loaded)?;
                        }
                    }
                }
                ParameterPassing::PointerLength => {
                    // The ABI pointer addresses the logical flat bytes. A variable flat
                    // value's LIR representation is its owner, whose header precedes them;
                    // a fixed flat value is inline. The length is the logical extent, not
                    // the owner's allocation capacity. Reads use the layout's size program.
                    let pointer = self.module.parameter(self.function, first);
                    match self.layout(layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            let pointer = if self.value_type(value)?.repr == ValueRepr::Uncounted {
                                pointer
                            } else {
                                self.uncounted_payload(pointer, payload)?.block.ok_or_else(
                                    || CodegenError::Invalid {
                                        definition: self.definition().clone(),
                                        what: "a flat extent parameter without its owner".into(),
                                    },
                                )?
                            };
                            if let Some(s) = slot {
                                let place = self.stack(layout)?;
                                self.store(Shape::Scalar(ScalarKind::Pointer), pointer, place);
                                self.slots[s] = Some(place);
                            } else {
                                self.define(value, pointer)?;
                            }
                        }
                        LayoutKind::Flat => {
                            let place = Place {
                                ptr: pointer,
                                layout,
                                align,
                                block: None,
                            };
                            if let Some(s) = slot {
                                self.slots[s] = Some(place);
                            } else {
                                self.define(value, self.load(self.layout_shape(layout)?, place))?;
                            }
                        }
                        _ => {
                            return self.invalid("a pointer/length parameter of a non-flat layout");
                        }
                    }
                }
            }
        }
        self.bind_captures()
    }

    /// A by-value parameter from its physical slots: none for unit, one scalar, or the
    /// scalar leaves of a multi-slot value at their layout offsets.
    fn assemble(&self, layout: LayoutId, args: &[Value<'m>]) -> Emitted<Value<'m>> {
        match (self.layout_shape(layout)?, args) {
            (Shape::Unit, []) => Ok(self.module.const_zero(self.llvm_type(Shape::Unit))),
            (Shape::Scalar(ScalarKind::Bool), [byte]) => {
                let zero = self.module.const_int(self.module.int_type(8), 0);
                Ok(self.builder.icmp(IntPredicate::Ne, *byte, zero))
            }
            (Shape::Scalar(_), [arg]) => Ok(*arg),
            (Shape::Callable, [code, environment]) => {
                let value = self.module.const_zero(self.llvm_type(Shape::Callable));
                let value = self.builder.insert(value, *code, 0);
                Ok(self.builder.insert(value, *environment, 1))
            }
            (shape @ Shape::Bytes { .. }, args) => {
                let leaves = &self.layout(layout)?.abi.scalars;
                if leaves.len() != args.len() {
                    return self
                        .unsupported("a by-value aggregate whose slots are not its scalar leaves");
                }
                let place = self.stack(layout)?;
                self.builder
                    .memset(place.ptr, 0, self.fixed_size(layout)?, place.align);
                for (&(offset, kind), &arg) in leaves.iter().zip(args) {
                    let at = self.field_place(place, offset, layout, false)?;
                    self.store(Shape::Scalar(kind), arg, at);
                }
                Ok(self.load(shape, place))
            }
            _ => self.invalid("a by-value parameter's slots do not match its layout"),
        }
    }

    /// The physical slots of a by-value argument (the inverse of `assemble`).
    fn disassemble(&self, layout: LayoutId, value: Value<'m>) -> Emitted<Vec<Value<'m>>> {
        Ok(match self.layout_shape(layout)? {
            Shape::Unit => Vec::new(),
            Shape::Scalar(ScalarKind::Bool) => {
                vec![
                    self.builder
                        .cast(Cast::ZExt, value, self.module.int_type(8)),
                ]
            }
            Shape::Scalar(_) => vec![value],
            Shape::Callable => vec![
                self.builder.extract(value, 0),
                self.builder.extract(value, 1),
            ],
            shape @ Shape::Bytes { .. } => {
                let place = self.stack(layout)?;
                self.store(shape, value, place);
                let mut slots = Vec::new();
                for &(offset, kind) in &self.layout(layout)?.abi.scalars {
                    let at = self.field_place(place, offset, layout, false)?;
                    let leaf = self.load(Shape::Scalar(kind), at);
                    slots.push(if kind == ScalarKind::Bool {
                        self.builder.cast(Cast::ZExt, leaf, self.module.int_type(8))
                    } else {
                        leaf
                    });
                }
                slots
            }
        })
    }

    /// The place `offset` bytes into `place`, holding a value of `layout`.
    fn field_place(
        &self,
        place: Place<'m>,
        offset: u64,
        layout: LayoutId,
        unaligned: bool,
    ) -> Emitted<Place<'m>> {
        let ptr = if offset == 0 {
            place.ptr
        } else {
            self.builder.offset(place.ptr, self.const_u64(offset))
        };
        let align = if unaligned {
            1
        } else {
            place
                .align
                .min(low_bit(offset))
                .min(self.layout(layout)?.align)
        };
        Ok(Place {
            ptr,
            layout,
            align,
            block: place.block,
        })
    }

    // Blocks and edges.

    fn emit_block(&mut self, b: BlockId) -> Emitted {
        let unit = self.unit;
        self.builder.position(self.blocks[b.index()]);
        for (i, inst) in self
            .pool(unit.blocks[b.index()].instructions, &unit.instructions)?
            .iter()
            .enumerate()
        {
            self.at = (b, i);
            self.profile_location(
                crate::profile::instruction_site(unit, b, inst),
                &format!("{:?}", inst.opcode()),
            )?;
            self.instruction(inst)?;
        }
        let terminator = unit.terminators[b.index()].clone();
        self.profile_location(
            crate::profile::terminator_site(unit, b),
            &format!("{:?}", terminator.opcode()),
        )?;
        self.terminator(&terminator)
    }

    fn profile_location(&mut self, site: SiteRange, operation: &str) -> Emitted {
        let Some((debug, scope, source)) = self.profile else {
            return Ok(());
        };
        let Some(position) = source.location(site.start) else {
            return self.invalid("profile anchor is outside its source revision");
        };
        let (line, column) = (position.line, position.column);
        debug.location(&self.builder, scope, line, column);
        if !self.inputs.measurement {
            return Ok(());
        }
        let digest = ContentHash::of_parts(
            "hirsel.profile-site.v1",
            [
                self.unit.header.definition.fingerprint().0.as_slice(),
                self.unit.header.implementation_revision.0.as_slice(),
                &site.start.to_le_bytes(),
                &site.end.to_le_bytes(),
                operation.as_bytes(),
            ],
        );
        let id = u64::from_le_bytes(digest.0[..8].try_into().expect("eight bytes")).max(1);
        let name = format!("hirsel.profile.site.{id}");
        let global = if let Some(global) = self.module.named_global(&name) {
            global
        } else {
            let mut strings = Vec::new();
            for (index, text) in [
                self.unit.header.definition.0.package.name.as_str(),
                &source.path.to_string_lossy(),
                &crate::path(&self.unit.header.definition),
                operation,
            ]
            .into_iter()
            .enumerate()
            {
                let bytes = self.module.const_bytes(&[text.as_bytes(), &[0]].concat());
                let string =
                    self.module
                        .private_constant(&format!("{name}.{index}"), bytes.ty(), 1);
                self.module.set_initializer(string, bytes);
                strings.push(string);
            }
            let initializer = self.module.const_struct(&[
                self.const_u64(id),
                strings[0],
                strings[1],
                strings[2],
                strings[3],
                self.module.const_int(self.module.int_type(32), line.into()),
                self.module
                    .const_int(self.module.int_type(32), column.into()),
            ]);
            let global = self.module.private_constant(&name, initializer.ty(), 8);
            self.module.set_initializer(global, initializer);
            global
        };
        let enter_name = "hirsel_rt_v2_profile_enter";
        let leave_name = "hirsel_rt_v2_profile_leave";
        let ptr = self.module.ptr_type();
        let enter = self
            .module
            .function(
                enter_name,
                self.module
                    .function_type(self.module.int_type(64), &[ptr, ptr]),
            )
            .or_else(|what| self.invalid(what))?;
        let leave = self
            .module
            .function(
                leave_name,
                self.module
                    .function_type(self.module.void_type(), &[ptr, self.module.int_type(64)]),
            )
            .or_else(|what| self.invalid(what))?;
        for name in [enter_name, leave_name] {
            self.manifested
                .imports
                .entry(name.into())
                .or_insert(ObjectSymbol {
                    name: name.into(),
                    kind: ObjectSymbolKind::Runtime,
                    definition: None,
                    abi: None,
                });
        }
        self.builder
            .profile_site(Some((enter, leave, self.context, global)));
        Ok(())
    }

    /// Adds `edge`'s arguments to its target's phis as coming from `from`.
    fn incoming(&self, edge: &Edge, from: BasicBlock<'m>) -> Emitted {
        let arguments = self.pool(edge.arguments, &self.unit.pools.values)?;
        let phis = &self.phis[edge.target.index()];
        if arguments.len() != phis.len() {
            return self.invalid("edge arguments differ from the target's parameters");
        }
        for (&argument, phi) in arguments.iter().zip(phis) {
            if let Some(phi) = phi {
                self.builder.add_incoming(*phi, self.get(argument)?, from);
            }
        }
        Ok(())
    }

    /// Branches from the current block along `edge`.
    fn jump(&self, edge: EdgeId) -> Emitted {
        let edge = self.edge(edge)?;
        self.incoming(edge, self.builder.current())?;
        self.builder.br(self.blocks[edge.target.index()]);
        Ok(())
    }

    /// A block that takes `edge` from a multi-way branch: the target itself when the edge
    /// passes no values, else a fresh block carrying the arguments.
    fn edge_block(&self, edge: EdgeId) -> Emitted<BasicBlock<'m>> {
        let e = self.edge(edge)?;
        let carries = self.phis[e.target.index()].iter().any(Option::is_some);
        if !carries {
            return Ok(self.blocks[e.target.index()]);
        }
        let here = self.builder.current();
        let block = self.module.append_block(self.function);
        self.builder.position(block);
        self.jump(edge)?;
        self.builder.position(here);
        Ok(block)
    }

    /// The block a failure edge starts in: it begins the failure (when the edge names a
    /// rule), disposes of the listed tokens and enters the cleanup edge.
    fn failure_block(&mut self, failure: FailureId) -> Emitted<BasicBlock<'m>> {
        if let Some(&block) = self.failures.get(&failure) {
            return Ok(block);
        }
        let edge = self.failure_edge(failure)?;
        let here = self.builder.current();
        let block = self.module.append_block(self.function);
        self.builder.position(block);
        if let Some(rule) = &edge.rule {
            let Some(code) = self.inputs.failures.codes.get(rule).copied() else {
                return Err(CodegenError::UnknownRule {
                    definition: self.definition().clone(),
                    rule: rule.clone(),
                });
            };
            let site = self.anchor_code(edge.anchor)?;
            let begin = self.runtime(RuntimeEntry::FailureBegin)?;
            let i32_ty = self.module.int_type(32);
            self.builder.call(
                begin,
                &[
                    self.context,
                    self.module.const_int(i32_ty, u128::from(code.kind)),
                    self.module.const_int(i32_ty, u128::from(code.rule)),
                    self.const_u64(site),
                ],
            );
        }
        let dispose = self.pool(edge.dispose, &self.unit.pools.values)?;
        if !dispose.is_empty() {
            let entry = self.runtime(RuntimeEntry::Dispose)?;
            for &token in dispose {
                self.builder.call(entry, &[self.context, self.get(token)?]);
            }
        }
        self.jump(edge.cleanup)?;
        self.builder.position(here);
        self.failures.insert(failure, block);
        Ok(block)
    }

    /// Branches to `failure` when `failed` holds and continues in a fresh block.
    fn guard(&mut self, failed: Value<'m>, failure: FailureId) -> Emitted {
        let fail = self.failure_block(failure)?;
        let next = self.module.append_block(self.function);
        self.builder.cold_cond_br(failed, fail, next);
        self.builder.position(next);
        Ok(())
    }

    /// A guard closure for the scalar module, bound to one failure edge.
    fn guard_for(
        &mut self,
        failure: FailureId,
    ) -> Emitted<impl FnMut(&Builder<'m>, Value<'m>) + use<'m>> {
        let fail = self.failure_block(failure)?;
        let module = self.module;
        let function = self.function;
        Ok(move |b: &Builder<'m>, failed: Value<'m>| {
            let next = module.append_block(function);
            b.cold_cond_br(failed, fail, next);
            b.position(next);
        })
    }

    /// Branches to `failure` when a runtime entry's status is nonzero. The runtime has
    /// already recorded the failure.
    fn status_guard(&mut self, status: Value<'m>, failure: FailureId) -> Emitted {
        let zero = self.module.const_int(self.module.int_type(8), 0);
        let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
        self.guard(failed, failure)
    }

    /// The site code of a source anchor.
    fn anchor_code(&self, anchor: SourceAnchorId) -> Emitted<u64> {
        if self.unit.header.origin == UnitOrigin::Library {
            return Ok(0);
        }
        let Some(anchor) = self.unit.tables.source_anchors.get(anchor.index()) else {
            return self.invalid("an anchor outside the anchor table");
        };
        Ok(site_code(
            &self.unit.header.definition,
            &self.unit.header.implementation_revision,
            anchor.site,
        ))
    }

    /// The site code of a cleanup anchor: the `anchor` a release or finalizer reports.
    fn cleanup_code(&self, anchor: CleanupAnchorId) -> Emitted<u64> {
        let Some(anchor) = self.unit.tables.cleanup_anchors.get(anchor.index()) else {
            return self.invalid("a cleanup anchor outside the anchor table");
        };
        Ok(site_code(
            &self.unit.header.definition,
            &self.unit.header.implementation_revision,
            anchor.site,
        ))
    }

    /// The `const uint64_t *live_mask` of a release: null when every field is live, else
    /// the mask's words.
    fn live_mask(&mut self, live: MaskRef) -> Emitted<Value<'m>> {
        let ptr = self.module.ptr_type();
        let i64_ty = self.module.int_type(64);
        match live {
            MaskRef::All => Ok(self.module.const_zero(ptr)),
            MaskRef::Static(mask) => {
                let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
                    return self.invalid("a mask outside the mask table");
                };
                let words: Vec<_> = self
                    .pool(mask.words, &self.unit.pools.mask_words)?
                    .iter()
                    .map(|&w| self.module.const_int(i64_ty, u128::from(w)))
                    .collect();
                if words.is_empty() {
                    return self.invalid("a static mask without words");
                }
                let ty = self.module.array_type(i64_ty, words.len() as u64);
                let global = self.module.private_constant("hirsel.mask", ty, 8);
                self.module
                    .set_initializer(global, self.module.const_array(i64_ty, &words));
                Ok(global)
            }
            MaskRef::Dynamic(mask) => {
                let Shape::Scalar(kind) = self.value_shape(mask)? else {
                    return self.invalid("a mask that is not a scalar");
                };
                let Some((width, false)) = scalar::integer(kind) else {
                    return self.invalid("a mask that is not an unsigned integer");
                };
                let mut word = self.get(mask)?;
                if width < 64 {
                    word = self.builder.cast(Cast::ZExt, word, i64_ty);
                } else if width > 64 {
                    return self.unsupported("a live mask wider than one word");
                }
                let spill = self.prologue.alloca(i64_ty, 8);
                self.builder.store(word, spill, 8);
                Ok(spill)
            }
        }
    }

    /// The LLVM types of `hirsel_rt_v2_layout` and `hirsel_rt_v2_step`.
    fn descriptor_types(&self) -> (Type<'m>, Type<'m>) {
        let (ptr, i32_ty, i64_ty) = (
            self.module.ptr_type(),
            self.module.int_type(32),
            self.module.int_type(64),
        );
        (
            self.module.struct_type(&[
                i32_ty, i32_ty, i64_ty, i64_ty, ptr, ptr, i64_ty, i32_ty, i32_ty,
            ]),
            self.module.struct_type(&[
                i32_ty, i32_ty, i32_ty, i32_ty, i64_ty, i64_ty, i64_ty, i64_ty, ptr,
            ]),
        )
    }

    /// The header stores the linked descriptor index and storage kind.
    fn block_descriptor(&self, block: Value<'m>) -> Value<'m> {
        let word = self.builder.load(
            self.module.int_type(32),
            self.builder.offset(block, self.const_u64(4)),
            4,
        );
        self.builder.binary(
            BinOp::And,
            word,
            self.module.const_int(self.module.int_type(32), 0x1fff_ffff),
        )
    }

    fn descriptor(&mut self, layout: LayoutId) -> Emitted<Value<'m>> {
        self.descriptor_pointer(layout)?;
        let Some(index) = self.inputs.registry.index(layout) else {
            return self.invalid("a layout absent from the linked descriptor registry");
        };
        Ok(self
            .module
            .const_int(self.module.int_type(32), index.into()))
    }

    fn empty_descriptor(&mut self) -> Value<'m> {
        let name = format!("hirsel.layout.empty.{}", self.inputs.registry.symbol());
        if let Some(global) = self.module.named_global(&name) {
            return global;
        }
        let (ty, _) = self.descriptor_types();
        self.manifested.descriptors.insert(name.clone());
        let global = self.module.linkonce_constant(&name, ty, 8);
        let null = self.module.const_zero(self.module.ptr_type());
        let initializer = self.descriptor_initializer(0, 1, null, null, 0, 0, 0);
        self.module.set_initializer(global, initializer);
        global
    }

    fn registry_table(&mut self) -> Emitted<()> {
        let name = self.inputs.registry.symbol();
        if self.module.named_global(&name).is_some() {
            return Ok(());
        }
        let ids = self.inputs.registry.layouts.clone();
        let mut values = vec![self.empty_descriptor()];
        for id in ids {
            values.push(self.descriptor_pointer(id)?);
        }
        let ptr = self.module.ptr_type();
        let global = self
            .module
            .registry_constant(&name, self.module.array_type(ptr, values.len() as u64));
        self.manifested.descriptors.insert(name);
        self.module
            .set_initializer(global, self.module.const_array(ptr, &values));
        let binding_name = "hirsel.program.registry";
        let binding = self.module.registry_constant(
            binding_name,
            self.module.struct_type(&[ptr, self.module.int_type(64)]),
        );
        self.manifested.descriptors.insert(binding_name.into());
        self.module.set_initializer(
            binding,
            self.module
                .const_struct(&[global, self.const_u64(values.len() as u64)]),
        );
        Ok(())
    }

    /// A descriptor's initializer.
    #[allow(clippy::too_many_arguments)]
    fn descriptor_initializer(
        &self,
        size: u64,
        align: u64,
        finalizer: Value<'m>,
        steps: Value<'m>,
        step_count: u64,
        field_count: u32,
        flags: u32,
    ) -> Value<'m> {
        let (i32_ty, i64_ty) = (self.module.int_type(32), self.module.int_type(64));
        let u32_ = |n: u32| self.module.const_int(i32_ty, u128::from(n));
        let u64_ = |n: u64| self.module.const_int(i64_ty, u128::from(n));
        self.module.const_struct(&[
            u32_(LAYOUT_MAGIC),
            u32_(LAYOUT_DESCRIPTOR_VERSION),
            u64_(size),
            u64_(align),
            finalizer,
            steps,
            u64_(step_count),
            u32_(field_count),
            u32_(flags),
        ])
    }

    /// The `hirsel_rt_v2_layout` (version 2) of `layout`, emitted once per module with every
    /// descriptor its steps name, as `hirsel_lir::rt_program` lowers them. A layout with a user
    /// finalizer names its drop wrapper.
    fn descriptor_pointer(&mut self, layout: LayoutId) -> Emitted<Value<'m>> {
        if let Some(global) = self
            .module
            .named_global(&descriptor_name(self.layout(layout)?))
        {
            return Ok(global);
        }
        let descriptors = match rt_program::descriptors(self.inputs.layouts, &[layout]) {
            Ok(descriptors) => descriptors,
            Err(error) => {
                return self.invalid(format!(
                    "no runtime descriptor for layout {}: {error:?}",
                    layout.0
                ));
            }
        };
        let ptr = self.module.ptr_type();
        let (layout_ty, step_ty) = self.descriptor_types();
        // Declare every new descriptor first: a Finalize step names another.
        let mut new = Vec::new();
        for d in &descriptors {
            let name = descriptor_name(self.layout(d.layout)?);
            if self.module.named_global(&name).is_none() {
                self.manifested.descriptors.insert(name.clone());
                new.push((d, self.module.linkonce_constant(&name, layout_ty, 8)));
            }
        }
        let i32_ty = self.module.int_type(32);
        let i64_ty = self.module.int_type(64);
        for (d, global) in new {
            let mut steps = Vec::new();
            for step in &d.steps {
                let target = match step.layout {
                    Some(l) => match self.module.named_global(&descriptor_name(self.layout(l)?)) {
                        Some(target) => target,
                        None => return self.invalid("a step naming a layout without a descriptor"),
                    },
                    None => self.module.const_zero(ptr),
                };
                debug_assert!(step.kind != StepKind::Finalize || step.layout.is_some());
                let u32_ = |n: u32| self.module.const_int(i32_ty, u128::from(n));
                let u64_ = |n: u64| self.module.const_int(i64_ty, u128::from(n));
                steps.push(self.module.const_struct(&[
                    u32_(step.kind.code()),
                    u32_(step.field),
                    u32_(step.variant),
                    u32_(step.span),
                    u64_(step.offset),
                    u64_(step.stride),
                    u64_(step.count),
                    u64_(step.count_offset),
                    target,
                ]));
            }
            let steps_ptr = if steps.is_empty() {
                self.module.const_zero(ptr)
            } else {
                let ty = self.module.array_type(step_ty, steps.len() as u64);
                let name = descriptor_name(self.layout(d.layout)?)
                    .replace("hirsel.layout.", "hirsel.steps.");
                self.manifested.descriptors.insert(name.clone());
                let array = self.module.linkonce_constant(&name, ty, 8);
                self.module
                    .set_initializer(array, self.module.const_array(step_ty, &steps));
                array
            };
            let finalizer = match &d.finalizer {
                Some(finalizer) => self.drop_wrapper(d.layout, finalizer)?,
                None => self.module.const_zero(ptr),
            };
            let initializer = self.descriptor_initializer(
                d.size,
                d.align,
                finalizer,
                steps_ptr,
                d.steps.len() as u64,
                d.field_count,
                d.flags,
            );
            self.module.set_initializer(global, initializer);
        }
        match self
            .module
            .named_global(&descriptor_name(self.layout(layout)?))
        {
            Some(global) => Ok(global),
            None => self.invalid("a descriptor that was not emitted"),
        }
    }

    /// The drop thunk of `layout`, whose finalizer is `finalizer`: a private wrapper with the
    /// runtime's `hirsel_rt_v2_drop_thunk` type that calls the drop body's unit, named
    /// `symbol_name` of its definition instance, with the self address (Borrow for source bodies, Mut for library bodies), and returns its
    /// status. The unit records its own failure; the cleanup anchor is not its to report.
    fn drop_wrapper(&mut self, layout: LayoutId, finalizer: &FinalizerRef) -> Emitted<Value<'m>> {
        let Some(instance) = self.inputs.finalizers.get(&layout) else {
            return self.invalid(format!(
                "layout {} has a finalizer but no drop body instance",
                layout.0
            ));
        };
        let ptr = self.module.ptr_type();
        let i8_ty = self.module.int_type(8);
        let body_name = symbol_name(&finalizer.definition, Some(instance));
        let body_ty = self.module.function_type(i8_ty, &[ptr, ptr]);
        let body = match self.module.function(&body_name, body_ty) {
            Ok(body) => body,
            Err(_) => {
                return self.invalid(format!(
                    "the drop body `{body_name}` does not take (context, self address)"
                ));
            }
        };
        self.manifested
            .imports
            .entry(body_name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: body_name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(finalizer.definition.clone()),
                abi: None,
            });
        let ty = self
            .module
            .function_type(i8_ty, &[ptr, ptr, self.module.int_type(64)]);
        let wrapper = match self
            .module
            .function(&format!("hirsel.drop.{}", layout.0), ty)
        {
            Ok(wrapper) => wrapper,
            Err(what) => return self.invalid(what),
        };
        self.module.make_private(wrapper);
        let builder = Builder::new(self.module);
        builder.position(self.module.append_block(wrapper));
        let status = builder.call(
            body,
            &[
                self.module.parameter(wrapper, 0),
                self.module.parameter(wrapper, 1),
            ],
        );
        builder.ret(status);
        Ok(wrapper.value)
    }

    /// Flat content needs an owner a callee can copy. Reserve its real immortal
    /// header once with the regional payload; other regional storage stays headerless.
    fn reserve_region(
        &mut self,
        mark: Value<'m>,
        layout: LayoutId,
        extent: Value<'m>,
        failure: FailureId,
    ) -> Emitted<Place<'m>> {
        let record = self.layout(layout)?;
        let (flat, align) = (record.kind == LayoutKind::Flat, record.align);
        let offset = if flat {
            match align_up(24, u64::from(align.max(8))) {
                Ok(n) => n,
                Err(error) => return self.invalid(format!("{error:?}")),
            }
        } else {
            0
        };
        // An immortal owner's copy may read its rounded header capacity.
        // Reserve that capacity, including padding, with the payload.
        let capacity = if flat {
            let rounded = self.builder.binary(BinOp::Add, extent, self.const_u64(15));
            self.builder
                .binary(BinOp::And, rounded, self.const_u64(!15))
        } else {
            extent
        };
        let size = self
            .builder
            .binary(BinOp::Add, capacity, self.const_u64(offset));
        let reserve = self.runtime(RuntimeEntry::RegionReserve)?;
        let out = self.prologue.alloca(self.module.ptr_type(), 8);
        let status = self.builder.call(
            reserve,
            &[
                self.context,
                mark,
                size,
                self.const_u64(u64::from(if flat { align.max(8) } else { align })),
                out,
            ],
        );
        self.status_guard(status, failure)?;
        let storage = self.builder.load(self.module.ptr_type(), out, 8);
        let owner = if flat {
            let header = self.builder.offset(storage, self.const_u64(offset - 8));
            let descriptor = self.descriptor(layout)?;
            let bind = self.runtime(RuntimeEntry::RegionOwner)?;
            self.builder.call(
                bind,
                &[
                    self.context,
                    mark,
                    header,
                    descriptor,
                    capacity,
                    self.const_u64(u64::from(align)),
                ],
            );
            Some(header)
        } else {
            None
        };
        Ok(Place {
            ptr: self.builder.offset(storage, self.const_u64(offset)),
            layout,
            align,
            block: owner,
        })
    }

    /// A static immortal block named `name`: a counted header with `COUNT_IMMORTAL`, the
    /// static domain and `descriptor`, then `bytes` at the payload offset of `align`. The
    /// runtime never writes it: dups and releases of an immortal count return at once.
    /// A private static immortal block of `bytes`, with the pointer-sized
    /// cell at each offset of `references` holding that block reference (a
    /// relocation; its bytes are zero).
    fn static_block(
        &mut self,
        name: &str,
        descriptor: Value<'m>,
        align: u32,
        bytes: &[u8],
        references: &[(u64, Value<'m>)],
    ) -> Emitted<Value<'m>> {
        let physical = align.max(8);
        let start = match align_up(24, u64::from(physical)) {
            Ok(n) => n,
            Err(_) => return self.invalid("static extent overflow"),
        };
        let header_offset = start - 8;
        if let Some(global) = self.module.named_global(name) {
            return Ok(self.module.const_offset(global, header_offset));
        }
        let (ptr, i8_ty, i32_ty) = (
            self.module.ptr_type(),
            self.module.int_type(8),
            self.module.int_type(32),
        );
        // Static extents have exactly their initialized bytes. Padding them
        // like heap classes would give [] spare capacity and send its first
        // push through a detach instead of directly through sequence growth.
        let capacity = bytes.len();
        let header_ty = self.module.struct_type(&[i32_ty, i32_ty]);
        let word = self
            .module
            .const_or(descriptor, self.module.const_int(i32_ty, 2u128 << 29));
        let header = self.module.const_struct(&[
            self.module.const_int(i32_ty, u128::from(COUNT_IMMORTAL)),
            word,
        ]);
        let padding = vec![0u8; (header_offset - 16) as usize];
        let extent = self.module.const_struct(&[
            self.const_u64(capacity as u64),
            self.const_u64(u64::from(physical)),
        ]);
        let (payload_ty, payload) = if references.is_empty() {
            (
                self.module.array_type(i8_ty, bytes.len() as u64),
                self.module.const_bytes(bytes),
            )
        } else {
            // Byte runs between the reference cells, packed so each cell lies
            // at its own offset.
            let word = u64::from(self.inputs.target.pointer_bytes);
            let mut sorted = references.to_vec();
            sorted.sort_by_key(|(offset, _)| *offset);
            let (mut types, mut parts) = (Vec::new(), Vec::new());
            let mut at = 0u64;
            for (offset, target) in sorted {
                if offset < at || offset + word > bytes.len() as u64 {
                    return self.invalid("a static reference outside its block");
                }
                let run = &bytes[at as usize..offset as usize];
                types.push(self.module.array_type(i8_ty, run.len() as u64));
                parts.push(self.module.const_bytes(run));
                types.push(ptr);
                parts.push(target);
                at = offset + word;
            }
            let run = &bytes[at as usize..];
            types.push(self.module.array_type(i8_ty, run.len() as u64));
            parts.push(self.module.const_bytes(run));
            (
                self.module.packed_struct_type(&types),
                self.module.const_packed_struct(&parts),
            )
        };
        let ty = self.module.packed_struct_type(&[
            self.module.array_type(i8_ty, padding.len() as u64),
            self.module
                .struct_type(&[self.module.int_type(64), self.module.int_type(64)]),
            header_ty,
            payload_ty,
        ]);
        let global = self.module.private_constant(name, ty, physical);
        let initializer = self.module.const_packed_struct(&[
            self.module.const_bytes(&padding),
            extent,
            header,
            payload,
        ]);
        self.module.set_initializer(global, initializer);
        Ok(self.module.const_offset(global, header_offset))
    }

    /// The environment of a function reference: one static immortal block with an empty
    /// payload and the empty descriptor, never released (a static is consumed without credit).
    fn empty_environment(&mut self) -> Emitted<Value<'m>> {
        self.empty_descriptor();
        let index = self.module.const_int(self.module.int_type(32), 2);
        self.static_block("hirsel.env.empty", index, 1, &[], &[])
    }

    /// The static block of a String or Bytes literal: a counted `Sequence` payload of `payload`
    /// (its u64 element count, its u64 capacity, the count, then the bytes).
    fn literal_block(&mut self, payload: LayoutId, bytes: &[u8]) -> Emitted<Value<'m>> {
        if self.inputs.target.endian != Endian::Little {
            return self.unsupported("a static literal on a big-endian target");
        }
        let LayoutKind::Sequence { element } = self.layout(payload)?.kind else {
            return self.invalid("a text literal whose storage is not a sequence");
        };
        if self.fixed_size(element)? != 1 {
            return self.invalid("a text literal of elements wider than a byte");
        }
        let start = match sequence_elements_offset(self.layout(element)?) {
            Ok(start) => start as usize,
            Err(_) => return self.invalid("a sequence element offset beyond u64"),
        };
        let mut data = vec![0u8; start];
        let count = (bytes.len() as u64).to_le_bytes();
        data[..8].copy_from_slice(&count);
        let capacity = SEQUENCE_CAPACITY_OFFSET as usize;
        data[capacity..capacity + 8].copy_from_slice(&count);
        data.extend_from_slice(bytes);
        let descriptor = self.descriptor(payload)?;
        let align = self.layout(payload)?.align;
        let name = format!(
            "hirsel.literal.{}",
            ContentHash::of_parts("hirsel.native-literal.v1", [data.as_slice()])
                .0
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        self.static_block(&name, descriptor, align, &data, &[])
    }

    // Instructions.

    fn scalar_kind(&self, value: ValueId) -> Emitted<ScalarKind> {
        match self.value_shape(value)? {
            Shape::Scalar(kind) => Ok(kind),
            _ => self.invalid(format!("value {} is not a scalar", value.0)),
        }
    }

    /// Brings deferred count words up to date before `inst` where it may read one
    /// from memory: a block moving or copied out of its slot, a view of unknown
    /// origin, or anything else that reaches blocks (runtime entries, structural
    /// operations). Instructions that read no count, and scalar arithmetic, keep
    /// the deferral, so a push loop stores its lengths once, after the loop.
    fn flush_before(&mut self, inst: &Inst) -> Emitted {
        let whole = |address| {
            self.whole_slot(address)
                .map(SlotId::index)
                .filter(|&s| self.deferred[s])
        };
        match *inst {
            Inst::Const { .. }
            | Inst::FunctionRef { .. }
            | Inst::ReadStatic { .. }
            | Inst::Unary { .. }
            | Inst::Binary { .. }
            | Inst::Convert { .. }
            | Inst::IntArith { .. }
            | Inst::MulAdd { .. }
            | Inst::Check { .. }
            | Inst::StorageLive { .. }
            | Inst::EndScope { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::FrameEnter { .. }
            | Inst::FrameLeave
            | Inst::Forget { .. }
            | Inst::Weaken { .. }
            | Inst::IsUnique { .. }
            | Inst::SeqCapacity { .. }
            | Inst::SeqSetLen { .. }
            | Inst::Dup { .. }
            | Inst::UpdateMask { .. }
            | Inst::TokenNone { .. }
            | Inst::Construct { .. }
            | Inst::Closure { .. }
            | Inst::Release { .. }
            | Inst::Reset { .. }
            | Inst::Reuse { .. }
            | Inst::Dispose { .. }
            | Inst::Drop { .. }
            | Inst::Unavailable { .. }
            | Inst::Store { .. }
            | Inst::Address { .. } => Ok(()),
            // Moved, copied or exchanged out of its slot: the block leaves with its
            // count current.
            Inst::Read { address, mode, .. } => match whole(address) {
                Some(slot) if mode != ReadMode::Borrow => self.flush_length(slot),
                _ => Ok(()),
            },
            Inst::Copy { address, .. } | Inst::Replace { address, .. } => match whole(address) {
                Some(slot) => self.flush_length(slot),
                None => Ok(()),
            },
            Inst::StorageDead { slot, .. } => self.flush_length(slot.index()),
            Inst::SeqLen { value, .. } => {
                if self.preserved_lengths[value.index()].is_some()
                    || self.value_type(value)?.repr != ValueRepr::Block
                {
                    return Ok(());
                }
                match self.layout_payload(self.value_type(value)?.layout)? {
                    Some(payload) => self.flush_viewed(payload),
                    None => Ok(()),
                }
            }
            _ => self.flush_lengths(true),
        }
    }

    fn instruction(&mut self, inst: &Inst) -> Emitted {
        // A shallow copy can add a child credit; regional cleanup can run user
        // finalizers. Neither may leave a previously observed uniqueness fact.
        if matches!(inst, Inst::UniqueCopy { .. } | Inst::RegionExit { .. }) {
            self.invalidate_uniqueness();
        }
        if let Inst::Release { layout, .. } | Inst::Reset { layout, .. } = *inst {
            let record = self.layout(layout)?;
            if record.finalizer.is_some() || !record.drop.is_empty() {
                self.invalidate_uniqueness();
                self.flush_lengths(true)?;
            }
        }
        self.flush_before(inst)?;
        match *inst {
            Inst::Const { result, constant } => {
                let value = self.constant(constant)?;
                self.define(result, value)
            }
            Inst::ReadStatic { result, symbol } => {
                let value = self.read_static(result, symbol)?;
                self.define(result, value)
            }
            Inst::FunctionRef { result, symbol } => {
                let value = self.function_ref(result, symbol)?;
                self.define(result, value)
            }
            // Its environment allocation cannot fail (runtime ABI 3), so the failure edge is
            // never taken.
            Inst::Closure {
                result,
                closure,
                failure: _,
            } => {
                let value = self.closure(result, closure)?;
                self.define(result, value)
            }
            // The same bits; only the copy recipe changes.
            Inst::Weaken { result, value } => {
                self.read_roots[result.index()] = self.read_roots[value.index()];
                self.preserved_lengths[result.index()] = self.preserved_lengths[value.index()];
                let value = self.get(value)?;
                self.define(result, value)
            }
            Inst::IsUnique { result, value } => {
                let state =
                    self.read_roots[value.index()].and_then(|s| self.sequence_states[s.index()]);
                let unique = if let Some(state) = state {
                    let known = self.builder.load(self.module.int_type(1), state.unique, 1);
                    let here = self.builder.current();
                    let test = self.module.append_block(self.function);
                    let done = self.module.append_block(self.function);
                    self.builder.cond_br(known, done, test);
                    self.builder.position(test);
                    let tested = crate::runtime::is_unique(&self.builder, self.get(value)?);
                    self.builder.store(tested, state.unique, 1);
                    self.builder.br(done);
                    self.builder.position(done);
                    let unique = self.builder.phi(self.module.int_type(1));
                    self.builder.add_incoming(unique, known, here);
                    self.builder.add_incoming(unique, tested, test);
                    unique
                } else {
                    crate::runtime::is_unique(&self.builder, self.get(value)?)
                };
                self.define(result, unique)
            }
            Inst::Unary {
                result,
                op,
                operand,
                failure,
            } => {
                let kind = self.scalar_kind(operand)?;
                let operand = self.get(operand)?;
                let mut guard = self.guard_for(failure)?;
                let value = scalar::unary(&self.builder, op, kind, operand, &mut guard)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
            }
            Inst::Binary {
                result,
                op,
                left,
                right,
                failure,
            } => {
                let kind = self.scalar_kind(left)?;
                if self.scalar_kind(right)? != kind {
                    return self.invalid("binary operands of different types");
                }
                let (l, r) = (self.get(left)?, self.get(right)?);
                let mut guard = self.guard_for(failure)?;
                let value = scalar::binary(&self.builder, op, kind, l, r, &mut guard)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
            }
            Inst::Convert {
                result,
                flag,
                operand,
                mode,
            } => {
                let (from, to) = (self.scalar_kind(operand)?, self.scalar_kind(result)?);
                let operand = self.get(operand)?;
                let (value, status) = scalar::convert(&self.builder, mode, from, to, operand)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)?;
                self.define_flag(flag, status)
            }
            Inst::IntArith {
                result,
                flag,
                op,
                mode,
                left,
                right,
            } => {
                let kind = self.scalar_kind(left)?;
                if self.scalar_kind(right)? != kind {
                    return self.invalid("integer operands of different types");
                }
                let (l, r) = (self.get(left)?, self.get(right)?);
                let (value, overflow) = scalar::int_arith(&self.builder, op, mode, kind, l, r)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)?;
                self.define_flag(flag, overflow)
            }
            Inst::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                let kind = self.scalar_kind(left)?;
                if self.scalar_kind(right)? != kind || self.scalar_kind(addend)? != kind {
                    return self.invalid("mul_add operands of different types");
                }
                let operands = [self.get(left)?, self.get(right)?, self.get(addend)?];
                let value = scalar::mul_add(&self.builder, kind, operands)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
            }
            Inst::Check { condition, failure } => {
                let condition = self.get(condition)?;
                let one = self.module.const_int(self.module.int_type(1), 1);
                let failed = self.builder.binary(BinOp::Xor, condition, one);
                self.guard(failed, failure)
            }
            // Static lifetime events: no machine bytes. Frame admission has no
            // numeric charge until spec #16 rules, so it cannot fail.
            Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::FrameEnter { .. }
            | Inst::FrameLeave => Ok(()),
            // Explicit suppression: the credit is abandoned and nothing is released.
            Inst::Forget { .. } => Ok(()),
            Inst::Address { address, failure } => {
                self.flat_content = None;
                let place = self.prepare(address, failure)?;
                self.addresses[address.index()] = Some(place);
                self.flat_contents[address.index()] = self.flat_content.take();
                Ok(())
            }
            Inst::Read {
                result,
                address,
                mode,
                ..
            } => {
                let place = self.place(address)?;
                let value = self.load_bounded(self.value_shape(result)?, place)?;
                if let Some(slot) = self.whole_slot(address)
                    && let Some(state) = self.sequence_states[slot.index()]
                {
                    if mode == ReadMode::Borrow {
                        self.read_roots[result.index()] = Some(slot);
                    }
                    self.preserved_lengths[result.index()] =
                        Some(self.builder.load(self.module.int_type(64), state.length, 8));
                    if mode == ReadMode::Take {
                        self.settled(slot.index());
                    }
                }
                self.define(result, value)
            }
            Inst::Store { address, value, .. } => {
                let place = self.place(address)?;
                self.store(self.value_shape(value)?, self.get(value)?, place);
                if let Some(slot) = self.whole_slot(address) {
                    self.settled(slot.index());
                }
                self.store_sequence_state(address, value)
            }
            Inst::Replace {
                address,
                value,
                displaced,
                ..
            } => {
                // The exchange: old content to the displaced slot, then the new content in,
                // with nothing that can fail or call between the two stores.
                let place = self.place(address)?;
                let shape = self.value_shape(value)?;
                let old = self.load(shape, place);
                let Some(Some(slot)) = self.slots.get(displaced.index()).copied() else {
                    return self.invalid("a Replace without its displaced slot");
                };
                self.store(shape, old, slot);
                self.store(shape, self.get(value)?, place);
                if let Some(state) = self.sequence_states[displaced.index()] {
                    let length = self.sequence_length(old, slot.layout)?;
                    self.builder.store(length, state.length, 8);
                    self.builder.store(
                        self.module.const_zero(self.module.int_type(1)),
                        state.unique,
                        1,
                    );
                }
                self.settled(displaced.index());
                if let Some(slot) = self.whole_slot(address) {
                    self.settled(slot.index());
                }
                self.store_sequence_state(address, value)
            }
            Inst::UpdateMask {
                result,
                mask,
                field,
                initialized,
            } => {
                let Shape::Scalar(kind) = self.value_shape(mask)? else {
                    return self.invalid("a mask that is not a scalar");
                };
                let Some((width, _)) = scalar::integer(kind) else {
                    return self.invalid("a mask that is not an integer");
                };
                let fields = match self.value_type(mask)?.repr {
                    ValueRepr::InitMask { fields } => fields,
                    _ => width,
                };
                if fields > hirsel_lir::ir::LIVE_MASK_WORD_BITS {
                    return self.unsupported(hirsel_lir::ir::live_mask_word_limit(
                        "initialization mask",
                        fields as usize,
                    ));
                }
                if field >= width {
                    return self.unsupported(format!("initialization mask requires bit {field}; its physical word has {width} bits"));
                }
                let ty = self.module.int_type(width);
                let bit = self.module.const_int(ty, 1u128 << field);
                let current = self.get(mask)?;
                let value = if initialized {
                    self.builder.binary(BinOp::Or, current, bit)
                } else {
                    let cleared = self.module.const_int(ty, !(1u128 << field));
                    self.builder.binary(BinOp::And, current, cleared)
                };
                self.define(result, value)
            }
            Inst::Construct {
                result,
                construct,
                failure,
            } => self.construct(result, construct, failure),
            Inst::FlatCopy {
                result,
                address,
                layout,
                failure,
            } => {
                let ty = self.value_type(result)?;
                if ty.layout != layout {
                    let source = self.place(address)?;
                    let length = self.load_u64(source.ptr);
                    let from = self.builder.offset(source.ptr, self.const_u64(8));
                    let anchor = self.failure_edge(failure)?.anchor;
                    match self.layout(ty.layout)?.kind.clone() {
                        LayoutKind::Block { payload } => {
                            let align = self.layout(payload)?.align;
                            let owner = self.alloc_block(payload, length, align, anchor)?;
                            let to = self.payload_of(owner, payload)?;
                            self.builder.memcpy(to.ptr, to.align, from, 1, length);
                            return self.define(result, owner);
                        }
                        LayoutKind::Slice { element } => {
                            // A whole fresh byte sequence: its count, its capacity (the
                            // count) and the bytes (runtime ABI 5).
                            let Some(payload) = slice_storage(self.inputs.layouts, element) else {
                                return self.invalid("a flat copy slice without sequence storage");
                            };
                            let (first, whole) = match (
                                sequence_elements_offset(self.layout(element)?),
                                sequence_slice_start(self.layout(element)?),
                            ) {
                                (Ok(first), Ok(whole)) => (first, whole),
                                _ => return self.invalid("a byte sequence offset beyond u64"),
                            };
                            let extent =
                                self.builder
                                    .binary(BinOp::Add, length, self.const_u64(first));
                            let align = self.layout(payload)?.align;
                            let owner = self.alloc_block(payload, extent, align, anchor)?;
                            let to = self.payload_of(owner, payload)?;
                            self.builder.store(length, to.ptr, to.align);
                            let capacity = self
                                .builder
                                .offset(to.ptr, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
                            self.builder
                                .store_access(length, capacity, 8, Access::Capacity);
                            let data = self.builder.offset(to.ptr, self.const_u64(first));
                            self.builder.memcpy(data, 1, from, 1, length);
                            let slice = self.stack(ty.layout)?;
                            for field in &self.layout(ty.layout)?.fields {
                                let at =
                                    self.builder.offset(slice.ptr, self.const_u64(field.offset));
                                let word = match field.ordinal {
                                    0 => owner,
                                    1 => self.const_u64(whole),
                                    2 => length,
                                    _ => {
                                        return self
                                            .invalid("a flat copy slice with a fourth word");
                                    }
                                };
                                self.builder.store(word, at, slice.align.min(8));
                            }
                            let value = self.load(self.layout_shape(ty.layout)?, slice);
                            return self.define(result, value);
                        }
                        _ => return self.invalid("a flat copy into another representation"),
                    }
                }
                match (self.layout(layout)?.kind.clone(), ty.repr) {
                    // A fixed flat extent is pointer-free bytes: the copy is its bytes, inline
                    // (C types the result at the flat layout itself).
                    (LayoutKind::Flat, ValueRepr::Aggregate) => {
                        let value = self.load(self.layout_shape(layout)?, self.place(address)?);
                        self.define(result, value)
                    }
                    // A variable flat value is a block of its extent: the copy is a new heap
                    // block holding the same bytes.
                    (
                        LayoutKind::Block { payload } | LayoutKind::Box { payload },
                        ValueRepr::Block,
                    ) if matches!(self.layout(payload)?.kind, LayoutKind::Flat) => {
                        let place = self.place(address)?;
                        let block =
                            self.builder
                                .load(self.module.ptr_type(), place.ptr, place.align);
                        let from = self.payload_of(block, payload)?;
                        let extent = self.extent(payload, from.ptr)?;
                        let anchor = self.failure_edge(failure)?.anchor;
                        let copy = self.alloc_block(payload, extent, from.align, anchor)?;
                        let to = self.payload_of(copy, payload)?;
                        self.builder
                            .memcpy(to.ptr, to.align, from.ptr, from.align, extent);
                        self.define(result, copy)
                    }
                    _ => self.invalid("a flat copy of a layout that is not flat"),
                }
            }
            Inst::UniqueCopy {
                result,
                value,
                construct,
                deep,
                failure,
            } => {
                let site = self.construct_site(construct)?;
                let ty = self.value_type(value)?;
                // An inline value (an aggregate, variant or inline array holding blocks) is
                // copied in place: a bit copy in a slot of its layout whose holders the
                // runtime rewrites to deep copies.
                if deep && ty.repr == ValueRepr::Aggregate {
                    let shape = self.layout_shape(ty.layout)?;
                    let slot = self.stack(ty.layout)?;
                    let image = self.get(value)?;
                    self.store(shape, image, slot);
                    let entry = self.runtime(RuntimeEntry::UniqueCopyValue)?;
                    let descriptor = self.descriptor(ty.layout)?;
                    let site = self.const_u64(self.anchor_code(site.anchor)?);
                    let status = self
                        .builder
                        .call(entry, &[self.context, descriptor, slot.ptr, site]);
                    self.status_guard(status, failure)?;
                    let copy = self.load(shape, slot);
                    return self.define(result, copy);
                }
                if !matches!(
                    site.placement,
                    Some(Placement::Fixed(StorageOwner::Heap(_)))
                ) {
                    return self.unsupported("a unique copy outside the heap");
                }
                if ty.repr != ValueRepr::Block {
                    return self.invalid("a unique copy of a value that is not a block");
                }
                // A shallow detach preserves the entire sequence payload. Save
                // the old length in SSA before the opaque allocation call.
                let old_length = if !deep {
                    match self.layout(ty.layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload }
                            if matches!(
                                self.layout(payload)?.kind,
                                LayoutKind::Sequence { .. }
                            ) =>
                        {
                            let data = self.payload_of(self.get(value)?, payload)?;
                            Some((
                                payload,
                                self.preserved_lengths[value.index()].unwrap_or_else(|| {
                                    self.builder.load_access(
                                        self.module.int_type(64),
                                        data.ptr,
                                        8,
                                        Access::Length,
                                    )
                                }),
                            ))
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some((_, length)) = old_length {
                    self.preserved_lengths[result.index()] = Some(length);
                }
                let entry = self.runtime(if deep {
                    RuntimeEntry::UniqueCopy
                } else {
                    RuntimeEntry::Detach
                })?;
                let out = self.prologue.alloca(self.module.ptr_type(), 8);
                let site = self.const_u64(self.anchor_code(site.anchor)?);
                let status = self
                    .builder
                    .call(entry, &[self.context, self.get(value)?, site, out]);
                self.status_guard(status, failure)?;
                let copy = self.builder.load(self.module.ptr_type(), out, 8);
                if !deep {
                    // copy.rs::hirsel_rt_v2_detach allocates count-one storage,
                    // copies the payload verbatim and only increments children.
                    // Its result joins through local storage-version tokens,
                    // not LLVM pointer/length phis, so direct SSA forwarding
                    // cannot represent the postconditions at those joins here.
                    // These assumptions apply only after successful detach;
                    // Dup and ordinary writes invalidate the tagged loads.
                    let unique = crate::runtime::is_unique(&self.builder, copy);
                    self.builder
                        .assume(unique)
                        .map_err(|what| CodegenError::Invalid {
                            definition: self.definition().clone(),
                            what,
                        })?;
                    if let Some((payload, length)) = old_length {
                        let data = self.payload_of(copy, payload)?;
                        let copied_length = self.builder.load_access(
                            self.module.int_type(64),
                            data.ptr,
                            8,
                            Access::Length,
                        );
                        let equal = self.builder.icmp(IntPredicate::Eq, copied_length, length);
                        self.builder
                            .assume(equal)
                            .map_err(|what| CodegenError::Invalid {
                                definition: self.definition().clone(),
                                what,
                            })?;
                    }
                }

                self.define(result, copy)
            }
            Inst::TokenNone { result } => {
                let null = self.module.const_zero(self.module.ptr_type());
                self.define(result, null)
            }
            Inst::Dup { value } => {
                self.invalidate_uniqueness();
                let block = self.get(value)?;
                self.dup_block(block)
            }
            Inst::Dispose { token } => {
                let dispose = self.runtime(RuntimeEntry::Dispose)?;
                self.builder
                    .call(dispose, &[self.context, self.get(token)?]);
                Ok(())
            }
            Inst::Compare {
                result,
                op,
                left,
                right,
                layout,
                failure,
            } => {
                let comparator = match compare::comparator(
                    self.module,
                    self.inputs.layouts,
                    layout,
                    matches!(op, CompareOp::Eq | CompareOp::Ne),
                ) {
                    Ok(comparator) => comparator,
                    Err(CompareError::Unsupported(what)) => return self.unsupported(what),
                    Err(CompareError::Invalid(what)) => return self.invalid(what),
                };
                if self.module.declares("hirsel_rt_v2_path_compare") {
                    self.manifested
                        .imports
                        .entry("hirsel_rt_v2_path_compare".into())
                        .or_insert_with(|| ObjectSymbol {
                            name: "hirsel_rt_v2_path_compare".into(),
                            kind: ObjectSymbolKind::Runtime,
                            definition: None,
                            abi: None,
                        });
                }
                // Both operands are read in place: each spilled to a slot of the layout.
                let shape = self.layout_shape(layout)?;
                let (l, r) = (self.stack(layout)?, self.stack(layout)?);
                let (left, right) = (self.counted_view(left)?, self.counted_view(right)?);
                self.store(shape, left, l);
                self.store(shape, right, r);
                let code = self
                    .builder
                    .call(comparator, &[l.ptr, r.ptr, self.const_u64(0), self.context]);
                let i8_ty = self.module.int_type(8);
                let is = |c: u8| {
                    self.builder.icmp(
                        IntPredicate::Eq,
                        code,
                        self.module.const_int(i8_ty, u128::from(c)),
                    )
                };
                let deep = is(ORDER_DEPTH);
                let (equal, less, greater) = (is(ORDER_EQUAL), is(ORDER_LESS), is(ORDER_GREATER));
                self.guard(deep, failure)?;
                let holds = match op {
                    CompareOp::Eq => equal,
                    CompareOp::Ne => self.builder.binary(
                        BinOp::Xor,
                        equal,
                        self.module.const_int(self.module.int_type(1), 1),
                    ),
                    CompareOp::Lt => less,
                    CompareOp::Le => self.builder.binary(BinOp::Or, equal, less),
                    CompareOp::Gt => greater,
                    CompareOp::Ge => self.builder.binary(BinOp::Or, equal, greater),
                };
                self.define(result, holds)
            }
            Inst::FlatView { result, address } => {
                // A borrowed slice of the String or Bytes encoding at `address`: the block
                // holding it is the owner, the encoding's offset in its payload the start,
                // and its prefix the length (LIR schema revision 4). No credit moves.
                let place = self.place(address)?;
                let Some(block) = place.block else {
                    return self.invalid("a flat view whose address crosses no counted block");
                };
                let layout = self.value_type(result)?.layout;
                let record = self.layout(layout)?;
                let i64_ty = self.module.int_type(64);
                let payload = self.payload_of(block, place.layout)?;
                let to_int = |ptr| self.builder.cast(Cast::PtrToInt, ptr, i64_ty);
                let start = self
                    .builder
                    .binary(BinOp::Sub, to_int(place.ptr), to_int(payload.ptr));
                let length = self.load_u64(place.ptr);
                let slice = self.stack(layout)?;
                for field in &record.fields {
                    let at = self.builder.offset(slice.ptr, self.const_u64(field.offset));
                    let word = match field.ordinal {
                        0 => block,
                        1 => start,
                        2 => length,
                        _ => return self.invalid("a slice layout with a fourth word"),
                    };
                    self.builder.store(word, at, slice.align.min(8));
                }
                let value = self.load(self.layout_shape(layout)?, slice);
                self.define(result, value)
            }
            Inst::FlatLength { result, address } => {
                let count = self.flat_count(self.place(address)?)?;
                self.define(result, count)
            }
            Inst::FlatNext { result, address } => {
                // The content offset just past the element: its end less where its
                // sequence's elements begin.
                let place = self.place(address)?;
                let Some(content) = self.flat_contents[address.index()] else {
                    return self.invalid("the end of a place that is no variable flat element");
                };
                let extent = self.extent(place.layout, place.ptr)?;
                let i64_ty = self.module.int_type(64);
                let to_int = |ptr| self.builder.cast(Cast::PtrToInt, ptr, i64_ty);
                let start = self
                    .builder
                    .binary(BinOp::Sub, to_int(place.ptr), to_int(content));
                let end = self.builder.binary(BinOp::Add, start, extent);
                self.define(result, end)
            }
            Inst::SeqLen { result, value } => {
                let count = if let Some(length) = self.preserved_lengths[value.index()] {
                    length
                } else {
                    match self.value_type(value)?.repr {
                        ValueRepr::Block => {
                            let data = match self.layout_payload(self.value_type(value)?.layout)? {
                                // A sequential Packed sequence counts its elements in its
                                // first word too (MEM-13).
                                Some(payload)
                                    if matches!(
                                        self.layout(payload)?.kind,
                                        LayoutKind::Sequential { .. }
                                    ) =>
                                {
                                    let pointer = self.counted_view(value)?;
                                    self.payload_of(pointer, payload)?.ptr
                                }
                                _ => self.sequence_payload(value)?.0,
                            };
                            self.builder.load_access(
                                self.module.int_type(64),
                                data,
                                8,
                                Access::Length,
                            )
                        }
                        _ => self.slice_bytes(value)?.1,
                    }
                };
                self.define(result, count)
            }
            Inst::SeqCapacity { result, value } => {
                let capacity = self.sequence_capacity(value)?;
                self.define(result, capacity)
            }
            Inst::SeqReserve {
                address, capacity, ..
            } => {
                let place = self.place(address)?;
                if let Some(payload) = self.layout_payload(place.layout)?
                    && let LayoutKind::Sequential { element } = self.layout(payload)?.kind
                {
                    // A flat builder's reservation: room for `capacity` of its least
                    // element in all, past the count and used words, the bytes
                    // saturating (LIR schema revision 22).
                    let first = self.sequential_offset(element)?;
                    let record = self.layout(element)?;
                    let least = match (&record.kind, &record.size, record.fields.as_slice()) {
                        (LayoutKind::Flat, SizeSpec::Dynamic(_), [content]) => content
                            .offset
                            .saturating_add(hirsel_lir::layout::least_size(
                                &self.layout(content.layout)?.size,
                            )),
                        _ => hirsel_lir::layout::least_size(&record.size),
                    };
                    let i64_ty = self.module.int_type(64);
                    let multiply = self
                        .module
                        .intrinsic("llvm.umul.with.overflow", &[i64_ty])
                        .or_else(|e| self.invalid(e))?;
                    let pair = self
                        .builder
                        .call(multiply, &[self.get(capacity)?, self.const_u64(least)]);
                    let bytes = self.builder.select(
                        self.builder.extract(pair, 1),
                        self.const_u64(u64::MAX),
                        self.builder.extract(pair, 0),
                    );
                    let align = u64::from(self.layout(payload)?.align);
                    let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                    let data = self.payload_of(old, payload)?.ptr;
                    let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
                    let keep = self.builder.binary(BinOp::Add, used, self.const_u64(first));
                    let (extent, _) = scalar::int_arith(
                        &self.builder,
                        BinaryOp::Add,
                        ArithMode::Saturating,
                        ScalarKind::U64,
                        bytes,
                        self.const_u64(first),
                    )
                    .or_else(|e| self.invalid(e))?;
                    let grow = self.runtime(RuntimeEntry::BlockGrow)?;
                    let grown = self.builder.call(
                        grow,
                        &[self.context, old, keep, extent, self.const_u64(align)],
                    );
                    self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
                    return Ok(());
                }
                let Some(element) = self.sequence_element(place.layout)? else {
                    return self.invalid("a reserve of a place that holds no sequence block");
                };
                let (size, align) = (
                    self.fixed_size(element)?,
                    u64::from(self.layout(element)?.align),
                );
                let reserve = self.runtime(RuntimeEntry::SeqReserve)?;
                let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                let grown = self.builder.call(
                    reserve,
                    &[
                        self.context,
                        old,
                        self.const_u64(size),
                        self.const_u64(align),
                        self.get(capacity)?,
                    ],
                );
                self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
                // Growth either retains the unique input or allocates count-one
                // storage. It preserves length on both paths.
                if let Some(state) = self.sequence_state_at(place) {
                    self.builder.store(
                        self.module.const_int(self.module.int_type(1), 1),
                        state.unique,
                        1,
                    );
                }
                Ok(())
            }
            Inst::SeqAppend { address, value, .. } => {
                // MEM-13: the element's active extent past the used bytes, growing the
                // storage geometrically when it is full (LIR schema revision 19).
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let temp = self.stack(element)?;
                self.store(self.value_shape(value)?, self.get(value)?, temp);
                let bytes = self.extent(element, temp.ptr)?;
                let (data, used) = self.sequential_room(place, payload, first, bytes)?;
                let at = self.builder.offset(
                    data,
                    self.builder.binary(BinOp::Add, used, self.const_u64(first)),
                );
                self.builder
                    .memcpy(at, temp.align, temp.ptr, temp.align, bytes);
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::FlatAppend {
                address, construct, ..
            } => {
                // One element written in its flat encoding past the used bytes: its
                // content's length, then the construction's parts (LIR schema
                // revision 22). Growth is `SeqAppend`'s.
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let site = self.construct_site(construct)?;
                let parts = self.flat_parts(site)?;
                let content = self.flat_extent(&parts)?;
                let bytes = self.flat_add(content, self.const_u64(8))?;
                let (data, used) = self.sequential_room(place, payload, first, bytes)?;
                let at = self.builder.offset(
                    data,
                    self.builder.binary(BinOp::Add, used, self.const_u64(first)),
                );
                self.builder.store(content, at, 1);
                self.write_flat(self.builder.offset(at, self.const_u64(8)), &parts)?;
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            // The open element's writes past the used bytes, whose field's length
            // word lies at `word` in the element (LIR schema revision 24). Growth
            // keeps the bytes written so far.
            Inst::FlatField {
                address, field, op, ..
            } => {
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let word = first + self.open_word(element, field)?;
                let (old, data, used) = self.sequential_used(place, payload)?;
                let at = self.builder.binary(BinOp::Add, used, self.const_u64(word));
                if op == FlatFieldOp::Open {
                    let need = self.builder.binary(BinOp::Add, at, self.const_u64(8));
                    let keep = self.builder.binary(BinOp::Add, used, self.const_u64(first));
                    let data = self.sequential_grow(place, payload, old, keep, need)?;
                    self.builder
                        .store(self.const_u64(0), self.builder.offset(data, at), 1);
                    return Ok(());
                }
                let length = self.load_u64(self.builder.offset(data, at));
                let past = self.builder.binary(BinOp::Add, at, self.const_u64(8));
                let keep = self.builder.binary(BinOp::Add, past, length);
                match op {
                    FlatFieldOp::Open => unreachable!("opened above"),
                    FlatFieldOp::Reserve { capacity } => {
                        let (need, _) = scalar::int_arith(
                            &self.builder,
                            BinaryOp::Add,
                            ArithMode::Saturating,
                            ScalarKind::U64,
                            keep,
                            self.get(capacity)?,
                        )
                        .or_else(|e| self.invalid(e))?;
                        self.sequential_grow(place, payload, old, keep, need)?;
                    }
                    FlatFieldOp::Push { value, spare } => {
                        let data = if spare {
                            data
                        } else {
                            let need = self.builder.binary(BinOp::Add, keep, self.const_u64(1));
                            self.sequential_grow(place, payload, old, keep, need)?
                        };
                        self.builder
                            .store(self.get(value)?, self.builder.offset(data, keep), 1);
                        let longer = self.builder.binary(BinOp::Add, length, self.const_u64(1));
                        self.builder.store(longer, self.builder.offset(data, at), 1);
                    }
                }
                Ok(())
            }
            // The open element completed: its prefix and the construction's other
            // fields written around the bytes in place, as `FlatAppend` writes an
            // element. Only fields after the open one need more room.
            Inst::FlatClose {
                address,
                construct,
                field,
                ..
            } => {
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let word = self.open_word(element, field)?;
                let (old, data, used) = self.sequential_used(place, payload)?;
                let start = self.builder.binary(BinOp::Add, used, self.const_u64(first));
                let at = self.builder.binary(BinOp::Add, start, self.const_u64(word));
                let length = self.load_u64(self.builder.offset(data, at));
                let site = self.construct_site(construct)?;
                let parts = self.flat_parts_around(site, Some((field, length)))?;
                let content = self.flat_extent(&parts)?;
                let bytes = self.flat_add(content, self.const_u64(8))?;
                let trailing = !matches!(parts.last(), Some(FlatPart::InPlace { .. }));
                let data = if trailing {
                    let past = self.builder.binary(BinOp::Add, at, self.const_u64(8));
                    let keep = self.builder.binary(BinOp::Add, past, length);
                    let need = self.builder.binary(BinOp::Add, start, bytes);
                    self.sequential_grow(place, payload, old, keep, need)?
                } else {
                    data
                };
                let at = self.builder.offset(data, start);
                self.builder.store(content, at, 1);
                self.write_flat(self.builder.offset(at, self.const_u64(8)), &parts)?;
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::SeqSetLen { address, len, .. } => {
                let place = self.place(address)?;
                // A deferred slot's block, unique here, takes its count at the next
                // flush; in a push loop, after the loop.
                if let Some(state) = self.sequence_state_at(place)
                    && self
                        .whole_slot(address)
                        .is_some_and(|s| self.deferred[s.index()])
                {
                    self.builder.store(self.get(len)?, state.length, 8);
                    self.builder.store(
                        self.module.const_int(self.module.int_type(1), 1),
                        state.dirty,
                        1,
                    );
                    return Ok(());
                }
                let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                let Some(payload) = self.layout_payload(place.layout)? else {
                    return self.invalid("a length of a place that holds no block");
                };
                let data = self.payload_of(pointer, payload)?;
                self.builder
                    .store_access(self.get(len)?, data.ptr, 8, Access::Length);
                if let Some(state) = self.sequence_state_at(place) {
                    self.builder.store(self.get(len)?, state.length, 8);
                }
                Ok(())
            }
            Inst::Hash {
                result,
                value,
                layout,
                failure,
            } => {
                let hash_bytes = self.runtime(RuntimeEntry::HashBytes)?;
                let hasher =
                    match compare::hasher(self.module, self.inputs.layouts, layout, hash_bytes) {
                        Ok(hasher) => hasher,
                        Err(CompareError::Unsupported(what)) => return self.unsupported(what),
                        Err(CompareError::Invalid(what)) => return self.invalid(what),
                    };
                // The value is read in place: spilled to a slot of the layout.
                let at = self.stack(layout)?;
                let operand = self.counted_view(value)?;
                self.store(self.layout_shape(layout)?, operand, at);
                let begin = self.runtime(RuntimeEntry::HashBegin)?;
                let state = self.builder.call(begin, &[self.context]);
                let i8_ty = self.module.int_type(8);
                let deep = self.prologue.alloca(i8_ty, 1);
                self.builder.store(self.module.const_int(i8_ty, 0), deep, 1);
                let hash = self
                    .builder
                    .call(hasher, &[at.ptr, state, self.const_u64(0), deep]);
                let flag = self.builder.load(i8_ty, deep, 1);
                let too_deep =
                    self.builder
                        .icmp(IntPredicate::Ne, flag, self.module.const_int(i8_ty, 0));
                self.guard(too_deep, failure)?;
                self.define(result, hash)
            }
            // The same pointer at another payload layout: no code.
            Inst::Recast { result, value } => {
                let pointer = self.get(value)?;
                self.define(result, pointer)
            }
            Inst::Release { value, .. } if self.adopted.contains_key(&value) => {
                // A builder whose block a flat record took over is not released.
                let taken = self.adopted.remove(&value).expect("an adopted release");
                let (release, done) = (
                    self.module.append_block(self.function),
                    self.module.append_block(self.function),
                );
                self.builder.cond_br(taken, done, release);
                self.builder.position(release);
                self.instruction(inst)?;
                self.builder.br(done);
                self.builder.position(done);
                Ok(())
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } if self.value_type(value)?.repr != ValueRepr::Block => {
                self.release_inline(value, layout, live, anchor, failure)
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } if live == MaskRef::All && glue::covered(self.inputs.layouts, layout) => {
                // Compiled drop glue; the runtime walks the step program of every other
                // layout and of a partly moved block.
                let block = self.get(value)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                let status = self.drop_glue(anchor, |walk| walk.release(block, Some(layout)))?;
                self.status_guard(status, failure)
            }
            Inst::Release {
                value,
                layout: _,
                live,
                anchor,
                failure,
            } => {
                let release = self.runtime(RuntimeEntry::Release)?;
                let block = self.get(value)?;
                let descriptor = self.block_descriptor(block);
                let arguments = [
                    self.context,
                    block,
                    descriptor,
                    self.live_mask(live)?,
                    self.const_u64(self.cleanup_code(anchor)?),
                ];
                let status = self.builder.call(release, &arguments);
                self.status_guard(status, failure)
            }
            Inst::Reset {
                result,
                value,
                layout: _,
                live,
                anchor,
                failure,
            } => {
                // A callable donates its environment block (LIR schema revision 5), which
                // `layout` describes.
                let block = match self.value_type(value)?.repr {
                    ValueRepr::Callable => self.callable_parts(value)?.1,
                    _ => self.get(value)?,
                };
                let reset = self.runtime(RuntimeEntry::Reset)?;
                let descriptor = self.block_descriptor(block);
                let out = self.prologue.alloca(self.module.ptr_type(), 8);
                let arguments = [
                    self.context,
                    block,
                    descriptor,
                    self.live_mask(live)?,
                    self.const_u64(self.cleanup_code(anchor)?),
                    out,
                ];
                let status = self.builder.call(reset, &arguments);
                self.status_guard(status, failure)?;
                let token = self.builder.load(self.module.ptr_type(), out, 8);
                self.define(result, token)
            }
            Inst::Reuse {
                result,
                token,
                construct,
                kept,
                failure,
            } => {
                let site = self.construct_site(construct)?;
                let token = self.get(token)?;
                self.construct_block(result, site, Some((token, kept)), failure)
            }
            Inst::RegionEnter { result, region } => {
                let Some(&record) = self.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                // An arena's region (its mark names an `ArenaToken`) enters warm, its cursor
                // seeded from the pool, so its first construction bumps inline.
                let arena =
                    self.unit.instructions.iter().any(
                        |inst| matches!(*inst, Inst::ArenaToken { mark, .. } if mark == result),
                    );
                let enter = self.runtime(if arena {
                    RuntimeEntry::ArenaEnter
                } else {
                    RuntimeEntry::RegionEnter
                })?;
                self.builder.call(enter, &[self.context, record]);
                self.define(result, record)
            }
            Inst::RegionReserve {
                result,
                mark,
                layout,
                extent,
                failure,
            } => {
                let size = match self.layout(layout)?.size {
                    SizeSpec::Fixed(size) => self.const_u64(size),
                    SizeSpec::Dynamic(_) => self.get(extent)?,
                };
                let storage = self.reserve_region(self.get(mark)?, layout, size, failure)?;
                self.define(result, storage.ptr)
            }
            // Publication is static: nothing moves or writes published storage afterwards.
            Inst::RegionPublish { .. } => Ok(()),
            // A token is its region's record address.
            Inst::ArenaToken { result, mark } => {
                let record = self.get(mark)?;
                self.define(result, record)
            }
            Inst::ArenaOpen { .. } => self.invalid("an arena opening after placement"),
            Inst::RegionExit {
                mark,
                region,
                failure,
            } => {
                let Some(instance) = self.unit.tables.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                let anchor = self.anchor_code(instance.anchor)?;
                let exit = self.runtime(RuntimeEntry::RegionExit)?;
                let status = self.builder.call(
                    exit,
                    &[self.context, self.get(mark)?, self.const_u64(anchor)],
                );
                self.status_guard(status, failure)
            }
            // The resource's state moves (Live -> Finalizing -> FieldsPending); the drop body
            // is an explicit call between the two, shielded by the runtime.
            // Defuse is a validated ownership-state transition. Native storage
            // already holds the fields, and their ordinary releases follow.
            Inst::Defuse { .. } => Ok(()),
            Inst::FinalizeBegin { anchor, .. } => {
                let begin = self.runtime(RuntimeEntry::FinalizeBegin)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                self.builder.call(begin, &[self.context, anchor]);
                Ok(())
            }
            Inst::FinalizeEnd { anchor, .. } => {
                let end = self.runtime(RuntimeEntry::FinalizeEnd)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                self.builder.call(end, &[self.context, anchor]);
                Ok(())
            }
            Inst::Copy {
                result, address, ..
            } => {
                // A semantic copy by the Dup or Fields recipe: the place's bits, then one count
                // increment per counted holder in scan order. It never allocates and never
                // fails, so its failure edge is never taken.
                let place = self.place(address)?;
                if self.value_type(result)?.layout != place.layout {
                    return self.invalid("a copy at another layout than its place");
                }
                let value = self.load(self.layout_shape(place.layout)?, place);
                self.dup_holders(place)?;
                self.define(result, value)
            }
            Inst::Drop { .. } | Inst::Unavailable { .. } => {
                self.invalid("an operation outside the Ready vocabulary")
            }
        }
    }

    fn constant(&mut self, constant: ConstantId) -> Emitted<Value<'m>> {
        let Some(data) = self.unit.tables.constants.get(constant.index()) else {
            return self.invalid("a constant outside the constant table");
        };
        let shape = self.shape(self.lir_type(data.ty)?)?;
        let module = self.module;
        Ok(match (&data.value, shape) {
            (ConstantValue::Unit, Shape::Unit) => module.const_zero(self.llvm_type(shape)),
            (ConstantValue::Bool(b), Shape::Scalar(ScalarKind::Bool)) => {
                module.const_int(module.int_type(1), u128::from(*b))
            }
            (ConstantValue::Integer(bits), Shape::Scalar(kind))
                if scalar::integer(kind).is_some() =>
            {
                let width = scalar::integer(kind).unwrap().0;
                if width < 128 && *bits >> width != 0 {
                    return self.invalid("an integer constant wider than its type");
                }
                module.const_int(module.int_type(width), *bits)
            }
            (ConstantValue::F32(bits), Shape::Scalar(ScalarKind::F32)) => {
                let int = module.const_int(module.int_type(32), u128::from(*bits));
                self.builder.cast(Cast::Bits, int, module.f32_type())
            }
            (ConstantValue::F64(bits), Shape::Scalar(ScalarKind::F64)) => {
                let int = module.const_int(module.int_type(64), u128::from(*bits));
                self.builder.cast(Cast::Bits, int, module.f64_type())
            }
            (ConstantValue::String(text), _) => {
                return self.text_literal(data.ty, text.as_bytes());
            }
            (ConstantValue::Bytes(bytes), _) => return self.text_literal(data.ty, bytes),
            _ => return self.invalid("a constant of another type"),
        })
    }

    /// A String or Bytes literal over a static immortal block: a Plain slice (owner, start 0,
    /// length) for a `Slice` layout, else the block reference itself.
    fn text_literal(&mut self, ty: LirTypeId, bytes: &[u8]) -> Emitted<Value<'m>> {
        let ty = self.lir_type(ty)?;
        let record = self.layout(ty.layout)?;
        match (ty.repr, &record.kind) {
            (ValueRepr::Aggregate, &LayoutKind::Slice { element }) => {
                let field = |ordinal: u32| record.fields.iter().find(|f| f.ordinal == ordinal);
                let (Some(owner), Some(start), Some(length)) = (field(0), field(1), field(2))
                else {
                    return self.invalid("a slice layout without owner, start and length");
                };
                // The owner is the sequence block of the slice's element the table derived
                // with it.
                let Some(payload) = slice_storage(self.inputs.layouts, element) else {
                    return self.invalid("a slice without its element's sequence layout");
                };
                let block = self.literal_block(payload, bytes)?;
                let whole = match sequence_slice_start(self.layout(element)?) {
                    Ok(whole) => whole,
                    Err(error) => return self.invalid(format!("{error:?}")),
                };
                let place = self.stack(ty.layout)?;
                let size = self.fixed_size(ty.layout)?;
                self.builder.memset(place.ptr, 0, size, place.align);
                let at = self.field_place(place, owner.offset, owner.layout, false)?;
                self.store(Shape::Scalar(ScalarKind::Pointer), block, at);
                let at = self.field_place(place, start.offset, start.layout, false)?;
                self.store(Shape::Scalar(ScalarKind::U64), self.const_u64(whole), at);
                let at = self.field_place(place, length.offset, length.layout, false)?;
                let length = self.const_u64(bytes.len() as u64);
                self.store(Shape::Scalar(ScalarKind::U64), length, at);
                Ok(self.load(self.layout_shape(ty.layout)?, place))
            }
            (
                ValueRepr::Block | ValueRepr::Uncounted,
                LayoutKind::Block { payload } | LayoutKind::Box { payload },
            ) => {
                let payload = *payload;
                let block = self.literal_block(payload, bytes)?;
                if ty.repr == ValueRepr::Block {
                    return Ok(block);
                }
                Ok(self.payload_of(block, payload)?.ptr)
            }
            _ => self.invalid("a text constant of another type"),
        }
    }

    /// The payload of a counted block, behind its header.
    fn payload_of(&self, block: Value<'m>, payload: LayoutId) -> Emitted<Place<'m>> {
        let align = self.layout(payload)?.align;
        let offset = match payload_offset(align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        Ok(Place {
            ptr: self.builder.offset(block, self.const_u64(offset)),
            layout: payload,
            align,
            block: Some(block),
        })
    }

    /// `ReadStatic`: the static immortal blocks of the symbol's data. A reference to the
    /// constant's block names its header (an uncounted reference its payload); any other
    /// value is read from that payload.
    fn read_static(&mut self, result: ValueId, symbol: SymbolId) -> Emitted<Value<'m>> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a static outside the symbol table");
        };
        let Some(data) = self.inputs.statics.iter().find(|d| d.symbol == *symbol) else {
            return self.invalid("a static without its data");
        };
        let Some(root) = data.blocks.first() else {
            return self.invalid("static data without a block");
        };
        let held = root.layout;
        let name = format!(
            "hirsel.static.{}",
            symbol_name(&symbol.definition, symbol.instance.as_ref())
        );
        // A block names only blocks after it, so each is emitted after its targets.
        let mut emitted: Vec<Option<Value<'m>>> = vec![None; data.blocks.len()];
        for (index, block) in data.blocks.iter().enumerate().rev() {
            if let SizeSpec::Fixed(size) = self.layout(block.layout)?.size
                && size != block.bytes.len() as u64
            {
                return self.invalid("static data of another size than its layout");
            }
            let mut references = Vec::with_capacity(block.references.len());
            for r in &block.references {
                let target = r.target as usize;
                let Some(Some(target)) = emitted.get(target).filter(|_| target > index) else {
                    return self.invalid("a static reference to no later block");
                };
                references.push((r.offset, *target));
            }
            let descriptor = self.descriptor(block.layout)?;
            let align = self.layout(block.layout)?.align;
            let block_name = match index {
                0 => name.clone(),
                _ => format!("{name}.{index}"),
            };
            emitted[index] = Some(self.static_block(
                &block_name,
                descriptor,
                align,
                &block.bytes,
                &references,
            )?);
        }
        let Some(block) = emitted[0] else {
            return self.invalid("static data without a block");
        };
        let ty = self.value_type(result)?;
        match (ty.repr, &self.layout(ty.layout)?.kind) {
            (
                ValueRepr::Block,
                LayoutKind::Block { payload: p } | LayoutKind::Box { payload: p },
            ) if *p == held => Ok(block),
            (ValueRepr::Uncounted, _) => Ok(self.payload_of(block, held)?.ptr),
            _ if ty.layout == held => {
                let place = self.payload_of(block, held)?;
                Ok(self.load(self.layout_shape(held)?, place))
            }
            _ => self.invalid("a static read at another layout than its data"),
        }
    }

    /// A callable value: the code pointer, then the environment block.
    fn callable(
        &self,
        layout: LayoutId,
        code: Value<'m>,
        environment: Value<'m>,
    ) -> Emitted<Value<'m>> {
        if self.layout_shape(layout)? == Shape::Callable {
            let value = self.module.const_zero(self.llvm_type(Shape::Callable));
            let value = self.builder.insert(value, code, 0);
            return Ok(self.builder.insert(value, environment, 1));
        }
        let place = self.stack(layout)?;
        let word = u64::from(self.inputs.target.pointer_bytes);
        self.builder
            .memset(place.ptr, 0, self.fixed_size(layout)?, place.align);
        let iterator = matches!(self.layout(layout)?.kind, LayoutKind::Iterator { .. });
        let code_at = self.field_place(place, if iterator { word } else { 0 }, layout, false)?;
        let env_at = self.field_place(place, if iterator { 0 } else { word }, layout, false)?;
        self.builder.store(code, code_at.ptr, code_at.align);
        self.builder.store(environment, env_at.ptr, env_at.align);
        Ok(self.load(self.layout_shape(layout)?, place))
    }

    /// The code pointer and environment block of a callable value.
    fn callable_parts(&self, callable: ValueId) -> Emitted<(Value<'m>, Value<'m>)> {
        let layout = self.value_type(callable)?.layout;
        if self.layout_shape(layout)? == Shape::Callable {
            let value = self.get(callable)?;
            return Ok((
                self.builder.extract(value, 0),
                self.builder.extract(value, 1),
            ));
        }
        let place = self.stack(layout)?;
        self.store(self.value_shape(callable)?, self.get(callable)?, place);
        let word = u64::from(self.inputs.target.pointer_bytes);
        let ptr_ty = self.module.ptr_type();
        let iterator = matches!(self.layout(layout)?.kind, LayoutKind::Iterator { .. });
        let code_at = self.field_place(place, if iterator { word } else { 0 }, layout, false)?;
        let code = self.builder.load(ptr_ty, code_at.ptr, code_at.align);
        let env_at = self.field_place(place, if iterator { 0 } else { word }, layout, false)?;
        let environment = self.builder.load(ptr_ty, env_at.ptr, env_at.align);
        Ok((code, environment))
    }

    /// Imports a function symbol the object calls or refers to, unless the object defines it.
    fn import_function(&mut self, name: &str, definition: &DefPath, abi: Option<ContentHash>) {
        self.manifested
            .imports
            .entry(name.to_owned())
            .or_insert_with(|| ObjectSymbol {
                name: name.to_owned(),
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi,
            });
    }

    /// `FunctionRef`: the definition's function-reference thunk with the static empty
    /// environment, a Plain static callable.
    fn function_ref(&mut self, result: ValueId, symbol: SymbolId) -> Emitted<Value<'m>> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a function reference outside the symbol table");
        };
        let name = function_thunk_name(&symbol.definition, symbol.instance.as_ref());
        self.import_function(&name, &symbol.definition, None);
        let code = self.module.function_address(&name);
        let environment = self.empty_environment()?;
        let layout = self.value_type(result)?.layout;
        self.callable(layout, code, environment)
    }

    /// `Closure`: a counted heap environment at the site's `environment()` layout holding the
    /// captures (a borrowed capture as its view, a `mut` capture as its address, a copied one
    /// with a new credit), or the static empty environment when it has no bytes.
    fn closure(&mut self, result: ValueId, closure: ClosureId) -> Emitted<Value<'m>> {
        let unit = self.unit;
        let Some(site) = unit.tables.closures.get(closure.index()) else {
            return self.invalid("a closure outside the closure table");
        };
        // A generic nested function's value has no code of its own (LIR
        // schema revision 9): its code word is null, and bound calls name
        // each instantiation's code.
        let code = match site.code {
            Some(c) => {
                let Some(symbol) = unit.tables.symbols.get(c.symbol.index()) else {
                    return self.invalid("closure code outside the symbol table");
                };
                let name = symbol_name(&symbol.definition, symbol.instance.as_ref());
                let abi = fn_abi(self.inputs.layouts, c.abi, self.definition())?;
                self.import_function(&name, &symbol.definition, Some(abi_digest(abi)));
                self.module.function_address(&name)
            }
            None => self.module.const_zero(self.module.ptr_type()),
        };
        let layout = self.value_type(result)?.layout;
        let size = self.fixed_size(site.environment)?;
        if size == 0 {
            let environment = self.empty_environment()?;
            return self.callable(layout, code, environment);
        }
        if !matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::Heap(_)))
        ) {
            return self.unsupported("a closure environment outside the heap");
        }
        let record = self.layout(site.environment)?;
        let align = record.align;
        let block = self.alloc_block(site.environment, self.const_u64(size), align, site.anchor)?;
        let payload = self.payload_of(block, site.environment)?;
        self.builder.memset(payload.ptr, 0, size, align);
        let captures = self.pool(site.captures, &unit.pools.captures)?;
        for (i, capture) in captures.iter().enumerate() {
            let Some(field) = record.fields.iter().find(|f| f.ordinal == i as u32) else {
                return self.invalid("a capture without its environment field");
            };
            let at = self.field_place(payload, field.offset, field.layout, field.unaligned)?;
            self.store(
                self.value_shape(capture.value)?,
                self.get(capture.value)?,
                at,
            );
            // A copied uncounted reference (region storage) holds no credit to duplicate.
            if capture.usage == CaptureUse::Copy
                && self.value_type(capture.value)?.repr != ValueRepr::Uncounted
            {
                self.dup_holders(at)?;
            }
        }
        if let LayoutKind::Environment { movable } = &record.kind {
            let field = record.fields.last().expect("live word");
            let at = self.field_place(payload, field.offset, field.layout, field.unaligned)?;
            let bits = u64::MAX >> (64 - movable.len());
            self.store(Shape::Scalar(ScalarKind::U64), self.const_u64(bits), at);
        }
        self.callable(layout, code, block)
    }

    /// Adds one credit to every counted holder of the value at `place` (a semantic Copy by
    /// the `Dup` and `Fields` recipes), reading its layout's scan program.
    fn dup_holders(&mut self, place: Place<'m>) -> Emitted {
        self.invalidate_uniqueness();
        let record = self.layout(place.layout)?;
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } = record.kind {
            let block = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            return self.dup_block(block);
        }
        self.dup_scan(place, &record.scan)
    }

    fn dup_scan(&mut self, place: Place<'m>, scan: &'a [ScanStep]) -> Emitted {
        let ptr_ty = self.module.ptr_type();
        for step in scan {
            match step {
                ScanStep::Counted { offset }
                | ScanStep::SliceOwner { offset }
                | ScanStep::Environment { offset } => {
                    let at = self.builder.offset(place.ptr, self.const_u64(*offset));
                    let align = place.align.min(low_bit(*offset)).min(8);
                    let block = self.builder.load(ptr_ty, at, align);
                    self.dup_block(block)?;
                }
                ScanStep::Uncounted { .. } | ScanStep::Handle { .. } => {}
                ScanStep::Inline { offset, layout } => {
                    let child = self.field_place(place, *offset, *layout, false)?;
                    let scan = &self.layout(*layout)?.scan;
                    self.dup_scan(child, scan)?;
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let scan = &self.layout(*element)?.scan;
                    for i in 0..*count {
                        let child =
                            self.field_place(place, offset + i * stride, *element, false)?;
                        self.dup_scan(child, scan)?;
                    }
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let active = self.tag_at(place, *tag_offset, *form, false);
                    let done = self.module.append_block(self.function);
                    let mut cases = Vec::new();
                    let here = self.builder.current();
                    for (variant, steps) in variants {
                        let arm = self.module.append_block(self.function);
                        self.builder.position(arm);
                        self.dup_scan(place, steps)?;
                        self.builder.br(done);
                        cases.push((
                            self.module
                                .const_int(self.module.int_type(32), u128::from(*variant)),
                            arm,
                        ));
                    }
                    self.builder.position(here);
                    self.builder.switch(active, done, &cases);
                    self.builder.position(done);
                }
                ScanStep::Sequence { .. } | ScanStep::Columns(_) => {
                    return self.unsupported("a copy of an inline sequence");
                }
            }
        }
        Ok(())
    }

    /// `Release` of an owning inline value: its drop program walked in drop
    /// order with the counts decremented, each counted leaf released through the runtime
    /// with the descriptor its own header names. A failing leaf does not stop the walk; the
    /// failure edge is taken after it, as a block release accumulates. `live` gates
    /// top-level fields.
    fn release_inline(
        &mut self,
        value: ValueId,
        layout: LayoutId,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    ) -> Emitted {
        let ty = self.value_type(value)?;
        if ty.layout != layout
            || !matches!(
                ty.repr,
                ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle
            )
        {
            return self.invalid("an inline release at another layout or representation");
        }
        let place = self.stack(layout)?;
        self.store(self.value_shape(value)?, self.get(value)?, place);
        let record = self.layout(layout)?;
        // Byte ranges of the top-level fields the mask marks dead.
        let dead: Vec<(u64, u64)> = match live {
            MaskRef::All => Vec::new(),
            MaskRef::Static(mask) => {
                let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
                    return self.invalid("a mask outside the mask table");
                };
                let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
                if record.tag.is_some() {
                    return self.unsupported("a live mask on an inline variant release");
                }
                let mut dead = Vec::new();
                for field in &record.fields {
                    let i = field.ordinal as usize;
                    let set = words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1);
                    if !set {
                        let size = self.fixed_size(field.layout)?;
                        dead.push((field.offset, field.offset + size));
                    }
                }
                dead
            }
            MaskRef::Dynamic(_) => {
                return self.unsupported("a dynamic live mask on an inline release");
            }
        };
        let anchor = self.const_u64(self.cleanup_code(anchor)?);
        let steps: Vec<&'a DropStep> = record
            .drop
            .iter()
            .filter(|step| {
                let offset = match step {
                    DropStep::Release { offset }
                    | DropStep::Environment { offset }
                    | DropStep::CloseHandle { offset }
                    | DropStep::Child { offset, .. } => Some(*offset),
                    DropStep::RepeatReverse { offset, .. } => Some(*offset),
                    _ => None,
                };
                offset.is_none_or(|o| !dead.iter().any(|&(a, b)| a <= o && o < b))
            })
            .collect();
        let scope = glue::Scope::of(layout);
        let status = self.drop_glue(anchor, |walk| {
            steps
                .into_iter()
                .try_for_each(|step| walk.step(&scope, place.ptr, place.align, step))
        })?;
        self.status_guard(status, failure)
    }

    /// Emits drop code through `emit` at the builder's position, then the body of every
    /// release function it called. Returns the accumulated `i8` status: a failing leaf does
    /// not stop a drop.
    fn drop_glue(
        &mut self,
        anchor: Value<'m>,
        emit: impl FnOnce(&mut glue::Walk<'m, '_>) -> Result<(), glue::GlueError>,
    ) -> Emitted<Value<'m>> {
        let measure_release = if self.inputs.measurement {
            let name = "hirsel_rt_v2_measure_release";
            let ptr = self.module.ptr_type();
            let ty = self
                .module
                .function_type(self.module.void_type(), &[ptr, ptr]);
            let function =
                self.module
                    .function(name, ty)
                    .map_err(|what| CodegenError::Invalid {
                        definition: self.unit.header.definition.clone(),
                        what,
                    })?;
            self.manifested
                .imports
                .entry(name.to_owned())
                .or_insert_with(|| ObjectSymbol {
                    name: name.to_owned(),
                    kind: ObjectSymbolKind::Runtime,
                    definition: None,
                    abi: None,
                });
            Some(function)
        } else {
            None
        };
        let runtime = glue::Runtime {
            release: self.runtime(RuntimeEntry::Release)?,
            free: self.runtime(RuntimeEntry::Free)?,
            close_handle: self.runtime(RuntimeEntry::CloseHandle)?,
            measure_release,
        };
        let i8_ty = self.module.int_type(8);
        let status = self.prologue.alloca(i8_ty, 1);
        self.builder
            .store(self.module.const_int(i8_ty, 0), status, 1);
        let mut pending = Vec::new();
        let emitted = emit(&mut glue::Walk {
            module: self.module,
            layouts: self.inputs.layouts,
            runtime,
            function: self.function,
            builder: &self.builder,
            context: self.context,
            anchor,
            budget: self.const_u64(glue::GLUE_MAX_DEPTH),
            status,
            pending: &mut pending,
        });
        let defined =
            emitted.and_then(|()| glue::define(self.module, self.inputs.layouts, runtime, pending));
        if let Err(glue::GlueError(what)) = defined {
            return self.invalid(what);
        }
        Ok(self.builder.load(i8_ty, status, 1))
    }

    fn place(&self, address: AddressId) -> Emitted<Place<'m>> {
        match self.addresses.get(address.index()).copied().flatten() {
            Some(place) => Ok(place),
            None => self.invalid(format!("address {} used before its preparation", address.0)),
        }
    }

    /// Computes an address from its root, running every bounds and tag check on the way;
    /// any failing check takes `failure` before the address is used.
    fn prepare(&mut self, address: AddressId, failure: FailureId) -> Emitted<Place<'m>> {
        let Some(info) = self.unit.addresses.get(address.index()) else {
            return self.invalid("no such address");
        };
        let mut place = match info.base {
            AddressBase::Slot(slot) => match self.slots.get(slot.index()).copied().flatten() {
                Some(place) => place,
                None => return self.invalid("an address of a slot without storage"),
            },
            AddressBase::Value(value) => {
                let ty = self.value_type(value)?;
                let layout = ty.layout;
                let pointer = self.get(value)?;
                if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                    self.layout(layout)?.kind
                {
                    // An uncounted value (region storage) points at its payload whatever its
                    // layout's holders are.
                    if ty.repr == ValueRepr::Uncounted {
                        self.uncounted_payload(pointer, payload)?
                    } else {
                        self.payload(pointer, layout)?
                    }
                } else {
                    // A pointer to a value of its own layout (a `mut` capture's address).
                    Place {
                        ptr: pointer,
                        layout,
                        align: self.layout(layout)?.align,
                        block: None,
                    }
                }
            }
            AddressBase::Address(base) => {
                // Bases are prepared from scratch along with their checks; the validator
                // rejects address cycles.
                self.prepare(base, failure)?
            }
        };
        let mut steps = self.pool(info.projection, &self.unit.pools.projections)?;
        // A slot of an uncounted type (region storage) holds a pointer at its payload,
        // whatever its layout's holders are: a projection starts there, as one through an
        // uncounted value does.
        if let (AddressBase::Slot(slot), false) = (info.base, steps.is_empty()) {
            let ty = match self.unit.slots.get(slot.index()) {
                Some(info) => self.lir_type(info.ty)?,
                None => return self.invalid("no such slot"),
            };
            if let (
                ValueRepr::Uncounted,
                LayoutKind::Block { payload } | LayoutKind::Box { payload },
            ) = (ty.repr, self.layout(ty.layout)?.kind.clone())
            {
                let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                place = self.uncounted_payload(pointer, payload)?;
                if steps.first() == Some(&ProjectionStep::Payload) {
                    steps = &steps[1..];
                }
            }
        }
        let mut known = match info.base {
            AddressBase::Value(value) => self.preserved_lengths[value.index()],
            _ => None,
        };
        for &step in steps {
            self.known_count = known.take();
            place = self.project(place, step, failure)?;
        }
        self.known_count = None;
        if place.layout != self.lir_type(info.ty)?.layout {
            return self.invalid("an address whose projection does not reach its type");
        }
        Ok(place)
    }

    /// A value as its layout reads it: an uncounted reference (region storage) at a counted
    /// block layout is rebased to where that block's header would be, so code that adds the
    /// payload offset reaches its payload. Only the address is computed; no header is read.
    fn counted_view(&mut self, value: ValueId) -> Emitted<Value<'m>> {
        let ty = self.value_type(value)?;
        let v = self.get(value)?;
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
            self.layout(ty.layout)?.kind
        else {
            return Ok(v);
        };
        let counted = !self
            .layout(ty.layout)?
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if ty.repr != ValueRepr::Uncounted || !counted {
            return Ok(v);
        }
        let offset = match payload_offset(self.layout(payload)?.align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        let back = self
            .module
            .const_int(self.module.int_type(64), (offset as u128).wrapping_neg());
        Ok(self.builder.offset(v, back))
    }

    /// Regional flat storage includes an immortal header before its payload.
    /// Other uncounted storage has no block that a view can credit.
    fn uncounted_payload(&self, pointer: Value<'m>, payload: LayoutId) -> Emitted<Place<'m>> {
        let record = self.layout(payload)?;
        let block = if record.kind == LayoutKind::Flat {
            let offset = match payload_offset(record.align) {
                Ok(offset) => offset,
                Err(error) => return self.invalid(format!("{error:?}")),
            };
            let back = self
                .module
                .const_int(self.module.int_type(64), (offset as u128).wrapping_neg());
            Some(self.builder.offset(pointer, back))
        } else {
            None
        };
        Ok(Place {
            ptr: pointer,
            layout: payload,
            align: record.align,
            block,
        })
    }

    /// The payload a block reference value or field points at. A counted block's payload
    /// follows its header; an uncounted reference points at its payload directly.
    /// Regional flat payloads carry a real immortal header before that address.
    fn payload(&self, pointer: Value<'m>, layout: LayoutId) -> Emitted<Place<'m>> {
        let record = self.layout(layout)?;
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) = record.kind else {
            return self.invalid("an address through a value that is not a block reference");
        };
        let align = self.layout(payload)?.align;
        let uncounted = record
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if uncounted {
            return self.uncounted_payload(pointer, payload);
        }
        let offset = match payload_offset(align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        let ptr = self.builder.offset(pointer, self.const_u64(offset));
        Ok(Place {
            ptr,
            layout: payload,
            align,
            block: Some(pointer),
        })
    }

    fn project(
        &mut self,
        place: Place<'m>,
        step: ProjectionStep,
        failure: FailureId,
    ) -> Emitted<Place<'m>> {
        let state = self.sequence_state_at(place);
        let mut place = place;
        if step == ProjectionStep::Payload {
            if !matches!(self.layout(place.layout)?.kind, LayoutKind::Box { .. }) {
                return self.invalid("Payload requires a Box address");
            }
            let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            return self.payload(pointer, place.layout);
        }
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } = self.layout(place.layout)?.kind {
            let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            place = self.payload(pointer, place.layout)?;
        }
        let record = self.layout(place.layout)?;
        match step {
            ProjectionStep::Payload => unreachable!("handled before implicit dereference"),
            ProjectionStep::Field(ordinal) => {
                let Some(field) = record.fields.iter().find(|f| f.ordinal == ordinal) else {
                    return self.invalid("a field the layout does not have");
                };
                // A flat field lies at its cursor: further than its static offset by each
                // earlier variable field's u64 length prefix (LIR schema revision 3).
                if record.kind == LayoutKind::Flat {
                    let mut earlier: Vec<_> = record
                        .fields
                        .iter()
                        .filter(|f| f.offset < field.offset)
                        .collect();
                    earlier.sort_by_key(|f| f.offset);
                    let i64_ty = self.module.int_type(64);
                    let mut cursor = self.const_u64(0);
                    let mut moved = false;
                    for earlier in earlier {
                        if matches!(self.layout(earlier.layout)?.size, SizeSpec::Dynamic(_)) {
                            let at = self
                                .builder
                                .offset(place.ptr, self.const_u64(earlier.offset));
                            let at = self.builder.offset(at, cursor);
                            let prefix = self.builder.load(i64_ty, at, 1);
                            cursor = self.builder.binary(BinOp::Add, cursor, prefix);
                            moved = true;
                        }
                    }
                    if moved {
                        let base = self.builder.offset(place.ptr, cursor);
                        let ptr = self.builder.offset(base, self.const_u64(field.offset));
                        return Ok(Place {
                            ptr,
                            layout: field.layout,
                            align: 1,
                            block: place.block,
                        });
                    }
                }
                self.field_place(place, field.offset, field.layout, field.unaligned)
            }
            ProjectionStep::VariantField { variant, field } => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant field of an untagged layout");
                };
                let Some(fields) = tag.variants.iter().find(|v| v.ordinal == variant) else {
                    return self.invalid("a variant the layout does not have");
                };
                let Some(field) = fields.fields.iter().find(|f| f.ordinal == field) else {
                    return self.invalid("a field the variant does not have");
                };
                // MEM-13: the active tag must be the projected variant.
                let active = self.active_tag(place)?;
                let expected = self
                    .module
                    .const_int(self.module.int_type(32), u128::from(variant));
                let wrong = self.builder.icmp(IntPredicate::Ne, active, expected);
                self.guard(wrong, failure)?;
                self.field_place(place, field.offset, field.layout, field.unaligned)
            }
            ProjectionStep::Slot(index) => {
                // An element slot below the capacity, with no count check (LIR schema
                // revision 7): the library body proves the bound.
                let LayoutKind::Sequence { element } = record.kind else {
                    return self.invalid("an element slot of a layout that is no sequence");
                };
                let offset = match sequence_elements_offset(self.layout(element)?) {
                    Ok(offset) => offset,
                    Err(error) => return self.invalid(format!("{error:?}")),
                };
                let stride = self.fixed_size(element)?;
                let scaled =
                    self.builder
                        .binary(BinOp::Mul, self.get(index)?, self.const_u64(stride));
                let at = self
                    .builder
                    .binary(BinOp::Add, scaled, self.const_u64(offset));
                let align = place
                    .align
                    .min(low_bit(stride))
                    .min(low_bit(offset))
                    .min(self.layout(element)?.align);
                Ok(Place {
                    ptr: self.builder.offset(place.ptr, at),
                    layout: element,
                    align,
                    block: place.block,
                })
            }
            ProjectionStep::FlatAt { index, offset }
                if matches!(record.kind, LayoutKind::Sequential { .. }) =>
            {
                // Element `index` of a sequential Packed sequence (MEM-13) at its known
                // offset from the first element, checked like `Index` (MEM-14).
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!("matched above")
                };
                let count = self.load_u64(place.ptr);
                self.checked_index(index, count, failure)?;
                let first = self.sequential_first(place.ptr, element)?;
                self.flat_content = Some(first);
                Ok(Place {
                    ptr: self.builder.offset(first, self.get(offset)?),
                    layout: element,
                    align: self.layout(element)?.align.min(place.align),
                    block: place.block,
                })
            }
            ProjectionStep::FlatAt { index, offset } => {
                // Element `index` of a variable-element flat sequence at its known content
                // offset, checked like `Index` (MEM-14).
                let count = self.flat_count(place)?;
                self.checked_index(index, count, failure)?;
                let (element, variable) = self.flat_sequence(place.layout)?;
                if !variable {
                    return self.invalid("a flat walk of a sequence of fixed elements");
                }
                let content = self.builder.offset(place.ptr, self.const_u64(16));
                self.flat_content = Some(content);
                Ok(Place {
                    ptr: self.builder.offset(content, self.get(offset)?),
                    layout: element,
                    align: 1,
                    block: place.block,
                })
            }
            ProjectionStep::Index(index)
                if matches!(record.kind, LayoutKind::Sequential { .. }) =>
            {
                // Element i of a sequential Packed sequence (MEM-13): walked to from the
                // first, past each earlier element's active extent.
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!("matched above")
                };
                let count = self.load_u64(place.ptr);
                let index = self.checked_index(index, count, failure)?;
                let first = self.sequential_first(place.ptr, element)?;
                self.flat_content = Some(first);
                let ptr = self.sequential_walk(first, element, index)?;
                Ok(Place {
                    ptr,
                    layout: element,
                    align: self.layout(element)?.align.min(place.align),
                    block: place.block,
                })
            }
            ProjectionStep::Index(index) if record.kind == LayoutKind::Flat => {
                // Element i of a flat sequence encoding lies at its cursor (LIR schema
                // revision 4): fixed elements by stride past the prefix, variable ones walked
                // past each earlier element's prefixed encoding.
                let count = self.flat_count(place)?;
                let index = self.checked_index(index, count, failure)?;
                let (element, variable) = self.flat_sequence(place.layout)?;
                let ptr = if variable {
                    let content = self.builder.offset(place.ptr, self.const_u64(16));
                    self.flat_content = Some(content);
                    self.flat_walk(content, index)
                } else {
                    let stride = self.fixed_size(element)?;
                    let scaled = self
                        .builder
                        .binary(BinOp::Mul, index, self.const_u64(stride));
                    let at = self.builder.offset(place.ptr, self.const_u64(8));
                    self.builder.offset(at, scaled)
                };
                Ok(Place {
                    ptr,
                    layout: element,
                    align: 1,
                    block: place.block,
                })
            }
            ProjectionStep::Index(index) => {
                let (element, count, base, element_offset) = match record.kind {
                    LayoutKind::Inline { element, count } => {
                        (element, self.const_u64(count), place, 0)
                    }
                    LayoutKind::Simd { element, lanes } => {
                        (element, self.const_u64(u64::from(lanes)), place, 0)
                    }
                    LayoutKind::Slice { element } => {
                        // Immutable byte indexing addresses this exact interval.
                        // The bounds guard uses length before forming the byte GEP.
                        let word = |ordinal| {
                            record
                                .fields
                                .iter()
                                .find(|field| field.ordinal == ordinal)
                                .map(|field| {
                                    self.builder.offset(place.ptr, self.const_u64(field.offset))
                                })
                        };
                        let (Some(owner_at), Some(start_at), Some(length_at)) =
                            (word(0), word(1), word(2))
                        else {
                            return self.invalid("a slice without owner, start and length");
                        };
                        let owner =
                            self.builder
                                .load(self.module.ptr_type(), owner_at, place.align.min(8));
                        let start = self.builder.load(
                            self.module.int_type(64),
                            start_at,
                            place.align.min(8),
                        );
                        let count = self.builder.load(
                            self.module.int_type(64),
                            length_at,
                            place.align.min(8),
                        );
                        let Some(storage) = slice_storage(self.inputs.layouts, element) else {
                            return self.invalid("a slice without its byte sequence layout");
                        };
                        let payload = self.payload_of(owner, storage)?;
                        let bytes = self.builder.offset(payload.ptr, self.const_u64(8));
                        let base = Place {
                            ptr: self.builder.offset(bytes, start),
                            layout: storage,
                            align: 1,
                            block: Some(owner),
                        };
                        (element, count, base, 0)
                    }
                    LayoutKind::Sequence { element } => {
                        let count_at = self.field_place(place, 0, place.layout, false)?;
                        // A source element address cannot reach the length word:
                        // it starts after the aligned sequence header. Scalar writes
                        // may therefore not clobber this load (MEM-14).
                        let count = match (state, self.known_count.take()) {
                            (Some(state), _) => {
                                self.builder.load(self.module.int_type(64), state.length, 8)
                            }
                            (None, Some(length)) => length,
                            (None, None) => {
                                self.flush_viewed(place.layout)?;
                                self.builder.load_access(
                                    self.module.int_type(64),
                                    count_at.ptr,
                                    8.min(place.align),
                                    Access::Length,
                                )
                            }
                        };
                        let offset = match sequence_elements_offset(self.layout(element)?) {
                            Ok(offset) => offset,
                            Err(error) => return self.invalid(format!("{error:?}")),
                        };
                        (element, count, place, offset)
                    }
                    _ => return self.unsupported("an index into this layout kind"),
                };
                let stride = self.fixed_size(element)?;
                let index64 = self.checked_index(index, count, failure)?;
                let scaled = self
                    .builder
                    .binary(BinOp::Mul, index64, self.const_u64(stride));
                let offset =
                    self.builder
                        .binary(BinOp::Add, scaled, self.const_u64(element_offset));
                let align = base
                    .align
                    .min(low_bit(stride))
                    .min(low_bit(element_offset))
                    .min(self.layout(element)?.align);
                Ok(Place {
                    ptr: self.builder.offset(base.ptr, offset),
                    layout: element,
                    align,
                    block: base.block,
                })
            }
        }
    }

    /// The integer `index` as an `i64` below `count`; a negative, wider or outside index
    /// takes `failure` (MEM-14).
    fn checked_index(
        &mut self,
        index: ValueId,
        count: Value<'m>,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        let index_kind = self.scalar_kind(index)?;
        let Some((width, signed)) = scalar::integer(index_kind) else {
            return self.invalid("an index that is not an integer");
        };
        let index = self.get(index)?;
        let i64_ty = self.module.int_type(64);
        let index_ty = self.module.int_type(width);
        let zero = self.module.const_int(index_ty, 0);
        let mut failed = self.module.const_int(self.module.int_type(1), 0);
        if signed {
            let negative = self.builder.icmp(IntPredicate::Slt, index, zero);
            failed = self.builder.binary(BinOp::Or, failed, negative);
        }
        let index64 = match width.cmp(&64) {
            std::cmp::Ordering::Less => self.builder.cast(Cast::ZExt, index, i64_ty),
            std::cmp::Ordering::Equal => index,
            std::cmp::Ordering::Greater => {
                let high =
                    self.builder
                        .binary(BinOp::LShr, index, self.module.const_int(index_ty, 64));
                let wide = self.builder.icmp(IntPredicate::Ne, high, zero);
                failed = self.builder.binary(BinOp::Or, failed, wide);
                self.builder.cast(Cast::Trunc, index, i64_ty)
            }
        };
        // MEM-14: the index must be below the count.
        let outside = self.builder.icmp(IntPredicate::Uge, index64, count);
        failed = self.builder.binary(BinOp::Or, failed, outside);
        self.guard(failed, failure)?;
        Ok(index64)
    }

    // Sequence blocks (LIR schema revision 7).

    /// The payload a block reference layout points at.
    fn layout_payload(&self, layout: LayoutId) -> Emitted<Option<LayoutId>> {
        Ok(match self.layout(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Some(payload),
            _ => None,
        })
    }

    /// The element layout of a counted sequence block layout.
    fn sequence_element(&self, layout: LayoutId) -> Emitted<Option<LayoutId>> {
        let Some(payload) = self.layout_payload(layout)? else {
            return Ok(None);
        };
        Ok(match self.layout(payload)?.kind {
            LayoutKind::Sequence { element } => Some(element),
            _ => None,
        })
    }

    /// The payload address of a sequence block value and its element layout.
    fn sequence_payload(&mut self, value: ValueId) -> Emitted<(Value<'m>, LayoutId)> {
        let layout = self.value_type(value)?.layout;
        let (Some(payload), Some(element)) =
            (self.layout_payload(layout)?, self.sequence_element(layout)?)
        else {
            return self.invalid("a sequence operand that is no sequence block");
        };
        let pointer = self.counted_view(value)?;
        Ok((self.payload_of(pointer, payload)?.ptr, element))
    }

    /// Elements the block's storage holds: its capacity word (runtime ABI 5), one load.
    /// The word is never more than the storage's slots, and never less than the count.
    /// Zero-sized elements report `u64::MAX`.
    fn sequence_capacity(&mut self, value: ValueId) -> Emitted<Value<'m>> {
        let (data, element) = self.sequence_payload(value)?;
        if self.fixed_size(element)? == 0 {
            return Ok(self.const_u64(u64::MAX));
        }
        let at = self
            .builder
            .offset(data, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
        Ok(self
            .builder
            .load_access(self.module.int_type(64), at, 8, Access::Capacity))
    }

    // FlatFormatV0 (LIR schema revision 4).

    /// The u64 at `at`, which flat values store unaligned.
    fn load_u64(&self, at: Value<'m>) -> Value<'m> {
        self.builder.load(self.module.int_type(64), at, 1)
    }

    /// The element of a flat sequence encoding (List, Array, String, Bytes) and whether its
    /// elements are variable (each its own prefixed encoding, after the prefix and count).
    fn flat_sequence(&self, layout: LayoutId) -> Emitted<(LayoutId, bool)> {
        let record = self.layout(layout)?;
        match (&record.kind, &record.size, record.fields.as_slice()) {
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only]) if only.offset == 16 => {
                Ok((only.layout, true))
            }
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only])
                if only.offset == 8
                    && matches!(self.layout(only.layout)?.size, SizeSpec::Fixed(_)) =>
            {
                Ok((only.layout, false))
            }
            _ => self.invalid("a flat part that is no sequence"),
        }
    }

    /// The element count of the flat sequence encoding at `place`: stored after the prefix
    /// for variable elements, else the prefix's byte length over the element size.
    fn flat_count(&self, place: Place<'m>) -> Emitted<Value<'m>> {
        let (element, variable) = self.flat_sequence(place.layout)?;
        if variable {
            return Ok(self.load_u64(self.builder.offset(place.ptr, self.const_u64(8))));
        }
        match self.fixed_size(element)? {
            0 => self.unsupported("a flat sequence of empty elements"),
            size => {
                let bytes = self.load_u64(place.ptr);
                Ok(self
                    .builder
                    .binary(BinOp::UDiv, bytes, self.const_u64(size)))
            }
        }
    }

    /// The address of the `index`th prefixed encoding from `first`, each `8 + prefix` bytes.
    fn flat_walk(&self, first: Value<'m>, index: Value<'m>) -> Value<'m> {
        let (i64_ty, ptr_ty) = (self.module.int_type(64), self.module.ptr_type());
        let entry = self.builder.current();
        let (head, body, exit) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.br(head);
        self.builder.position(head);
        let k = self.builder.phi(i64_ty);
        let cursor = self.builder.phi(ptr_ty);
        let more = self.builder.icmp(IntPredicate::Ult, k, index);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let length = self.load_u64(cursor);
        let past = self.builder.offset(cursor, self.const_u64(8));
        let next = self.builder.offset(past, length);
        let k1 = self.builder.binary(BinOp::Add, k, self.const_u64(1));
        self.builder.br(head);
        self.builder.add_incoming(k, self.const_u64(0), entry);
        self.builder.add_incoming(k, k1, body);
        self.builder.add_incoming(cursor, first, entry);
        self.builder.add_incoming(cursor, next, body);
        self.builder.position(exit);
        cursor
    }

    /// The payload offset of a sequential sequence's first element (MEM-13): past its
    /// count and used-bytes words, aligned to the element.
    fn sequential_offset(&self, element: LayoutId) -> Emitted<u64> {
        match sequential_elements_offset(self.layout(element)?) {
            Ok(offset) => Ok(offset),
            Err(_) => self.invalid("a sequential element offset beyond u64"),
        }
    }

    fn sequential_first(&self, payload: Value<'m>, element: LayoutId) -> Emitted<Value<'m>> {
        let offset = self.sequential_offset(element)?;
        Ok(self.builder.offset(payload, self.const_u64(offset)))
    }

    /// The element `index` elements past `first` in a sequential sequence: each step
    /// passes one element's active extent.
    fn sequential_walk(
        &self,
        first: Value<'m>,
        element: LayoutId,
        index: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let (i64_ty, ptr_ty) = (self.module.int_type(64), self.module.ptr_type());
        let entry = self.builder.current();
        let (head, body, exit) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.br(head);
        self.builder.position(head);
        let k = self.builder.phi(i64_ty);
        let cursor = self.builder.phi(ptr_ty);
        let more = self.builder.icmp(IntPredicate::Ult, k, index);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let bytes = self.extent(element, cursor)?;
        let next = self.builder.offset(cursor, bytes);
        let k1 = self.builder.binary(BinOp::Add, k, self.const_u64(1));
        let latch = self.builder.current();
        self.builder.br(head);
        self.builder.add_incoming(k, self.const_u64(0), entry);
        self.builder.add_incoming(k, k1, latch);
        self.builder.add_incoming(cursor, first, entry);
        self.builder.add_incoming(cursor, next, latch);
        self.builder.position(exit);
        Ok(cursor)
    }

    /// Stores the dynamic Packed `value` at `at` within its active extent, which it
    /// returns: a sequential element holds no bytes past it (MEM-13).
    fn store_packed(&self, value: ValueId, element: LayoutId, at: Value<'m>) -> Emitted<Value<'m>> {
        let temp = self.stack(element)?;
        self.store(self.value_shape(value)?, self.get(value)?, temp);
        let bytes = self.extent(element, temp.ptr)?;
        self.builder
            .memcpy(at, temp.align, temp.ptr, temp.align, bytes);
        Ok(bytes)
    }

    /// `load`, except that a dynamic Packed value reads only its active extent: as an
    /// element of a sequential sequence it may end its storage (MEM-13).
    fn load_bounded(&self, shape: Shape, place: Place<'m>) -> Emitted<Value<'m>> {
        if hirsel_lir::layout::packed_extents(self.layout(place.layout)?).is_none() {
            return Ok(self.load(shape, place));
        }
        let temp = self.stack(place.layout)?;
        let bytes = self.extent(place.layout, place.ptr)?;
        self.builder
            .memcpy(temp.ptr, temp.align, place.ptr, place.align, bytes);
        Ok(self.load(shape, temp))
    }

    /// The byte extent of the value of `layout` at `at`: its fixed size, or its size
    /// program run over the stored bytes, as `hirsel_lir::layout::extent` reads it.
    fn extent(&self, layout: LayoutId, at: Value<'m>) -> Emitted<Value<'m>> {
        match &self.layout(layout)?.size {
            SizeSpec::Fixed(size) => Ok(self.const_u64(*size)),
            SizeSpec::Dynamic(program) => self.size_program(program, at),
        }
    }

    fn size_program(&self, program: &[SizeTerm], at: Value<'m>) -> Emitted<Value<'m>> {
        let i64_ty = self.module.int_type(64);
        let mut stack: Vec<Value<'m>> = Vec::new();
        let pop = |stack: &mut Vec<Value<'m>>| match stack.pop() {
            Some(v) => Ok(v),
            None => self.invalid("a size program that underflows"),
        };
        for term in program {
            let value = match term {
                SizeTerm::Constant(n) => self.const_u64(*n),
                SizeTerm::ReadLength { offset, scalar } => {
                    let Some((width, false)) = scalar::integer(*scalar) else {
                        return self.invalid("a length that is not an unsigned integer");
                    };
                    if width > 64 {
                        return self.unsupported("a length wider than u64");
                    }
                    let length = self.builder.load(
                        self.module.int_type(width),
                        self.builder.offset(at, self.const_u64(*offset)),
                        1,
                    );
                    if width < 64 {
                        self.builder.cast(Cast::ZExt, length, i64_ty)
                    } else {
                        length
                    }
                }
                SizeTerm::Add | SizeTerm::Multiply => {
                    let (b, a) = (pop(&mut stack)?, pop(&mut stack)?);
                    let op = match term {
                        SizeTerm::Add => BinOp::Add,
                        _ => BinOp::Mul,
                    };
                    self.builder.binary(op, a, b)
                }
                SizeTerm::AlignUp(n) => {
                    if !n.is_power_of_two() {
                        return self.invalid("a size alignment that is not a power of two");
                    }
                    let a = pop(&mut stack)?;
                    let bumped = self.builder.binary(BinOp::Add, a, self.const_u64(n - 1));
                    self.builder
                        .binary(BinOp::And, bumped, self.const_u64(!(n - 1)))
                }
                SizeTerm::Dup => match stack.last() {
                    Some(&top) => top,
                    None => return self.invalid("a size program that underflows"),
                },
                SizeTerm::At(inner) => {
                    let offset = pop(&mut stack)?;
                    self.size_program(inner, self.builder.offset(at, offset))?
                }
                SizeTerm::ByTag {
                    tag_offset,
                    variants,
                } => {
                    let tag = self.builder.load(
                        self.module.int_type(32),
                        self.builder.offset(at, self.const_u64(*tag_offset)),
                        1,
                    );
                    let done = self.module.append_block(self.function);
                    let bad = self.module.append_block(self.function);
                    let here = self.builder.current();
                    let mut cases = Vec::new();
                    let mut incoming = Vec::new();
                    for (variant, inner) in variants {
                        let arm = self.module.append_block(self.function);
                        self.builder.position(arm);
                        let size = self.size_program(inner, at)?;
                        incoming.push((size, self.builder.current()));
                        self.builder.br(done);
                        cases.push((
                            self.module
                                .const_int(self.module.int_type(32), u128::from(*variant)),
                            arm,
                        ));
                    }
                    // A tag without a size is an invariant failure, never a guess.
                    self.builder.position(bad);
                    self.builder.unreachable();
                    self.builder.position(here);
                    self.builder.switch(tag, bad, &cases);
                    self.builder.position(done);
                    let size = self.builder.phi(i64_ty);
                    for (value, from) in incoming {
                        self.builder.add_incoming(size, value, from);
                    }
                    size
                }
            };
            stack.push(value);
        }
        match stack.as_slice() {
            [size] => Ok(*size),
            _ => self.invalid("a size program that leaves no single size"),
        }
    }

    /// The bytes a `Slice { u8 }` value shows: its owner's sequence payload from `8 + start`,
    /// and its length.
    fn slice_bytes(&self, slice: ValueId) -> Emitted<(Value<'m>, Value<'m>)> {
        let layout = self.value_type(slice)?.layout;
        let record = self.layout(layout)?;
        let LayoutKind::Slice { element } = record.kind else {
            return self.invalid("an encoded byte source that is not a slice");
        };
        let place = self.stack(layout)?;
        self.store(self.value_shape(slice)?, self.get(slice)?, place);
        let word = |ordinal: u32| match record.fields.iter().find(|f| f.ordinal == ordinal) {
            Some(field) => Ok(self.builder.offset(place.ptr, self.const_u64(field.offset))),
            None => self.invalid("a slice layout without its words"),
        };
        let owner = self.builder.load(self.module.ptr_type(), word(0)?, 8);
        let (start, length) = (self.load_u64(word(1)?), self.load_u64(word(2)?));
        let Some(storage) = slice_storage(self.inputs.layouts, element) else {
            return self.invalid("a slice without its element's sequence layout");
        };
        let payload = self.payload_of(owner, storage)?;
        let data = self.builder.offset(payload.ptr, self.const_u64(8));
        Ok((self.builder.offset(data, start), length))
    }

    /// The place of a sequential sequence block an append writes, its payload
    /// and element layouts.
    fn sequential_place(&self, address: AddressId) -> Emitted<(Place<'m>, LayoutId, LayoutId)> {
        let place = self.place(address)?;
        let Some(payload) = self.layout_payload(place.layout)? else {
            return self.invalid("an append to a place that holds no block");
        };
        let LayoutKind::Sequential { element } = self.layout(payload)?.kind else {
            return self.invalid("an append to a block that is not sequential");
        };
        Ok((place, payload, element))
    }

    /// Room for `bytes` more past the used bytes of the sequential block at
    /// `place`, growing the storage geometrically when it is full (MEM-13): the
    /// payload address and the used bytes before the append.
    fn sequential_room(
        &mut self,
        place: Place<'m>,
        payload: LayoutId,
        first: u64,
        bytes: Value<'m>,
    ) -> Emitted<(Value<'m>, Value<'m>)> {
        let (old, _, used) = self.sequential_used(place, payload)?;
        let keep = self.builder.binary(BinOp::Add, used, self.const_u64(first));
        let need = self.builder.binary(BinOp::Add, keep, bytes);
        let data = self.sequential_grow(place, payload, old, keep, need)?;
        Ok((data, used))
    }

    /// The block the sequential place holds, its payload address and its used bytes.
    fn sequential_used(
        &self,
        place: Place<'m>,
        payload: LayoutId,
    ) -> Emitted<(Value<'m>, Value<'m>, Value<'m>)> {
        let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
        let data = self.payload_of(old, payload)?.ptr;
        let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
        Ok((old, data, used))
    }

    /// Room for `need` bytes from the payload's start of the sequential block
    /// `old` at `place`, growing it geometrically when it is full and keeping
    /// its first `keep` bytes: the payload address.
    fn sequential_grow(
        &mut self,
        place: Place<'m>,
        payload: LayoutId,
        old: Value<'m>,
        keep: Value<'m>,
        need: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let align = u64::from(self.layout(payload)?.align);
        let ptr_ty = self.module.ptr_type();
        let usable = crate::runtime::capacity(&self.builder, self.function, old);
        let full = self.builder.icmp(IntPredicate::Ugt, need, usable);
        let here = self.builder.current();
        let (grow, done) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cond_br(full, grow, done);
        self.builder.position(grow);
        let twice = self.builder.binary(BinOp::Add, usable, usable);
        let more = self.builder.icmp(IntPredicate::Ugt, need, twice);
        let wanted = self.builder.select(more, need, twice);
        let entry = self.runtime(RuntimeEntry::BlockGrow)?;
        let grown = self.builder.call(
            entry,
            &[self.context, old, keep, wanted, self.const_u64(align)],
        );
        let grew = self.builder.current();
        self.builder.br(done);
        self.builder.position(done);
        let block = self.builder.phi(ptr_ty);
        self.builder.add_incoming(block, old, here);
        self.builder.add_incoming(block, grown, grew);
        self.store(Shape::Scalar(ScalarKind::Pointer), block, place);
        Ok(self.payload_of(block, payload)?.ptr)
    }

    /// Where the length word of the open element's field `field` lies in a
    /// builder's element of layout `element` (LIR schema revision 24): past
    /// the prefix and the fixed fields before it.
    fn open_word(&self, element: LayoutId, field: u32) -> Emitted<u64> {
        let record = self.layout(element)?;
        let content = match record.fields.as_slice() {
            [only] if record.kind == LayoutKind::Flat && only.offset == 8 => only.layout,
            _ => return self.invalid("an open element of an unprefixed builder"),
        };
        let content = self.layout(content)?;
        let Some(at) = content.fields.iter().find(|f| f.ordinal == field) else {
            return self.invalid("an open element of a missing field");
        };
        for earlier in content.fields.iter().filter(|f| f.ordinal < field) {
            if !matches!(self.layout(earlier.layout)?.size, SizeSpec::Fixed(_)) {
                return self.invalid("an open field after a variable one");
            }
        }
        Ok(8 + at.offset)
    }

    /// One more element of `bytes` in the sequential payload at `data`.
    fn sequential_grew(&self, data: Value<'m>, used: Value<'m>, bytes: Value<'m>) {
        let count = self.load_u64(data);
        let count = self.builder.binary(BinOp::Add, count, self.const_u64(1));
        self.builder.store(count, data, 8);
        let used = self.builder.binary(BinOp::Add, used, bytes);
        self.builder
            .store(used, self.builder.offset(data, self.const_u64(8)), 8);
    }

    /// The FlatFormatV0 parts of a flat construction in field order: a fixed field's value,
    /// or an encoded source's bytes (a String or Bytes slice, or a variable flat record's
    /// block payload) behind its u64 length prefix. Encoded sources are only read.
    fn flat_parts(&mut self, site: &'a ConstructSite) -> Emitted<Vec<FlatPart<'m>>> {
        self.flat_parts_around(site, None)
    }

    /// `flat_parts` of a construction completing an open element: `open` is
    /// the field built in place and its length (LIR schema revision 24).
    fn flat_parts_around(
        &mut self,
        site: &'a ConstructSite,
        open: Option<(u32, Value<'m>)>,
    ) -> Emitted<Vec<FlatPart<'m>>> {
        let record = self.layout(site.layout)?;
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let mut declared: Vec<&FieldLayout> = record.fields.iter().collect();
        declared.sort_by_key(|f| f.ordinal);
        if declared.len() != values.len() + usize::from(open.is_some()) {
            return self.invalid("a flat construction without a value for every field");
        }
        let mut parts = Vec::new();
        for field in declared {
            if let Some((ordinal, length)) = open
                && field.ordinal == ordinal
            {
                parts.push(FlatPart::InPlace { length });
                continue;
            }
            let Some(value) = values.iter().find(|v| v.field == field.ordinal) else {
                return self.invalid("a flat construction without a value for its field");
            };
            let source = self.value_type(value.value)?.layout;
            let field_record = self.layout(field.layout)?;
            if let SizeSpec::Fixed(size) = field_record.size {
                parts.push(FlatPart::Fixed {
                    value: value.value,
                    size,
                });
                continue;
            }
            if let LayoutKind::Block { payload } = self.layout(source)?.kind
                && let LayoutKind::Sequential { element } = self.layout(payload)?.kind
            {
                let data = self.payload_of(self.get(value.value)?, payload)?.ptr;
                let first = self.sequential_offset(element)?;
                parts.push(FlatPart::Built { data, first });
                continue;
            }
            if let LayoutKind::Block { payload } = self.layout(source)?.kind
                && matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. })
            {
                let source = if self.value_type(value.value)?.repr == ValueRepr::Uncounted {
                    self.uncounted_payload(self.get(value.value)?, payload)?
                } else {
                    self.payload_of(self.get(value.value)?, payload)?
                };
                parts.push(FlatPart::Sequence {
                    source,
                    encoding: field.layout,
                });
                continue;
            }
            let content = match field_record.fields.as_slice() {
                [only] if field_record.kind == LayoutKind::Flat && only.offset == 8 => only.layout,
                _ => return self.unsupported("encoding a list into a flat field"),
            };
            let (bytes, length) = match self.layout(source)?.kind {
                LayoutKind::Slice { .. } => self.slice_bytes(value.value)?,
                LayoutKind::Block { payload } | LayoutKind::Box { payload }
                    if payload == content =>
                {
                    let at = self.payload_of(self.get(value.value)?, payload)?;
                    (at.ptr, self.extent(payload, at.ptr)?)
                }
                _ => return self.invalid("a flat field source that has no encoding"),
            };
            parts.push(FlatPart::Encoded { bytes, length });
        }
        Ok(parts)
    }

    /// The byte extent of `parts` once written.
    fn flat_extent(&self, parts: &[FlatPart<'m>]) -> Emitted<Value<'m>> {
        let mut total = self.const_u64(0);
        for part in parts {
            let size = match *part {
                FlatPart::Fixed { size, .. } => self.const_u64(size),
                FlatPart::Encoded { length, .. } => self.flat_add(length, self.const_u64(8))?,
                FlatPart::Sequence { source, encoding } => {
                    self.flat_encode_sequence(source, encoding, None)?
                }
                FlatPart::Built { data, .. } => {
                    let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
                    self.flat_add(used, self.const_u64(16))?
                }
                FlatPart::InPlace { length } => self.flat_add(length, self.const_u64(8))?,
            };
            total = self.flat_add(total, size)?;
        }
        Ok(total)
    }

    /// Writes `parts` one after another from `at`.
    fn write_flat(&self, at: Value<'m>, parts: &[FlatPart<'m>]) -> Emitted {
        let mut cursor = at;
        for part in parts {
            match *part {
                FlatPart::Fixed { value, size } => {
                    let layout = self.value_type(value)?.layout;
                    let place = Place {
                        ptr: cursor,
                        layout,
                        align: 1,
                        block: None,
                    };
                    self.store(self.value_shape(value)?, self.get(value)?, place);
                    cursor = self.builder.offset(cursor, self.const_u64(size));
                }
                FlatPart::Encoded { bytes, length } => {
                    self.builder.store(length, cursor, 1);
                    let data = self.builder.offset(cursor, self.const_u64(8));
                    self.builder.memcpy(data, 1, bytes, 1, length);
                    cursor = self.builder.offset(data, length);
                }
                FlatPart::Sequence { source, encoding } => {
                    let size = self.flat_encode_sequence(source, encoding, Some(cursor))?;
                    cursor = self.builder.offset(cursor, size);
                }
                FlatPart::Built { data, first } => {
                    let count = self.load_u64(data);
                    let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
                    let length = self.builder.binary(BinOp::Add, used, self.const_u64(8));
                    self.builder.store(length, cursor, 1);
                    self.builder
                        .store(count, self.builder.offset(cursor, self.const_u64(8)), 1);
                    let to = self.builder.offset(cursor, self.const_u64(16));
                    let from = self.builder.offset(data, self.const_u64(first));
                    self.builder.memcpy(to, 1, from, 1, used);
                    cursor = self.builder.offset(to, used);
                }
                FlatPart::InPlace { length } => {
                    let data = self.builder.offset(cursor, self.const_u64(8));
                    cursor = self.builder.offset(data, length);
                }
            }
        }
        Ok(())
    }

    /// Size overflow is an implementation invariant fault, before allocating or
    /// writing an extent. It must not wrap into an undersized allocation.
    fn flat_add(&self, left: Value<'m>, right: Value<'m>) -> Emitted<Value<'m>> {
        let (sum, overflow) = scalar::int_arith(
            &self.builder,
            BinaryOp::Add,
            ArithMode::Checked,
            ScalarKind::U64,
            left,
            right,
        )
        .or_else(|e| self.invalid(e))?;
        let bad = self.module.append_block(self.function);
        let good = self.module.append_block(self.function);
        self.builder
            .cond_br(overflow.expect("checked add flag"), bad, good);
        self.builder.position(bad);
        let trap = self
            .module
            .intrinsic("llvm.trap", &[])
            .or_else(|e| self.invalid(e))?;
        self.builder.call(trap, &[]);
        self.builder.unreachable();
        self.builder.position(good);
        Ok(sum)
    }

    /// Size and optionally write an ordinary sequence at its flat encoding.
    /// Both walks read the same borrowed, stable source; no element credit moves.
    fn flat_encode_sequence(
        &self,
        source: Place<'m>,
        encoding: LayoutId,
        output: Option<Value<'m>>,
    ) -> Emitted<Value<'m>> {
        let LayoutKind::Sequence { element } = self.layout(source.layout)?.kind else {
            return self.invalid("a flat sequence source without sequence storage");
        };
        let (encoded_element, variable) = self.flat_sequence(encoding)?;
        let count = self.load_u64(source.ptr);
        let stride = self.fixed_size(element)?;
        let start = sequence_elements_offset(self.layout(element)?)
            .or_else(|e| self.invalid(format!("{e:?}")))?;
        let entry = self.builder.current();
        let head = self.module.append_block(self.function);
        let body = self.module.append_block(self.function);
        let exit = self.module.append_block(self.function);
        self.builder.br(head);
        self.builder.position(head);
        let index = self.builder.phi(self.module.int_type(64));
        let total = self.builder.phi(self.module.int_type(64));
        let more = self.builder.icmp(IntPredicate::Ult, index, count);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let offset = self
            .builder
            .binary(BinOp::Mul, index, self.const_u64(stride));
        let base = self.builder.offset(source.ptr, self.const_u64(start));
        let child = Place {
            ptr: self.builder.offset(base, offset),
            layout: element,
            align: self.layout(element)?.align,
            block: source.block,
        };
        let at = output.map(|p| self.builder.offset(p, total));
        let size = self.flat_encode_element(child, encoded_element, at)?;
        let next_total = self.flat_add(total, size)?;
        let next_index = self.builder.binary(BinOp::Add, index, self.const_u64(1));
        let back = self.builder.current();
        self.builder.br(head);
        self.builder.add_incoming(index, self.const_u64(0), entry);
        self.builder.add_incoming(index, next_index, back);
        self.builder
            .add_incoming(total, self.const_u64(if variable { 16 } else { 8 }), entry);
        self.builder.add_incoming(total, next_total, back);
        self.builder.position(exit);
        if let Some(output) = output {
            let length = self.builder.binary(BinOp::Sub, total, self.const_u64(8));
            self.builder.store(length, output, 1);
            if variable {
                self.builder
                    .store(count, self.builder.offset(output, self.const_u64(8)), 1);
            }
        }
        Ok(total)
    }

    /// Encode one source element at the layout C has independently checked.
    fn flat_encode_element(
        &self,
        source: Place<'m>,
        encoding: LayoutId,
        output: Option<Value<'m>>,
    ) -> Emitted<Value<'m>> {
        if let SizeSpec::Fixed(size) = self.layout(encoding)?.size {
            if let Some(output) = output {
                self.builder
                    .memcpy(output, 1, source.ptr, source.align, self.const_u64(size));
            }
            return Ok(self.const_u64(size));
        }
        let [content] = self.layout(encoding)?.fields.as_slice() else {
            return self.invalid("a flat element encoding without its content");
        };
        let (bytes, length) = match self.layout(source.layout)?.kind {
            LayoutKind::Block { payload } => {
                let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), source);
                let payload_place = self.payload(pointer, source.layout)?;
                if matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. }) {
                    return self.flat_encode_sequence(payload_place, encoding, output);
                }
                if content.offset != 8 || payload != content.layout {
                    return self.invalid("a flat element source of another payload layout");
                }
                (payload_place.ptr, self.extent(payload, payload_place.ptr)?)
            }
            LayoutKind::Slice { element } => {
                let record = self.layout(source.layout)?;
                let word = |ordinal| {
                    record
                        .fields
                        .iter()
                        .find(|f| f.ordinal == ordinal)
                        .map(|f| self.builder.offset(source.ptr, self.const_u64(f.offset)))
                        .ok_or_else(|| CodegenError::Invalid {
                            definition: self.definition().clone(),
                            what: "a flat slice source without its words".into(),
                        })
                };
                let owner = self
                    .builder
                    .load(self.module.ptr_type(), word(0)?, source.align);
                let start = self.load_u64(word(1)?);
                let length = self.load_u64(word(2)?);
                let storage = slice_storage(self.inputs.layouts, element).ok_or_else(|| {
                    CodegenError::Invalid {
                        definition: self.definition().clone(),
                        what: "a flat slice source without byte storage".into(),
                    }
                })?;
                let payload = self.payload_of(owner, storage)?;
                let data = self.builder.offset(payload.ptr, self.const_u64(8));
                (self.builder.offset(data, start), length)
            }
            _ => return self.invalid("a flat element source without an encoding"),
        };
        let size = self.flat_add(length, self.const_u64(8))?;
        if let Some(output) = output {
            self.builder.store(length, output, 1);
            let data = self.builder.offset(output, self.const_u64(8));
            self.builder.memcpy(data, 1, bytes, 1, length);
        }
        Ok(size)
    }

    /// The active variant ordinal (an `i32`) of the tagged value at `place`.
    fn active_tag(&self, place: Place<'m>) -> Emitted<Value<'m>> {
        let record = self.layout(place.layout)?;
        let Some(tag) = &record.tag else {
            return self.invalid("a tag read of an untagged layout");
        };
        Ok(self.tag_at(place, tag.offset, tag.form, record.abi.unaligned))
    }

    /// The active variant ordinal (an `i32`) of a variant whose tag lies `offset` bytes into
    /// `place` in `form`: the only tag reader, so the tag encoding has one place to change.
    fn tag_at(&self, place: Place<'m>, offset: u64, form: TagForm, unaligned: bool) -> Value<'m> {
        load_tag(
            &self.builder,
            place.ptr,
            place.align,
            offset,
            form,
            unaligned,
        )
    }

    /// Makes `variant` the active variant of the tagged storage at `place`; the only tag
    /// writer.
    fn store_tag(&self, place: Place<'m>, variant: u32) -> Emitted {
        let record = self.layout(place.layout)?;
        let Some(tag) = &record.tag else {
            return self.invalid("a tag write of an untagged layout");
        };
        let at = self.field_place(place, tag.offset, place.layout, record.abi.unaligned)?;
        match tag.form {
            TagForm::Ordinal => {
                let i32_ty = self.module.int_type(32);
                self.builder.store(
                    self.module.const_int(i32_ty, u128::from(variant)),
                    at.ptr,
                    at.align.min(4),
                );
            }
            // A null-pointer niche: the fieldless variant's word is null; the dataful
            // variant's block reference field fills it.
            TagForm::Niche { nullary, .. } if variant == nullary => {
                let ptr_ty = self.module.ptr_type();
                self.builder
                    .store(self.module.const_zero(ptr_ty), at.ptr, at.align.min(8));
            }
            TagForm::LiveBit { .. } => return self.invalid("a live word is not a variant tag"),
            TagForm::Niche { .. } => {}
        }
        Ok(())
    }

    /// An inline construction: zeroed storage (so padding and inactive payload bytes are
    /// defined), then each field at its offset, read back as the value's bytes.
    fn construct_site(&self, construct: ConstructId) -> Emitted<&'a ConstructSite> {
        match self.unit.tables.constructs.get(construct.index()) {
            Some(site) => Ok(site),
            None => self.invalid("a construction outside the table"),
        }
    }

    fn construct(
        &mut self,
        result: ValueId,
        construct: ConstructId,
        failure: FailureId,
    ) -> Emitted {
        let site = self.construct_site(construct)?;
        let ty = self.value_type(result)?;
        if matches!(ty.repr, ValueRepr::Block | ValueRepr::Uncounted) {
            if ty.repr == ValueRepr::Block
                && let Some((builder, released)) = self.dying_builder(site)?
            {
                return self.adopt_builder(result, site, builder, released, failure);
            }
            return self.construct_block(result, site, None, failure);
        }
        if ty.layout != site.layout || site.constructor == ConstructorKind::Box {
            return self.invalid("an inline construction of another layout");
        }
        let shape = self.shape(ty)?;
        let place = self.stack(site.layout)?;
        if self.layout(site.layout)?.kind == LayoutKind::Flat {
            // A fixed flat record: its fields' bytes, which fill it exactly.
            let parts = self.flat_parts(site)?;
            self.write_flat(place.ptr, &parts)?;
            let value = self.load(shape, place);
            return self.define(result, value);
        }
        self.builder
            .memset(place.ptr, 0, self.fixed_size(site.layout)?, place.align);
        self.fill(place, site, &[])?;
        let value = self.load(shape, place);
        self.define(result, value)
    }

    /// A heap flat record whose content is exactly one list field built by a flat builder
    /// (LIR schema revision 22) that this block borrows for the construction and then
    /// takes and releases with nothing between: the builder and its released value.
    /// The record's encoding is the builder's payload with its two header words
    /// rewritten, so a unique builder's block can become the record's.
    fn dying_builder(&self, site: &ConstructSite) -> Emitted<Option<(ValueId, ValueId)>> {
        let record = self.layout(site.layout)?;
        if !matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::Heap(_)))
        ) || site.constructor == ConstructorKind::Box
            || record.kind != LayoutKind::Flat
        {
            return Ok(None);
        }
        let [field] = record.fields.as_slice() else {
            return Ok(None);
        };
        let list = self.layout(field.layout)?;
        let encoded = matches!(
            (&list.kind, &list.size, list.fields.as_slice()),
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [content]) if content.offset == 16
        );
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let ([value], 0, true) = (values, field.offset, encoded) else {
            return Ok(None);
        };
        let LayoutKind::Block { payload } = self.layout(self.value_type(value.value)?.layout)?.kind
        else {
            return Ok(None);
        };
        let LayoutKind::Sequential { element } = self.layout(payload)?.kind else {
            return Ok(None);
        };
        if self.sequential_offset(element)? != 16 {
            return Ok(None);
        }
        let (b, i) = self.at;
        let code = self.pool(
            self.unit.blocks[b.index()].instructions,
            &self.unit.instructions,
        )?;
        let borrowed = code[..i].iter().rev().find_map(|inst| match *inst {
            Inst::Read {
                result,
                address,
                reaching,
                mode: ReadMode::Borrow,
            } if result == value.value => Some((address, reaching)),
            _ => None,
        });
        let Some((address, reaching)) = borrowed else {
            return Ok(None);
        };
        let mut taken = None;
        for inst in &code[i + 1..] {
            match *inst {
                Inst::EndLoan { .. } => {}
                Inst::Read {
                    result,
                    address: a,
                    reaching: r,
                    mode: ReadMode::Take,
                } if taken.is_none() && a == address && r == reaching => taken = Some(result),
                Inst::Release { value: v, .. } if Some(v) == taken => {
                    return Ok(Some((value.value, v)));
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// `dying_builder`'s construction. A unique builder's block becomes the record's: its
    /// count and used words become the list's length prefix and count, and its descriptor
    /// the record's; the builder's release is then skipped. A shared one is copied.
    fn adopt_builder(
        &mut self,
        result: ValueId,
        site: &'a ConstructSite,
        builder: ValueId,
        released: ValueId,
        failure: FailureId,
    ) -> Emitted {
        let block = self.get(builder)?;
        let i32_ty = self.module.int_type(32);
        let count = self.builder.load(i32_ty, block, 8);
        let unique = self
            .builder
            .icmp(IntPredicate::Eq, count, self.module.const_int(i32_ty, 1));
        let (adopt, copy, join) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cond_br(unique, adopt, copy);
        self.builder.position(copy);
        self.construct_block(result, site, None, failure)?;
        let Some(copied) = self.values[result.index()].take() else {
            return self.invalid("a flat construction that defined no value");
        };
        let copied_end = self.builder.current();
        self.builder.br(join);
        self.builder.position(adopt);
        let Some(payload) = self.layout_payload(self.value_type(builder)?.layout)? else {
            return self.invalid("a builder that holds no block");
        };
        let data = self.payload_of(block, payload)?.ptr;
        let used_at = self.builder.offset(data, self.const_u64(8));
        let elements = self.load_u64(data);
        let used = self.load_u64(used_at);
        let length = self.builder.binary(BinOp::Add, used, self.const_u64(8));
        self.builder.store(length, data, 8);
        self.builder.store(elements, used_at, 8);
        let descriptor_at = self.builder.offset(block, self.const_u64(4));
        let old = self.builder.load(i32_ty, descriptor_at, 4);
        let kept = self.builder.binary(
            BinOp::And,
            old,
            self.module.const_int(
                i32_ty,
                u128::from(!hirsel_lir::runtime_abi::DESCRIPTOR_INDEX_MASK),
            ),
        );
        let index = self.descriptor(site.layout)?;
        let retyped = self.builder.binary(BinOp::Or, kept, index);
        self.builder.store(retyped, descriptor_at, 4);
        let adopt_end = self.builder.current();
        self.builder.br(join);
        self.builder.position(join);
        let value = self.builder.phi(self.module.ptr_type());
        self.builder.add_incoming(value, copied, copied_end);
        self.builder.add_incoming(value, block, adopt_end);
        let i1 = self.module.int_type(1);
        let taken = self.builder.phi(i1);
        self.builder
            .add_incoming(taken, self.module.const_zero(i1), copied_end);
        self.builder
            .add_incoming(taken, self.module.const_int(i1, 1), adopt_end);
        self.adopted.insert(released, taken);
        self.define(result, value)
    }

    /// The fields a record, tuple or variant construction stores: the record's, or its own
    /// variant's.
    fn constructed_fields(&self, site: &'a ConstructSite) -> Emitted<&'a [FieldLayout]> {
        let record = self.layout(site.layout)?;
        match site.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => Ok(&record.fields),
            ConstructorKind::Variant { variant, .. } => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant construction of an untagged layout");
                };
                match tag.variants.iter().find(|v| v.ordinal == variant) {
                    Some(active) => Ok(&active.fields),
                    None => self.invalid("a variant the layout does not have"),
                }
            }
            _ => self.invalid("fields of a construction that is no record, tuple or variant"),
        }
    }

    /// The fields a `Reuse` keeps (LIR schema revision 5): the set ordinals of `mask` among
    /// the construction's fields.
    fn kept_fields(&self, site: &'a ConstructSite, mask: MaskId) -> Emitted<Vec<&'a FieldLayout>> {
        let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
            return self.invalid("a mask outside the mask table");
        };
        let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
        let fields = self.constructed_fields(site)?;
        let set = |i: u32| {
            words
                .get(i as usize / 64)
                .is_some_and(|w| w >> (i % 64) & 1 == 1)
        };
        Ok(fields.iter().filter(|f| set(f.ordinal)).collect())
    }

    /// Stores the construction's value of `field` at its offset in `place`.
    fn store_field(
        &self,
        place: Place<'m>,
        site: &'a ConstructSite,
        field: &FieldLayout,
    ) -> Emitted {
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let Some(value) = values.iter().find(|v| v.field == field.ordinal) else {
            return self.invalid("a construction without a value for its field");
        };
        let at = self.field_place(place, field.offset, field.layout, field.unaligned)?;
        self.store(self.value_shape(value.value)?, self.get(value.value)?, at);
        Ok(())
    }

    /// Stores a construction's fields, except the ordinals in `skip`, into zeroed storage of
    /// its layout at `place`.
    fn fill(&mut self, place: Place<'m>, site: &'a ConstructSite, skip: &[u32]) -> Emitted {
        let record = self.layout(site.layout)?;
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        if !skip.is_empty()
            && !matches!(
                site.constructor,
                ConstructorKind::Tuple
                    | ConstructorKind::Record(_)
                    | ConstructorKind::Variant { .. }
            )
        {
            return self
                .invalid("kept fields of a construction that is no record, tuple or variant");
        }
        match site.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => {}
            ConstructorKind::Variant { variant, .. } => self.store_tag(place, variant)?,
            // The boxed value is the whole payload.
            ConstructorKind::Box => {
                let [value] = values else {
                    return self.invalid("a box of other than one value");
                };
                self.store(
                    self.value_shape(value.value)?,
                    self.get(value.value)?,
                    place,
                );
                return Ok(());
            }
            ConstructorKind::Sequence => {
                let (element, start) = match record.kind {
                    LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. } => {
                        (element, 0)
                    }
                    LayoutKind::Sequential { element } => {
                        // MEM-13: the count, the used bytes, then each element back to
                        // back at its active variant's extent.
                        let count = self.const_u64(values.len() as u64);
                        self.builder.store(count, place.ptr, place.align.min(8));
                        let mut ordered: Vec<_> = values.iter().collect();
                        ordered.sort_by_key(|v| v.field);
                        let first = self.sequential_first(place.ptr, element)?;
                        let mut used = self.const_u64(0);
                        for value in ordered {
                            let at = self.builder.offset(first, used);
                            let bytes = self.store_packed(value.value, element, at)?;
                            used = self.builder.binary(BinOp::Add, used, bytes);
                        }
                        let used_at = self.builder.offset(place.ptr, self.const_u64(8));
                        self.builder.store(used, used_at, place.align.min(8));
                        return Ok(());
                    }
                    LayoutKind::Sequence { element } => {
                        // The element count, the capacity (the count: never more than the
                        // block's slots), then the elements from their alignment (runtime
                        // ABI 5).
                        let count = self.const_u64(values.len() as u64);
                        self.builder.store(count, place.ptr, place.align.min(8));
                        let capacity = self
                            .builder
                            .offset(place.ptr, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
                        self.builder.store_access(
                            count,
                            capacity,
                            place.align.min(8),
                            Access::Capacity,
                        );
                        match sequence_elements_offset(self.layout(element)?) {
                            Ok(start) => (element, start),
                            Err(_) => return self.invalid("a sequence element offset beyond u64"),
                        }
                    }
                    _ => return self.unsupported("a sequence of this layout"),
                };
                let stride = self.fixed_size(element)?;
                for value in values {
                    let offset = start + u64::from(value.field) * stride;
                    let at = self.field_place(place, offset, element, false)?;
                    self.store(self.value_shape(value.value)?, self.get(value.value)?, at);
                }
                return Ok(());
            }
        }
        let fields = self.constructed_fields(site)?;
        for value in values {
            let Some(field) = fields.iter().find(|f| f.ordinal == value.field) else {
                return self.invalid("a constructed field the layout does not have");
            };
            if !skip.contains(&field.ordinal) {
                self.store_field(place, site, field)?;
            }
        }
        Ok(())
    }

    /// `hirsel_rt_v2_alloc` of a heap block of `layout` with `extent` payload bytes. It cannot
    /// fail (runtime ABI 3): host exhaustion terminates environmentally. The block's count is
    /// the alloc postcondition, one holder, restated as a store so that LLVM folds the
    /// uniqueness tests and the release of a block that has not escaped, and may then delete
    /// the allocation with its free (`runtime::allocator`). A constant small request calls
    /// its pool's fast path, inlined after optimization (runtime ABI 4, `runtime::alloc_fast`).
    fn alloc_block(
        &mut self,
        layout: LayoutId,
        extent: Value<'m>,
        align: u32,
        anchor: SourceAnchorId,
    ) -> Emitted<Value<'m>> {
        let descriptor = self.descriptor(layout)?;
        let alloc = self.runtime(RuntimeEntry::Alloc)?;
        let anchor = self.anchor_code(anchor)?;
        let i8_ty = self.module.int_type(8);
        // A constant small request calls its pool's fast path (runtime ABI 4).
        let alloc = match extent
            .known_u64()
            .and_then(|extent| small_pool(extent, u64::from(align)))
        {
            Some(pool) => crate::runtime::alloc_fast(self.module, alloc, pool),
            None => alloc,
        };
        let block = self.builder.call(
            alloc,
            &[
                self.context,
                descriptor,
                extent,
                self.const_u64(u64::from(align)),
                self.module.const_int(i8_ty, DomainTag::Heap as u128),
                self.const_u64(anchor),
            ],
        );
        let count = self
            .builder
            .offset(block, self.const_u64(COUNT_OFFSET.into()));
        let i32_ty = self.module.int_type(32);
        self.builder.store_access(
            self.module.const_int(i32_ty, COUNT_UNIQUE.into()),
            count,
            4,
            Access::Count,
        );
        Ok(block)
    }

    /// T03 Dup of the counted block `block`, inline below the context's dup bound.
    fn dup_block(&mut self, block: Value<'m>) -> Emitted {
        let dup = self.runtime(RuntimeEntry::Dup)?;
        crate::runtime::dup(&self.builder, self.function, dup, self.context, block);
        Ok(())
    }

    /// `hirsel_rt_v2_reuse` of `token` as a heap block of `layout` with `extent` payload
    /// bytes: the block, or `failure` when the runtime reports one.
    fn reuse_block(
        &mut self,
        token: Value<'m>,
        layout: LayoutId,
        extent: Value<'m>,
        align: u32,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        let descriptor = self.descriptor(layout)?;
        let out = self.prologue.alloca(self.module.ptr_type(), 8);
        let reuse = self.runtime(RuntimeEntry::Reuse)?;
        let status = self.builder.call(
            reuse,
            &[
                self.context,
                token,
                descriptor,
                extent,
                self.const_u64(u64::from(align)),
                out,
            ],
        );
        self.status_guard(status, failure)?;
        Ok(self.builder.load(self.module.ptr_type(), out, 8))
    }

    /// A block at an arena token: a counted heap block when the
    /// token is null (`heap`), else an immortal block bump-allocated in that arena. An `Arena`
    /// value is never null (C admits only open arena records), so only the hidden caller
    /// destination (`nullable`) tests for the heap.
    fn arena_block(
        &mut self,
        token: Value<'m>,
        nullable: bool,
        site: &'a ConstructSite,
        extent: Value<'m>,
        align: u32,
        flat: bool,
    ) -> Emitted<Value<'m>> {
        let ptr = self.module.ptr_type();
        let out = self.prologue.alloca(ptr, 8);
        let done = self.module.append_block(self.function);
        if nullable {
            let heap = self
                .builder
                .icmp(IntPredicate::Eq, token, self.module.const_zero(ptr));
            let (on_heap, in_arena) = (
                self.module.append_block(self.function),
                self.module.append_block(self.function),
            );
            self.builder.cond_br(heap, on_heap, in_arena);
            self.builder.position(on_heap);
            let block = self.alloc_block(site.layout, extent, align, site.anchor)?;
            self.builder.store(block, out, 8);
            self.builder.br(done);
            self.builder.position(in_arena);
        }
        // A flat immortal owner's copy may read its capacity rounded to 16, as a region's.
        let capacity = if flat {
            let rounded = self.builder.binary(BinOp::Add, extent, self.const_u64(15));
            self.builder
                .binary(BinOp::And, rounded, self.const_u64(!15))
        } else {
            extent
        };
        let slow = self.module.append_block(self.function);
        let fast = self.module.append_block(self.function);
        let cursor_address = self.builder.offset(
            token,
            self.const_u64(hirsel_lir::runtime_abi::REGION_CURSOR_OFFSET),
        );
        let limit_address = self.builder.offset(
            token,
            self.const_u64(hirsel_lir::runtime_abi::REGION_END_OFFSET),
        );
        let cursor = self.builder.load(ptr, cursor_address, 8);
        let limit = self.builder.load(ptr, limit_address, 8);
        let word = self.module.int_type(64);
        let cursor = self.builder.cast(Cast::PtrToInt, cursor, word);
        let limit = self.builder.cast(Cast::PtrToInt, limit, word);
        let physical = u64::from(align.max(8));
        let record = self.layout(site.layout)?;
        // The cursor is always word aligned (every reservation ends on a word), so a block
        // of a fixed layout (LAYOUT_NO_EXTENT) is its header at the cursor, or at the first
        // padded position for a wider payload alignment, then its payload: one bump and one
        // limit test. A null limit (none yet, or diagnostics) fails the test.
        let (block, next) = match (hirsel_lir::rt_program::no_extent(record), &record.size) {
            (true, &SizeSpec::Fixed(size)) => {
                let block = if physical == 8 {
                    cursor
                } else {
                    let payload =
                        self.builder
                            .binary(BinOp::Add, cursor, self.const_u64(8 + physical - 1));
                    let payload =
                        self.builder
                            .binary(BinOp::And, payload, self.const_u64(!(physical - 1)));
                    self.builder.binary(BinOp::Sub, payload, self.const_u64(8))
                };
                let next = self.builder.binary(
                    BinOp::Add,
                    block,
                    self.const_u64(8 + size.next_multiple_of(8)),
                );
                let fits = self.builder.icmp(IntPredicate::Ule, next, limit);
                self.builder.cond_br(fits, fast, slow);
                self.builder.position(fast);
                (block, next)
            }
            _ => {
                // Worst-case payload padding after the 16-byte extent and the header. No
                // inbounds GEP or overflowing request reaches a store.
                let overhead = 24 + physical - 8;
                let request = self
                    .builder
                    .binary(BinOp::Add, capacity, self.const_u64(overhead));
                let room = self.builder.binary(BinOp::Sub, limit, cursor);
                let in_segment = self.builder.icmp(IntPredicate::Ule, cursor, limit);
                let fits = self.builder.icmp(IntPredicate::Ule, request, room);
                // The runtime rejects larger requests before host-size conversion.
                let bounded = self.builder.icmp(
                    IntPredicate::Ule,
                    capacity,
                    self.const_u64(i64::MAX as u64 / 2 - overhead),
                );
                let fits = self.builder.binary(BinOp::And, in_segment, fits);
                let fits = self.builder.binary(BinOp::And, fits, bounded);
                self.builder.cond_br(fits, fast, slow);
                self.builder.position(fast);
                let payload =
                    self.builder
                        .binary(BinOp::Add, cursor, self.const_u64(24 + physical - 1));
                let payload =
                    self.builder
                        .binary(BinOp::And, payload, self.const_u64(!(physical - 1)));
                let block = self.builder.binary(BinOp::Sub, payload, self.const_u64(8));
                let rounded = self.builder.binary(BinOp::Add, capacity, self.const_u64(7));
                let rounded = self.builder.binary(BinOp::And, rounded, self.const_u64(!7));
                let next = self.builder.binary(BinOp::Add, payload, rounded);
                let at = self.builder.cast(Cast::IntToPtr, block, ptr);
                let extent_address = self
                    .builder
                    .offset(at, self.const_u64(0u64.wrapping_sub(16)));
                self.builder.store(capacity, extent_address, 8);
                let align_address = self
                    .builder
                    .offset(at, self.const_u64(0u64.wrapping_sub(8)));
                self.builder
                    .store(self.const_u64(physical), align_address, 8);
                (block, next)
            }
        };
        let next = self.builder.cast(Cast::IntToPtr, next, ptr);
        self.builder.store(next, cursor_address, 8);
        let block = self.builder.cast(Cast::IntToPtr, block, ptr);
        let i32_ty = self.module.int_type(32);
        self.builder.store(
            self.module.const_int(i32_ty, u128::from(COUNT_IMMORTAL)),
            block,
            8,
        );
        let descriptor = self.descriptor(site.layout)?;
        let kind = self.module.const_int(
            i32_ty,
            u128::from(hirsel_lir::runtime_abi::KIND_REGION << hirsel_lir::runtime_abi::KIND_SHIFT),
        );
        let descriptor = self.builder.binary(BinOp::Or, descriptor, kind);
        let descriptor_address = self.builder.offset(block, self.const_u64(4));
        self.builder.store(descriptor, descriptor_address, 4);
        self.builder.store(block, out, 8);
        self.builder.br(done);
        self.builder.position(slow);
        let descriptor = self.descriptor(site.layout)?;
        let alloc = self.runtime(RuntimeEntry::ArenaAlloc)?;
        let block = self.builder.call(
            alloc,
            &[
                self.context,
                token,
                descriptor,
                capacity,
                self.const_u64(u64::from(align)),
            ],
        );
        self.builder.store(block, out, 8);
        self.builder.br(done);
        self.builder.position(done);
        Ok(self.builder.load(ptr, out, 8))
    }

    /// After an arena block's payload is filled: when the token names an arena and the
    /// payload holds a heap credit (a counted child that is not immortal, or any owned
    /// content other than counted children), the arena schedules the payload's drop program
    /// for its close (MEM-4). A payload of arena children schedules nothing.
    fn arena_hold(
        &mut self,
        token: Value<'m>,
        nullable: bool,
        payload: Place<'m>,
        layout: LayoutId,
        failure: FailureId,
    ) -> Emitted {
        let record = self.layout(layout)?;
        if record.drop.is_empty() && record.finalizer.is_none() {
            return Ok(());
        }
        let steps = record.drop.clone();
        let finalized = record.finalizer.is_some();
        let ptr = self.module.ptr_type();
        let flag_ty = self.module.int_type(1);
        let flag = self.prologue.alloca(flag_ty, 1);
        self.builder.store(
            self.module.const_int(flag_ty, u128::from(finalized)),
            flag,
            1,
        );
        let (schedule, done) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        if nullable {
            let walk = self.module.append_block(self.function);
            let heap = self
                .builder
                .icmp(IntPredicate::Eq, token, self.module.const_zero(ptr));
            self.builder.cond_br(heap, done, walk);
            self.builder.position(walk);
        }
        self.arena_credits(payload.ptr, payload.align, &steps, flag)?;
        let holds = self.builder.load(flag_ty, flag, 1);
        self.builder.cond_br(holds, schedule, done);
        self.builder.position(schedule);
        let descriptor = self.descriptor(layout)?;
        let hold = self.runtime(RuntimeEntry::RegionSchedule)?;
        let status = self
            .builder
            .call(hold, &[self.context, token, descriptor, payload.ptr]);
        self.status_guard(status, failure)?;
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }

    /// Set `flag` where the drop program `steps` over the bytes at `base` would release a
    /// counted block that is not immortal, or would do anything but release counted children.
    fn arena_credits(
        &mut self,
        base: Value<'m>,
        align: u32,
        steps: &[DropStep],
        flag: Value<'m>,
    ) -> Emitted {
        let ptr = self.module.ptr_type();
        let flag_ty = self.module.int_type(1);
        let i32_ty = self.module.int_type(32);
        let at = |this: &Self, offset: u64| {
            if offset == 0 {
                base
            } else {
                this.builder.offset(base, this.const_u64(offset))
            }
        };
        for step in steps {
            match step {
                DropStep::Release { offset } => {
                    let child = self.builder.load(
                        ptr,
                        at(self, *offset),
                        align.min(low_bit(*offset)).min(8),
                    );
                    let word = if hirsel_lir::runtime_abi::COUNT_OFFSET == 0 {
                        child
                    } else {
                        self.builder
                            .offset(child, self.const_u64(hirsel_lir::runtime_abi::COUNT_OFFSET))
                    };
                    let count = self.builder.load(i32_ty, word, 4);
                    let counted = self.builder.icmp(
                        IntPredicate::Ne,
                        count,
                        self.module.const_int(i32_ty, u128::from(COUNT_IMMORTAL)),
                    );
                    let old = self.builder.load(flag_ty, flag, 1);
                    self.builder
                        .store(self.builder.binary(BinOp::Or, old, counted), flag, 1);
                }
                DropStep::Child { offset, layout } => {
                    let record = self.layout(*layout)?;
                    if record.finalizer.is_some() {
                        self.builder
                            .store(self.module.const_int(flag_ty, 1), flag, 1);
                    }
                    let steps = record.drop.clone();
                    let base = at(self, *offset);
                    self.arena_credits(base, align.min(low_bit(*offset)), &steps, flag)?;
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let tag = load_tag(&self.builder, base, align, *tag_offset, *form, false);
                    let join = self.module.append_block(self.function);
                    for (variant, steps) in variants {
                        if steps.is_empty() {
                            continue;
                        }
                        let (yes, next) = (
                            self.module.append_block(self.function),
                            self.module.append_block(self.function),
                        );
                        let active = self.builder.icmp(
                            IntPredicate::Eq,
                            tag,
                            self.module.const_int(i32_ty, u128::from(*variant)),
                        );
                        self.builder.cond_br(active, yes, next);
                        self.builder.position(yes);
                        self.arena_credits(base, align, steps, flag)?;
                        self.builder.br(join);
                        self.builder.position(next);
                    }
                    self.builder.br(join);
                    self.builder.position(join);
                }
                // Finalizers, environments, handles and element loops: schedule.
                _ => self
                    .builder
                    .store(self.module.const_int(flag_ty, 1), flag, 1),
            }
        }
        Ok(())
    }

    /// A block construction: a heap block (allocated, or installed in `token`'s storage), or
    /// uncounted storage reserved in a compiler-local region, whose zeroed payload then
    /// receives the fields. A region payload whose layout drops anything is scheduled with the
    /// region, which drops it at exit.
    fn construct_block(
        &mut self,
        result: ValueId,
        site: &'a ConstructSite,
        reuse: Option<(Value<'m>, Option<MaskId>)>,
        failure: FailureId,
    ) -> Emitted {
        let token = reuse.map(|(token, _)| token);
        let repr = self.value_type(result)?.repr;
        let record = self.layout(site.layout)?;
        let align = record.align;
        let flat = record.kind == LayoutKind::Flat && site.constructor != ConstructorKind::Box;
        if flat && reuse.is_some() {
            return self.invalid("a reuse into a flat construction");
        }
        let parts = if flat {
            self.flat_parts(site)?
        } else {
            Vec::new()
        };
        let extent = match (&record.size, record.kind.clone()) {
            (SizeSpec::Fixed(size), _) => *size,
            // A flat record's extent is its parts' bytes, known once they are read.
            (SizeSpec::Dynamic(_), LayoutKind::Flat) if flat => 0,
            (SizeSpec::Dynamic(_), LayoutKind::Sequence { element }) => {
                let count = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
                let start = sequence_elements_offset(self.layout(element)?).ok();
                let stride = self.fixed_size(element)?;
                match count
                    .checked_mul(stride)
                    .and_then(|bytes| bytes.checked_add(start?))
                {
                    Some(extent) => extent,
                    None => return self.invalid("a sequence extent beyond u64"),
                }
            }
            // The header; the elements' bytes are known once their tags are read.
            (SizeSpec::Dynamic(_), LayoutKind::Sequential { element }) => {
                self.sequential_offset(element)?
            }
            (SizeSpec::Dynamic(_), _) => {
                return self.unsupported("a dynamic block extent other than a sequence");
            }
        };
        let extent_value = if flat {
            self.flat_extent(&parts)?
        } else if let LayoutKind::Sequential { element } = record.kind {
            let mut bytes = self.const_u64(extent);
            for value in self.pool(site.fields, &self.unit.pools.fields)? {
                let temp = self.stack(element)?;
                self.store(self.value_shape(value.value)?, self.get(value.value)?, temp);
                let extent = self.extent(element, temp.ptr)?;
                bytes = self.builder.binary(BinOp::Add, bytes, extent);
            }
            bytes
        } else {
            self.const_u64(extent)
        };
        // The token of an arena or caller destination, whose credits it may hold.
        let mut arena = None;
        let (value, payload) = match (site.placement, repr) {
            (Some(Placement::Fixed(StorageOwner::Heap(_))), ValueRepr::Block) => {
                let block = match token {
                    None => self.alloc_block(site.layout, extent_value, align, site.anchor)?,
                    Some(token) => {
                        self.reuse_block(token, site.layout, extent_value, align, failure)?
                    }
                };
                let payload = self.payload_of(block, site.layout)?;
                (block, payload)
            }
            (Some(Placement::Fixed(StorageOwner::LocalRegion(region))), ValueRepr::Uncounted)
                if token.is_none() =>
            {
                let Some(&record) = self.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                let payload = self.reserve_region(record, site.layout, extent_value, failure)?;
                let storage = payload.ptr;
                if !self.layout(site.layout)?.drop.is_empty() {
                    let descriptor = self.descriptor(site.layout)?;
                    let schedule = self.runtime(RuntimeEntry::RegionSchedule)?;
                    let status = self
                        .builder
                        .call(schedule, &[self.context, record, descriptor, storage]);
                    self.status_guard(status, failure)?;
                }
                (storage, payload)
            }
            (Some(Placement::Fixed(StorageOwner::Arena(id))), ValueRepr::Block)
                if token.is_none() =>
            {
                let Some(instance) = self.unit.tables.arenas.get(id.index()) else {
                    return self.invalid("an arena outside the arena table");
                };
                let Some(Some(slot)) = self.slots.get(instance.slot.index()).copied() else {
                    return self.invalid("an arena token slot without storage");
                };
                let token = self.load(Shape::Scalar(ScalarKind::Pointer), slot);
                let block = self.arena_block(token, false, site, extent_value, align, flat)?;
                arena = Some((token, false));
                (block, self.payload_of(block, site.layout)?)
            }
            (Some(Placement::CallerDestination), ValueRepr::Block) if token.is_none() => {
                let Some(index) = self
                    .signature
                    .hidden_index(|h| matches!(h, Hidden::Destination))
                else {
                    return self.invalid("a caller destination in a function without one");
                };
                let token = self.module.parameter(self.function, index);
                let block = self.arena_block(token, true, site, extent_value, align, flat)?;
                arena = Some((token, true));
                (block, self.payload_of(block, site.layout)?)
            }
            _ => {
                return self
                    .invalid("a block construction whose placement and representation disagree");
            }
        };
        if flat {
            self.write_flat(payload.ptr, &parts)?;
            return self.define(result, value);
        }
        // Reused storage keeps the bytes of each kept field (LIR schema revision 5): neither
        // zeroed nor stored, unless the token was null and the storage is fresh.
        let kept = match reuse {
            Some((_, Some(mask))) => self.kept_fields(site, mask)?,
            _ => Vec::new(),
        };
        let mut ranges = Vec::new();
        for field in &kept {
            ranges.push((field.offset, field.offset + self.fixed_size(field.layout)?));
        }
        ranges.sort_unstable();
        let mut at = 0;
        for (start, end) in ranges.iter().copied().chain([(extent, extent)]) {
            if start > at {
                let gap = self.field_place(payload, at, site.layout, false)?;
                self.builder
                    .memset(gap.ptr, 0, start - at, align.min(low_bit(at)));
            }
            at = at.max(end);
        }
        let skip: Vec<u32> = kept.iter().map(|f| f.ordinal).collect();
        self.fill(payload, site, &skip)?;
        if let Some((token, nullable)) = arena.filter(|_| !site.arena_no_drop) {
            self.arena_hold(token, nullable, payload, site.layout, failure)?;
        }
        if let (Some(token), false) = (token, kept.is_empty()) {
            let null = self.module.const_zero(self.module.ptr_type());
            let fresh = self.builder.icmp(IntPredicate::Eq, token, null);
            let (store, done) = (
                self.module.append_block(self.function),
                self.module.append_block(self.function),
            );
            self.builder.cond_br(fresh, store, done);
            self.builder.position(store);
            for field in kept {
                self.store_field(payload, site, field)?;
            }
            self.builder.br(done);
            self.builder.position(done);
        }
        self.define(result, value)
    }

    // Terminators.

    /// The one block every return of a register-result entry branches to. Status and
    /// word meet in scalar phis and the pair is built once, so neither return merging
    /// nor the inliner forms a phi of pairs: after inlining, a caller's status test
    /// sees each return site's constant and threads, as it does for a bare status.
    fn register_return(&mut self, kind: ScalarKind) -> (BasicBlock<'m>, Value<'m>, Value<'m>) {
        if let Some(found) = self.register_return {
            return found;
        }
        let block = self.module.append_block(self.function);
        let builder = Builder::new(self.module);
        builder.position(block);
        let status = builder.phi(self.module.int_type(8));
        let word = builder.phi(value_word(self.module, kind));
        let returned = self.module.const_zero(self.signature.returns);
        let returned = builder.insert(returned, status, self.signature.status_field);
        let (_, field) = self.signature.value.expect("a register result");
        builder.ret(builder.insert(returned, word, field));
        self.register_return = Some((block, status, word));
        (block, status, word)
    }

    /// Returns `status` and, on success with a register result, `value`.
    fn return_status(&mut self, status: u128, value: Option<Value<'m>>) -> Emitted {
        let status_byte = self.module.const_int(self.module.int_type(8), status);
        let mut returned = self.module.const_zero(self.signature.returns);
        if let (true, Some((kind, _))) = (self.signature.roots.is_empty(), self.signature.value) {
            let ty = value_word(self.module, kind);
            let word = match value {
                Some(value) => encode_word(&self.builder, kind, value, ty),
                None => self.module.const_zero(ty),
            };
            let (block, status_phi, word_phi) = self.register_return(kind);
            let here = self.builder.current();
            self.builder.br(block);
            self.builder.add_incoming(status_phi, status_byte, here);
            self.builder.add_incoming(word_phi, word, here);
            return Ok(());
        }
        if self.signature.roots.is_empty() {
            self.builder.ret(
                self.signature
                    .returned(&self.builder, returned, status_byte, value),
            );
            return Ok(());
        }
        for (field, &i) in self.signature.roots.iter().enumerate() {
            let source = self.signature.abi.parameters[i].source;
            let slot = self
                .unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(source))
                .and_then(|s| self.slots[s])
                .ok_or_else(|| CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: "missing returned root slot".into(),
                })?;
            let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), slot);
            let (unique, length) = if let Some(state) = self.sequence_state_at(slot) {
                (
                    self.builder.load(self.module.int_type(1), state.unique, 1),
                    self.builder.load(self.module.int_type(64), state.length, 8),
                )
            } else {
                (
                    self.module.const_zero(self.module.int_type(1)),
                    if self.sequence_element(slot.layout)?.is_some() {
                        self.sequence_length(pointer, slot.layout)?
                    } else {
                        self.const_u64(0)
                    },
                )
            };
            returned = self.builder.insert(returned, pointer, field as u32 * 2);
            let Some(index) = self.signature.uniques[field] else {
                returned = self.builder.insert(
                    returned,
                    self.const_u64(status as u64),
                    field as u32 * 2 + 1,
                );
                continue;
            };
            returned = self.builder.insert(returned, unique, index);
            returned = self.builder.insert(returned, length, field as u32 * 2 + 1);
        }
        if self.signature.sequence_roots[0] {
            returned = self.builder.insert(
                returned,
                self.const_u64(status as u64),
                self.signature.status_field,
            );
        }
        self.builder.ret(
            self.signature
                .returned(&self.builder, returned, status_byte, value),
        );
        Ok(())
    }

    fn terminator(&mut self, terminator: &Terminator) -> Emitted {
        match *terminator {
            Terminator::Jump(edge) => self.jump(edge),
            Terminator::Branch { condition, yes, no } => {
                let condition = self.get(condition)?;
                let yes = self.edge_block(yes)?;
                let no = self.edge_block(no)?;
                self.builder.cond_br(condition, yes, no);
                Ok(())
            }
            Terminator::Switch {
                value,
                arms,
                otherwise,
            } => {
                // A switch on a variant value selects by its active ordinal: the value is
                // spilled and its tag read as a place's is.
                let layout = self.value_type(value)?.layout;
                let record = self.layout(layout)?;
                let (ty, on) = match (self.value_shape(value)?, &record.tag) {
                    (Shape::Bytes { .. }, Some(tag)) => {
                        let place = self.stack(layout)?;
                        self.store(self.value_shape(value)?, self.get(value)?, place);
                        let on = load_tag(
                            &self.builder,
                            place.ptr,
                            place.align,
                            tag.offset,
                            tag.form,
                            false,
                        );
                        (self.module.int_type(32), on)
                    }
                    _ => {
                        let kind = self.scalar_kind(value)?;
                        let Some((width, _)) = scalar::integer(kind) else {
                            return self.invalid("a switch on a value that is not an integer");
                        };
                        (self.module.int_type(width), self.get(value)?)
                    }
                };
                let mut cases = Vec::new();
                for arm in self.pool(arms, &self.unit.pools.switch_arms)? {
                    cases.push((
                        self.module.const_int(ty, u128::from(arm.value)),
                        self.edge_block(arm.edge)?,
                    ));
                }
                let otherwise = self.edge_block(otherwise)?;
                self.builder.switch(on, otherwise, &cases);
                Ok(())
            }
            Terminator::Call {
                call,
                normal,
                failure,
            } => self.call(call, normal, failure),
            Terminator::Return(value) => {
                self.flush_on_return()?;
                if self.signature.value.is_some() {
                    let Some(value) = value else {
                        return self.invalid("a return without the entry's result");
                    };
                    return self.return_status(0, Some(self.get(value)?));
                }
                if let (Some(value), Some(index)) = (
                    value,
                    self.signature.hidden_index(|h| matches!(h, Hidden::Result)),
                ) {
                    let Some(result) = &self.signature.abi.result else {
                        return self.invalid("a result slot without a result");
                    };
                    let align = self.layout(result.layout)?.align;
                    let out = Place {
                        ptr: self.module.parameter(self.function, index),
                        layout: result.layout,
                        align,
                        block: None,
                    };
                    self.store(self.value_shape(value)?, self.get(value)?, out);
                }
                self.return_status(0, None)?;
                Ok(())
            }
            Terminator::Raise(failure) => {
                let block = self.failure_block(failure)?;
                self.builder.br(block);
                Ok(())
            }
            Terminator::Propagate => {
                self.flush_on_return()?;
                self.return_status(1, None)?;
                Ok(())
            }
            Terminator::InvariantFault { .. } => {
                let fatal = self.runtime(RuntimeEntry::InvariantFault)?;
                self.builder.call(fatal, &[]);
                self.builder.unreachable();
                Ok(())
            }
            Terminator::Unreachable => {
                // Proved unreachable by the validator; still a trap rather than an
                // optimizer assumption.
                let trap = match self.module.intrinsic("llvm.trap", &[]) {
                    Ok(trap) => trap,
                    Err(what) => return self.invalid(what),
                };
                self.builder.call(trap, &[]);
                self.builder.unreachable();
                Ok(())
            }
        }
    }

    /// The link name of function `symbol`, imported at `abi`.
    fn named_function(&mut self, symbol: SymbolId, abi: &FnAbi) -> Emitted<String> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a callee outside the symbol table");
        };
        if symbol.kind != SymbolKind::Function {
            return self.invalid("a call of a symbol that is not a function");
        }
        let name = symbol_name(&symbol.definition, symbol.instance.as_ref());
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(symbol.definition.clone()),
                abi: Some(abi_digest(abi)),
            });
        Ok(name)
    }

    fn call(&mut self, call: CallId, normal: Option<EdgeId>, failure: FailureId) -> Emitted {
        let unit = self.unit;
        let Some(site) = unit.tables.calls.get(call.index()) else {
            return self.invalid("a call outside the call table");
        };
        let abi = fn_abi(self.inputs.layouts, site.abi, self.definition())?;
        let arguments = self.pool(site.arguments, &unit.pools.call_arguments)?;
        // The callee: a named function, or a code pointer and its environment.
        let (callee, environment, name): (Value<'m>, Option<Value<'m>>, Option<String>) = match site
            .callee
        {
            Callee::Direct(symbol) => (
                self.module.const_zero(self.module.ptr_type()),
                None,
                Some(self.named_function(symbol, abi)?),
            ),
            // A bound call (LIR schema revision 9): the named instance with
            // its closure's environment.
            Callee::Bound { code, closure } => {
                if !abi.environment {
                    return self.invalid("a bound call through an ABI without an environment");
                }
                let (_, env) = self.callable_parts(closure)?;
                (
                    self.module.const_zero(self.module.ptr_type()),
                    Some(env),
                    Some(self.named_function(code, abi)?),
                )
            }
            Callee::Operation(op) => {
                let Some(id) = unit.tables.operations.get(op.index()) else {
                    return self.invalid("an operation outside the operation table");
                };
                let Some(row) = self.inputs.manifest.row(id) else {
                    return self.invalid("an operation the manifest does not list");
                };
                match row.dispositions.get(Engine::Native) {
                    // A runtime family's template names the symbol for this call's scalar
                    // kinds (LIR schema revision 6); a scalar intrinsic is an instruction at
                    // Ready, never a call.
                    Disposition::RuntimePrimitive(template) => {
                        let Some(symbol) =
                            hirsel_lir::abi::resolve_call_symbol(template, unit, site)
                        else {
                            return self.invalid(format!(
                                "runtime template `{template}` names no symbol for this call"
                            ));
                        };
                        self.manifested
                            .imports
                            .entry(symbol.clone())
                            .or_insert_with(|| ObjectSymbol {
                                name: symbol.clone(),
                                kind: ObjectSymbolKind::Runtime,
                                definition: Some(id.definition.clone()),
                                abi: Some(abi_digest(abi)),
                            });
                        (
                            self.module.const_zero(self.module.ptr_type()),
                            None,
                            Some(symbol.clone()),
                        )
                    }
                    other => {
                        return self.unsupported(format!(
                            "an operation with native disposition {other:?}"
                        ));
                    }
                }
            }
            Callee::Indirect(callable) => {
                if !abi.environment {
                    return self.invalid("an indirect call through an ABI without an environment");
                }
                let (code, env) = self.callable_parts(callable)?;
                (code, Some(env), None)
            }
        };
        let root_name = name
            .as_ref()
            .map(|n| format!("{n}{ROOTS_SUFFIX}"))
            .filter(|n| self.manifested.library.contains(n));
        let signature = if root_name.is_some() {
            Signature::of(self.module, self.inputs.layouts, abi, self.definition())?
                .with_roots(self.module, self.inputs.layouts)
        } else if let Callee::Operation(_) = site.callee {
            Signature::runtime(self.module, abi, self.definition())?
        } else {
            Signature::of(self.module, self.inputs.layouts, abi, self.definition())?
        };
        let name = root_name.or(name);
        let function = match &name {
            Some(name) => Some(signature.declare(self.module, name, self.definition(), false)?),
            None => None,
        };
        let mut args = Vec::new();
        let mut result_place = None;
        for hidden in &signature.hidden {
            match hidden {
                Hidden::Context => args.push(self.context),
                Hidden::Environment => match environment {
                    Some(env) => args.push(env),
                    None => return self.invalid("an environment argument without a callable"),
                },
                Hidden::Destination => {
                    let destination = match &site.destination {
                        Some(DestinationClass::Heap) => {
                            self.module.const_zero(self.module.ptr_type())
                        }
                        Some(DestinationClass::Caller(_)) => {
                            match self
                                .signature
                                .hidden_index(|h| matches!(h, Hidden::Destination))
                            {
                                Some(index) => self.module.parameter(self.function, index),
                                None => {
                                    return self
                                        .invalid("a caller destination in a function without one");
                                }
                            }
                        }
                        Some(other) => {
                            return self.unsupported(format!("a call destination {other:?}"));
                        }
                        None => return self.invalid("a destination slot without a destination"),
                    };
                    args.push(destination);
                }
                Hidden::Result => {
                    let Some(result) = &abi.result else {
                        return self.invalid("a result slot without a result");
                    };
                    let place = self.stack(result.layout)?;
                    args.push(place.ptr);
                    result_place = Some(place);
                }
            }
        }
        let mut root_places = Vec::new();
        for (i, parameter) in abi.parameters.iter().enumerate() {
            let Some(argument) = arguments.iter().find(|a| a.parameter == parameter.source) else {
                return self.invalid("a call without an argument for a parameter");
            };
            match (parameter.passing, argument.value) {
                (ParameterPassing::ByValue, ArgumentValue::Value(value)) => {
                    let slots = self.disassemble(parameter.layout, self.get(value)?)?;
                    if slots.len() != parameter.slots.len() {
                        return self.invalid("an argument's slots differ from its parameter's");
                    }
                    args.extend(slots);
                }
                (
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::TransferAddress,
                    ArgumentValue::Value(value),
                ) => {
                    // The callee reads (or takes) the value from caller storage.
                    let place = self.stack(parameter.layout)?;
                    self.store(self.value_shape(value)?, self.get(value)?, place);
                    args.push(place.ptr);
                }
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { address, .. }) => {
                    let place = self.place(address)?;
                    if signature.roots.contains(&i) {
                        args.push(self.load(Shape::Scalar(ScalarKind::Pointer), place));
                        root_places.push(place);
                    } else {
                        args.push(place.ptr);
                    }
                }
                (ParameterPassing::PointerLength, ArgumentValue::Value(value)) => {
                    let place = match self.layout(parameter.layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            if self.value_type(value)?.repr == ValueRepr::Uncounted {
                                self.uncounted_payload(self.get(value)?, payload)?
                            } else {
                                self.payload(self.get(value)?, parameter.layout)?
                            }
                        }
                        LayoutKind::Flat => {
                            let place = self.stack(parameter.layout)?;
                            self.store(self.value_shape(value)?, self.get(value)?, place);
                            place
                        }
                        _ => return self.invalid("a pointer/length argument of a non-flat layout"),
                    };
                    args.push(place.ptr);
                    args.push(self.extent(place.layout, place.ptr)?);
                }
                _ => return self.invalid("an argument passed against its parameter's convention"),
            }
        }
        for (field, place) in root_places.iter().copied().enumerate() {
            if signature.companions[field].is_some() {
                let (length, unique) = if let Some(state) = self.sequence_state_at(place) {
                    (
                        self.builder.load(self.module.int_type(64), state.length, 8),
                        self.builder.load(self.module.int_type(1), state.unique, 1),
                    )
                } else {
                    let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                    (
                        self.sequence_length(pointer, place.layout)?,
                        self.module.const_zero(self.module.int_type(1)),
                    )
                };
                args.extend([length, unique]);
            }
        }
        // A must-unique fact names a block with one holder, this frame's
        // unescaped slot. A callee can change that count only through a
        // reference it is given: an environment, a caller destination, or a
        // parameter or result whose layout holds one. Private root bodies
        // cannot mutate unrelated, unescaped scalar roots, and a call handed
        // no reference at all (`step(h: u64, value: u64) -> u64`) reaches no
        // counted block; counts are task-local (OWN-6). Every other call
        // retains the conservative invalidation of must-unique facts.
        let plain = |layout: LayoutId| {
            self.inputs
                .layouts
                .layout(layout)
                .is_some_and(|l| l.scan.is_empty() && l.drop.is_empty() && l.finalizer.is_none())
        };
        let isolated_roots = !signature.roots.is_empty()
            && !abi.environment
            && !abi.destination
            && abi.result.is_none()
            && abi
                .parameters
                .iter()
                .enumerate()
                .all(|(i, parameter)| signature.roots.contains(&i) || plain(parameter.layout));
        let referenceless = !abi.environment
            && !abi.destination
            && abi.parameters.iter().all(|p| plain(p.layout))
            && abi.result.as_ref().is_none_or(|r| plain(r.layout));
        if !isolated_roots && !referenceless {
            self.invalidate_uniqueness();
            self.flush_lengths(true)?;
        }
        let returned = match function {
            Some(function) => self.builder.call(function, &args),
            None => self.builder.call_typed(signature.ty, callee, &args),
        };
        signature.attribute_call(self.module, returned);
        // Hand back every root before branching on status: cleanup and Error observe
        // exactly the same callee mutations as successful continuation.
        let status = signature.status(&self.builder, returned);
        if !signature.roots.is_empty() {
            let succeeded = self.builder.icmp(
                IntPredicate::Eq,
                status,
                self.module.const_int(self.module.int_type(8), 0),
            );
            for (field, place) in root_places.into_iter().enumerate() {
                let pointer = signature.root_pointer(&self.builder, returned, field);
                self.store(Shape::Scalar(ScalarKind::Pointer), pointer, place);
                if signature.companions[field].is_some() && self.sequence_state_at(place).is_none()
                {
                    // The callee hands back a length its block may lag.
                    let length = self.builder.extract(returned, field as u32 * 2 + 1);
                    self.store_length_if_differs(pointer, place.layout, length)?;
                }
                if let Some(state) = self.sequence_state_at(place) {
                    self.builder.store(
                        self.builder.extract(returned, field as u32 * 2 + 1),
                        state.length,
                        8,
                    );
                    self.builder.store(
                        self.module.const_int(self.module.int_type(1), 1),
                        state.dirty,
                        1,
                    );
                    // list_grow's SeqReserve hands back unique storage. Its
                    // cold body stays out of line, so LLVM cannot recover this
                    // fact from the tagged aggregate return at the call site.
                    let reserved = matches!(site.callee, Callee::Direct(symbol)
                        if self.unit.tables.symbols[symbol.index()].definition
                            == core_library_path("$native.list_grow"));
                    let unique = if reserved {
                        succeeded
                    } else {
                        signature.root_unique(&self.builder, returned, field)
                    };
                    self.builder.store(unique, state.unique, 1);
                }
            }
        }
        let ok = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module.const_int(self.module.int_type(8), 0),
        );
        let failure_block = self.failure_block(failure)?;
        let fail = if self.unit.header.origin == UnitOrigin::Library {
            failure_block
        } else {
            let block = self.module.append_block(self.function);
            let here = self.builder.current();
            self.builder.position(block);
            let reanchor = self.runtime(RuntimeEntry::FailureReanchor)?;
            let anchor = self.const_u64(self.anchor_code(site.anchor)?);
            self.builder.call(reanchor, &[self.context, anchor]);
            self.builder.br(failure_block);
            self.builder.position(here);
            block
        };
        let fail = if let Callee::Operation(op) = site.callee
            && let Some(rule) = self
                .inputs
                .manifest
                .row(&unit.tables.operations[op.index()])
                .and_then(|row| row.failure.as_ref())
        {
            let Some(code) = self.inputs.failures.codes.get(&rule.0).copied() else {
                return Err(CodegenError::UnknownRule {
                    definition: self.definition().clone(),
                    rule: rule.0.clone(),
                });
            };
            if self.failure_edge(failure)?.rule.is_some() {
                return self.invalid("a runtime call failure rule must be recorded exactly once");
            }
            let here = self.builder.current();
            let record = self.module.append_block(self.function);
            self.builder.position(record);
            let begin = self.runtime(RuntimeEntry::FailureBegin)?;
            let site = self.anchor_code(site.anchor)?;
            let i32_ty = self.module.int_type(32);
            self.builder.call(
                begin,
                &[
                    self.context,
                    self.module.const_int(i32_ty, u128::from(code.kind)),
                    self.module.const_int(i32_ty, u128::from(code.rule)),
                    self.const_u64(site),
                ],
            );
            self.builder.br(fail);
            self.builder.position(here);
            record
        } else {
            fail
        };
        let next = self.module.append_block(self.function);
        self.builder.cond_br(ok, next, fail);
        self.builder.position(next);
        // A diverging callee (LIR schema revision 19) reports only failure:
        // its success status is impossible. It is not `noreturn`, since a
        // language failure returns status 1 to this frame's failure edge.
        let Some(normal) = normal else {
            self.builder.unreachable();
            return Ok(());
        };
        if let Some(result) = site.result {
            let value = match (signature.result(&self.builder, returned), result_place) {
                (Some(value), _) => value,
                (None, Some(place)) => self.load(self.value_shape(result)?, place),
                (None, None) => return self.invalid("a call result without a result slot"),
            };
            self.define(result, value)?;
        }
        self.jump(normal)
    }
}

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

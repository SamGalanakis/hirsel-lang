//! Owned LLVM handles and the typed builder layer over them.
//!
//! Each owning wrapper (`Context`, `Module`, `Builder`, `TargetMachine`) owns exactly one LLVM
//! object and disposes of it on drop. A `Module` borrows its `Context`, and every `Type`,
//! `Value`, `BasicBlock` and `Builder` borrows the `Module` it was made in, so the borrow
//! checker orders their disposal and no handle outlives its owner. No raw handle leaves this
//! module except to the other modules of this crate, and none is public.

use llvm_sys::debuginfo::*;
use llvm_sys::prelude::{LLVMDIBuilderRef, LLVMMetadataRef};
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_uint};
use std::marker::PhantomData;
use std::ptr;
use std::sync::OnceLock;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::*;
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMGetErrorMessage};
use llvm_sys::ir_reader::LLVMParseIRInContext2;
use llvm_sys::prelude::{
    LLVMBasicBlockRef, LLVMBuilderRef, LLVMContextRef, LLVMMemoryBufferRef, LLVMModuleRef,
    LLVMTypeRef, LLVMValueRef,
};
use llvm_sys::target::{
    LLVMABIAlignmentOfType, LLVMABISizeOfType, LLVMByteOrder, LLVMByteOrdering,
    LLVMCopyStringRepOfTargetData, LLVMDisposeTargetData, LLVMInitializeAArch64AsmPrinter,
    LLVMInitializeAArch64Target, LLVMInitializeAArch64TargetInfo, LLVMInitializeAArch64TargetMC,
    LLVMInitializeX86AsmPrinter, LLVMInitializeX86Target, LLVMInitializeX86TargetInfo,
    LLVMInitializeX86TargetMC, LLVMPointerSize, LLVMSetModuleDataLayout, LLVMTargetDataRef,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetDataLayout,
    LLVMCreateTargetMachine, LLVMDisposeTargetMachine, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToMemoryBuffer, LLVMTargetMachineRef, LLVMTargetRef,
};
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
use llvm_sys::{LLVMIntPredicate, LLVMOpcode, LLVMRealPredicate};

use hirsel_lir::layout_types::TargetSpec;

use crate::LlvmVersion;

#[cfg(test)]
mod count_tests;
mod masks;

/// The version of the libLLVM the loader resolved for this process.
pub(crate) fn loaded_version() -> LlvmVersion {
    let (mut major, mut minor, mut patch) = (0, 0, 0);
    // SAFETY: LLVMGetVersion only writes the three integers.
    unsafe { LLVMGetVersion(&mut major, &mut minor, &mut patch) };
    LlvmVersion {
        major,
        minor,
        patch,
    }
}

/// Registers the X86 and AArch64 code generators with LLVM, once per process.
fn initialize_targets() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // SAFETY: target registration has no preconditions; OnceLock runs it once.
        unsafe {
            LLVMInitializeX86TargetInfo();
            LLVMInitializeX86Target();
            LLVMInitializeX86TargetMC();
            LLVMInitializeX86AsmPrinter();
            LLVMInitializeAArch64TargetInfo();
            LLVMInitializeAArch64Target();
            LLVMInitializeAArch64TargetMC();
            LLVMInitializeAArch64AsmPrinter();
        }
    });
}

/// Copies an LLVM-allocated message and frees it. A null message is empty.
///
/// # Safety
/// `message` is null or a string LLVM allocated for the caller to dispose of.
unsafe fn take_message(message: *mut c_char) -> String {
    if message.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a live NUL-terminated string that it owns.
    let text = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: as above; ownership ends here.
    unsafe { LLVMDisposeMessage(message) };
    text
}

/// Copies a string LLVM still owns.
///
/// # Safety
/// `text` is null or a live NUL-terminated string.
unsafe fn borrowed_string(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a live NUL-terminated string.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// Interior NULs never occur in the names and pass pipelines this crate builds: symbol names
/// are mangled to `[A-Za-z0-9_$]` and every other string is a literal of this crate.
fn c_string(text: &str) -> CString {
    CString::new(text).expect("LLVM strings built by hirsel-llvm contain no NUL")
}

/// The empty instruction name.
const NO_NAME: *const c_char = c"".as_ptr();

/// An owned `LLVMContext`.
pub(crate) struct Context {
    raw: LLVMContextRef,
}

impl Context {
    pub(crate) fn new() -> Self {
        // SAFETY: creating a context has no preconditions.
        Self {
            raw: unsafe { LLVMContextCreate() },
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context is live, and every module borrowing it has been dropped.
        unsafe { LLVMContextDispose(self.raw) };
    }
}

/// An LLVM type of a module's context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Type<'m> {
    raw: LLVMTypeRef,
    _module: PhantomData<&'m ()>,
}

/// An LLVM value (constant, instruction, argument or function) of a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Value<'m> {
    raw: LLVMValueRef,
    _module: PhantomData<&'m ()>,
}

/// A basic block of a function of a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BasicBlock<'m> {
    raw: LLVMBasicBlockRef,
    _module: PhantomData<&'m ()>,
}

/// A function and its function type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Function<'m> {
    pub value: Value<'m>,
    pub ty: Type<'m>,
}

fn ty<'m>(raw: LLVMTypeRef) -> Type<'m> {
    Type {
        raw,
        _module: PhantomData,
    }
}

fn value<'m>(raw: LLVMValueRef) -> Value<'m> {
    Value {
        raw,
        _module: PhantomData,
    }
}

fn block<'m>(raw: LLVMBasicBlockRef) -> BasicBlock<'m> {
    BasicBlock {
        raw,
        _module: PhantomData,
    }
}

/// A parameter or return attribute this crate places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Attribute {
    ZeroExt,
    SignExt,
    /// A function attribute: prefer inlining calls of it.
    InlineHint,
    AlwaysInline,
    Cold,
    NoInline,
    NoRedZone,
    NoUnwind,
    WillReturn,
    CapturesNone,
    WriteOnly,
    ReadOnly,
    Memory(MemoryEffects),
    /// A return or parameter attribute: the pointer has exclusive access to its object.
    NoAlias,
    /// The parameter addresses at least this many live bytes.
    Dereferenceable(u64),
    /// The parameter's address has this byte alignment.
    Align(u32),
    /// A function attribute: an allocator entry of [`ALLOC_FAMILY`].
    AllocKind(AllocKind),
    /// A parameter attribute: the block a [`AllocKind::Free`] entry returns.
    AllocPtr,
    /// A function attribute naming [`ALLOC_FAMILY`], which pairs allocation with free.
    AllocFamily,
}

/// The allocator family of the runtime's heap blocks: LLVM may delete an allocation
/// whose only uses are stores into it and a free of this family, as C's malloc/free.
pub(crate) const ALLOC_FAMILY: &str = "hirsel_rt_v2";

/// LLVM 23.1.4's `AllocFnKind` bits for the entries the runtime has. Neither returns
/// uninitialized or zeroed memory: the runtime writes the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AllocKind {
    Alloc,
    Free,
}

/// LLVM 23.1.4's ModRef.h encodes two bits per location, in order:
/// argument, inaccessible, errno, other, and two target-state locations.
/// Keep the pinned C API encoding here (IRMemLocation in llvm/Support/ModRef.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryEffects {
    None,
    Read,
    ArgumentRead,
    ArgumentReadWrite,
    ReadWrite,
}

impl MemoryEffects {
    fn bits(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Read => 0b0101_0101_0101,
            Self::ArgumentRead => 1,
            Self::ArgumentReadWrite => 3,
            Self::ReadWrite => 0b1111_1111_1111,
        }
    }
}

/// Disjoint storage classes, not source scalar types. A sequence header's
/// length and capacity precede its elements; the count precedes its payload.
/// An open flat field's length prefix precedes its bytes, and a sequential
/// builder's used-byte word precedes every element. Representation-changing
/// copies and stores stay unclassified.
/// Validated scalar accesses within proved payloads cannot name either header.
/// Copies and arbitrary runtime calls stay unclassified and may alias every class.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Access {
    Count,
    Length,
    Capacity,
    Used,
    Value,
    /// The task's LANG-10 counters (runtime ABI 65); only runtime calls write them.
    Depth,
}

impl Access {
    fn name(self) -> &'static str {
        match self {
            Self::Count => "holder count",
            Self::Length => "sequence length",
            Self::Capacity => "sequence capacity",
            Self::Used => "builder used bytes",
            Self::Value => "scalar value",
            Self::Depth => "invocation depth",
        }
    }
}

impl Attribute {
    fn name(self) -> &'static str {
        match self {
            Attribute::ZeroExt => "zeroext",
            Attribute::SignExt => "signext",
            Attribute::InlineHint => "inlinehint",
            Attribute::AlwaysInline => "alwaysinline",
            Attribute::Cold => "cold",
            Attribute::NoInline => "noinline",
            Attribute::NoRedZone => "noredzone",
            Attribute::NoUnwind => "nounwind",
            Attribute::WillReturn => "willreturn",
            Attribute::CapturesNone => "captures",
            Attribute::WriteOnly => "writeonly",
            Attribute::ReadOnly => "readonly",
            Attribute::Memory(_) => "memory",
            Attribute::NoAlias => "noalias",
            Attribute::Dereferenceable(_) => "dereferenceable",
            Attribute::Align(_) => "align",
            Attribute::AllocKind(_) => "allockind",
            Attribute::AllocPtr => "allocptr",
            Attribute::AllocFamily => "alloc-family",
        }
    }
}

/// Where an attribute applies: the return value or a parameter (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttributeSlot {
    Return,
    Parameter(u32),
    /// The function itself.
    Function,
}

impl AttributeSlot {
    fn index(self) -> c_uint {
        match self {
            AttributeSlot::Return => llvm_sys::LLVMAttributeReturnIndex,
            AttributeSlot::Parameter(i) => i + 1,
            AttributeSlot::Function => llvm_sys::LLVMAttributeFunctionIndex,
        }
    }
}

/// An owned `LLVMModule` within a borrowed `Context`.
pub(crate) struct Module<'context> {
    raw: LLVMModuleRef,
    context: &'context Context,
}

impl<'context> Module<'context> {
    /// An empty module.
    pub(crate) fn new(context: &'context Context, name: &str) -> Self {
        let name = c_string(name);
        // SAFETY: the context is live; LLVM copies the name.
        let raw = unsafe { LLVMModuleCreateWithNameInContext(name.as_ptr(), context.raw) };
        Self { raw, context }
    }

    /// Parses textual IR. The parser's diagnostic is the error.
    pub(crate) fn parse(
        context: &'context Context,
        name: &str,
        text: &str,
    ) -> Result<Self, String> {
        let name = c_string(name);
        // SAFETY: the range is `text`'s bytes; LLVM copies them and appends a NUL.
        let buffer = unsafe {
            LLVMCreateMemoryBufferWithMemoryRangeCopy(
                text.as_ptr().cast(),
                text.len(),
                name.as_ptr(),
            )
        };
        let mut raw = ptr::null_mut();
        let mut message = ptr::null_mut();
        // SAFETY: the context and buffer are live. The parser borrows the buffer;
        // dispose of our copy after parsing on either outcome (LLVM23 IRReader.h).
        let failed = unsafe {
            let failed = LLVMParseIRInContext2(context.raw, buffer, &mut raw, &mut message);
            LLVMDisposeMemoryBuffer(buffer);
            failed
        };
        // SAFETY: on either outcome the message is null or ours to dispose of.
        let message = unsafe { take_message(message) };
        if failed != 0 || raw.is_null() {
            return Err(message);
        }
        Ok(Self { raw, context })
    }

    pub(crate) fn triple(&self) -> String {
        // SAFETY: the module is live; LLVM owns the returned string.
        unsafe { borrowed_string(LLVMGetTarget(self.raw)) }
    }

    pub(crate) fn data_layout(&self) -> String {
        // SAFETY: the module is live; LLVM owns the returned string.
        unsafe { borrowed_string(LLVMGetDataLayoutStr(self.raw)) }
    }

    /// Sets the module's triple and data layout to the machine's.
    pub(crate) fn set_target(&self, machine: &TargetMachine) {
        let triple = c_string(&machine.triple);
        // SAFETY: the module and machine are live; LLVM copies the triple and the layout,
        // and the target data is disposed of after it is copied.
        unsafe {
            LLVMSetTarget(self.raw, triple.as_ptr());
            let data = LLVMCreateTargetDataLayout(machine.raw);
            LLVMSetModuleDataLayout(self.raw, data);
            LLVMDisposeTargetData(data);
        }
    }

    /// Runs the LLVM verifier. The verifier's report is the error.
    pub(crate) fn verify(&self) -> Result<(), String> {
        let mut message = ptr::null_mut();
        // SAFETY: the module is live; the verifier only reads it and writes the message.
        let failed = unsafe {
            LLVMVerifyModule(
                self.raw,
                LLVMVerifierFailureAction::LLVMReturnStatusAction,
                &mut message,
            )
        };
        // SAFETY: the message is null or ours to dispose of.
        let message = unsafe { take_message(message) };
        if failed != 0 { Err(message) } else { Ok(()) }
    }

    /// Runs a new-pass-manager pipeline such as `default<O3>`.
    pub(crate) fn run_passes(&self, pipeline: &str, machine: &TargetMachine) -> Result<(), String> {
        if matches!(pipeline, "default<O2>" | "default<O3>") {
            unsafe extern "C" {
                fn hirsel_llvm_count_pipeline(
                    module: *mut std::ffi::c_void,
                    machine: *mut std::ffi::c_void,
                    level: u32,
                );
            }
            // SAFETY: both handles are live. The bridge constructs LLVM's
            // standard pipeline with a guarded-count pass at VectorizerStart.
            unsafe {
                hirsel_llvm_count_pipeline(
                    self.raw.cast(),
                    machine.raw.cast(),
                    if pipeline == "default<O3>" { 3 } else { 2 },
                );
            }
            return Ok(());
        }
        let pipeline = c_string(pipeline);
        // SAFETY: the module and machine are live; the options are created and disposed of
        // here, and a returned error is consumed by LLVMGetErrorMessage.
        unsafe {
            let options = LLVMCreatePassBuilderOptions();
            let error = LLVMRunPasses(self.raw, pipeline.as_ptr(), machine.raw, options);
            LLVMDisposePassBuilderOptions(options);
            if error.is_null() {
                return Ok(());
            }
            let message = LLVMGetErrorMessage(error);
            let text = borrowed_string(message);
            LLVMDisposeErrorMessage(message);
            Err(text)
        }
    }

    /// Generates an object file and copies its bytes out of LLVM.
    pub(crate) fn emit_object(&self, machine: &TargetMachine) -> Result<Vec<u8>, String> {
        let mut message = ptr::null_mut();
        let mut buffer: LLVMMemoryBufferRef = ptr::null_mut();
        // SAFETY: the module and machine are live; LLVM writes either an error message or a
        // buffer that we own.
        let failed = unsafe {
            LLVMTargetMachineEmitToMemoryBuffer(
                machine.raw,
                self.raw,
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut message,
                &mut buffer,
            )
        };
        // SAFETY: the message is null or ours to dispose of.
        let message = unsafe { take_message(message) };
        if failed != 0 || buffer.is_null() {
            return Err(message);
        }
        // SAFETY: the buffer is live and owned here; its bytes are copied before disposal.
        unsafe {
            let start = LLVMGetBufferStart(buffer).cast::<u8>();
            let bytes = std::slice::from_raw_parts(start, LLVMGetBufferSize(buffer)).to_vec();
            LLVMDisposeMemoryBuffer(buffer);
            Ok(bytes)
        }
    }

    // Types.

    pub(crate) fn int_type(&self, bits: u32) -> Type<'_> {
        // SAFETY: the context is live; types are owned by it.
        ty(unsafe { LLVMIntTypeInContext(self.context.raw, bits) })
    }

    pub(crate) fn f32_type(&self) -> Type<'_> {
        // SAFETY: as above.
        ty(unsafe { LLVMFloatTypeInContext(self.context.raw) })
    }

    pub(crate) fn f64_type(&self) -> Type<'_> {
        // SAFETY: as above.
        ty(unsafe { LLVMDoubleTypeInContext(self.context.raw) })
    }

    /// The opaque pointer type of address space 0.
    pub(crate) fn ptr_type(&self) -> Type<'_> {
        // SAFETY: as above.
        ty(unsafe { LLVMPointerTypeInContext(self.context.raw, 0) })
    }

    pub(crate) fn void_type(&self) -> Type<'_> {
        // SAFETY: as above.
        ty(unsafe { LLVMVoidTypeInContext(self.context.raw) })
    }

    pub(crate) fn array_type<'m>(&'m self, element: Type<'m>, count: u64) -> Type<'m> {
        // SAFETY: `element` belongs to this module's context.
        ty(unsafe { LLVMArrayType2(element.raw, count) })
    }

    pub(crate) fn struct_type<'m>(&'m self, fields: &[Type<'m>]) -> Type<'m> {
        let mut raw: Vec<_> = fields.iter().map(|t| t.raw).collect();
        // SAFETY: the field types belong to this context; LLVM copies the array.
        ty(unsafe {
            LLVMStructTypeInContext(self.context.raw, raw.as_mut_ptr(), raw.len() as c_uint, 0)
        })
    }

    pub(crate) fn function_type<'m>(
        &'m self,
        result: Type<'m>,
        parameters: &[Type<'m>],
    ) -> Type<'m> {
        let mut raw: Vec<_> = parameters.iter().map(|t| t.raw).collect();
        // SAFETY: the types belong to this context; LLVM copies the array.
        ty(unsafe { LLVMFunctionType(result.raw, raw.as_mut_ptr(), raw.len() as c_uint, 0) })
    }

    // Functions and globals.

    /// The function named `name`, declared with `ty` if the module has none. A function of
    /// that name with another type is an error.
    pub(crate) fn function<'m>(
        &'m self,
        name: &str,
        fn_ty: Type<'m>,
    ) -> Result<Function<'m>, String> {
        let c_name = c_string(name);
        // SAFETY: the module is live and the name NUL-terminated.
        let existing = unsafe { LLVMGetNamedFunction(self.raw, c_name.as_ptr()) };
        if !existing.is_null() {
            // SAFETY: `existing` is a live function of this module.
            let existing_ty = unsafe { LLVMGlobalGetValueType(existing) };
            if existing_ty != fn_ty.raw {
                // SAFETY: both types belong to this live context; LLVM allocates
                // each printed message, which take_message disposes.
                let (existing, requested) = unsafe {
                    (
                        take_message(LLVMPrintTypeToString(existing_ty)),
                        take_message(LLVMPrintTypeToString(fn_ty.raw)),
                    )
                };
                return Err(format!(
                    "`{name}` is declared with two different types: {existing}; requested {requested}"
                ));
            }
            return Ok(Function {
                value: value(existing),
                ty: fn_ty,
            });
        }
        // SAFETY: as above; the function type belongs to this context.
        let raw = unsafe { LLVMAddFunction(self.raw, c_name.as_ptr(), fn_ty.raw) };
        let function = Function {
            value: value(raw),
            ty: fn_ty,
        };
        if name.starts_with("hirsel_rt_v2_") {
            crate::runtime::declaration(self, function);
        }
        Ok(function)
    }

    /// The address of the function `name`: the module's function of that name whatever its
    /// type, or a new declaration (an import whose type only its callers know).
    pub(crate) fn function_address(&self, name: &str) -> Value<'_> {
        let c_name = c_string(name);
        // SAFETY: the module is live and the name NUL-terminated.
        let existing = unsafe { LLVMGetNamedFunction(self.raw, c_name.as_ptr()) };
        if !existing.is_null() {
            return value(existing);
        }
        let fn_ty = self.function_type(self.int_type(8), &[]);
        // SAFETY: as above; the function type belongs to this context.
        let function = Function {
            value: value(unsafe { LLVMAddFunction(self.raw, c_name.as_ptr(), fn_ty.raw) }),
            ty: fn_ty,
        };
        if name.starts_with("hirsel_rt_v2_") {
            crate::runtime::declaration(self, function);
        }
        function.value
    }

    /// Whether the module has a function body named `name`.
    pub(crate) fn declares(&self, name: &str) -> bool {
        let name = CString::new(name).expect("symbol without NUL");
        !unsafe { LLVMGetNamedFunction(self.raw, name.as_ptr()) }.is_null()
    }

    pub(crate) fn defines(&self, name: &str) -> bool {
        let c_name = c_string(name);
        // SAFETY: the module is live.
        let existing = unsafe { LLVMGetNamedFunction(self.raw, c_name.as_ptr()) };
        // SAFETY: `existing` is null or a live function.
        !existing.is_null() && unsafe { LLVMIsDeclaration(existing) } == 0
    }

    /// A new private, unnamed-address constant global of `ty` aligned to `align`, without an
    /// initializer yet, so constants may refer to each other before any is complete.
    pub(crate) fn private_constant<'m>(
        &'m self,
        name: &str,
        of: Type<'m>,
        align: u32,
    ) -> Value<'m> {
        let c_name = c_string(name);
        // SAFETY: the module is live, the name NUL-terminated, the type of this context.
        let raw = unsafe { LLVMAddGlobal(self.raw, of.raw, c_name.as_ptr()) };
        // SAFETY: `raw` is a live global of this module.
        unsafe {
            LLVMSetLinkage(raw, llvm_sys::LLVMLinkage::LLVMPrivateLinkage);
            LLVMSetGlobalConstant(raw, 1);
            LLVMSetUnnamedAddress(raw, llvm_sys::LLVMUnnamedAddr::LLVMGlobalUnnamedAddr);
            LLVMSetAlignment(raw, align);
        }
        value(raw)
    }

    /// One address-bearing constant per digest across the linked program.
    /// Its address is identity, so LLVM may not merge it as unnamed data.
    pub(crate) fn linkonce_constant<'m>(
        &'m self,
        name: &str,
        of: Type<'m>,
        align: u32,
    ) -> Value<'m> {
        let global = self.private_constant(name, of, align);
        // SAFETY: the constant belongs to this live module.
        unsafe {
            LLVMSetLinkage(global.raw, llvm_sys::LLVMLinkage::LLVMLinkOnceODRLinkage);
            LLVMSetUnnamedAddress(global.raw, llvm_sys::LLVMUnnamedAddr::LLVMNoUnnamedAddr);
        }
        global
    }

    /// An exported constant global of `ty` aligned to `align`, without an initializer yet:
    /// a glue cell or block other objects read by name.
    pub(crate) fn exported_constant<'m>(
        &'m self,
        name: &str,
        of: Type<'m>,
        align: u32,
    ) -> Value<'m> {
        let global = self.private_constant(name, of, align);
        // SAFETY: the constant belongs to this live module.
        unsafe {
            LLVMSetLinkage(global.raw, llvm_sys::LLVMLinkage::LLVMExternalLinkage);
            LLVMSetUnnamedAddress(global.raw, llvm_sys::LLVMUnnamedAddr::LLVMNoUnnamedAddr);
        }
        global
    }

    /// Keeps `functions` in the object though nothing in it refers to them
    /// (`llvm.compiler.used`).
    pub(crate) fn keep(&self, functions: &[Value<'_>]) {
        if functions.is_empty() {
            return;
        }
        let ptr = self.ptr_type();
        let ty = self.array_type(ptr, functions.len() as u64);
        let c_name = c_string("llvm.compiler.used");
        // SAFETY: the module is live, the name NUL-terminated, the type of this context.
        let raw = unsafe { LLVMAddGlobal(self.raw, ty.raw, c_name.as_ptr()) };
        let section = c_string("llvm.metadata");
        // SAFETY: `raw` is a live global of this module and the initializer a constant of
        // its type.
        unsafe {
            LLVMSetLinkage(raw, llvm_sys::LLVMLinkage::LLVMAppendingLinkage);
            LLVMSetSection(raw, section.as_ptr());
            LLVMSetInitializer(raw, self.const_array(ptr, functions).raw);
        }
    }

    /// Defines the hidden absolute symbol `name` of value `value`: an alias of a constant
    /// address, which the object writer emits as `name = value`.
    pub(crate) fn absolute_symbol(&self, name: &str, value: u64) {
        let c_name = c_string(name);
        let address = self.const_int(self.int_type(64), u128::from(value));
        // SAFETY: the module is live, the name NUL-terminated, the aliasee a constant of
        // this context.
        unsafe {
            let aliasee = LLVMConstIntToPtr(address.raw, self.ptr_type().raw);
            let raw = LLVMAddAlias2(self.raw, self.int_type(8).raw, 0, aliasee, c_name.as_ptr());
            LLVMSetLinkage(raw, llvm_sys::LLVMLinkage::LLVMExternalLinkage);
            LLVMSetVisibility(raw, llvm_sys::LLVMVisibility::LLVMHiddenVisibility);
        }
    }

    /// The constant `address` as an integer of `of`, truncated.
    pub(crate) fn const_ptr_to_int<'m>(&'m self, address: Value<'m>, of: Type<'m>) -> Value<'m> {
        // SAFETY: `address` is a constant pointer of this context, `of` an integer type.
        value(unsafe { LLVMConstPtrToInt(address.raw, of.raw) })
    }

    /// The constant `name` another object defines, with its `initializer` so the optimizer
    /// folds reads of it; this object emits no definition (`available_externally`).
    pub(crate) fn available_constant<'m>(
        &'m self,
        name: &str,
        of: Type<'m>,
        align: u32,
        initializer: Value<'m>,
    ) -> Value<'m> {
        if let Some(global) = self.named_global(name) {
            return global;
        }
        let c_name = c_string(name);
        // SAFETY: the module is live, the name NUL-terminated, the type of this context.
        let raw = unsafe { LLVMAddGlobal(self.raw, of.raw, c_name.as_ptr()) };
        // SAFETY: `raw` is a live global of this module, the initializer a constant of `of`.
        unsafe {
            LLVMSetLinkage(raw, llvm_sys::LLVMLinkage::LLVMAvailableExternallyLinkage);
            LLVMSetGlobalConstant(raw, 1);
            LLVMSetAlignment(raw, align);
            LLVMSetInitializer(raw, initializer.raw);
        }
        value(raw)
    }

    /// The constant global `name` another object defines (a glue cell or block), declared
    /// once as `ty`.
    pub(crate) fn external_constant<'m>(&'m self, name: &str, of: Type<'m>) -> Value<'m> {
        if let Some(global) = self.named_global(name) {
            return global;
        }
        let c_name = c_string(name);
        // SAFETY: the module is live, the name NUL-terminated, the type of this context.
        let raw = unsafe { LLVMAddGlobal(self.raw, of.raw, c_name.as_ptr()) };
        // SAFETY: `raw` is a live global of this module.
        unsafe {
            LLVMSetLinkage(raw, llvm_sys::LLVMLinkage::LLVMExternalLinkage);
            LLVMSetGlobalConstant(raw, 1);
        }
        value(raw)
    }

    /// A program registry survives until the separately compiled launcher refers to it.
    pub(crate) fn registry_constant<'m>(&'m self, name: &str, of: Type<'m>) -> Value<'m> {
        let global = self.linkonce_constant(name, of, 8);
        unsafe {
            LLVMSetLinkage(global.raw, llvm_sys::LLVMLinkage::LLVMWeakODRLinkage);
        }
        global
    }

    pub(crate) fn const_offset(&self, address: Value<'_>, offset: u64) -> Value<'_> {
        let index = self.const_int(self.int_type(64), offset.into());
        unsafe {
            value(LLVMConstGEP2(
                self.int_type(8).raw,
                address.raw,
                [index.raw].as_mut_ptr(),
                1,
            ))
        }
    }
    pub(crate) fn const_or(&self, left: Value<'_>, right: Value<'_>) -> Value<'_> {
        unsafe { value(LLVMConstXor(left.raw, right.raw)) }
    }

    /// Gives a function private linkage: a helper of this object only (a thunk, a drop
    /// wrapper or a comparator), never exported.
    pub(crate) fn make_private(&self, function: Function<'_>) {
        // SAFETY: `function` is a live function of this module.
        unsafe {
            LLVMSetLinkage(
                function.value.raw,
                llvm_sys::LLVMLinkage::LLVMPrivateLinkage,
            )
        };
    }

    /// Preserve object-local function identities for final stack-size relocations.
    /// LLVM private linkage can omit the symbol entirely; internal linkage retains
    /// a local STT_FUNC without adding an export or authorizing a private call.
    pub(crate) fn retain_local_function_symbols(&self) {
        // SAFETY: this module owns each function throughout the walk. Linkage does
        // not change the function list, and only object-local definitions change.
        unsafe {
            let mut function = LLVMGetFirstFunction(self.raw);
            while !function.is_null() {
                if LLVMGetLinkage(function) == llvm_sys::LLVMLinkage::LLVMPrivateLinkage {
                    LLVMSetLinkage(function, llvm_sys::LLVMLinkage::LLVMInternalLinkage);
                }
                function = LLVMGetNextFunction(function);
            }
        }
    }

    /// Retain a template's out-of-line implementation in its own object even when
    /// this module inlines every local call. Other programs may share this object.
    pub(crate) fn make_weak_odr(&self, function: Function<'_>) {
        // SAFETY: `function` is a live function of this module.
        unsafe {
            LLVMSetLinkage(
                function.value.raw,
                llvm_sys::LLVMLinkage::LLVMWeakODRLinkage,
            )
        };
    }

    /// Gives a function `available_externally` linkage: another object implements it, and
    /// this copy only lets the optimizer inline it here. It never reaches the object file;
    /// a call the optimizer keeps stays an import of the implementing object's export.
    pub(crate) fn make_available_externally(&self, function: Function<'_>) {
        // SAFETY: `function` is a live function of this module.
        unsafe {
            LLVMSetLinkage(
                function.value.raw,
                llvm_sys::LLVMLinkage::LLVMAvailableExternallyLinkage,
            )
        };
    }

    /// A constant array of bytes.
    pub(crate) fn const_bytes(&self, bytes: &[u8]) -> Value<'_> {
        let i8_ty = self.int_type(8);
        let items: Vec<_> = bytes
            .iter()
            .map(|&b| self.const_int(i8_ty, u128::from(b)))
            .collect();
        self.const_array(i8_ty, &items)
    }

    /// The global variable named `name`, if the module has one.
    pub(crate) fn named_global(&self, name: &str) -> Option<Value<'_>> {
        let c_name = c_string(name);
        // SAFETY: the module is live and the name NUL-terminated.
        let raw = unsafe { LLVMGetNamedGlobal(self.raw, c_name.as_ptr()) };
        (!raw.is_null()).then(|| value(raw))
    }

    /// Completes a global made by `private_constant`.
    pub(crate) fn set_initializer<'m>(&'m self, global: Value<'m>, initializer: Value<'m>) {
        // SAFETY: `global` is a global of this module and `initializer` a constant of its type.
        unsafe { LLVMSetInitializer(global.raw, initializer.raw) };
    }

    /// The literal packed struct type of `fields`: no padding between them.
    pub(crate) fn packed_struct_type<'m>(&'m self, fields: &[Type<'m>]) -> Type<'m> {
        let mut raw: Vec<_> = fields.iter().map(|t| t.raw).collect();
        // SAFETY: the field types belong to this context; LLVM copies the array.
        ty(unsafe {
            LLVMStructTypeInContext(self.context.raw, raw.as_mut_ptr(), raw.len() as c_uint, 1)
        })
    }

    /// A constant of the literal packed struct type of `fields`.
    pub(crate) fn const_packed_struct<'m>(&'m self, fields: &[Value<'m>]) -> Value<'m> {
        let mut raw: Vec<_> = fields.iter().map(|v| v.raw).collect();
        // SAFETY: the fields are constants of this context; LLVM copies the array.
        value(unsafe {
            LLVMConstStructInContext(self.context.raw, raw.as_mut_ptr(), raw.len() as c_uint, 1)
        })
    }

    /// A constant of the literal (unpacked) struct type of `fields`.
    pub(crate) fn const_struct<'m>(&'m self, fields: &[Value<'m>]) -> Value<'m> {
        let mut raw: Vec<_> = fields.iter().map(|v| v.raw).collect();
        // SAFETY: the fields are constants of this context; LLVM copies the array.
        value(unsafe {
            LLVMConstStructInContext(self.context.raw, raw.as_mut_ptr(), raw.len() as c_uint, 0)
        })
    }

    /// A constant array of `element` values.
    pub(crate) fn const_array<'m>(&'m self, element: Type<'m>, items: &[Value<'m>]) -> Value<'m> {
        let mut raw: Vec<_> = items.iter().map(|v| v.raw).collect();
        // SAFETY: the items are constants of `element`; LLVM copies the array.
        value(unsafe { LLVMConstArray2(element.raw, raw.as_mut_ptr(), raw.len() as u64) })
    }

    /// The declaration of an overloaded intrinsic such as `llvm.sadd.with.overflow`.
    pub(crate) fn intrinsic<'m>(
        &'m self,
        name: &str,
        overloads: &[Type<'m>],
    ) -> Result<Function<'m>, String> {
        // SAFETY: the name is read for its length only.
        let id = unsafe { LLVMLookupIntrinsicID(name.as_ptr().cast(), name.len()) };
        if id == 0 {
            return Err(format!("LLVM has no intrinsic `{name}`"));
        }
        let mut raw: Vec<_> = overloads.iter().map(|t| t.raw).collect();
        // SAFETY: the module is live and the overload types belong to its context.
        unsafe {
            let function = LLVMGetIntrinsicDeclaration(self.raw, id, raw.as_mut_ptr(), raw.len());
            let fn_ty = LLVMIntrinsicGetType(self.context.raw, id, raw.as_mut_ptr(), raw.len());
            Ok(Function {
                value: value(function),
                ty: ty(fn_ty),
            })
        }
    }

    pub(crate) fn parameter<'m>(&'m self, function: Function<'m>, index: u32) -> Value<'m> {
        // SAFETY: the caller passes an index below the function's arity.
        value(unsafe { LLVMGetParam(function.value.raw, index) })
    }

    pub(crate) fn string_attribute(&self, function: Function<'_>, name: &str, value: &str) {
        // SAFETY: LLVM copies both strings; the function belongs to this module.
        unsafe {
            let attribute = LLVMCreateStringAttribute(
                self.context.raw,
                name.as_ptr().cast(),
                name.len() as u32,
                value.as_ptr().cast(),
                value.len() as u32,
            );
            LLVMAddAttributeAtIndex(
                function.value.raw,
                llvm_sys::LLVMAttributeFunctionIndex,
                attribute,
            );
        }
    }

    fn attribute(&self, attribute: Attribute) -> llvm_sys::prelude::LLVMAttributeRef {
        let name = attribute.name();
        if attribute == Attribute::AllocFamily {
            // SAFETY: both strings are read for their lengths; LLVM copies them.
            return unsafe {
                LLVMCreateStringAttribute(
                    self.context.raw,
                    name.as_ptr().cast(),
                    name.len() as c_uint,
                    ALLOC_FAMILY.as_ptr().cast(),
                    ALLOC_FAMILY.len() as c_uint,
                )
            };
        }
        // SAFETY: the name is read for its length; the context is live.
        unsafe {
            let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
            assert_ne!(kind, 0, "the pinned LLVM knows {name}");
            let bits = match attribute {
                Attribute::Memory(effects) => effects.bits(),
                Attribute::Dereferenceable(bytes) => bytes,
                Attribute::Align(align) => u64::from(align),
                Attribute::AllocKind(AllocKind::Alloc) => 1,
                Attribute::AllocKind(AllocKind::Free) => 1 << 2,
                _ => 0,
            };
            LLVMCreateEnumAttribute(self.context.raw, kind, bits)
        }
    }

    pub(crate) fn add_attribute(
        &self,
        function: Function<'_>,
        slot: AttributeSlot,
        attribute: Attribute,
    ) {
        let attribute = self.attribute(attribute);
        // SAFETY: the function belongs to this module.
        unsafe { LLVMAddAttributeAtIndex(function.value.raw, slot.index(), attribute) };
    }

    /// Removes an enum `attribute` (its kind, whatever its value) from `function`'s `slot`.
    pub(crate) fn remove_attribute(
        &self,
        function: Function<'_>,
        slot: AttributeSlot,
        attribute: Attribute,
    ) {
        let name = attribute.name();
        // SAFETY: the name is read for its length; the function belongs to this module.
        unsafe {
            let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
            LLVMRemoveEnumAttributeAtIndex(function.value.raw, slot.index(), kind);
        }
    }

    /// The module's functions whose names start with `prefix`.
    pub(crate) fn functions_named(&self, prefix: &str) -> Vec<Function<'_>> {
        let mut found = Vec::new();
        // SAFETY: the module is live; each function is walked while it is.
        unsafe {
            let mut raw = LLVMGetFirstFunction(self.raw);
            while !raw.is_null() {
                let mut length = 0;
                let name = LLVMGetValueName2(raw, &mut length);
                let name = std::slice::from_raw_parts(name.cast::<u8>(), length);
                if name.starts_with(prefix.as_bytes()) {
                    found.push(Function {
                        value: value(raw),
                        ty: ty(LLVMGlobalGetValueType(raw)),
                    });
                }
                raw = LLVMGetNextFunction(raw);
            }
        }
        found
    }

    pub(crate) fn add_call_attribute(
        &self,
        call: Value<'_>,
        slot: AttributeSlot,
        attribute: Attribute,
    ) {
        let attribute = self.attribute(attribute);
        // SAFETY: `call` is a call instruction of this module.
        unsafe { LLVMAddCallSiteAttribute(call.raw, slot.index(), attribute) };
    }

    /// The scalar TBAA siblings describe storage that cannot overlap in
    /// validated LIR. These nodes are context-uniqued; nothing is marked constant
    /// (SeqSetLen, Dup and Release must invalidate their own class normally).
    fn access(&self, instruction: Value<'_>, access: Access) {
        // SAFETY: metadata and instruction share this context. The value is a
        // load/store instruction; scalar descriptors and the access tag follow
        // LangRef's struct-path TBAA format, with offset zero.
        unsafe {
            let context = self.context.raw;
            let string = |s: &str| LLVMMDStringInContext2(context, s.as_ptr().cast(), s.len());
            let node = |mut operands: Vec<_>| {
                LLVMMDNodeInContext2(context, operands.as_mut_ptr(), operands.len())
            };
            let root = node(vec![string("Hirsel storage v1")]);
            let zero = LLVMValueAsMetadata(self.const_int(self.int_type(64), 0).raw);
            let scalar = node(vec![string(access.name()), root, zero]);
            let tag = node(vec![scalar, scalar, zero]);
            let kind = LLVMGetMDKindIDInContext(context, c"tbaa".as_ptr(), 4);
            LLVMSetMetadata(instruction.raw, kind, LLVMMetadataAsValue(context, tag));
        }
    }

    pub(crate) fn text(&self) -> String {
        // SAFETY: LLVM returns an owned message for this live module.
        unsafe { take_message(LLVMPrintModuleToString(self.raw)) }
    }

    pub(crate) fn append_block<'m>(&'m self, function: Function<'m>) -> BasicBlock<'m> {
        // SAFETY: the function belongs to this module's context.
        block(unsafe {
            LLVMAppendBasicBlockInContext(self.context.raw, function.value.raw, NO_NAME)
        })
    }

    // Constants.

    /// An integer constant of `ty` from the low bits of `bits`.
    pub(crate) fn const_int<'m>(&'m self, int_ty: Type<'m>, bits: u128) -> Value<'m> {
        let words = [bits as u64, (bits >> 64) as u64];
        // SAFETY: `int_ty` is an integer type of this context; LLVM truncates to its width.
        value(unsafe { LLVMConstIntOfArbitraryPrecision(int_ty.raw, 2, words.as_ptr()) })
    }

    /// The all-zero value of `ty` (a null pointer for pointers).
    pub(crate) fn const_zero<'m>(&'m self, of: Type<'m>) -> Value<'m> {
        // SAFETY: `of` belongs to this context.
        value(unsafe { LLVMConstNull(of.raw) })
    }
}

impl Drop for Module<'_> {
    fn drop(&mut self) {
        // SAFETY: the module is live and owned; its context outlives it.
        unsafe { LLVMDisposeModule(self.raw) };
    }
}

/// Integer comparison predicates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntPredicate {
    Eq,
    Ne,
    Ult,
    Ule,
    Ugt,
    Uge,
    Slt,
    Sle,
    Sgt,
    Sge,
}

/// Float comparison predicates: ordered except `Une`, as IEEE `==`/`!=`/`<` require.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatPredicate {
    Oeq,
    Une,
    Olt,
    Ole,
    Ogt,
    Oge,
}

/// Two-operand instructions. None carries a fast-math, `nsw`, `nuw` or `exact` flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    UDiv,
    SDiv,
    URem,
    SRem,
    Shl,
    LShr,
    AShr,
    And,
    Or,
    Xor,
    FAdd,
    FSub,
    FMul,
    FDiv,
    FRem,
}

impl BinOp {
    fn opcode(self) -> LLVMOpcode {
        use LLVMOpcode as O;
        match self {
            BinOp::Add => O::LLVMAdd,
            BinOp::Sub => O::LLVMSub,
            BinOp::Mul => O::LLVMMul,
            BinOp::UDiv => O::LLVMUDiv,
            BinOp::SDiv => O::LLVMSDiv,
            BinOp::URem => O::LLVMURem,
            BinOp::SRem => O::LLVMSRem,
            BinOp::Shl => O::LLVMShl,
            BinOp::LShr => O::LLVMLShr,
            BinOp::AShr => O::LLVMAShr,
            BinOp::And => O::LLVMAnd,
            BinOp::Or => O::LLVMOr,
            BinOp::Xor => O::LLVMXor,
            BinOp::FAdd => O::LLVMFAdd,
            BinOp::FSub => O::LLVMFSub,
            BinOp::FMul => O::LLVMFMul,
            BinOp::FDiv => O::LLVMFDiv,
            BinOp::FRem => O::LLVMFRem,
        }
    }
}

/// Conversions between integer widths and between same-sized types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cast {
    ZExt,
    SExt,
    Trunc,
    /// f32 to f64, exact.
    FPExt,
    /// f64 to f32, rounded to nearest, ties to even.
    FPTrunc,
    SIToFP,
    UIToFP,
    /// Reinterpret the bits as another type of the same size.
    Bits,
    /// A pointer's address as an integer.
    PtrToInt,
    IntToPtr,
}

impl Cast {
    fn opcode(self) -> LLVMOpcode {
        match self {
            Cast::ZExt => LLVMOpcode::LLVMZExt,
            Cast::SExt => LLVMOpcode::LLVMSExt,
            Cast::Trunc => LLVMOpcode::LLVMTrunc,
            Cast::FPExt => LLVMOpcode::LLVMFPExt,
            Cast::FPTrunc => LLVMOpcode::LLVMFPTrunc,
            Cast::SIToFP => LLVMOpcode::LLVMSIToFP,
            Cast::UIToFP => LLVMOpcode::LLVMUIToFP,
            Cast::Bits => LLVMOpcode::LLVMBitCast,
            Cast::PtrToInt => LLVMOpcode::LLVMPtrToInt,
            Cast::IntToPtr => LLVMOpcode::LLVMIntToPtr,
        }
    }
}

/// An owned `LLVMBuilder` positioned in one module.
pub(crate) struct Builder<'m> {
    raw: LLVMBuilderRef,
    module: &'m Module<'m>,
    profile: Cell<Option<(Function<'m>, Function<'m>, Value<'m>, Value<'m>)>>,
}

impl<'m> Builder<'m> {
    pub(crate) fn new(module: &'m Module<'m>) -> Self {
        // SAFETY: the context is live.
        let raw = unsafe { LLVMCreateBuilderInContext(module.context.raw) };
        Self {
            raw,
            module,
            profile: Cell::new(None),
        }
    }

    pub(crate) fn module(&self) -> &'m Module<'m> {
        self.module
    }

    pub(crate) fn position(&self, at: BasicBlock<'m>) {
        // SAFETY: the block belongs to this module.
        unsafe { LLVMPositionBuilderAtEnd(self.raw, at.raw) };
    }

    /// The block the builder appends to.
    pub(crate) fn current(&self) -> BasicBlock<'m> {
        // SAFETY: the builder is positioned in a block of this module.
        block(unsafe { LLVMGetInsertBlock(self.raw) })
    }

    pub(crate) fn binary(&self, op: BinOp, left: Value<'m>, right: Value<'m>) -> Value<'m> {
        // SAFETY: both operands belong to this module and have the operation's type.
        value(unsafe { LLVMBuildBinOp(self.raw, op.opcode(), left.raw, right.raw, NO_NAME) })
    }

    pub(crate) fn fneg(&self, operand: Value<'m>) -> Value<'m> {
        // SAFETY: the operand is a float value of this module.
        value(unsafe { LLVMBuildFNeg(self.raw, operand.raw, NO_NAME) })
    }

    pub(crate) fn icmp(
        &self,
        predicate: IntPredicate,
        left: Value<'m>,
        right: Value<'m>,
    ) -> Value<'m> {
        use LLVMIntPredicate as P;
        let predicate = match predicate {
            IntPredicate::Eq => P::LLVMIntEQ,
            IntPredicate::Ne => P::LLVMIntNE,
            IntPredicate::Ult => P::LLVMIntULT,
            IntPredicate::Ule => P::LLVMIntULE,
            IntPredicate::Ugt => P::LLVMIntUGT,
            IntPredicate::Uge => P::LLVMIntUGE,
            IntPredicate::Slt => P::LLVMIntSLT,
            IntPredicate::Sle => P::LLVMIntSLE,
            IntPredicate::Sgt => P::LLVMIntSGT,
            IntPredicate::Sge => P::LLVMIntSGE,
        };
        // SAFETY: both operands are integers or pointers of one type in this module.
        value(unsafe { LLVMBuildICmp(self.raw, predicate, left.raw, right.raw, NO_NAME) })
    }

    pub(crate) fn fcmp(
        &self,
        predicate: FloatPredicate,
        left: Value<'m>,
        right: Value<'m>,
    ) -> Value<'m> {
        use LLVMRealPredicate as P;
        let predicate = match predicate {
            FloatPredicate::Oeq => P::LLVMRealOEQ,
            FloatPredicate::Une => P::LLVMRealUNE,
            FloatPredicate::Olt => P::LLVMRealOLT,
            FloatPredicate::Ole => P::LLVMRealOLE,
            FloatPredicate::Ogt => P::LLVMRealOGT,
            FloatPredicate::Oge => P::LLVMRealOGE,
        };
        // SAFETY: both operands are floats of one type in this module.
        value(unsafe { LLVMBuildFCmp(self.raw, predicate, left.raw, right.raw, NO_NAME) })
    }

    pub(crate) fn cast(&self, cast: Cast, operand: Value<'m>, to: Type<'m>) -> Value<'m> {
        // SAFETY: the operand and type belong to this module; the caller picks a valid cast.
        value(unsafe { LLVMBuildCast(self.raw, cast.opcode(), operand.raw, to.raw, NO_NAME) })
    }

    pub(crate) fn select(&self, condition: Value<'m>, yes: Value<'m>, no: Value<'m>) -> Value<'m> {
        // SAFETY: the condition is an i1 and both arms have one type.
        value(unsafe { LLVMBuildSelect(self.raw, condition.raw, yes.raw, no.raw, NO_NAME) })
    }

    /// A load of `of` from `address`, assuming only `align`.
    pub(crate) fn load(&self, of: Type<'m>, address: Value<'m>, align: u32) -> Value<'m> {
        // SAFETY: the address is a pointer of this module.
        unsafe {
            let load = LLVMBuildLoad2(self.raw, of.raw, address.raw, NO_NAME);
            LLVMSetAlignment(load, align);
            value(load)
        }
    }

    /// A store of `stored` to `address`, assuming only `align`.
    pub(crate) fn store(&self, stored: Value<'m>, address: Value<'m>, align: u32) {
        // SAFETY: the value and address belong to this module.
        unsafe {
            let store = LLVMBuildStore(self.raw, stored.raw, address.raw);
            LLVMSetAlignment(store, align);
        }
    }

    /// A monotonic (relaxed) atomic load of `of` from `address`, aligned to its width.
    pub(crate) fn load_relaxed(&self, of: Type<'m>, address: Value<'m>, align: u32) -> Value<'m> {
        // SAFETY: the address is a pointer of this module and `of` an integer type.
        unsafe {
            let load = LLVMBuildLoad2(self.raw, of.raw, address.raw, NO_NAME);
            LLVMSetAlignment(load, align);
            LLVMSetOrdering(
                load,
                llvm_sys::LLVMAtomicOrdering::LLVMAtomicOrderingMonotonic,
            );
            value(load)
        }
    }

    /// A monotonic compare-exchange of `expected` for `new` at `address`: whether it stored.
    pub(crate) fn cmpxchg_relaxed(
        &self,
        address: Value<'m>,
        expected: Value<'m>,
        new: Value<'m>,
    ) -> Value<'m> {
        use llvm_sys::LLVMAtomicOrdering::LLVMAtomicOrderingMonotonic as Monotonic;
        // SAFETY: the operands belong to this module; `expected` and `new` share an integer type.
        unsafe {
            let pair = LLVMBuildAtomicCmpXchg(
                self.raw,
                address.raw,
                expected.raw,
                new.raw,
                Monotonic,
                Monotonic,
                0,
            );
            value(LLVMBuildExtractValue(self.raw, pair, 1, NO_NAME))
        }
    }

    pub(crate) fn load_access(
        &self,
        of: Type<'m>,
        address: Value<'m>,
        align: u32,
        access: Access,
    ) -> Value<'m> {
        let loaded = self.load(of, address, align);
        self.module.access(loaded, access);
        loaded
    }

    pub(crate) fn store_access(
        &self,
        stored: Value<'m>,
        address: Value<'m>,
        align: u32,
        access: Access,
    ) {
        // SAFETY: the value and address belong to this module.
        let instruction = unsafe {
            let store = LLVMBuildStore(self.raw, stored.raw, address.raw);
            LLVMSetAlignment(store, align);
            value(store)
        };
        self.module.access(instruction, access);
    }

    /// An existing instruction's runtime postcondition, never an unchecked
    /// source assertion. Callers document the Rust body that establishes it.
    pub(crate) fn assume(&self, condition: Value<'m>) -> Result<(), String> {
        let intrinsic = self.module.intrinsic("llvm.assume", &[])?;
        self.call(intrinsic, &[condition]);
        Ok(())
    }

    /// A stack slot of `of` at `align`. Callers position the builder in the entry block.
    pub(crate) fn alloca(&self, of: Type<'m>, align: u32) -> Value<'m> {
        // SAFETY: the type belongs to this module.
        unsafe {
            let slot = LLVMBuildAlloca(self.raw, of.raw, NO_NAME);
            LLVMSetAlignment(slot, align);
            value(slot)
        }
    }

    /// `address + offset` bytes, where `offset` is an integer value.
    pub(crate) fn offset(&self, address: Value<'m>, offset: Value<'m>) -> Value<'m> {
        let byte = self.module.int_type(8);
        let mut indices = [offset.raw];
        // SAFETY: the address is a pointer and the index an integer of this module. The GEP
        // is not `inbounds`: no in-bounds assumption is granted beyond the checks emitted.
        value(unsafe {
            LLVMBuildGEP2(
                self.raw,
                byte.raw,
                address.raw,
                indices.as_mut_ptr(),
                1,
                NO_NAME,
            )
        })
    }

    pub(crate) fn call(&self, callee: Function<'m>, arguments: &[Value<'m>]) -> Value<'m> {
        self.call_typed(callee.ty, callee.value, arguments)
    }

    /// A call through `callee` (a function or a code pointer) of type `fn_ty`.
    pub(crate) fn call_typed(
        &self,
        fn_ty: Type<'m>,
        callee: Value<'m>,
        arguments: &[Value<'m>],
    ) -> Value<'m> {
        if let Some((enter, leave, context, site)) = self.profile.get() {
            let previous = self.raw_call(enter.ty, enter.value, &[context, site]);
            let result = self.raw_call(fn_ty, callee, arguments);
            self.raw_call(leave.ty, leave.value, &[context, previous]);
            result
        } else {
            self.raw_call(fn_ty, callee, arguments)
        }
    }

    pub(crate) fn profile_site(
        &self,
        hooks: Option<(Function<'m>, Function<'m>, Value<'m>, Value<'m>)>,
    ) {
        self.profile.set(hooks);
    }

    fn raw_call(&self, fn_ty: Type<'m>, callee: Value<'m>, arguments: &[Value<'m>]) -> Value<'m> {
        let mut raw: Vec<_> = arguments.iter().map(|v| v.raw).collect();
        // SAFETY: the callee and arguments belong to this module and match `fn_ty`; a
        // mismatch is reported by the verifier.
        value(unsafe {
            LLVMBuildCall2(
                self.raw,
                fn_ty.raw,
                callee.raw,
                raw.as_mut_ptr(),
                raw.len() as c_uint,
                NO_NAME,
            )
        })
    }

    pub(crate) fn insert(&self, aggregate: Value<'m>, field: Value<'m>, index: u32) -> Value<'m> {
        // SAFETY: the aggregate field and value have the same type in this module.
        value(unsafe { LLVMBuildInsertValue(self.raw, aggregate.raw, field.raw, index, NO_NAME) })
    }

    pub(crate) fn extract(&self, aggregate: Value<'m>, index: u32) -> Value<'m> {
        // SAFETY: `aggregate` is a struct value with at least `index + 1` fields.
        value(unsafe { LLVMBuildExtractValue(self.raw, aggregate.raw, index, NO_NAME) })
    }

    pub(crate) fn memset(&self, address: Value<'m>, byte: u8, length: u64, align: u32) {
        let i8_ty = self.module.int_type(8);
        let i64_ty = self.module.int_type(64);
        let byte = self.module.const_int(i8_ty, u128::from(byte));
        let length = self.module.const_int(i64_ty, u128::from(length));
        // SAFETY: the operands belong to this module.
        unsafe { LLVMBuildMemSet(self.raw, address.raw, byte.raw, length.raw, align) };
    }

    /// Copies `length` bytes (an `i64` value) between nonoverlapping places.
    pub(crate) fn memcpy(
        &self,
        to: Value<'m>,
        to_align: u32,
        from: Value<'m>,
        from_align: u32,
        length: Value<'m>,
    ) {
        // SAFETY: the operands belong to this module.
        unsafe { LLVMBuildMemCpy(self.raw, to.raw, to_align, from.raw, from_align, length.raw) };
    }

    pub(crate) fn phi(&self, of: Type<'m>) -> Value<'m> {
        // SAFETY: the type belongs to this module; the builder is at the head of its block.
        value(unsafe { LLVMBuildPhi(self.raw, of.raw, NO_NAME) })
    }

    pub(crate) fn add_incoming(&self, phi: Value<'m>, incoming: Value<'m>, from: BasicBlock<'m>) {
        let mut values = [incoming.raw];
        let mut blocks = [from.raw];
        // SAFETY: `phi` is a phi node and the value and block belong to this module.
        unsafe { LLVMAddIncoming(phi.raw, values.as_mut_ptr(), blocks.as_mut_ptr(), 1) };
    }

    pub(crate) fn br(&self, to: BasicBlock<'m>) {
        // SAFETY: the block belongs to this module.
        unsafe { LLVMBuildBr(self.raw, to.raw) };
    }

    pub(crate) fn cond_br(&self, condition: Value<'m>, yes: BasicBlock<'m>, no: BasicBlock<'m>) {
        // SAFETY: the condition is an i1 and the blocks belong to this module.
        unsafe { LLVMBuildCondBr(self.raw, condition.raw, yes.raw, no.raw) };
    }

    /// Failure is possible but cold; weights guide layout, never remove the edge.
    pub(crate) fn cold_cond_br(
        &self,
        condition: Value<'m>,
        failure: BasicBlock<'m>,
        next: BasicBlock<'m>,
    ) {
        // SAFETY: condition, blocks, branch and metadata share the live context.
        unsafe {
            let branch = LLVMBuildCondBr(self.raw, condition.raw, failure.raw, next.raw);
            let context = self.module.context.raw;
            let mut operands = [
                LLVMMDStringInContext2(context, c"branch_weights".as_ptr(), 14),
                LLVMValueAsMetadata(self.module.const_int(self.module.int_type(32), 1).raw),
                LLVMValueAsMetadata(self.module.const_int(self.module.int_type(32), 2000).raw),
            ];
            let node = LLVMMDNodeInContext2(context, operands.as_mut_ptr(), operands.len());
            let kind = LLVMGetMDKindIDInContext(context, c"prof".as_ptr(), 4);
            LLVMSetMetadata(branch, kind, LLVMMetadataAsValue(context, node));
        }
    }

    pub(crate) fn switch(
        &self,
        on: Value<'m>,
        otherwise: BasicBlock<'m>,
        cases: &[(Value<'m>, BasicBlock<'m>)],
    ) {
        // SAFETY: `on` is an integer, the cases are distinct constants of its type and the
        // blocks belong to this module.
        unsafe {
            let switch = LLVMBuildSwitch(self.raw, on.raw, otherwise.raw, cases.len() as c_uint);
            for (case, to) in cases {
                LLVMAddCase(switch, case.raw, to.raw);
            }
        }
    }

    pub(crate) fn ret(&self, returned: Value<'m>) {
        // SAFETY: the value has the function's return type.
        unsafe { LLVMBuildRet(self.raw, returned.raw) };
    }

    pub(crate) fn ret_void(&self) {
        // SAFETY: the builder is positioned in a block of a void function.
        unsafe { LLVMBuildRetVoid(self.raw) };
    }

    pub(crate) fn unreachable(&self) {
        // SAFETY: the builder is positioned in a block.
        unsafe { LLVMBuildUnreachable(self.raw) };
    }
}

impl Drop for Builder<'_> {
    fn drop(&mut self) {
        // SAFETY: the builder is live and owned.
        unsafe { LLVMDisposeBuilder(self.raw) };
    }
}

/// What LLVM's DataLayout for a target machine says about the facts `TargetSpec` pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DataLayoutFacts {
    pub text: String,
    pub little_endian: bool,
    pub pointer_bytes: u32,
    /// ABI size and alignment of i8, i16, i32, i64, i128, f32, f64 and ptr, in that order.
    pub scalars: [(u64, u32); 8],
}

/// An owned `LLVMTargetMachine` for one target.
pub(crate) struct TargetMachine {
    raw: LLVMTargetMachineRef,
    triple: String,
}

impl TargetMachine {
    pub(crate) fn new(target: &TargetSpec, optimize: bool) -> Result<Self, String> {
        initialize_targets();
        let triple = c_string(&target.triple);
        let cpu = c_string(&target.cpu);
        let features = c_string(&target.features.join(","));
        let mut llvm_target: LLVMTargetRef = ptr::null_mut();
        let mut message = ptr::null_mut();
        // SAFETY: the triple is NUL-terminated; LLVM writes a target or a message.
        let failed =
            unsafe { LLVMGetTargetFromTriple(triple.as_ptr(), &mut llvm_target, &mut message) };
        // SAFETY: the message is null or ours to dispose of.
        let message = unsafe { take_message(message) };
        if failed != 0 {
            return Err(message);
        }
        let level = if optimize {
            LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive
        } else {
            LLVMCodeGenOptLevel::LLVMCodeGenLevelNone
        };
        // SAFETY: the target is registered and the strings are NUL-terminated; LLVM copies them.
        let raw = unsafe {
            LLVMCreateTargetMachine(
                llvm_target,
                triple.as_ptr(),
                cpu.as_ptr(),
                features.as_ptr(),
                level,
                LLVMRelocMode::LLVMRelocPIC,
                LLVMCodeModel::LLVMCodeModelDefault,
            )
        };
        if raw.is_null() {
            return Err(format!(
                "LLVM could not create a target machine for {}",
                target.triple
            ));
        }
        Ok(Self {
            raw,
            triple: target.triple.clone(),
        })
    }

    pub(crate) fn stack_sizes(&self) {
        unsafe extern "C" {
            fn hirsel_llvm_emit_stack_sizes(machine: *mut std::ffi::c_void);
        }
        // SAFETY: the per-machine bridge only sets TargetOptions before emission.
        unsafe { hirsel_llvm_emit_stack_sizes(self.raw.cast()) };
    }

    /// The machine's DataLayout string and the scalar facts it implies.
    pub(crate) fn data_layout(&self) -> DataLayoutFacts {
        let context = Context::new();
        let module = Module::new(&context, "layout");
        let types = [
            module.int_type(8),
            module.int_type(16),
            module.int_type(32),
            module.int_type(64),
            module.int_type(128),
            module.f32_type(),
            module.f64_type(),
            module.ptr_type(),
        ];
        // SAFETY: the machine is live; the target data is disposed of here, after every
        // query, and the queried types belong to a live context.
        unsafe {
            let data: LLVMTargetDataRef = LLVMCreateTargetDataLayout(self.raw);
            let text = take_message(LLVMCopyStringRepOfTargetData(data));
            let scalars = types.map(|t| {
                (
                    LLVMABISizeOfType(data, t.raw),
                    LLVMABIAlignmentOfType(data, t.raw),
                )
            });
            let facts = DataLayoutFacts {
                text,
                little_endian: LLVMByteOrder(data) == LLVMByteOrdering::LLVMLittleEndian,
                pointer_bytes: LLVMPointerSize(data),
                scalars,
            };
            LLVMDisposeTargetData(data);
            facts
        }
    }
}

impl Drop for TargetMachine {
    fn drop(&mut self) {
        // SAFETY: the machine is live and owned.
        unsafe { LLVMDisposeTargetMachine(self.raw) };
    }
}

#[cfg(test)]
mod optimization_tests {
    use super::*;
    use hirsel_lir::layout_types::ScalarKind;

    /// The store's pointer is deliberately unrelated in LLVM's syntax. Only the
    /// justified storage metadata can distinguish it from the length pointer.
    fn loads_after_store(metadata: bool, same_class: bool) -> String {
        let context = Context::new();
        let module = Module::new(&context, "length-alias-control");
        let machine = TargetMachine::new(&TargetSpec::x86_64_unknown_linux_gnu(), true).unwrap();
        module.set_target(&machine);
        let i64_ty = module.int_type(64);
        let pointer = module.ptr_type();
        let function = module
            .function("probe", module.function_type(i64_ty, &[pointer, pointer]))
            .unwrap();
        let builder = Builder::new(&module);
        builder.position(module.append_block(function));
        let length = module.parameter(function, 0);
        let element = module.parameter(function, 1);
        let load = || {
            if metadata {
                builder.load_access(i64_ty, length, 8, Access::Length)
            } else {
                builder.load(i64_ty, length, 8)
            }
        };
        let before = load();
        let stored = module.const_int(i64_ty, 7);
        if metadata {
            builder.store_access(
                stored,
                element,
                8,
                if same_class {
                    Access::Length
                } else {
                    Access::Value
                },
            );
        } else {
            builder.store(stored, element, 8);
        }
        let after = load();
        builder.ret(builder.binary(BinOp::Add, before, after));
        drop(builder);
        module.verify().unwrap();
        module.run_passes("default<O2>", &machine).unwrap();
        module.verify().unwrap();
        module.text()
    }

    #[test]
    fn scalar_writes_do_not_clobber_lengths_but_length_writes_do() {
        let unclassified = loads_after_store(false, false);
        let scalar_write = loads_after_store(true, false);
        let length_write = loads_after_store(true, true);
        assert_eq!(
            unclassified.matches("load i64").count(),
            2,
            "{unclassified}"
        );
        assert_eq!(
            scalar_write.matches("load i64").count(),
            1,
            "{scalar_write}"
        );
        assert_eq!(
            length_write.matches("load i64").count(),
            2,
            "{length_write}"
        );
    }
}

/// DIBuilder owns deferred metadata until finalized before optimization. Locations are
/// content-only (`crate::debug`): one fixed file and compile unit per module.
pub(crate) struct DebugInfo<'m> {
    raw: LLVMDIBuilderRef,
    module: &'m Module<'m>,
    optimized: bool,
    file: LLVMMetadataRef,
}
#[derive(Clone, Copy)]
pub(crate) struct DebugScope(LLVMMetadataRef);
impl<'m> DebugInfo<'m> {
    pub(crate) fn new(module: &'m Module<'m>, optimized: bool) -> Self {
        // SAFETY: all metadata belongs to the live module/context; LLVM copies strings.
        unsafe {
            for (name, version) in [("Debug Info Version", 3), ("Dwarf Version", 5)] {
                LLVMAddModuleFlag(
                    module.raw,
                    llvm_sys::LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorWarning,
                    name.as_ptr().cast(),
                    name.len(),
                    LLVMValueAsMetadata(module.const_int(module.int_type(32), version).raw),
                );
            }
            let raw = LLVMCreateDIBuilder(module.raw);
            let (name, directory, producer) = ("hirsel.sites", "/", "hirsel");
            let file = LLVMDIBuilderCreateFile(
                raw,
                name.as_ptr().cast(),
                name.len(),
                directory.as_ptr().cast(),
                directory.len(),
            );
            LLVMDIBuilderCreateCompileUnit(
                raw,
                LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageC,
                file,
                producer.as_ptr().cast(),
                producer.len(),
                optimized as i32,
                "".as_ptr().cast(),
                0,
                0,
                "".as_ptr().cast(),
                0,
                LLVMDWARFEmissionKind::LLVMDWARFEmissionKindLineTablesOnly,
                0,
                1,
                1,
                "".as_ptr().cast(),
                0,
                "".as_ptr().cast(),
                0,
            );
            Self {
                raw,
                module,
                optimized,
                file,
            }
        }
    }
    /// The scope of `function`, the definition `name`, declared at content line `line`.
    pub(crate) fn function(&self, function: Function<'m>, name: &str, line: u32) -> DebugScope {
        let mut length = 0;
        // SAFETY: LLVM returns the name bytes for the lifetime of the live function.
        let link = unsafe {
            let name = LLVMGetValueName2(function.value.raw, &mut length);
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name.cast(), length))
        };
        // SAFETY: all handles share a live context, and byte slices have the stated lengths.
        unsafe {
            let ty = LLVMDIBuilderCreateSubroutineType(
                self.raw,
                self.file,
                ptr::null_mut(),
                0,
                LLVMDIFlagZero,
            );
            let scope = LLVMDIBuilderCreateFunction(
                self.raw,
                self.file,
                name.as_ptr().cast(),
                name.len(),
                link.as_ptr().cast(),
                link.len(),
                self.file,
                line,
                ty,
                0,
                1,
                line,
                LLVMDIFlagZero,
                self.optimized as i32,
            );
            LLVMSetSubprogram(function.value.raw, scope);
            DebugScope(scope)
        }
    }
    pub(crate) fn location(
        &self,
        builder: &Builder<'m>,
        scope: DebugScope,
        line: u32,
        column: u32,
    ) {
        // SAFETY: scope and builder share this context; no inlined-at location initially.
        unsafe {
            let location = LLVMDIBuilderCreateDebugLocation(
                self.module.context.raw,
                line,
                column,
                scope.0,
                ptr::null_mut(),
            );
            LLVMSetCurrentDebugLocation2(builder.raw, location);
        }
    }
    /// Places every instruction `function` has at `line` and `column` of `scope`: for a
    /// function built without locations, whose calls must have one once it has a scope.
    pub(crate) fn locate_all(
        &self,
        function: Function<'m>,
        scope: DebugScope,
        line: u32,
        column: u32,
    ) {
        // SAFETY: the function, its blocks and instructions and the scope share this context.
        unsafe {
            let location = LLVMDIBuilderCreateDebugLocation(
                self.module.context.raw,
                line,
                column,
                scope.0,
                ptr::null_mut(),
            );
            let mut block = llvm_sys::core::LLVMGetFirstBasicBlock(function.value.raw);
            while !block.is_null() {
                let mut instruction = llvm_sys::core::LLVMGetFirstInstruction(block);
                while !instruction.is_null() {
                    llvm_sys::debuginfo::LLVMInstructionSetDebugLoc(instruction, location);
                    instruction = llvm_sys::core::LLVMGetNextInstruction(instruction);
                }
                block = llvm_sys::core::LLVMGetNextBasicBlock(block);
            }
        }
    }
    pub(crate) fn finalize(&self) {
        // SAFETY: no metadata is constructed after this call.
        unsafe { LLVMDIBuilderFinalize(self.raw) };
    }
}
impl Drop for DebugInfo<'_> {
    fn drop(&mut self) {
        // SAFETY: this owns the builder, which is disposed before its module.
        unsafe { LLVMDisposeDIBuilder(self.raw) };
    }
}

impl<'m> Value<'m> {
    pub(crate) fn ty(self) -> Type<'m> {
        // SAFETY: every Value owns a live handle of this context.
        ty(unsafe { LLVMTypeOf(self.raw) })
    }

    /// The value of an integer constant of at most 64 bits, zero-extended; `None` for
    /// anything computed.
    pub(crate) fn known_u64(self) -> Option<u64> {
        // SAFETY: every Value owns a live handle of this context; the width is read only for
        // a constant integer, whose zero-extended value LLVM returns for widths up to 64.
        unsafe {
            (!LLVMIsAConstantInt(self.raw).is_null()
                && LLVMGetIntTypeWidth(LLVMTypeOf(self.raw)) <= 64)
                .then(|| LLVMConstIntGetZExtValue(self.raw))
        }
    }
}

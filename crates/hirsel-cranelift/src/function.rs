//! One Ready unit as one Cranelift function: values, blocks and edges, failure edges,
//! terminators and calls.
//!
//! A LIR value lives in SSA form as nothing (unit), one Cranelift value (a scalar; a Bool is
//! an `i8` holding 0 or 1), a code and environment pair (a callable), or the address of an
//! immutable stack copy of its bytes (any other fixed-size inline value). A block parameter
//! of bytes is a stack slot of its own that every edge into the block copies into. Blocks
//! a failure edge or a call's failure path starts in are filled after the body, once every
//! LIR value they read is defined.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cranelift_codegen::ir::{
    self, Block, BlockArg, FuncRef, GlobalValue, InstBuilder, MemFlagsData, StackSlot,
    StackSlotData, StackSlotKind, TrapCode, Value, condcodes::IntCC, types,
};
use cranelift_frontend::{FunctionBuilder, Switch};
use cranelift_module::{DataId, FuncId, Module};
use hirsel_core_contract::{Disposition, Engine, Passing};
use hirsel_lir::abi::abi_digest;
use hirsel_lir::abi_types::{CallableShape, FnAbi, ParameterPassing};
use hirsel_lir::emission::{register_result, site_code, symbol_name};
use hirsel_lir::ir::*;
use hirsel_lir::layout::{align_up, payload_offset};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutRecord, ScalarKind, TagForm};
use hirsel_lir::runtime_abi::RuntimeEntry;
use hirsel_lir::stage::{ObjectSymbolKind, Ready};
use hirsel_syntax::DefPath;

use crate::CodegenError;
use crate::module::{Emitted, Emitter, Hidden, PTR, Sig, scalar_type};

/// The trap a proved-unreachable point or an emitter invariant takes.
pub(crate) const TRAP: TrapCode = TrapCode::unwrap_user(1);

/// How a value is held in SSA form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Unit,
    Scalar(ScalarKind),
    /// Code and environment pointers.
    Callable,
    /// The bytes of a fixed-size inline value, read through memory.
    Bytes {
        size: u64,
        align: u32,
    },
}

/// A value in SSA form (`Shape`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum V {
    Unit,
    One(Value),
    Pair(Value, Value),
    /// The address of the value's bytes, never written while the value is live, and the
    /// stack slot holding them when it is a block parameter's.
    Mem {
        addr: Value,
        slot: Option<StackSlot>,
    },
}

impl V {
    pub(crate) fn one(self) -> Option<Value> {
        match self {
            V::One(v) => Some(v),
            _ => None,
        }
    }
}

/// A prepared address: its pointer, layout and the alignment it is known to have.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Place {
    pub(crate) columns: Option<crate::soa::ColumnRow>,
    pub(crate) ptr: Value,
    pub(crate) layout: LayoutId,
    pub(crate) align: u32,
    /// The counted block (its header) whose payload holds this place, if any.
    pub(crate) block: Option<Value>,
}

/// A block parameter: none for a storage version or a unit, the Cranelift parameters of a
/// scalar or callable, or the stack slot of bytes.
#[derive(Clone, Copy, Debug)]
enum Param {
    None,
    Ssa(usize),
    Slot(StackSlot, u64, u32),
}

/// A block filled after the body.
enum Deferred {
    /// A failure edge's start: the rule's begin, the token disposals, the cleanup edge.
    Failure { block: Block, failure: FailureId },
    /// An unanchored function-value invocation establishes its own boundary.
    CallBoundary {
        block: Block,
        site: u64,
        then: Block,
    },
    /// A failing runtime call whose operation records its own failure rule.
    Record {
        block: Block,
        rule: String,
        site: u64,
        forward_caller: bool,
        then: Block,
    },
    /// A bounded arena's `OutOfSpace` (MEM-6): recorded, then the failure edge's tokens
    /// and its cleanup.
    Exhausted {
        block: Block,
        region: Value,
        site: u64,
        failure: FailureId,
        values: Vec<ValueId>,
    },
}

pub(crate) struct FnEmitter<'e, 'a, 'f> {
    pub(crate) em: &'e mut Emitter<'a>,
    pub(crate) unit: &'a LirUnit<Ready>,
    pub(crate) b: FunctionBuilder<'f>,
    pub(crate) sig: Sig<'a>,
    pub(crate) context: Value,
    pub(crate) environment: Option<Value>,
    /// The function's own parameters.
    pub(crate) entry_params: Vec<Value>,
    values: Vec<Option<V>>,
    /// Storage version names: never operands, so they have no value.
    versions: BTreeSet<ValueId>,
    pub(crate) slots: Vec<Option<Place>>,
    /// Slots that always hold one static's pointer-free bytes, addressed in place in
    /// read-only data (`hirsel_lir::statics::constant_slots`).
    pub(crate) constant_slots: Vec<Option<hirsel_lir::statics::ConstantSlot<'a>>>,
    pub(crate) addresses: Vec<Option<Place>>,
    /// Per address of a variable flat element, where its sequence's elements begin.
    pub(crate) flat_contents: Vec<Option<Value>>,
    /// Set by the last variable flat element step `project` took.
    pub(crate) flat_content: Option<Value>,
    blocks: Vec<Block>,
    params: Vec<Vec<Param>>,
    failures: BTreeMap<FailureId, Block>,
    deferred: Vec<(ir::SourceLoc, Option<crate::profile::CallSite>, Deferred)>,
    pub(crate) measured: Option<&'a hirsel_lir::profile::ProfileSource>,
    profile: Option<crate::profile::CallSite>,
    debug_sites: Option<hirsel_lir::debug::SiteTable>,
    /// Per compiler-local region instance, its `hirsel_rt_v2_region` record in the frame.
    pub(crate) regions: Vec<Value>,
    funcs: HashMap<FuncId, FuncRef>,
    globals: HashMap<DataId, GlobalValue>,
    /// Per block, whether it charges fuel (`hirsel_lir::fuel`); empty without `inputs.fuel`.
    fuel_points: Vec<bool>,
    pub(crate) instruction_index: usize,
    pub(crate) root_facts: Vec<Option<cranelift_frontend::Variable>>,
    pub(crate) read_roots: Vec<Option<cranelift_frontend::Variable>>,
    pub(crate) ahead_state: Option<Value>,
    pub(crate) ahead_counter: Option<cranelift_frontend::Variable>,
    pub(crate) ownership: Option<&'a hirsel_lir::ownership_trace::native::LeafPlan>,
    pub(crate) ownership_frame: Option<Value>,
    pub(crate) ownership_cleanup: Option<hirsel_lir::ownership_trace::native::NativeMarker>,
    /// Set while emitting one state of a resumable continuation alternate.
    pub(crate) resume: Option<crate::resume::Resume<'a>>,
    /// Admissions of depth-leaf callees, lowered to room checks with no retirement.
    pub(crate) room: hirsel_lir::invocations::room::RoomChecks,
    pub(crate) counters: hirsel_lir::invocations::room::RoomChecks,
    counter_cell: Option<(Value, Value)>,
}

/// Bytes of a `hirsel_rt_v2_region` record.

pub(crate) fn low_bit(n: u64) -> u32 {
    if n == 0 {
        u32::MAX
    } else {
        1u32 << n.trailing_zeros().min(31)
    }
}

/// Memory flags of every access: Hirsel addresses are valid, alignment is not promised.
pub(crate) fn flags() -> MemFlagsData {
    MemFlagsData::new()
}

/// The alignment a constant-size copy or fill of `size` bytes may assume.
fn copy_align(size: u64, align: u32) -> u8 {
    let size_align = if size == 0 {
        1
    } else {
        1u64 << size.trailing_zeros().min(7)
    };
    (u64::from(align).min(size_align).min(128)) as u8
}

impl<'e, 'a, 'f> FnEmitter<'e, 'a, 'f> {
    pub(crate) fn new(
        em: &'e mut Emitter<'a>,
        unit: &'a LirUnit<Ready>,
        mut b: FunctionBuilder<'f>,
        sig: Sig<'a>,
    ) -> Self {
        let debug_sites = em.debug.then(|| hirsel_lir::debug::SiteTable::new(unit));
        if let Some(sites) = &debug_sites {
            let site =
                unit.tables.source_anchors[unit.blocks[unit.entry.index()].anchor.index()].site;
            b.set_srcloc(ir::SourceLoc::new(sites.code(site).unwrap_or(u32::MAX)));
        }
        let prologue = b.create_block();
        b.append_block_params_for_function_params(prologue);
        b.switch_to_block(prologue);
        let entry_params = b.block_params(prologue).to_vec();
        let context = entry_params[0];
        let environment = sig
            .hidden_index(Hidden::Environment)
            .map(|index| entry_params[index]);
        let constant_slots = hirsel_lir::statics::constant_slots(unit, em.inputs.unit().statics);
        let fuel_points = if em.inputs.unit().fuel || em.inputs.unit().alternate {
            hirsel_lir::fuel::fuel_points(unit)
        } else {
            Vec::new()
        };
        Self {
            debug_sites,
            em,
            unit,
            b,
            sig,
            context,
            environment,
            entry_params,
            values: vec![None; unit.values.len()],
            versions: BTreeSet::new(),
            slots: vec![None; unit.slots.len()],
            constant_slots,
            addresses: vec![None; unit.addresses.len()],
            flat_contents: vec![None; unit.addresses.len()],
            flat_content: None,
            blocks: Vec::new(),
            params: Vec::new(),
            failures: BTreeMap::new(),
            deferred: Vec::new(),
            measured: None,
            profile: None,
            regions: Vec::new(),
            funcs: HashMap::new(),
            globals: HashMap::new(),
            fuel_points,
            instruction_index: 0,
            root_facts: vec![None; unit.slots.len()],
            read_roots: vec![None; unit.values.len()],
            ahead_state: None,
            ahead_counter: None,
            ownership: None,
            ownership_frame: None,
            ownership_cleanup: None,
            resume: None,
            room: Default::default(),
            counters: Default::default(),
            counter_cell: None,
        }
    }

    // Errors and tables.

    pub(crate) fn definition(&self) -> &'a DefPath {
        &self.unit.header.definition
    }

    pub(crate) fn invalid<T>(&self, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Invalid {
            definition: self.definition().clone(),
            what: what.into(),
        })
    }

    pub(crate) fn unsupported<T>(&self, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Unsupported {
            definition: self.definition().clone(),
            what: what.into(),
        })
    }

    pub(crate) fn layout(&self, id: LayoutId) -> Emitted<&'a LayoutRecord> {
        match self.em.layouts().layout(id) {
            Some(record) => Ok(record),
            None => self.invalid(format!("no layout {}", id.0)),
        }
    }

    /// The bytes a value of `id` occupies in storage.
    pub(crate) fn fixed_size(&self, id: LayoutId) -> Emitted<u64> {
        match self.layout(id)?.size.storage() {
            Some(size) => Ok(size),
            None => self.unsupported("a value of dynamic extent"),
        }
    }

    pub(crate) fn lir_type(&self, id: LirTypeId) -> Emitted<&'a LirType> {
        match self.unit.types.get(id.index()) {
            Some(ty) => Ok(ty),
            None => self.invalid(format!("no type {}", id.0)),
        }
    }

    pub(crate) fn value_type(&self, value: ValueId) -> Emitted<&'a LirType> {
        match self.unit.values.get(value.index()) {
            Some(info) => self.lir_type(info.ty),
            None => self.invalid(format!("no value {}", value.0)),
        }
    }

    pub(crate) fn shape(&self, ty: &LirType) -> Emitted<Shape> {
        if matches!(ty.repr, ValueRepr::Token | ValueRepr::RegionMark) {
            return Ok(Shape::Scalar(ScalarKind::Pointer));
        }
        self.layout_shape(ty.layout)
    }

    pub(crate) fn layout_shape(&self, layout: LayoutId) -> Emitted<Shape> {
        let record = self.layout(layout)?;
        if let Some(kind) = hirsel_lir::emission::scalar_shape(&record.kind) {
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

    pub(crate) fn value_shape(&self, value: ValueId) -> Emitted<Shape> {
        self.shape(self.value_type(value)?)
    }

    pub(crate) fn scalar_kind(&self, value: ValueId) -> Emitted<ScalarKind> {
        match self.value_shape(value)? {
            Shape::Scalar(kind) => Ok(kind),
            _ => self.invalid(format!("value {} is not a scalar", value.0)),
        }
    }

    pub(crate) fn pool<T: Copy>(&self, range: PoolRange<T>, pool: &'a [T]) -> Emitted<&'a [T]> {
        match range.get(pool) {
            Some(items) => Ok(items),
            None => self.invalid("a pool range outside its pool"),
        }
    }

    pub(crate) fn edge(&self, edge: EdgeId) -> Emitted<&'a Edge> {
        match self.unit.tables.edges.get(edge.index()) {
            Some(e) => Ok(e),
            None => self.invalid(format!("no edge {}", edge.0)),
        }
    }

    pub(crate) fn failure_edge(&self, failure: FailureId) -> Emitted<&'a FailureEdge> {
        match self.unit.tables.failures.get(failure.index()) {
            Some(f) => Ok(f),
            None => self.invalid(format!("no failure edge {}", failure.0)),
        }
    }

    // Values.

    pub(crate) fn get(&self, value: ValueId) -> Emitted<V> {
        match self.values.get(value.index()).copied().flatten() {
            Some(v) => Ok(v),
            None if self.versions.contains(&value) => {
                self.invalid(format!("storage version {} used as an operand", value.0))
            }
            None => self.invalid(format!("value {} used before its definition", value.0)),
        }
    }

    /// A scalar or pointer value's one Cranelift value.
    pub(crate) fn get1(&self, value: ValueId) -> Emitted<Value> {
        match self.get(value)? {
            V::One(v) => Ok(v),
            _ => self.invalid(format!("value {} is not one word", value.0)),
        }
    }

    pub(crate) fn define(&mut self, value: ValueId, v: V) -> Emitted {
        match self.values.get_mut(value.index()) {
            Some(slot @ None) => {
                *slot = Some(v);
                Ok(())
            }
            Some(Some(_)) => self.invalid(format!("value {} defined twice", value.0)),
            None => self.invalid(format!("no value {}", value.0)),
        }
    }

    pub(crate) fn define1(&mut self, value: ValueId, v: Value) -> Emitted {
        self.define(value, V::One(v))
    }

    /// Takes a defined value back (a construction redone on another path).
    #[allow(dead_code)]
    pub(crate) fn undefine(&mut self, value: ValueId) -> Option<V> {
        self.values.get_mut(value.index()).and_then(Option::take)
    }

    /// Define a scalar operation's flag, present exactly when the operation reports one.
    pub(crate) fn define_flag(&mut self, flag: Option<ValueId>, v: Option<Value>) -> Emitted {
        match (flag, v) {
            (Some(flag), Some(v)) => self.define1(flag, v),
            (None, None) => Ok(()),
            _ => self.invalid("a scalar flag the operation does not report"),
        }
    }

    // Constants and addresses.

    pub(crate) fn u64c(&mut self, n: u64) -> Value {
        self.b.ins().iconst(types::I64, n as i64)
    }

    /// The integer constant `bits` of type `ty`, `i128` included.
    pub(crate) fn int_const(&mut self, ty: ir::Type, bits: u128) -> Value {
        if ty == types::I128 {
            let low = self.b.ins().iconst(types::I64, bits as u64 as i64);
            let high = self.b.ins().iconst(types::I64, (bits >> 64) as u64 as i64);
            return self.b.ins().iconcat(low, high);
        }
        let mask = if ty.bits() >= 64 {
            u64::MAX
        } else {
            (1u64 << ty.bits()) - 1
        };
        self.b.ins().iconst(ty, ((bits as u64) & mask) as i64)
    }

    pub(crate) fn offset(&mut self, ptr: Value, n: u64) -> Value {
        if n == 0 {
            ptr
        } else {
            self.b.ins().iadd_imm_s(ptr, n as i64)
        }
    }

    pub(crate) fn offset_by(&mut self, ptr: Value, by: Value) -> Value {
        self.b.ins().iadd(ptr, by)
    }

    pub(crate) fn load_ty(&mut self, ty: ir::Type, at: Value, offset: u64) -> Value {
        if offset > i32::MAX as u64 {
            let at = self.offset(at, offset);
            return self.b.ins().load(ty, flags(), at, 0);
        }
        self.b.ins().load(ty, flags(), at, offset as i32)
    }

    pub(crate) fn store_at(&mut self, value: Value, at: Value, offset: u64) {
        if offset > i32::MAX as u64 {
            let at = self.offset(at, offset);
            self.b.ins().store(flags(), value, at, 0);
            return;
        }
        self.b.ins().store(flags(), value, at, offset as i32);
    }

    /// The u64 at `at`, which flat values store unaligned.
    pub(crate) fn load_u64(&mut self, at: Value) -> Value {
        self.load_ty(types::I64, at, 0)
    }

    pub(crate) fn load_ptr(&mut self, at: Value) -> Value {
        self.load_ty(PTR, at, 0)
    }

    /// Fresh stack storage of `size` bytes aligned to `align` (at most 16), at the current
    /// position: its address and slot.
    pub(crate) fn stack_bytes(&mut self, size: u64, align: u32) -> (Value, StackSlot) {
        let shift = align.clamp(1, 16).trailing_zeros() as u8;
        let slot = self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size.max(1) as u32,
            shift,
        ));
        (self.b.ins().stack_addr(PTR, slot, 0), slot)
    }

    /// Fresh stack storage for a value of `layout`.
    pub(crate) fn stack(&mut self, layout: LayoutId) -> Emitted<Place> {
        let size = self.fixed_size(layout)?;
        let align = self.layout(layout)?.align;
        let (ptr, _) = self.stack_bytes(size, align);
        Ok(Place {
            columns: None,
            ptr,
            layout,
            align,
            block: None,
        })
    }

    /// One pointer-sized out word in the frame.
    pub(crate) fn out_word(&mut self) -> Value {
        self.stack_bytes(8, 8).0
    }

    pub(crate) fn copy(&mut self, dest: Value, src: Value, size: u64, align: u32) {
        if size == 0 {
            return;
        }
        let config = self.em.module.target_config();
        let align = copy_align(size, align);
        self.b
            .emit_small_memory_copy(config, dest, src, size, align, align, true, flags());
    }

    pub(crate) fn copy_dynamic(&mut self, dest: Value, src: Value, size: Value) {
        let config = self.em.module.target_config();
        self.b.call_memcpy(config, dest, src, size);
    }

    pub(crate) fn zero(&mut self, at: Value, size: u64, align: u32) {
        if size == 0 {
            return;
        }
        // Alternate frames must not acquire an unbounded libc helper through
        // aggregate lowering. Keep every bounded image store in the proved body.
        if self.em.inputs.unit().alternate && size <= hirsel_lir::ahead::MAX_IMAGE_BYTES {
            let mut offset = 0;
            for (bytes, ty) in [
                (8, types::I64),
                (4, types::I32),
                (2, types::I16),
                (1, types::I8),
            ] {
                if size - offset >= bytes {
                    let zero = self.b.ins().iconst(ty, 0);
                    while size - offset >= bytes {
                        self.b.ins().store(flags(), zero, at, offset as i32);
                        offset += bytes;
                    }
                }
            }
            return;
        }
        let config = self.em.module.target_config();
        let align = copy_align(size, align);
        self.b
            .emit_small_memset(config, at, 0, size, align, flags());
    }

    pub(crate) fn func_ref(&mut self, id: FuncId) -> FuncRef {
        if let Some(&r) = self.funcs.get(&id) {
            return r;
        }
        let r = self.em.module.declare_func_in_func(id, self.b.func);
        self.funcs.insert(id, r);
        r
    }

    /// The address of data object `id`, `offset` bytes in.
    pub(crate) fn data_addr(&mut self, id: DataId, offset: u64) -> Value {
        let gv = match self.globals.get(&id) {
            Some(&gv) => gv,
            None => {
                let gv = self.em.module.declare_data_in_func(id, self.b.func);
                self.globals.insert(id, gv);
                gv
            }
        };
        let base = self.b.ins().symbol_value(PTR, gv);
        self.offset(base, offset)
    }

    pub(crate) fn func_addr(&mut self, id: FuncId) -> Value {
        let r = self.func_ref(id);
        self.b.ins().func_addr(PTR, r)
    }

    /// Calls `id` with `args`: its results.
    pub(crate) fn call(&mut self, id: FuncId, args: &[Value]) -> Vec<Value> {
        let previous = self.profile_enter();
        let returned = self.raw_call(id, args);
        self.profile_leave(previous);
        returned
    }

    /// Calls without profiling the profile hooks themselves.
    fn raw_call(&mut self, id: FuncId, args: &[Value]) -> Vec<Value> {
        let r = self.func_ref(id);
        let call = self.b.ins().call(r, args);
        self.b.inst_results(call).to_vec()
    }

    fn profile_enter(&mut self) -> Option<Value> {
        let site = self.profile?;
        let metadata = self.data_addr(site.metadata, 0);
        Some(self.raw_call(site.enter, &[self.context, metadata])[0])
    }

    fn profile_leave(&mut self, previous: Option<Value>) {
        if let Some(previous) = previous {
            self.raw_call(
                self.profile.expect("entered site").leave,
                &[self.context, previous],
            );
        }
    }

    /// Calls the runtime entry `entry`: its result, if it has one.
    pub(crate) fn rt(&mut self, entry: RuntimeEntry, args: &[Value]) -> Emitted<Option<Value>> {
        let id = self.em.runtime(entry)?;
        Ok(self.call(id, args).first().copied())
    }

    /// Calls a runtime entry that returns a value.
    pub(crate) fn rt1(&mut self, entry: RuntimeEntry, args: &[Value]) -> Emitted<Value> {
        match self.rt(entry, args)? {
            Some(v) => Ok(v),
            None => self.invalid(format!("runtime entry {entry:?} returns nothing")),
        }
    }

    /// Calls a function the toolchain provides (`TOOLCHAIN_IMPORTS`).
    pub(crate) fn toolchain_call(
        &mut self,
        name: &str,
        params: &[ir::Type],
        returns: ir::Type,
        args: &[Value],
    ) -> Emitted<Value> {
        let mut sig = ir::Signature::new(self.em.call_conv);
        sig.params
            .extend(params.iter().map(|&t| ir::AbiParam::new(t)));
        sig.returns.push(ir::AbiParam::new(returns));
        let id = self.em.import_function(name, &sig)?;
        Ok(self.call(id, args)[0])
    }

    // Loads and stores of values.

    pub(crate) fn load(&mut self, shape: Shape, place: Place) -> V {
        match shape {
            Shape::Unit => V::Unit,
            Shape::Scalar(ScalarKind::Bool) => {
                let byte = self.load_ty(types::I8, place.ptr, 0);
                V::One(self.b.ins().icmp_imm_s(IntCC::NotEqual, byte, 0))
            }
            Shape::Scalar(kind) => V::One(self.load_ty(scalar_type(kind), place.ptr, 0)),
            Shape::Callable => {
                let code = self.load_ty(PTR, place.ptr, 0);
                let env = self.load_ty(PTR, place.ptr, 8);
                V::Pair(code, env)
            }
            Shape::Bytes { size, align } => {
                let (addr, _) = self.stack_bytes(size, align);
                self.copy(addr, place.ptr, size, align.min(place.align));
                V::Mem { addr, slot: None }
            }
        }
    }

    pub(crate) fn store(&mut self, shape: Shape, value: V, place: Place) -> Emitted {
        match (shape, value) {
            (Shape::Unit, _) => {}
            (Shape::Scalar(_), V::One(v)) => self.store_at(v, place.ptr, 0),
            (Shape::Callable, V::Pair(code, env)) => {
                self.store_at(code, place.ptr, 0);
                self.store_at(env, place.ptr, 8);
            }
            (Shape::Bytes { size, align }, V::Mem { addr, .. }) => {
                self.copy(place.ptr, addr, size, align.min(place.align));
            }
            _ => return self.invalid("a store of a value of another shape"),
        }
        Ok(())
    }

    /// The place `offset` bytes into `place`, holding a value of `layout`.
    pub(crate) fn field_place(
        &mut self,
        place: Place,
        offset: u64,
        layout: LayoutId,
        unaligned: bool,
    ) -> Emitted<Place> {
        if place.columns.is_some() {
            return self.column_field(place, offset, layout);
        }
        let ptr = self.offset(place.ptr, offset);
        let align = if unaligned {
            1
        } else {
            place
                .align
                .min(low_bit(offset))
                .min(self.layout(layout)?.align)
        };
        Ok(Place {
            columns: None,
            ptr,
            layout,
            align,
            block: place.block,
        })
    }

    /// The payload of a counted block, behind its header.
    pub(crate) fn payload_of(&mut self, block: Value, payload: LayoutId) -> Emitted<Place> {
        let align = self.layout(payload)?.align;
        let offset = match payload_offset(align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        Ok(Place {
            columns: None,
            ptr: self.offset(block, offset),
            layout: payload,
            align,
            block: Some(block),
        })
    }

    // The function.

    pub(crate) fn emit(mut self) -> Emitted {
        if !self.counters.enters.is_empty() {
            self.counter_cell = Some(self.make_counter_cell());
        }
        if self.resume.is_some() {
            return self.emit_resume();
        }
        let unit = self.unit;
        if !self.em.inputs.unit().alternate
            && self.ownership.is_none()
            && let Some(plan) = self
                .em
                .inputs
                .unit()
                .continuation
                .and_then(|p| p.task_entry(unit))
        {
            return self.emit_resume_entry(plan);
        }
        self.ownership_init()?;
        self.ahead_init()?;
        self.find_versions()?;
        let order = self.reverse_postorder()?;
        for _ in &self.unit.blocks {
            let block = self.b.create_block();
            self.blocks.push(block);
        }
        self.create_params()?;
        self.allocate_slots()?;
        self.bind_parameters()?;
        self.root_facts_init()?;
        let entry = self.blocks[self.unit.entry.index()];
        if self.params[self.unit.entry.index()]
            .iter()
            .any(|p| !matches!(p, Param::None))
        {
            return self.invalid("an entry block with parameters");
        }
        self.b.ins().jump(entry, &[]);
        for b in order {
            self.emit_block(BlockId(b as u32))?;
        }
        while let Some((location, profile, deferred)) = self.deferred.pop() {
            self.b.set_srcloc(location);
            self.profile = profile;
            self.fill_deferred(deferred)?;
        }
        self.b.seal_all_blocks();
        let config = self.em.module.target_config();
        self.b.finalize(config);
        Ok(())
    }

    /// Storage version names: stores', replaces' and slot-backed parameters' versions, and
    /// every block parameter an edge passes one to.
    fn find_versions(&mut self) -> Emitted {
        let unit = self.unit;
        if let Some(entry) = &unit.tables.receipt_entry {
            self.versions.extend(entry.versions());
        }
        for inst in &unit.instructions {
            match *inst {
                Inst::Store { version, .. }
                | Inst::SeqReserve { version, .. }
                | Inst::ArenaPush { version, .. }
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
        for terminator in &unit.terminators {
            let Terminator::Call { call, .. } = *terminator else {
                continue;
            };
            let Some(call) = unit.tables.calls.get(call.index()) else {
                return self.invalid("a call outside the call table");
            };
            for argument in self.pool(call.arguments, &unit.pools.call_arguments)? {
                if let ArgumentValue::Address { version, .. } = argument.value {
                    self.versions.insert(version);
                }
            }
            if let Some(receipts) = &call.receipts {
                self.versions
                    .extend(receipts.writebacks().map(|(_, version)| version));
            }
        }
        for parameter in &self.sig.abi.parameters {
            let backed = unit
                .slots
                .iter()
                .any(|s| s.kind == SlotKind::Parameter(parameter.source));
            if backed && let Some(&value) = unit.parameters.get(parameter.source as usize) {
                self.versions.insert(value);
            }
        }
        // Captures name their environment slots' initial storage versions.
        self.versions.extend(unit.captures.iter().copied());
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
        if let Terminator::Call { call, .. } = terminator {
            failures.extend(unit.tables.calls[call.index()].pretransfer_failure);
        }
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
    /// uses it dominates.
    fn reverse_postorder(&self) -> Emitted<Vec<usize>> {
        let count = self.unit.blocks.len();
        if self.unit.entry.index() >= count {
            return self.invalid("no entry block");
        }
        let adjacent: Vec<Vec<usize>> = (0..count)
            .map(|b| self.successors(b))
            .collect::<Emitted<_>>()?;
        if adjacent.iter().flatten().any(|&b| b >= count) {
            return self.invalid("an edge to a missing block");
        }
        let post = hirsel_graph::reverse_postorder(&adjacent, self.unit.entry.index());
        if post.len() != count {
            return self.invalid("an unreachable block");
        }
        Ok(post)
    }

    fn create_params(&mut self) -> Emitted {
        let unit = self.unit;
        for (b, block) in unit.blocks.iter().enumerate() {
            let mut params = Vec::new();
            for &p in self.pool(block.parameters, &unit.pools.values)? {
                if self.versions.contains(&p) {
                    params.push(Param::None);
                    continue;
                }
                let target = self.blocks[b];
                let param = match self.value_shape(p)? {
                    Shape::Unit => Param::None,
                    Shape::Scalar(kind) => {
                        let index = self.b.block_params(target).len();
                        self.b.append_block_param(target, scalar_type(kind));
                        Param::Ssa(index)
                    }
                    Shape::Callable => {
                        let index = self.b.block_params(target).len();
                        self.b.append_block_param(target, PTR);
                        self.b.append_block_param(target, PTR);
                        Param::Ssa(index)
                    }
                    Shape::Bytes { size, align } => {
                        let shift = align.clamp(1, 16).trailing_zeros() as u8;
                        let slot = self.b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            size.max(1) as u32,
                            shift,
                        ));
                        Param::Slot(slot, size, align)
                    }
                };
                params.push(param);
            }
            self.params.push(params);
        }
        Ok(())
    }

    fn allocate_slots(&mut self) -> Emitted {
        let unit = self.unit;
        for (s, slot) in unit.slots.iter().enumerate() {
            match slot.kind {
                SlotKind::ReceiptParameter(_)
                | SlotKind::ReceiptCapture(_)
                | SlotKind::ReceiptResult => continue,
                SlotKind::Parameter(_) | SlotKind::Capture(_) => continue,
                SlotKind::Local
                | SlotKind::Temporary
                | SlotKind::ReturnSlot
                | SlotKind::LoopCarried(_)
                | SlotKind::Displaced => {}
            }
            let layout = self.lir_type(slot.ty)?.layout;
            self.slots[s] = Some(match self.constant_slots[s] {
                // Its bytes in a private read-only global named by their content, which every
                // read indexes in place and no store writes.
                Some(constant) => {
                    let align = self.layout(layout)?.align;
                    let data = self.em.constant_data(align, constant.bytes)?;
                    Place {
                        columns: None,
                        ptr: self.data_addr(data, 0),
                        layout,
                        align,
                        block: None,
                    }
                }
                None => self.stack(layout)?,
            });
        }
        for region in &unit.tables.regions {
            let (record, _) = self.stack_bytes(region.record_bytes(), 8);
            self.regions.push(record);
        }
        self.resume_storage()
    }

    /// One state of a resumable alternate: state 0 enters the body; state k
    /// reloads its live values and reissues its boundary call. Only blocks that
    /// start reaches are emitted; no boundary lies on a cycle.
    fn emit_resume(mut self) -> Emitted {
        let resume = self.resume.as_ref().unwrap();
        let (plan, state) = (resume.plan, resume.state);
        let frame = *self.entry_params.last().unwrap();
        self.resume.as_mut().unwrap().frame = Some(frame);
        self.ahead_init()?;
        self.find_versions()?;
        for _ in &self.unit.blocks {
            let block = self.b.create_block();
            self.blocks.push(block);
        }
        self.create_params()?;
        self.allocate_slots()?;
        self.bind_parameters()?;
        let start = match state {
            0 => self.unit.entry,
            k => match plan.states.get(k as usize - 1) {
                Some(s) => s.boundary.block,
                None => match plan.cleanups.iter().find(|s| s.state == k) {
                    Some(s) => s.block,
                    None => return self.invalid("a resume state outside its plan"),
                },
            },
        };
        let cleanup = plan.cleanups.iter().find(|s| s.state == state);
        let count = self.unit.blocks.len();
        let mut adjacent: Vec<Vec<usize>> = (0..count)
            .map(|b| self.successors(b))
            .collect::<Emitted<_>>()?;
        if state != 0 && cleanup.is_none() {
            let at = plan.states[state as usize - 1].boundary.block;
            adjacent[at.index()] = hirsel_lir::continuation::resumed_successors(self.unit, at)
                .iter()
                .map(|b| b.index())
                .collect();
        }
        if let Some(s) = cleanup {
            adjacent[s.block.index()] = hirsel_lir::continuation::cleanup_successors(
                self.unit,
                s.block,
                s.instruction as usize,
            )
            .iter()
            .map(|b| b.index())
            .collect();
        }
        let order = hirsel_graph::reverse_postorder(&adjacent, start.index());
        if state == 0 {
            let entry = self.blocks[self.unit.entry.index()];
            self.b.ins().jump(entry, &[]);
        } else if let Some(s) = cleanup {
            let mut skipped = vec![true; count];
            for &b in &order {
                skipped[b] = b == start.index();
            }
            self.reload_live(&s.live)?;
            let unit = self.unit;
            for (block, i) in
                hirsel_lir::continuation::resume_preparations(unit, plan, state, &skipped)
            {
                let inst =
                    &self.pool(unit.blocks[block.index()].instructions, &unit.instructions)?[i];
                self.instruction(inst)?;
            }
            self.emit_block_suffix(s.block, s.instruction as usize, true)?;
        } else {
            let s = &plan.states[state as usize - 1];
            let mut skipped = vec![true; count];
            for &b in &order {
                skipped[b] = b == start.index();
            }
            self.reload(s)?;
            let unit = self.unit;
            for (block, i) in
                hirsel_lir::continuation::resume_preparations(unit, plan, state, &skipped)
            {
                let inst =
                    &self.pool(unit.blocks[block.index()].instructions, &unit.instructions)?[i];
                self.instruction(inst)?;
            }
            self.resume.as_mut().unwrap().resuming = Some(s.boundary.call);
            self.terminator(&unit.terminators[start.index()])?;
        }
        let mut emitted = vec![false; count];
        for b in order {
            if state != 0 && b == start.index() {
                continue;
            }
            emitted[b] = true;
            self.emit_block(BlockId(b as u32))?;
        }
        // Blocks before this state's boundary never run here; a re-run
        // preparation's failure edge, whose check passed before the save, may
        // still name one.
        for (b, done) in emitted.into_iter().enumerate() {
            if !done {
                self.b.switch_to_block(self.blocks[b]);
                self.b.ins().trap(TRAP);
            }
        }
        while let Some((location, profile, deferred)) = self.deferred.pop() {
            self.b.set_srcloc(location);
            self.profile = profile;
            self.fill_deferred(deferred)?;
        }
        self.b.seal_all_blocks();
        let config = self.em.module.target_config();
        self.b.finalize(config);
        Ok(())
    }

    /// Binds each declared parameter: its value, or its slot's storage.
    fn bind_parameters(&mut self) -> Emitted {
        if self.resume.is_some() {
            return self.resume_parameters();
        }
        let unit = self.unit;
        let abi = self.sig.abi;
        if unit.parameters.len() != abi.parameters.len() {
            return self.invalid("parameter count differs from the FnAbi");
        }
        for (i, parameter) in abi.parameters.iter().enumerate() {
            let value = unit.parameters[i];
            let slot = unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(parameter.source));
            let first = self.sig.first[i];
            let layout = parameter.layout;
            let align = self.layout(layout)?.align;
            match parameter.passing {
                ParameterPassing::ByValue => {
                    let args = self.entry_params[first..first + parameter.slots.len()].to_vec();
                    let assembled = self.assemble(layout, &args)?;
                    match slot {
                        Some(s) => {
                            let place = self.stack(layout)?;
                            let shape = self.layout_shape(layout)?;
                            self.store(shape, assembled, place)?;
                            self.slots[s] = Some(place);
                        }
                        None => self.define(value, assembled)?,
                    }
                }
                ParameterPassing::ReadOnlyAddress
                | ParameterPassing::TransferAddress
                | ParameterPassing::ExclusiveAddress => {
                    let place = Place {
                        columns: None,
                        ptr: self.entry_params[first],
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
                            let shape = self.layout_shape(layout)?;
                            let loaded = self.load(shape, place);
                            self.define(value, loaded)?;
                        }
                    }
                }
                ParameterPassing::PointerLength => {
                    // The ABI pointer addresses the logical flat bytes; a variable flat
                    // value's LIR representation is its owner, whose header precedes them.
                    let pointer = self.entry_params[first];
                    match self.layout(layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            let pointer = if self.value_type(value)?.repr == ValueRepr::Uncounted {
                                pointer
                            } else {
                                match self.uncounted_payload(pointer, payload)?.block {
                                    Some(owner) => owner,
                                    None => {
                                        return self
                                            .invalid("a flat extent parameter without its owner");
                                    }
                                }
                            };
                            if let Some(s) = slot {
                                let place = self.stack(layout)?;
                                self.store_at(pointer, place.ptr, 0);
                                self.slots[s] = Some(place);
                            } else {
                                self.define1(value, pointer)?;
                            }
                        }
                        LayoutKind::Flat => {
                            let place = Place {
                                columns: None,
                                ptr: pointer,
                                layout,
                                align,
                                block: None,
                            };
                            if let Some(s) = slot {
                                self.slots[s] = Some(place);
                            } else {
                                let shape = self.layout_shape(layout)?;
                                let loaded = self.load(shape, place);
                                self.define(value, loaded)?;
                            }
                        }
                        _ => {
                            return self.invalid("a pointer/length parameter of a non-flat layout");
                        }
                    }
                }
            }
        }
        self.bind_captures()?;
        self.bind_receipt_entry()
    }

    /// Binds each capture slot to its field of the environment the body was entered with:
    /// the captures in ordinal order, each at the layout of its slot, behind the counted
    /// block header.
    pub(crate) fn bind_captures(&mut self) -> Emitted {
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
        if let Some(entry) = &unit.tables.receipt_entry
            && let Some(environment_layout) = entry.environment
        {
            entry
                .check_transport(self.sig.abi, unit, self.em.layouts())
                .map_err(|what| crate::CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: what.into(),
                })?;
            let (base, _) = hirsel_lir::receipt_closure::transport_environment(
                self.em.layouts(),
                environment_layout,
                self.sig.abi.receipts.as_ref().unwrap(),
            )
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
            let record = self.layout(base)?;
            let payload = self.payload_of(environment, environment_layout)?;
            if record.fields.len() != captures.len() {
                return self.invalid("receipt wrapper base differs from capture storage");
            }
            for ((ordinal, slot, layout), field) in captures.iter().zip(&record.fields) {
                if *ordinal != field.ordinal || *layout != field.layout {
                    return self.invalid("receipt wrapper field differs from capture slot");
                }
                self.slots[*slot] =
                    Some(self.field_place(payload, field.offset, field.layout, field.unaligned)?);
            }
            return Ok(());
        }
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
        let payload = self.offset(environment, start);
        for (s, offset, layout) in fields {
            let ptr = self.offset(payload, offset);
            let align = align.min(low_bit(offset)).min(self.layout(layout)?.align);
            self.slots[s] = Some(Place {
                columns: None,
                ptr,
                layout,
                align,
                block: Some(environment),
            });
        }
        Ok(())
    }

    /// A by-value parameter from its physical slots: none for unit, one scalar, or the
    /// scalar leaves of a multi-slot value at their layout offsets.
    pub(crate) fn assemble(&mut self, layout: LayoutId, args: &[Value]) -> Emitted<V> {
        match (self.layout_shape(layout)?, args) {
            (Shape::Unit, []) => Ok(V::Unit),
            (Shape::Scalar(ScalarKind::Bool), [byte]) => {
                Ok(V::One(self.b.ins().icmp_imm_s(IntCC::NotEqual, *byte, 0)))
            }
            (Shape::Scalar(_), [arg]) => Ok(V::One(*arg)),
            (Shape::Callable, [code, environment]) => Ok(V::Pair(*code, *environment)),
            (Shape::Bytes { size, align }, args) => {
                let leaves = &self.layout(layout)?.abi.scalars;
                if leaves.len() != args.len() {
                    return self
                        .unsupported("a by-value aggregate whose slots are not its scalar leaves");
                }
                let (addr, _) = self.stack_bytes(size, align);
                self.zero(addr, size, align);
                for (&(offset, kind), &arg) in leaves.iter().zip(args) {
                    let arg = if kind == ScalarKind::Bool {
                        self.b.ins().icmp_imm_s(IntCC::NotEqual, arg, 0)
                    } else {
                        arg
                    };
                    self.store_at(arg, addr, offset);
                }
                Ok(V::Mem { addr, slot: None })
            }
            _ => self.invalid("a by-value parameter's slots do not match its layout"),
        }
    }

    /// The physical slots of a by-value argument (the inverse of `assemble`).
    pub(crate) fn disassemble(&mut self, layout: LayoutId, value: V) -> Emitted<Vec<Value>> {
        Ok(match (self.layout_shape(layout)?, value) {
            (Shape::Unit, _) => Vec::new(),
            (Shape::Scalar(_), V::One(v)) => vec![v],
            (Shape::Callable, V::Pair(code, env)) => vec![code, env],
            (Shape::Bytes { .. }, V::Mem { addr, .. }) => {
                let mut slots = Vec::new();
                for &(offset, kind) in &self.layout(layout)?.abi.scalars {
                    slots.push(self.load_ty(scalar_type(kind), addr, offset));
                }
                slots
            }
            _ => return self.invalid("a by-value argument of another shape"),
        })
    }

    // Blocks and edges.

    fn emit_block(&mut self, b: BlockId) -> Emitted {
        let unit = self.unit;
        let block = self.blocks[b.index()];
        self.b.switch_to_block(block);
        self.source_location(
            unit.tables.source_anchors[unit.blocks[b.index()].anchor.index()].site,
        );
        // The block's parameter values.
        let parameters = self.pool(unit.blocks[b.index()].parameters, &unit.pools.values)?;
        let params = self.params[b.index()].clone();
        for (&p, param) in parameters.iter().zip(params) {
            match param {
                Param::None => {
                    if !self.versions.contains(&p) {
                        self.define(p, V::Unit)?;
                    }
                }
                Param::Ssa(index) => {
                    let values = self.b.block_params(block).to_vec();
                    let v = match self.value_shape(p)? {
                        Shape::Callable => V::Pair(values[index], values[index + 1]),
                        _ => V::One(values[index]),
                    };
                    self.define(p, v)?;
                }
                Param::Slot(slot, _, _) => {
                    let addr = self.b.ins().stack_addr(PTR, slot, 0);
                    self.define(
                        p,
                        V::Mem {
                            addr,
                            slot: Some(slot),
                        },
                    )?;
                }
            }
        }
        if self.fuel_points.get(b.index()).copied().unwrap_or(false) {
            self.charge_fuel()?;
        }
        self.emit_block_suffix(b, 0, false)
    }
    fn emit_block_suffix(&mut self, b: BlockId, start: usize, resuming: bool) -> Emitted {
        let unit = self.unit;
        for (i, inst) in self
            .pool(unit.blocks[b.index()].instructions, &unit.instructions)?
            .iter()
            .enumerate()
            .skip(start)
        {
            self.instruction_index = unit.blocks[b.index()].instructions.start as usize + i;
            self.profile_location(
                hirsel_lir::profile::instruction_site(unit, b, inst),
                inst.opcode(),
            )?;
            if !(resuming && i == start) {
                self.ownership_events(b, i as u32, hirsel_lir::ownership_trace::Moment::Before)?;
            }
            self.resume_cleanup_bind(b, i)?;
            self.instruction(inst)?;
            self.ownership_events(b, i as u32, hirsel_lir::ownership_trace::Moment::After)?;
        }
        let terminator = &unit.terminators[b.index()];
        self.profile_location(
            hirsel_lir::profile::terminator_site(unit, b),
            terminator.opcode(),
        )?;
        if self.ownership.is_some() {
            self.ownership_empty_drop(b)?;
        }
        if matches!(terminator, Terminator::Return(_)) {
            self.ownership_point(hirsel_lir::ownership_trace::Point::Return(b))?;
        }
        self.terminator(terminator)
    }

    // Runtime ABI 26 puts the running worker's heap id right after the fuel word. Cranelift's
    // code frees every block through the runtime, which checks the page owner itself, so it
    // reads neither; the layout it shares with LLVM's inline paths is pinned here too.
    const _HEAP_ID_AFTER_FUEL: () = assert!(
        hirsel_lir::runtime_abi::CTX_HEAP_ID_OFFSET == hirsel_lir::runtime_abi::CTX_FUEL_OFFSET + 8
    );

    /// One unit of the run's fuel (`EmitInputs::fuel`): the context's fuel word, the running
    /// task's chunk, loses one, and a negative result calls `hirsel_rt_v2_fuel_refill`, which
    /// stores the next chunk (or stops the run) and returns to the code after the charge.
    fn charge_fuel(&mut self) -> Emitted {
        if self.em.inputs.unit().alternate {
            return self.ahead_point(true);
        }
        let offset = hirsel_lir::runtime_abi::CTX_FUEL_OFFSET;
        let fuel = self.load_ty(types::I64, self.context, offset);
        let left = self.b.ins().iadd_imm(fuel, -1);
        self.b
            .ins()
            .store(flags(), left, self.context, offset as i32);
        let spent = self.b.ins().icmp_imm(IntCC::SignedLessThan, left, 0);
        let (cold, next) = (self.b.create_block(), self.b.create_block());
        self.b.set_cold_block(cold);
        self.b.ins().brif(spent, cold, &[], next, &[]);
        self.b.switch_to_block(cold);
        let context = self.context;
        self.rt(RuntimeEntry::FuelRefill, &[context])?;
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(next);
        Ok(())
    }

    /// A depth-leaf callee's admission (`invocations::room`): whether the task has no
    /// room for one more activation, as `hirsel_rt_v2_depth_enter_at_site` decides. In
    /// cleanup mode (nesting or cleanup count nonzero) the cleanup count is tested,
    /// otherwise the foreground count.
    pub(crate) fn no_room(&mut self) -> Value {
        use hirsel_core_contract::depth::{CLEANUP_LIMIT, FOREGROUND_LIMIT};
        use hirsel_lir::runtime_abi::{
            CTX_CLEANUP_COUNT_OFFSET, CTX_CLEANUP_MODE_OFFSET, CTX_FOREGROUND_COUNT_OFFSET,
        };
        let foreground = self.load_ty(types::I16, self.context, CTX_FOREGROUND_COUNT_OFFSET);
        let cleanup = self.load_ty(types::I16, self.context, CTX_CLEANUP_COUNT_OFFSET);
        let mode = self.load_ty(types::I32, self.context, CTX_CLEANUP_MODE_OFFSET);
        let wide = self.b.ins().uextend(types::I32, cleanup);
        let either = self.b.ins().bor(mode, wide);
        let cleaning = self.b.ins().icmp_imm(IntCC::NotEqual, either, 0);
        let cleanup_full = self.b.ins().icmp_imm(
            IntCC::UnsignedGreaterThanOrEqual,
            cleanup,
            i64::from(CLEANUP_LIMIT),
        );
        let foreground_full = self.b.ins().icmp_imm(
            IntCC::UnsignedGreaterThanOrEqual,
            foreground,
            i64::from(FOREGROUND_LIMIT),
        );
        self.b.ins().select(cleaning, cleanup_full, foreground_full)
    }

    fn source_location(&mut self, site: SiteRange) {
        if let Some(sites) = &self.debug_sites {
            self.b
                .set_srcloc(ir::SourceLoc::new(sites.code(site).unwrap_or(u32::MAX)));
        }
    }

    fn profile_location(&mut self, site: SiteRange, operation: hirsel_lir::Opcode) -> Emitted {
        self.source_location(site);
        self.profile = match self.measured {
            // Explicit source-less glue inherits its caller's active context.
            // Mapped selectors/ranges still go through strict source validation.
            Some(source) if site.source != u32::MAX => Some(crate::profile::call_site(
                self.em,
                self.unit,
                source,
                site,
                &format!("{operation:?}"),
            )?),
            _ => None,
        };
        Ok(())
    }

    /// The arguments an edge passes to its target's SSA parameters, after copying each bytes
    /// argument into its parameter slot.
    fn edge_args(&mut self, edge: &Edge) -> Emitted<Vec<BlockArg>> {
        let arguments = self.pool(edge.arguments, &self.unit.pools.values)?;
        let params = self.params[edge.target.index()].clone();
        if arguments.len() != params.len() {
            return self.invalid("edge arguments differ from the target's parameters");
        }
        let mut ssa = Vec::new();
        let mut copies = Vec::new();
        for (&argument, param) in arguments.iter().zip(&params) {
            match *param {
                Param::None => {}
                Param::Ssa(_) => match self.get(argument)? {
                    V::One(v) => ssa.push(BlockArg::Value(v)),
                    V::Pair(code, env) => {
                        ssa.push(BlockArg::Value(code));
                        ssa.push(BlockArg::Value(env));
                    }
                    _ => return self.invalid("an edge argument of another shape"),
                },
                Param::Slot(slot, size, align) => match self.get(argument)? {
                    V::Mem { addr, slot: from } => copies.push((slot, size, align, addr, from)),
                    _ => return self.invalid("an edge argument of another shape"),
                },
            }
        }
        // Copies are parallel: an argument read from another of the target's slots is
        // saved first.
        let targets: Vec<StackSlot> = copies.iter().map(|c| c.0).collect();
        let mut sources = Vec::new();
        for &(slot, size, align, addr, from) in &copies {
            match from {
                Some(from) if from == slot => sources.push(None),
                Some(from) if targets.contains(&from) => {
                    let (temp, _) = self.stack_bytes(size, align);
                    self.copy(temp, addr, size, align);
                    sources.push(Some(temp));
                }
                _ => sources.push(Some(addr)),
            }
        }
        for (&(slot, size, align, _, _), source) in copies.iter().zip(sources) {
            if let Some(source) = source {
                let to = self.b.ins().stack_addr(PTR, slot, 0);
                self.copy(to, source, size, align);
            }
        }
        Ok(ssa)
    }

    /// Branches from the current block along `edge`.
    pub(crate) fn jump(&mut self, edge: EdgeId) -> Emitted {
        self.ownership_point(hirsel_lir::ownership_trace::Point::Edge(edge))?;
        let edge = self.edge(edge)?;
        let args = self.edge_args(edge)?;
        let target = self.blocks[edge.target.index()];
        self.b.ins().jump(target, &args);
        Ok(())
    }

    /// A block that takes `edge` from a multi-way branch: the target itself when the edge
    /// passes nothing, else a fresh block passing the arguments, filled by `trampolines`.
    fn edge_block(
        &mut self,
        edge: EdgeId,
        trampolines: &mut Vec<(Block, EdgeId)>,
    ) -> Emitted<Block> {
        let e = self.edge(edge)?;
        let carries = self.params[e.target.index()]
            .iter()
            .any(|p| !matches!(p, Param::None));
        let observed = self.ownership.is_some_and(|plan| {
            plan.at_point(hirsel_lir::ownership_trace::Point::Edge(edge))
                .next()
                .is_some()
        });
        if !carries && !observed {
            return Ok(self.blocks[e.target.index()]);
        }
        let block = self.b.create_block();
        trampolines.push((block, edge));
        Ok(block)
    }

    fn fill_trampolines(&mut self, trampolines: Vec<(Block, EdgeId)>) -> Emitted {
        for (block, edge) in trampolines {
            self.b.switch_to_block(block);
            self.jump(edge)?;
        }
        Ok(())
    }

    /// The block a failure edge starts in, filled after the body.
    pub(crate) fn failure_block(&mut self, failure: FailureId) -> Emitted<Block> {
        if let Some(&block) = self.failures.get(&failure) {
            return Ok(block);
        }
        self.failure_edge(failure)?;
        let block = self.b.create_block();
        self.b.set_cold_block(block);
        self.defer(Deferred::Failure { block, failure });
        self.failures.insert(failure, block);
        Ok(block)
    }

    fn defer(&mut self, deferred: Deferred) {
        self.deferred
            .push((self.b.srcloc(), self.profile, deferred));
    }

    fn fill_deferred(&mut self, deferred: Deferred) -> Emitted {
        match deferred {
            Deferred::Failure { block, failure } => {
                self.b.switch_to_block(block);
                self.ahead_failure_barrier()?;
                let edge = self.failure_edge(failure)?;
                if edge.rule.as_deref() == Some("MEM-9") {
                    let site = self.anchor_code(edge.anchor)?;
                    let site = self.b.ins().iconst(types::I64, site as i64);
                    self.rt(RuntimeEntry::UncheckedAddOverflow, &[self.context, site])?;
                    self.b
                        .ins()
                        .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
                    return Ok(());
                }
                if let Some(rule) = &edge.rule {
                    let site = self.anchor_code(edge.anchor)?;
                    self.failure_begin(rule, site)?;
                }
                if edge.rule.is_none() {
                    let code = self.anchor_code(edge.anchor)?;
                    let (site, operation) = self.failure_sites(code);
                    let context = self.context;
                    self.rt(RuntimeEntry::FailureReanchor, &[context, site, operation])?;
                }
                self.dispose_tokens(edge)?;
                self.jump(edge.cleanup)
            }
            Deferred::CallBoundary { block, site, then } => {
                self.b.switch_to_block(block);
                self.ahead_barrier()?;
                let (site, operation) = self.failure_sites_for(site, false);
                let context = self.context;
                self.rt(RuntimeEntry::FailureReanchor, &[context, site, operation])?;
                self.b.ins().jump(then, &[]);
                Ok(())
            }
            Deferred::Record {
                block,
                rule,
                site,
                forward_caller,
                then,
            } => {
                self.b.switch_to_block(block);
                self.ahead_barrier()?;
                self.failure_begin_for(&rule, site, forward_caller)?;
                self.b.ins().jump(then, &[]);
                Ok(())
            }
            Deferred::Exhausted {
                block,
                region,
                site,
                failure,
                values,
            } => {
                self.b.switch_to_block(block);
                self.ahead_barrier()?;
                let (kind, rule) = self.failure_code(hirsel_lir::emission::BOUNDED_RULE)?;
                let (site, operation_site) = self.failure_sites(site);
                let context = self.context;
                self.rt(
                    RuntimeEntry::ArenaExhausted,
                    &[context, region, kind, rule, site, operation_site],
                )?;
                for value in values.into_iter().rev() {
                    let ty = self.value_type(value)?;
                    if self.unit.values[value.index()].holding != Holding::Owned
                        || self.layout(ty.layout)?.drop.is_empty()
                    {
                        continue;
                    }
                    let place = self.stack(ty.layout)?;
                    let shape = self.value_shape(value)?;
                    let value = self.get(value)?;
                    self.store(shape, value, place)?;
                    let descriptor = self.descriptor_value(ty.layout)?;
                    let mask = self.b.ins().iconst(PTR, 0);
                    let (continuation, _) = self.stack_bytes(32, 8);
                    self.rt1(
                        RuntimeEntry::Finalize,
                        &[
                            context,
                            descriptor,
                            place.ptr,
                            mask,
                            continuation,
                            site,
                            operation_site,
                        ],
                    )?;
                }
                let edge = self.failure_edge(failure)?;
                self.dispose_tokens(edge)?;
                self.jump(edge.cleanup)
            }
        }
    }

    /// A failure's `(kind, rule)` codes for `hirsel_rt_v2_failure_begin`: the category,
    /// and the rule's code loaded from its glue cell (`hirsel_lir::glue`).
    pub(crate) fn failure_code(&mut self, rule: &str) -> Emitted<(Value, Value)> {
        let cell = self
            .em
            .glue_data(&hirsel_lir::glue::failure_code_symbol(rule))?;
        let address = self.data_addr(cell, 0);
        let kind = self
            .b
            .ins()
            .iconst(types::I32, i64::from(hirsel_lir::emission::KIND_BUG));
        Ok((kind, self.load_ty(types::I32, address, 0)))
    }

    fn failure_begin(&mut self, rule: &str, site: u64) -> Emitted {
        self.failure_begin_for(rule, site, true)
    }
    fn failure_begin_for(&mut self, rule: &str, site: u64, forward: bool) -> Emitted {
        self.ahead_failure_barrier()?;
        let (kind, rule) = self.failure_code(rule)?;
        let (site, operation) = self.failure_sites_for(site, forward);
        let context = self.context;
        self.rt(
            RuntimeEntry::FailureBegin,
            &[context, kind, rule, site, operation],
        )?;
        Ok(())
    }

    fn dispose_tokens(&mut self, edge: &FailureEdge) -> Emitted {
        let dispose = self.pool(edge.dispose, &self.unit.pools.values)?;
        for &token in dispose {
            let token = self.get1(token)?;
            let context = self.context;
            self.rt(RuntimeEntry::Dispose, &[context, token])?;
        }
        Ok(())
    }

    /// A block that records the arena `region`'s own `OutOfSpace` at `anchor` and takes
    /// `failure`, filled after the body.
    pub(crate) fn exhausted_block(
        &mut self,
        region: Value,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted<Block> {
        self.exhausted_block_with_values(region, anchor, failure, Vec::new())
    }

    pub(crate) fn exhausted_block_with_values(
        &mut self,
        region: Value,
        anchor: SourceAnchorId,
        failure: FailureId,
        values: Vec<ValueId>,
    ) -> Emitted<Block> {
        self.failure_edge(failure)?;
        let site = self.anchor_code(anchor)?;
        let block = self.b.create_block();
        self.b.set_cold_block(block);
        self.defer(Deferred::Exhausted {
            block,
            region,
            site,
            failure,
            values,
        });
        Ok(block)
    }

    /// Branches to `failure` when `failed` (an `i8` truth value) holds and continues in a
    /// fresh block.
    pub(crate) fn guard(&mut self, failed: Value, failure: FailureId) -> Emitted {
        let fail = self.failure_block(failure)?;
        self.branch_to(failed, fail)
    }

    /// Branches to `to` when `taken` holds and continues in a fresh block.
    pub(crate) fn branch_to(&mut self, taken: Value, to: Block) -> Emitted {
        let next = self.b.create_block();
        self.b.ins().brif(taken, to, &[], next, &[]);
        self.b.switch_to_block(next);
        Ok(())
    }

    /// Branches to `failure` when a runtime entry's status is nonzero. The runtime has
    /// already recorded the failure.
    pub(crate) fn status_guard(&mut self, status: Value, failure: FailureId) -> Emitted {
        self.guard(status, failure)
    }

    /// Traps when `bad` holds: an implementation invariant, never a language failure.
    pub(crate) fn trap_if(&mut self, bad: Value) {
        self.b.ins().trapnz(bad, TRAP);
    }

    /// The site code of a source anchor.
    pub(crate) fn caller_sites(&mut self, code: u64, forward: bool) -> (Value, Value) {
        let operation = if self.unit.header.origin == UnitOrigin::Library {
            self.sig
                .hidden_index(Hidden::OperationSite)
                .map(|i| self.entry_params[i])
                .unwrap_or_else(|| self.u64c(0))
        } else {
            self.u64c(code)
        };
        let caller = if forward
            && (self.unit.header.track_caller || self.unit.header.origin == UnitOrigin::Library)
        {
            self.sig
                .hidden_index(Hidden::CallerSite)
                .map(|i| self.entry_params[i])
                .unwrap_or_else(|| self.u64c(0))
        } else {
            operation
        };
        (caller, operation)
    }
    pub(crate) fn failure_sites(&mut self, code: u64) -> (Value, Value) {
        self.failure_sites_for(code, true)
    }
    fn failure_sites_for(&mut self, code: u64, forward: bool) -> (Value, Value) {
        let (caller, operation) = self.caller_sites(code, forward);
        let present = self.b.ins().icmp_imm_s(IntCC::NotEqual, caller, 0);
        (self.b.ins().select(present, caller, operation), operation)
    }

    pub(crate) fn anchor_code(&self, anchor: SourceAnchorId) -> Emitted<u64> {
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

    /// The site code of a cleanup anchor.
    pub(crate) fn cleanup_code(&self, anchor: CleanupAnchorId) -> Emitted<u64> {
        let Some(anchor) = self.unit.tables.cleanup_anchors.get(anchor.index()) else {
            return self.invalid("a cleanup anchor outside the anchor table");
        };
        Ok(site_code(
            &self.unit.header.definition,
            &self.unit.header.implementation_revision,
            anchor.site,
        ))
    }

    // Terminators.

    /// A resumable alternate's suspension: its frame holds the recorded state.
    pub(crate) fn return_suspended(&mut self) -> Emitted {
        self.return_status(i64::from(hirsel_lir::runtime_abi::STATUS_SUSPENDED), None)
    }

    pub(crate) fn versions_contains(&self, value: ValueId) -> bool {
        self.versions.contains(&value)
    }

    /// Returns `status` and, with a register result, `value` (a zero word when absent).
    fn return_status(&mut self, status: i64, value: Option<V>) -> Emitted {
        self.ownership_leave()?;
        self.ahead_point(false)?;
        self.ahead_flush();
        let status = self.b.ins().iconst(types::I8, status);
        let mut returned = Vec::new();
        match self.sig.value {
            Some(kind) => {
                let word = match value {
                    Some(V::One(v)) => self.encode_word(kind, v),
                    Some(_) => return self.invalid("a register result of another shape"),
                    None => self.b.ins().iconst(types::I64, 0),
                };
                returned.extend([word, status]);
            }
            None => {
                returned.push(status);
            }
        }
        if self.sig.unique.is_some() {
            let root = self
                .unit
                .slots
                .iter()
                .position(|slot| slot.kind == SlotKind::Parameter(0));
            let fact = root.and_then(|slot| self.root_facts[slot]);
            let fact = match fact {
                Some(var) => self.b.use_var(var),
                None => self.b.ins().iconst(types::I8, 0),
            };
            returned.push(fact);
        }
        self.b.ins().return_(&returned);
        Ok(())
    }

    /// `value`, an SSA scalar of `kind`, as a zero-extended 64-bit word.
    fn encode_word(&mut self, kind: ScalarKind, value: Value) -> Value {
        match kind {
            ScalarKind::Pointer | ScalarKind::U64 | ScalarKind::I64 => value,
            ScalarKind::F64 => self.b.ins().bitcast(types::I64, flags(), value),
            ScalarKind::F32 => {
                let bits = self.b.ins().bitcast(types::I32, flags(), value);
                self.b.ins().uextend(types::I64, bits)
            }
            _ => self.b.ins().uextend(types::I64, value),
        }
    }

    /// The scalar of `kind` a returned word carries in its low bits.
    fn decode_word(&mut self, kind: ScalarKind, word: Value) -> Value {
        match kind {
            ScalarKind::Pointer | ScalarKind::U64 | ScalarKind::I64 => word,
            ScalarKind::Bool => {
                let byte = self.b.ins().ireduce(types::I8, word);
                self.b.ins().icmp_imm_s(IntCC::NotEqual, byte, 0)
            }
            ScalarKind::F64 => self.b.ins().bitcast(types::F64, flags(), word),
            ScalarKind::F32 => {
                let bits = self.b.ins().ireduce(types::I32, word);
                self.b.ins().bitcast(types::F32, flags(), bits)
            }
            kind => self.b.ins().ireduce(scalar_type(kind), word),
        }
    }

    fn terminator(&mut self, terminator: &'a Terminator) -> Emitted {
        if self.ahead_state.is_some()
            && self
                .em
                .inputs
                .unit()
                .ahead
                .and_then(|p| p.host_spine(self.unit.header.instance))
                .is_some()
            && match terminator {
                Terminator::Jump(_) | Terminator::Call { .. } => false,
                // Checked mapper returns transfer private data; explicit cleanup
                // retains its barriers before this owner-free control edge.
                Terminator::Return(_)
                    if self
                        .em
                        .inputs
                        .unit()
                        .ahead
                        .and_then(|plan| {
                            plan.cpu_ordinal(
                                self.unit.header.instance,
                                self.em.inputs.unit().layouts,
                            )
                        })
                        .is_some() =>
                {
                    false
                }
                Terminator::Branch {
                    condition: value, ..
                }
                | Terminator::Switch { value, .. } => !hirsel_lir::ahead::scalar_image(
                    self.em.inputs.unit().layouts,
                    self.unit.types[self.unit.values[value.index()].ty.index()].layout,
                ),
                _ => true,
            }
        {
            self.ahead_barrier()?;
        }
        match *terminator {
            Terminator::Jump(edge) => self.jump(edge),
            Terminator::Branch { condition, yes, no } => {
                let condition = self.get1(condition)?;
                let mut trampolines = Vec::new();
                let yes = self.edge_block(yes, &mut trampolines)?;
                let no = self.edge_block(no, &mut trampolines)?;
                self.b.ins().brif(condition, yes, &[], no, &[]);
                self.fill_trampolines(trampolines)
            }
            Terminator::Switch {
                value,
                arms,
                otherwise,
            } => {
                // A switch on a variant value selects by its active ordinal.
                let layout = self.value_type(value)?.layout;
                let record = self.layout(layout)?;
                let on = match (self.value_shape(value)?, &record.tag) {
                    (Shape::Bytes { .. }, Some(tag)) => {
                        let V::Mem { addr, .. } = self.get(value)? else {
                            return self.invalid("a variant value that is not bytes");
                        };
                        let tag = self.load_tag(addr, tag.offset, tag.form);
                        self.b.ins().uextend(types::I64, tag)
                    }
                    _ => {
                        let kind = self.scalar_kind(value)?;
                        if crate::scalar::integer(kind).is_none() {
                            return self.invalid("a switch on a value that is not an integer");
                        }
                        let v = self.get1(value)?;
                        match scalar_type(kind) {
                            types::I64 => v,
                            types::I128 => self.b.ins().ireduce(types::I64, v),
                            _ => self.b.ins().uextend(types::I64, v),
                        }
                    }
                };
                let mut trampolines = Vec::new();
                let mut switch = Switch::new();
                let mut seen = BTreeSet::new();
                for arm in self.pool(arms, &self.unit.pools.switch_arms)? {
                    let block = self.edge_block(arm.edge, &mut trampolines)?;
                    // The first arm of a value wins, as an ordered switch does.
                    if seen.insert(arm.value) {
                        switch.set_entry(u128::from(arm.value), block);
                    }
                }
                let otherwise = self.edge_block(otherwise, &mut trampolines)?;
                switch.emit(&mut self.b, on, otherwise);
                self.fill_trampolines(trampolines)
            }
            Terminator::Call {
                call,
                normal,
                failure,
            } => self.call_site(call, normal, failure),
            Terminator::Return(value) => {
                if self.sig.value.is_some() {
                    let Some(value) = value else {
                        return self.invalid("a return without the entry's result");
                    };
                    let v = self.get(value)?;
                    return self.return_status(0, Some(v));
                }
                if let (Some(value), Some(index)) = (value, self.sig.hidden_index(Hidden::Result)) {
                    let Some(result) = &self.sig.abi.result else {
                        return self.invalid("a result slot without a result");
                    };
                    let align = self.layout(result.layout)?.align;
                    let out = Place {
                        columns: None,
                        ptr: self.entry_params[index],
                        layout: result.layout,
                        align,
                        block: None,
                    };
                    let shape = self.value_shape(value)?;
                    let v = self.get(value)?;
                    self.store(shape, v, out)?;
                }
                self.return_status(0, None)
            }
            Terminator::Raise(failure) => {
                let block = self.failure_block(failure)?;
                self.b.ins().jump(block, &[]);
                Ok(())
            }
            Terminator::Recover {
                arena,
                caught,
                failed,
            } => {
                let arena = self.get1(arena)?;
                let context = self.context;
                let status = self.rt1(RuntimeEntry::ArenaRecover, &[context, arena])?;
                let mut trampolines = Vec::new();
                let caught = self.edge_block(caught, &mut trampolines)?;
                let failed = self.edge_block(failed, &mut trampolines)?;
                self.b.ins().brif(status, caught, &[], failed, &[]);
                self.fill_trampolines(trampolines)
            }
            Terminator::Isolate { start, caught } => {
                let start = self.get1(start)?;
                let context = self.context;
                self.rt(RuntimeEntry::IsolateCatch, &[context, start])?;
                let mut trampolines = Vec::new();
                let caught = self.edge_block(caught, &mut trampolines)?;
                self.b.ins().jump(caught, &[]);
                self.fill_trampolines(trampolines)
            }
            Terminator::Propagate => self.return_status(1, None),
            Terminator::RejectTransfer => self.return_status(
                hirsel_lir::runtime_abi::STATUS_PRETRANSFER_FAILURE as i64,
                None,
            ),
            Terminator::InvariantFault { .. } => {
                self.rt(RuntimeEntry::InvariantFault, &[])?;
                self.b.ins().trap(TRAP);
                Ok(())
            }
            Terminator::Unreachable => {
                // Proved unreachable by the validator; still a trap.
                self.b.ins().trap(TRAP);
                Ok(())
            }
        }
    }

    /// The u32 variant ordinal at `offset` bytes from `base` in `form`, as an `i32`.
    pub(crate) fn load_tag(&mut self, base: Value, offset: u64, form: TagForm) -> Value {
        match form {
            TagForm::Ordinal => self.load_ty(types::I32, base, offset),
            TagForm::LiveBit { bit } => {
                let word = self.load_ty(types::I64, base, offset);
                let set = self.b.ins().band_imm_s(word, (1u64 << bit) as i64);
                let live = self.b.ins().icmp_imm_s(IntCC::NotEqual, set, 0);
                self.b.ins().uextend(types::I32, live)
            }
            TagForm::Niche { nullary, dataful } => {
                let word = self.load_ty(PTR, base, offset);
                let null = self.b.ins().icmp_imm_s(IntCC::Equal, word, 0);
                let nullary = self.b.ins().iconst(types::I32, i64::from(nullary));
                let dataful = self.b.ins().iconst(types::I32, i64::from(dataful));
                self.b.ins().select(null, nullary, dataful)
            }
        }
    }

    /// The link name of function `symbol`, imported at `abi`.
    pub(crate) fn named_function(&mut self, symbol: SymbolId, abi: &FnAbi) -> Emitted<String> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a callee outside the symbol table");
        };
        if symbol.kind != SymbolKind::Function {
            return self.invalid("a call of a symbol that is not a function");
        }
        let name = if self.em.inputs.unit().alternate {
            let Some(id) = symbol.instance else {
                return self.invalid("unknown alternate callee");
            };
            if !self
                .em
                .inputs
                .unit()
                .ahead
                .is_some_and(|p| p.has_private_edge(self.unit.header.instance, id))
            {
                if self
                    .em
                    .inputs
                    .unit()
                    .ahead
                    .and_then(|p| p.host_spine(self.unit.header.instance))
                    .is_none()
                {
                    return self.invalid("missing real alternate callee");
                }
                symbol_name(&symbol.definition, symbol.instance.as_ref())
            } else {
                hirsel_lir::ahead::alternate_name(&symbol.definition, id)
            }
        } else {
            symbol_name(&symbol.definition, symbol.instance.as_ref())
        };
        self.em.manifest_import(
            &name,
            ObjectSymbolKind::Function,
            Some(&symbol.definition),
            Some(abi),
        );
        Ok(name)
    }

    /// The code pointer and environment block of a callable value.
    pub(crate) fn callable_parts(&mut self, callable: ValueId) -> Emitted<(Value, Value)> {
        let layout = self.value_type(callable)?.layout;
        match self.get(callable)? {
            V::Pair(code, env) => Ok((code, env)),
            V::Mem { addr, .. } => {
                let iterator = matches!(self.layout(layout)?.kind, LayoutKind::Iterator { .. });
                let (code_at, env_at) = if iterator { (8, 0) } else { (0, 8) };
                let code = self.load_ty(PTR, addr, code_at);
                let env = self.load_ty(PTR, addr, env_at);
                Ok((code, env))
            }
            _ => self.invalid("a callable of another shape"),
        }
    }

    fn call_site(&mut self, call: CallId, normal: Option<EdgeId>, failure: FailureId) -> Emitted {
        let unit = self.unit;
        let Some(site) = unit.tables.calls.get(call.index()) else {
            return self.invalid("a call outside the call table");
        };
        let abi = self.em.fn_abi(site.abi, self.definition())?;
        self.call_site_with_abi(call, normal, failure, abi)
    }

    /// Physical lowering below the unchanged unit/ABI admission boundary.
    fn call_site_with_abi(
        &mut self,
        call: CallId,
        normal: Option<EdgeId>,
        failure: FailureId,
        abi: &'a FnAbi,
    ) -> Emitted {
        let unit = self.unit;
        let Some(site) = unit.tables.calls.get(call.index()) else {
            return self.invalid("a call outside the call table");
        };
        self.check_receipt_call(site, abi)?;
        if abi.shape == CallableShape::Foreign {
            return self.foreign_call(call, abi, normal);
        }
        // Any boundary call can suspend its frame: a resumable callee, or an
        // operation that holds or parks from the saved frame (`STATUS_SUSPENDED`).
        // An operation's library body returns its entry's suspension to that
        // boundary call.
        let boundary = self.resume_boundary_call(call);
        let suspends = boundary
            || matches!(site.callee, Callee::Operation(op) if unit
                .tables
                .operations
                .get(op.index())
                .is_some_and(|id| hirsel_lir::continuation::leaves_in_library(&id.definition)));
        let nested = self.resume_boundary(call)?;
        let arguments = self.pool(site.arguments, &unit.pools.call_arguments)?;
        let mut runtime_primitive = false;
        let (code, environment, name): (Option<Value>, Option<Value>, Option<String>) = match site
            .callee
        {
            Callee::Direct(symbol) => (None, None, Some(self.named_function(symbol, abi)?)),
            Callee::Bound { code, closure } => {
                if !abi.environment {
                    return self.invalid("a bound call through an ABI without an environment");
                }
                let (_, env) = self.callable_parts(closure)?;
                (None, Some(env), Some(self.named_function(code, abi)?))
            }
            Callee::Operation(op) => {
                let Some(id) = unit.tables.operations.get(op.index()) else {
                    return self.invalid("an operation outside the operation table");
                };
                let Some(row) = self.em.inputs.unit().manifest.row(id) else {
                    return self.invalid("an operation the manifest does not list");
                };
                match row.dispositions.get(Engine::Native) {
                    Disposition::RuntimePrimitive(template) => {
                        let Some(symbol) =
                            hirsel_lir::abi::resolve_call_symbol(template, unit, site)
                        else {
                            return self.invalid(format!(
                                "runtime template `{template}` names no symbol for this call"
                            ));
                        };
                        self.em.manifest_import(
                            &symbol,
                            ObjectSymbolKind::Runtime,
                            Some(&id.definition),
                            Some(abi),
                        );
                        runtime_primitive = true;
                        (None, None, Some(symbol))
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
                let symbol = self
                    .em
                    .inputs
                    .unit()
                    .ahead
                    .filter(|_| self.em.inputs.unit().alternate)
                    .and_then(|plan| plan.cpu_callee(unit, call))
                    .cloned();
                let name = symbol.map(|symbol| {
                    let name = hirsel_lir::ahead::alternate_name(
                        &symbol.definition,
                        symbol.instance.unwrap(),
                    );
                    self.em.manifest_import(
                        &name,
                        ObjectSymbolKind::Function,
                        Some(&symbol.definition),
                        Some(abi),
                    );
                    name
                });
                (Some(code), Some(env), name)
            }
        };
        let signature = if runtime_primitive {
            self.em.runtime_signature(abi, self.definition())?
        } else {
            self.em.entry_signature(abi, self.definition())?
        };
        let helper = match site.callee {
            Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => {
                Some(&unit.tables.symbols[symbol.index()])
            }
            Callee::Indirect(_) => self
                .em
                .inputs
                .unit()
                .ahead
                .and_then(|plan| plan.cpu_callee(unit, call)),
            _ => None,
        };
        let signature = if self.em.inputs.unit().alternate
            && helper.is_some_and(|symbol| {
                let definition = &symbol.definition;
                (definition.is_core("list.push")
                    || definition.is_core("list.reserve")
                    || definition.is_core("$native.list_push_spare"))
                    && symbol.instance.is_some_and(|id| {
                        name.as_ref().is_some_and(|name| {
                            *name == hirsel_lir::ahead::alternate_name(definition, id)
                        })
                    })
            }) {
            signature.with_ahead()
        } else {
            signature
        };
        let mut args = Vec::new();
        let mut result_place = None;
        let invocation_site = self.anchor_code(site.anchor)?;
        let codes = self.caller_sites(invocation_site, site.forward_caller);
        for hidden in signature.hidden.clone() {
            match hidden {
                Hidden::Context => args.push(self.context),
                Hidden::CallerSite => args.push(codes.0),
                Hidden::OperationSite => args.push(codes.1),
                Hidden::Environment => match environment {
                    Some(env) => args.push(env),
                    None => return self.invalid("an environment argument without a callable"),
                },
                Hidden::Destination => {
                    let destination = match &site.destination {
                        Some(DestinationClass::Heap) => self.b.ins().iconst(PTR, 0),
                        Some(DestinationClass::Caller(_)) => {
                            match self.sig.hidden_index(Hidden::Destination) {
                                Some(index) => self.entry_params[index],
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
                    let place = match self.runtime_result(call, result.layout)? {
                        Some(place) => place,
                        None => self.stack(result.layout)?,
                    };
                    args.push(place.ptr);
                    result_place = Some(place);
                }
            }
        }
        let mut column_returns = Vec::new();
        for parameter in &abi.parameters {
            let Some(argument) = arguments.iter().find(|a| a.parameter == parameter.source) else {
                return self.invalid("a call without an argument for a parameter");
            };
            match (parameter.passing, argument.value) {
                (ParameterPassing::ByValue, ArgumentValue::Value(value)) => {
                    let v = self.get(value)?;
                    let slots = self.disassemble(parameter.layout, v)?;
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
                    let shape = self.value_shape(value)?;
                    let v = self.get(value)?;
                    self.store(shape, v, place)?;
                    args.push(place.ptr);
                }
                // A resumable callee's frame holds the address it is passed:
                // only a slot of this frame, proved for this edge.
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { .. })
                    if nested.is_some() =>
                {
                    let Some(proof) = self.resume_exclusive(call, parameter.source) else {
                        return self.invalid(
                            "a nested resumable callee's exclusive parameter without its edge proof",
                        );
                    };
                    let address = self.exclusive_slot_address(&proof)?;
                    args.push(address);
                }
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { address, .. }) => {
                    let place =
                        match self.runtime_argument(call, parameter.source, parameter.layout)? {
                            Some(place) => place,
                            None => self.place(address)?,
                        };
                    let passed = self.gather(place)?;
                    if place.columns.is_some() {
                        column_returns.push((place, passed));
                    }
                    args.push(passed.ptr);
                }
                (ParameterPassing::PointerLength, ArgumentValue::Value(value)) => {
                    let place = match self.layout(parameter.layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            let pointer = self.get1(value)?;
                            if self.value_type(value)?.repr == ValueRepr::Uncounted {
                                self.uncounted_payload(pointer, payload)?
                            } else {
                                self.payload(pointer, parameter.layout)?
                            }
                        }
                        LayoutKind::Flat => {
                            let place = self.stack(parameter.layout)?;
                            let shape = self.value_shape(value)?;
                            let v = self.get(value)?;
                            self.store(shape, v, place)?;
                            place
                        }
                        _ => return self.invalid("a pointer/length argument of a non-flat layout"),
                    };
                    args.push(place.ptr);
                    let extent = self.extent(place.layout, place.ptr)?;
                    args.push(extent);
                }
                _ => return self.invalid("an argument passed against its parameter's convention"),
            }
        }
        let receipt_result = self.receipt_call_arguments(site, &mut args)?;
        let root_fact = if signature.unique.is_some() {
            arguments.iter().find_map(|arg| match arg.value {
                ArgumentValue::Address { address, .. } if arg.parameter == 0 => self
                    .whole_slot(address)
                    .and_then(|slot| self.root_facts[slot.index()]),
                _ => None,
            })
        } else {
            None
        };
        if signature.ahead.is_some() {
            args.push(self.ahead_state.expect("a private job state"));
        }
        if signature.unique.is_some() {
            args.push(match root_fact {
                Some(var) => self.b.use_var(var),
                None => self.b.ins().iconst(types::I8, 0),
            });
        }
        self.read_facts_invalidate();
        let plain =
            |id| {
                self.em.inputs.unit().layouts.layout(id).is_some_and(|r| {
                    r.scan.is_empty() && r.drop.is_empty() && r.finalizer.is_none()
                })
            };
        let isolated = signature.unique.is_some()
            && !abi.environment
            && !abi.destination
            && abi.result.is_none()
            && abi
                .parameters
                .iter()
                .enumerate()
                .all(|(i, p)| i == 0 || plain(p.layout));
        let referenceless = !abi.environment
            && !abi.destination
            && abi.parameters.iter().all(|p| plain(p.layout))
            && abi.result.as_ref().is_none_or(|r| plain(r.layout));
        if !isolated && !referenceless {
            self.root_facts_invalidate();
        }
        if args.len() != signature.ir.params.len() {
            return self.invalid("a call whose arguments differ from its signature");
        }
        let observation = self
            .em
            .inputs
            .unit()
            .ahead
            .is_some_and(|p| p.cpu_observation(unit.header.instance, call));
        if !observation
            && (matches!(site.callee, Callee::Operation(_))
                || self
                    .em
                    .inputs
                    .unit()
                    .ahead
                    .and_then(|p| p.host_spine(unit.header.instance))
                    .is_some_and(|s| !s.scalar_calls.contains(&call)))
        {
            self.ahead_barrier()?;
        }
        self.host_read_boundary(call, false)?;
        self.ahead_flush();
        if let Some((root, identity, source, source_abi)) =
            self.em.inputs.unit().ahead.and_then(|p| {
                let root = p.cpu_start(unit, call)?;
                let recipe = &p.roots[&root].recipe;
                Some((
                    root,
                    p.identity,
                    recipe.source_code.clone(),
                    recipe.source_abi,
                ))
            })
        {
            let slot = unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(3))
                .and_then(|s| self.slots[s])
                .ok_or_else(|| CodegenError::Invalid {
                    definition: unit.header.definition.clone(),
                    what: "missing CPU mapper slot".into(),
                })?;
            let actual = self.load_ptr(slot.ptr);
            let name = symbol_name(&source.definition, source.instance.as_ref());
            let abi = self.em.fn_abi(source_abi, &source.definition)?;
            let signature = self.em.entry_signature(abi, &source.definition)?;
            let id = self.em.import_function(&name, &signature.ir)?;
            self.em.manifest_import(
                &name,
                ObjectSymbolKind::Function,
                Some(&source.definition),
                Some(abi),
            );
            let expected = self.func_addr(id);
            let metadata = hirsel_lir::ahead::metadata_name(root, identity);
            self.em
                .manifest_import(&metadata, ObjectSymbolKind::Data, None, None);
            let (id, _) = self
                .em
                .declare_data(&metadata, cranelift_module::Linkage::Import)?;
            let metadata = self.data_addr(id, 0);
            let version = self.load_ty(types::I32, metadata, 0);
            let certified = self.b.ins().icmp_imm_s(IntCC::Equal, version, 6);
            let matches = self.b.ins().icmp(IntCC::Equal, actual, expected);
            let matches = self.b.ins().band(certified, matches);
            let certified = self.b.ins().iconst(types::I32, 4);
            args[4] = self.b.ins().select(matches, certified, args[4]);
        }
        let ahead_root = self.em.inputs.unit().ahead.and_then(|p| {
            p.spawns
                .get(&(unit.header.instance, call))
                .map(|root| (*root, p.identity))
        });
        let mut physical_signature = signature.ir.clone();
        let name = if let Some((frame, callee)) = nested {
            // A resumable callee continues in its own frame inside this one.
            args.push(frame);
            physical_signature.params.push(ir::AbiParam::new(PTR));
            let (Callee::Direct(symbol) | Callee::Bound { code: symbol, .. }) = site.callee else {
                return self.invalid("a nested resumable call without a known callee");
            };
            let symbol = &unit.tables.symbols[symbol.index()];
            let name = if self.em.inputs.unit().alternate {
                hirsel_lir::ahead::alternate_resume_name(&symbol.definition, callee.body)
            } else {
                hirsel_lir::continuation::resume_name(&symbol.definition, callee.body, None)
            };
            self.em
                .manifest_import(&name, ObjectSymbolKind::Function, None, None);
            Some(name)
        } else if let Some((root, identity)) = ahead_root {
            let metadata = hirsel_lir::ahead::metadata_name(root, identity);
            self.em
                .manifest_import(&metadata, ObjectSymbolKind::Data, None, None);
            let (id, _) = self
                .em
                .declare_data(&metadata, cranelift_module::Linkage::Import)?;
            args.push(self.data_addr(id, 0));
            physical_signature.params.push(ir::AbiParam::new(PTR));
            let name = RuntimeEntry::TaskSpawnAhead.declaration().symbol.to_owned();
            self.em
                .manifest_import(&name, ObjectSymbolKind::Runtime, None, None);
            Some(name)
        } else {
            name
        };
        let name = if let Some(entry) = self.ordered_read(call) {
            args.splice(1..1, [codes.0, codes.1]);
            physical_signature.params.splice(
                1..1,
                [ir::AbiParam::new(types::I64), ir::AbiParam::new(types::I64)],
            );
            let helper = entry.declaration().symbol.to_owned();
            self.em
                .manifest_import(&helper, ObjectSymbolKind::Runtime, None, None);
            Some(helper)
        } else if let Some(entry) = self.ordered_host(call) {
            let helper = entry.declaration().symbol.to_owned();
            self.em
                .manifest_import(&helper, ObjectSymbolKind::Runtime, None, None);
            Some(helper)
        } else if let Some(align) = self.ordered_join_align(call) {
            args.push(self.b.ins().iconst(types::I32, align as i64));
            physical_signature
                .params
                .push(ir::AbiParam::new(types::I32));
            let helper = "hirsel_rt_v2_ordered_join".to_owned();
            self.em
                .manifest_import(&helper, ObjectSymbolKind::Runtime, None, None);
            Some(helper)
        } else {
            name
        };
        self.ownership_call(call, false)?;
        self.bind_runtime(call, name.as_deref(), codes)?;
        self.resume_tail_bind(call)?;
        let returned = match (&name, code) {
            (Some(name), _) => {
                let id = self.em.import_function(name, &physical_signature)?;
                self.call(id, &args)
            }
            (None, Some(code)) => {
                let sig_ref = self.b.import_signature(signature.ir.clone());
                let previous = self.profile_enter();
                let call = self.b.ins().call_indirect(sig_ref, code, &args);
                let returned = self.b.inst_results(call).to_vec();
                self.profile_leave(previous);
                returned
            }
            (None, None) => return self.invalid("a call without a callee"),
        };
        self.ahead_reload();
        if let (Some(var), Some((_, index))) = (root_fact, signature.unique) {
            self.b.def_var(var, returned[index]);
        }
        let status = returned[usize::from(signature.value.is_some())];
        if suspends {
            self.resume_propagate(status)?;
        }
        if let Some(rejected) = site.pretransfer_failure {
            let rejected = self.failure_block(rejected)?;
            let entered = self.b.create_block();
            let pending = self.b.ins().icmp_imm(
                IntCC::Equal,
                status,
                hirsel_lir::runtime_abi::STATUS_PRETRANSFER_FAILURE as i64,
            );
            self.b.ins().brif(pending, rejected, &[], entered, &[]);
            self.b.switch_to_block(entered);
        }
        // Both entered continuations observe the callee's mutations, including partial failure.
        for (destination, temporary) in column_returns {
            let shape = self.layout_shape(destination.layout)?;
            let value = self.load(shape, temporary);
            self.scatter(destination, shape, value)?;
        }
        self.host_read_boundary(call, true)?;
        let failure_block = self.failure_block(failure)?;
        let fail = if site.forward_caller {
            failure_block
        } else {
            let block = self.b.create_block();
            self.b.set_cold_block(block);
            let site = self.anchor_code(site.anchor)?;
            self.defer(Deferred::CallBoundary {
                block,
                site,
                then: failure_block,
            });
            block
        };
        let fail = if let Callee::Operation(op) = site.callee
            && let Some(rule) = self
                .em
                .inputs
                .unit()
                .manifest
                .row(&unit.tables.operations[op.index()])
                .and_then(|row| row.failure.as_ref())
        {
            let rule = rule.0.clone();
            if self.failure_edge(failure)?.rule.is_some() {
                return self.invalid("a runtime call failure rule must be recorded exactly once");
            }
            let block = self.b.create_block();
            self.b.set_cold_block(block);
            let site_forward_caller = site.forward_caller;
            let site = self.anchor_code(site.anchor)?;
            self.defer(Deferred::Record {
                block,
                rule,
                site,
                forward_caller: site_forward_caller,
                then: fail,
            });
            block
        } else {
            fail
        };
        self.branch_to(status, fail)?;
        // A diverging callee reports only failure: its success status is impossible.
        let Some(normal) = normal else {
            self.b.ins().trap(TRAP);
            return Ok(());
        };
        if let Some((result, place)) = receipt_result {
            let receipt = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            self.define(result, receipt)?;
        }
        if let Some(result) = site.result {
            let value = match (signature.value, result_place) {
                (Some(kind), _) => {
                    let word = returned[hirsel_lir::abi_types::REGISTER_VALUE_INDEX];
                    V::One(self.decode_word(kind, word))
                }
                (None, Some(place)) => {
                    let shape = self.value_shape(result)?;
                    self.load(shape, place)
                }
                (None, None) => return self.invalid("a call result without a result slot"),
            };
            self.define(result, value)?;
        }
        self.cpu_publish(call)?;
        self.ownership_call(call, true)?;
        self.jump(normal)
    }

    pub(crate) fn place(&self, address: AddressId) -> Emitted<Place> {
        match self.addresses.get(address.index()).copied().flatten() {
            Some(place) => Ok(place),
            None => self.invalid(format!("address {} used before its preparation", address.0)),
        }
    }

    /// Whether the value is a register result of `abi`.
    #[allow(dead_code)]
    pub(crate) fn register_result(&self, abi: &FnAbi) -> Option<ScalarKind> {
        register_result(self.em.layouts(), abi)
    }

    /// Records the import of `name`, a function of `definition`, for the manifest.
    pub(crate) fn import_definition(
        &mut self,
        name: &str,
        definition: &DefPath,
        abi: Option<&FnAbi>,
    ) {
        self.em
            .manifest_import(name, ObjectSymbolKind::Function, Some(definition), abi);
    }

    /// The digest of `abi`.
    #[allow(dead_code)]
    pub(crate) fn digest(abi: &FnAbi) -> hirsel_syntax::ContentHash {
        abi_digest(abi)
    }
}

#[cfg(test)]
mod receipt_call_tests;

#[cfg(test)]
mod guard_tests;

impl FnEmitter<'_, '_, '_> {
    fn make_counter_cell(&mut self) -> (Value, Value) {
        use hirsel_core_contract::depth::{CLEANUP_LIMIT, FOREGROUND_LIMIT};
        use hirsel_lir::runtime_abi::{
            CTX_CLEANUP_COUNT_OFFSET, CTX_CLEANUP_MODE_OFFSET, CTX_FOREGROUND_COUNT_OFFSET,
        };
        let cleanup = self.load_ty(types::I16, self.context, CTX_CLEANUP_COUNT_OFFSET);
        let mode = self.load_ty(types::I32, self.context, CTX_CLEANUP_MODE_OFFSET);
        let wide = self.b.ins().uextend(types::I32, cleanup);
        let either = self.b.ins().bor(mode, wide);
        let cleaning = self.b.ins().icmp_imm(IntCC::NotEqual, either, 0);
        let fg_at = self
            .b
            .ins()
            .iadd_imm(self.context, CTX_FOREGROUND_COUNT_OFFSET as i64);
        let cl_at = self
            .b
            .ins()
            .iadd_imm(self.context, CTX_CLEANUP_COUNT_OFFSET as i64);
        let fg_limit = self.b.ins().iconst(types::I16, i64::from(FOREGROUND_LIMIT));
        let cl_limit = self.b.ins().iconst(types::I16, i64::from(CLEANUP_LIMIT));
        (
            self.b.ins().select(cleaning, cl_at, fg_at),
            self.b.ins().select(cleaning, cl_limit, fg_limit),
        )
    }
    pub(crate) fn counter_enter(&mut self, failure: FailureId) -> Emitted {
        let (at, limit) = self
            .counter_cell
            .expect("counter scopes initialize their allowance in the prologue");
        let count = self.b.ins().load(types::I16, flags(), at, 0);
        let full = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, count, limit);
        self.guard(full, failure)?;
        let next = self.b.ins().iadd_imm(count, 1);
        self.b.ins().store(flags(), next, at, 0);
        Ok(())
    }
    pub(crate) fn counter_leave(&mut self) {
        let (at, _) = self
            .counter_cell
            .expect("counter scopes initialize their allowance in the prologue");
        let count = self.b.ins().load(types::I16, flags(), at, 0);
        let next = self.b.ins().iadd_imm(count, -1);
        self.b.ins().store(flags(), next, at, 0);
    }
}

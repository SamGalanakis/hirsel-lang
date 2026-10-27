//! Runtime declarations describe the Rust bodies, rather than pretending all C
//! calls clobber all memory. Unknown library leaves retain conservative effects.
use hirsel_lir::runtime_abi::RuntimeEntry;

use crate::context::{AllocKind, Attribute, AttributeSlot, Function, MemoryEffects, Module};

/// All runtime entries are non-unwinding extern C boundaries. This also covers
/// manifest exports and the Path comparator, which do not use RuntimeEntry.
/// Callbacks, allocation, host Io and retained pointers require the conservative
/// default: neither willreturn nor captures(none) follows from the C ABI.
pub(crate) fn declaration(module: &Module<'_>, function: Function<'_>) {
    module.add_attribute(function, AttributeSlot::Function, Attribute::NoUnwind);
    module.add_attribute(
        function,
        AttributeSlot::Function,
        Attribute::Memory(MemoryEffects::ReadWrite),
    );
}

pub(crate) fn substrate(module: &Module<'_>, function: Function<'_>, entry: RuntimeEntry) {
    use RuntimeEntry as R;
    let (memory, returns, borrowed): (_, _, &[u32]) = match entry {
        R::DepthEnter | R::DepthEnterAtSite | R::DepthLeave => {
            (MemoryEffects::ReadWrite, true, &[0])
        }
        // Read-only operand walk, with ordinary physical scratch allocation.
        R::DepthPreflight | R::StructuralOrder | R::StructuralHash => {
            (MemoryEffects::ReadWrite, false, &[])
        }
        // Scalar constants and pointer arithmetic never touch memory. Payload
        // returns its argument's provenance, so its parameter is NOT nocapture.
        R::AbiVersion | R::Payload => (MemoryEffects::None, true, &[]),
        // Capacity reads slab/prefix metadata. Keep memory(read), since the
        // metadata is outside the block's payload; it is not invariant memory.
        R::Capacity => (MemoryEffects::Read, true, &[0]),
        // These bodies only read their input and contain no callbacks, allocation
        // or unbounded loops. HashBytes walks a finite readable byte interval.
        R::HashBegin | R::Shielded | R::FailureActive | R::FailureCauseCount => {
            (MemoryEffects::ArgumentRead, true, &[0])
        }
        R::AheadState => (MemoryEffects::ArgumentRead, true, &[]),
        R::HostActive => (MemoryEffects::ArgumentRead, true, &[0]),
        R::HostEnter => (MemoryEffects::ArgumentReadWrite, true, &[0, 1]),
        R::CpuAheadEnter => (MemoryEffects::ArgumentReadWrite, true, &[0]),
        // The block pointer is retained in the fixed task-owned cleanup slot.
        R::HostDefer => (MemoryEffects::ArgumentReadWrite, true, &[0]),
        R::HostArm | R::CpuAheadArm => (MemoryEffects::ReadWrite, false, &[]),
        R::HashBytes => (MemoryEffects::ArgumentRead, true, &[1]),
        R::HashInit => (MemoryEffects::ArgumentReadWrite, true, &[0]),
        // The integer state carrier is not argument memory. The typed private
        // adapters below preserve its storage provenance during optimization.
        R::HashUpdate => (MemoryEffects::ReadWrite, true, &[1]),
        R::HashFinish => (MemoryEffects::Read, true, &[]),
        // Queries copy scalar FailureRecords; no pointer escapes. Reanchoring
        // visits the finite cause vector, without allocating or invoking code.
        R::FailurePrimary => (MemoryEffects::ArgumentReadWrite, true, &[0, 1]),
        R::FailureCause => (MemoryEffects::ArgumentReadWrite, true, &[0, 2]),
        R::FailureReanchor => (MemoryEffects::ArgumentReadWrite, true, &[0]),
        // Dup can update saturation/measurement bookkeeping as well as the count.
        R::Dup => (MemoryEffects::ReadWrite, false, &[0, 1]),
        // A constructor-tail close writes the block's header word (diagnostic mode reads it).
        R::HoleClose => (MemoryEffects::ReadWrite, false, &[0, 1]),
        // A root dup may key the block in the overflow table, so the block is not nocapture.
        R::RootDup => (MemoryEffects::ReadWrite, false, &[0]),
        // Metadata custody can reclaim a receipt DAG. Keep conservative effects
        // and do not promise termination or pointer capture properties.
        R::ReceiptRetain
        | R::ReceiptRelease
        | R::SealedRoot
        | R::SealedRelease
        | R::ReceiptRegionEnter
        | R::ReceiptRegionExit
        | R::ReceiptArenaInventory
        | R::ReceiptConstruct
        | R::ReceiptBits
        | R::SealPrepareCommit => (MemoryEffects::ReadWrite, false, &[]),
        // Failure bookkeeping can allocate/deallocate, hence memory(readwrite)
        // and no willreturn promise; the record's scalar contents are copied.
        R::FailureBegin | R::FailureClear | R::IsolateCatch => {
            (MemoryEffects::ReadWrite, false, &[0])
        }
        R::FailureAppend => (MemoryEffects::ReadWrite, false, &[0, 1]),
        // A bounded arena's bookkeeping (MEM-6): the record's words and the failure
        // state, which recording or forgetting a failure can allocate or free.
        R::ArenaBound | R::ArenaExhausted | R::ArenaRecover => {
            (MemoryEffects::ReadWrite, false, &[0, 1])
        }
        // Allocator/free-list entries retain block pointers. Cleanup/finalization
        // invokes user code, which may diverge or store pointers; host operations
        // may terminate. No stronger promise is justified for these families.
        R::CtxCreate
        | R::CtxDestroy
        | R::TaskExit
        | R::CtxStats
        | R::Trim
        | R::Alloc
        | R::CpuAheadAlloc
        | R::TaskSpawnAhead
        | R::CpuMapScoped
        | R::OrderedJoin
        | R::OrderedSleepUntil
        | R::OrderedStdinRead
        | R::OrderedWatchNext
        | R::OrderedPrint
        | R::ResumeTail
        | R::ResumeCleanup
        | R::ResumePoint
        | R::ResumeRun
        | R::AheadBarrier
        | R::AheadHandoff
        | R::Release
        | R::RootRelease
        | R::RootDrop
        | R::Free
        | R::Reset
        | R::Reuse
        | R::Dispose
        | R::SeqReserve
        | R::BlockGrow
        | R::CodecEncode
        | R::CodecDecode
        | R::UniqueCopy
        | R::UniqueCopyValue
        | R::Detach
        | R::Freeze
        | R::Thaw
        | R::Finalize
        | R::FinalizeBegin
        | R::FinalizeEnd
        | R::CloseHandle
        | R::RegionEnter
        | R::RegionReserve
        | R::ArenaAlloc
        | R::ArenaPush
        | R::ArenaReserve
        | R::ArenaRegrow
        | R::ArenaEnter
        | R::RegionOwner
        | R::RegionHold
        | R::RegionSchedule
        | R::RegionExit
        | R::BuilderBegin
        | R::BuilderGrow
        | R::BuilderPublish
        | R::InvariantFault
        | R::UncheckedAddOverflow
        | R::DiagPanic
        | R::CallbackFailed
        | R::ForeignViolation
        | R::ExitReport
        | R::FuelRefill
        | R::WorkerCreate
        | R::WorkerRetire
        | R::WorkerCollect => (MemoryEffects::ReadWrite, false, &[]),
        // A dispatch may wait for a `serial` gate or another thread, and the run may stop
        // meanwhile (S-13).
        R::ForeignEnter | R::ForeignLeave => (MemoryEffects::ReadWrite, false, &[]),
        // The calling thread's foreign context cell and the run's compute permits: no block
        // memory, no callbacks.
        R::CallbackContext | R::CallbackLeave => (MemoryEffects::ReadWrite, true, &[]),
    };
    module.add_attribute(function, AttributeSlot::Function, Attribute::Memory(memory));
    if returns {
        module.add_attribute(function, AttributeSlot::Function, Attribute::WillReturn);
    }
    // These addresses are used only as scalar output/in-out cells. Allocation
    // may retain the block stored *in* a cell, never the address of the cell.
    let cells: &[u32] = match entry {
        R::CtxStats | R::BuilderGrow | R::BuilderPublish => &[1],
        R::Reuse => &[5],
        R::UniqueCopy => &[5],
        R::ArenaPush => &[4],
        R::ArenaReserve => &[5],
        R::Detach => &[3],
        R::UniqueCopyValue => &[2],
        R::RegionReserve | R::BuilderBegin => &[4],
        _ => &[],
    };
    for &parameter in borrowed.iter().chain(cells) {
        module.add_attribute(
            function,
            AttributeSlot::Parameter(parameter),
            Attribute::CapturesNone,
        );
    }
    if entry == R::HashInit {
        module.add_attribute(function, AttributeSlot::Parameter(0), Attribute::WriteOnly);
    }
    allocator(module, function, entry);
}

/// Alloc and Free are one allocator family, as C's malloc and free: a heap block is
/// fresh (it aliases nothing live), and no program observes allocation (it cannot fail;
/// exhaustion is environmental, runtime ABI 3). So LLVM may delete a block whose only
/// uses are stores into it and its free, as clang deletes a malloc/free pair. Free reads
/// only the header words the runtime wrote; the glue frees only after reading count 1.
fn allocator(module: &Module<'_>, function: Function<'_>, entry: RuntimeEntry) {
    let (kind, slot, attribute) = match entry {
        RuntimeEntry::Alloc => (AllocKind::Alloc, AttributeSlot::Return, Attribute::NoAlias),
        RuntimeEntry::Free => (
            AllocKind::Free,
            AttributeSlot::Parameter(1),
            Attribute::AllocPtr,
        ),
        _ => return,
    };
    module.add_attribute(
        function,
        AttributeSlot::Function,
        Attribute::AllocKind(kind),
    );
    module.add_attribute(function, AttributeSlot::Function, Attribute::AllocFamily);
    module.add_attribute(function, slot, attribute);
}

/// The production uniqueness query: OWN-6 makes the count task-local, and the
/// validated operand holds a live credit. This is also the detach postcondition
/// query, so the Dup control exercises the exact same emission as source code.
pub(crate) fn is_unique<'m>(
    builder: &crate::context::Builder<'m>,
    block: crate::context::Value<'m>,
) -> crate::context::Value<'m> {
    use crate::context::{Access, IntPredicate};
    use hirsel_lir::runtime_abi::{COUNT_OFFSET, COUNT_UNIQUE};
    let module = builder.module();
    let word = builder.offset(
        block,
        module.const_int(module.int_type(64), COUNT_OFFSET.into()),
    );
    let ty = module.int_type(32);
    let count = builder.load_access(ty, word, 4, Access::Count);
    builder.icmp(
        IntPredicate::Eq,
        count,
        module.const_int(ty, COUNT_UNIQUE.into()),
    )
}

/// Whether a write may change `block` in place (`Inst::IsWritable`, MEM-6): its count is one,
/// or it is an unshared tracked arena block.
pub(crate) fn is_writable<'m>(
    builder: &crate::context::Builder<'m>,
    block: crate::context::Value<'m>,
) -> crate::context::Value<'m> {
    use crate::context::{Access, BinOp, IntPredicate};
    use hirsel_lir::runtime_abi::{COUNT_ARENA_UNIQUE, COUNT_OFFSET, COUNT_UNIQUE};
    let module = builder.module();
    let word = builder.offset(
        block,
        module.const_int(module.int_type(64), COUNT_OFFSET.into()),
    );
    let ty = module.int_type(32);
    let count = builder.load_access(ty, word, 4, Access::Count);
    let unique = builder.icmp(
        IntPredicate::Eq,
        count,
        module.const_int(ty, COUNT_UNIQUE.into()),
    );
    let unshared = builder.icmp(
        IntPredicate::Eq,
        count,
        module.const_int(ty, COUNT_ARENA_UNIQUE.into()),
    );
    builder.binary(BinOp::Or, unique, unshared)
}

/// Whether `block` is a tracked arena block (MEM-6, runtime ABI 11): its count is one of the
/// two tracked arena counts.
pub(crate) fn is_tracked<'m>(
    builder: &crate::context::Builder<'m>,
    block: crate::context::Value<'m>,
) -> crate::context::Value<'m> {
    use crate::context::{Access, BinOp, IntPredicate};
    use hirsel_lir::runtime_abi::{COUNT_ARENA_SHARED, COUNT_OFFSET};
    let module = builder.module();
    let word = builder.offset(
        block,
        module.const_int(module.int_type(64), COUNT_OFFSET.into()),
    );
    let ty = module.int_type(32);
    let count = builder.load_access(ty, word, 4, Access::Count);
    let offset = builder.binary(
        BinOp::Sub,
        count,
        module.const_int(ty, COUNT_ARENA_SHARED.into()),
    );
    builder.icmp(IntPredicate::Ult, offset, module.const_int(ty, 2))
}

/// Metadata-only capacity query. Each path reads exactly the metadata its
/// storage kind owns; selecting pointers before loading would read foreign
/// slab/prefix memory for immortal owners.
pub(crate) fn capacity<'m>(
    builder: &crate::context::Builder<'m>,
    function: Function<'m>,
    block: crate::context::Value<'m>,
) -> crate::context::Value<'m> {
    use crate::context::{BinOp, Cast};
    use hirsel_lir::runtime_abi::*;
    let module = builder.module();
    let i32_ty = module.int_type(32);
    let i64_ty = module.int_type(64);
    let word = |n: u64| module.const_int(i64_ty, n.into());
    let descriptor = builder.load(i32_ty, builder.offset(block, word(LAYOUT_OFFSET)), 4);
    let kind = builder.binary(
        BinOp::LShr,
        descriptor,
        module.const_int(i32_ty, KIND_SHIFT.into()),
    );
    let kind = builder.binary(BinOp::And, kind, module.const_int(i32_ty, 3));
    let small = module.append_block(function);
    let large = module.append_block(function);
    let immortal = module.append_block(function);
    let done = module.append_block(function);
    builder.switch(
        kind,
        immortal,
        &[
            (module.const_int(i32_ty, KIND_SMALL.into()), small),
            (module.const_int(i32_ty, KIND_LARGE.into()), large),
        ],
    );
    builder.position(small);
    let bits = builder.cast(Cast::PtrToInt, block, i64_ty);
    let base = builder.binary(BinOp::And, bits, word(!(SLAB_SIZE - 1)));
    let slab = builder.cast(Cast::IntToPtr, base, module.ptr_type());
    let pool = builder.load(i32_ty, builder.offset(slab, word(SLAB_POOL_OFFSET)), 4);
    let class = builder.binary(BinOp::And, pool, module.const_int(i32_ty, 31));
    let index = builder.cast(Cast::ZExt, class, i64_ty);
    let offset = builder.binary(BinOp::Mul, index, word(8));
    let classes =
        module.private_constant("hirsel.capacity.classes", module.array_type(i64_ty, 32), 8);
    module.set_initializer(classes, module.const_array(i64_ty, &SIZE_CLASSES.map(word)));
    let small_capacity = builder.load(i64_ty, builder.offset(classes, offset), 8);
    builder.br(done);
    builder.position(large);
    let prefix = builder.load(
        module.ptr_type(),
        builder.offset(block, word(0u64.wrapping_sub(LARGE_PREFIX_POINTER_BACK))),
        8,
    );
    let large_capacity = builder.load(
        i64_ty,
        builder.offset(prefix, word(LARGE_CAPACITY_OFFSET)),
        8,
    );
    builder.br(done);
    builder.position(immortal);
    let immortal_capacity = builder.load(
        i64_ty,
        builder.offset(block, word(0u64.wrapping_sub(IMMORTAL_CAPACITY_BACK))),
        8,
    );
    builder.br(done);
    builder.position(done);
    let usable = builder.phi(i64_ty);
    builder.add_incoming(usable, small_capacity, small);
    builder.add_incoming(usable, large_capacity, large);
    builder.add_incoming(usable, immortal_capacity, immortal);
    usable
}

/// Name prefix of the private fast path functions that [`expand_fast_paths`] inlines.
const FAST: &str = "hirsel.fast.";

/// The private function running `hirsel_rt_v2_alloc`'s fast path for small pool `pool`
/// ([`alloc_small`]), with the entry's signature and allocator attributes. It stays a
/// `noinline` call while the module is optimized, so LLVM still deletes a block that never
/// escapes together with its free, as clang deletes a malloc/free pair; afterwards
/// [`expand_fast_paths`] inlines every call that remains.
pub(crate) fn alloc_fast<'m>(
    module: &'m Module<'m>,
    runtime: Function<'m>,
    pool: u64,
) -> Function<'m> {
    fast(
        module,
        runtime,
        RuntimeEntry::Alloc,
        &format!("{FAST}alloc.{pool}"),
        6,
        |builder, function, parameters| {
            let block = alloc_small(builder, function, runtime, parameters, pool, parameters[1]);
            builder.ret(block);
        },
    )
}

/// The private function running `hirsel_rt_v2_free`'s fast path ([`free`]), with the
/// entry's signature and allocator attributes, as [`alloc_fast`].
pub(crate) fn free_fast<'m>(module: &'m Module<'m>, runtime: Function<'m>) -> Function<'m> {
    fast(
        module,
        runtime,
        RuntimeEntry::Free,
        &format!("{FAST}free"),
        2,
        |builder, function, parameters| {
            free(builder, function, runtime, parameters[0], parameters[1]);
            builder.ret_void();
        },
    )
}

fn fast<'m>(
    module: &'m Module<'m>,
    runtime: Function<'m>,
    entry: RuntimeEntry,
    name: &str,
    arity: u32,
    body: impl FnOnce(&crate::context::Builder<'m>, Function<'m>, &[crate::context::Value<'m>]),
) -> Function<'m> {
    let fresh = !module.defines(name);
    let function = module
        .function(name, runtime.ty)
        .expect("a fast path has its entry's type");
    if fresh {
        module.make_private(function);
        declaration(module, function);
        substrate(module, function, entry);
        module.add_attribute(function, AttributeSlot::Function, Attribute::NoInline);
        let builder = crate::context::Builder::new(module);
        builder.position(module.append_block(function));
        let parameters: Vec<_> = (0..arity).map(|i| module.parameter(function, i)).collect();
        body(&builder, function, &parameters);
    }
    function
}

/// Keep caller-owned hashing scratch typed while LLVM removes dead work. The
/// public runtime still carries its address in u64; these private adapters are
/// expanded after optimization, as allocation fast paths are. Their memory
/// effects describe the runtime bodies, whose only writes are to this scratch.
pub(crate) fn hash_stream<'m>(
    module: &'m Module<'m>,
    runtime: Function<'m>,
    finish: bool,
) -> Function<'m> {
    use crate::context::{Builder, Cast};
    let (pointer, word) = (module.ptr_type(), module.int_type(64));
    let (name, result, parameters, memory) = if finish {
        (
            "hirsel.fast.hash.finish",
            word,
            vec![pointer],
            MemoryEffects::ArgumentRead,
        )
    } else {
        (
            "hirsel.fast.hash.update",
            module.void_type(),
            vec![pointer, pointer, word],
            MemoryEffects::ArgumentReadWrite,
        )
    };
    let fresh = !module.defines(name);
    let function = module
        .function(name, module.function_type(result, &parameters))
        .expect("hash scratch adapter type");
    if fresh {
        module.make_private(function);
        module.add_attribute(function, AttributeSlot::Function, Attribute::NoUnwind);
        module.add_attribute(function, AttributeSlot::Function, Attribute::WillReturn);
        module.add_attribute(function, AttributeSlot::Function, Attribute::NoInline);
        module.add_attribute(function, AttributeSlot::Function, Attribute::Memory(memory));
        module.add_attribute(
            function,
            AttributeSlot::Parameter(0),
            Attribute::CapturesNone,
        );
        if !finish {
            module.add_attribute(
                function,
                AttributeSlot::Parameter(1),
                Attribute::CapturesNone,
            );
            module.add_attribute(function, AttributeSlot::Parameter(1), Attribute::ReadOnly);
        }
        let builder = Builder::new(module);
        builder.position(module.append_block(function));
        let state = builder.cast(Cast::PtrToInt, module.parameter(function, 0), word);
        if finish {
            let value = builder.call(runtime, &[state]);
            builder.ret(value);
        } else {
            builder.call(
                runtime,
                &[
                    state,
                    module.parameter(function, 1),
                    module.parameter(function, 2),
                ],
            );
            builder.ret_void();
        }
    }
    function
}

/// After optimization: every remaining fast path call is inlined into its caller, so a
/// surviving allocation or free costs its fast path's loads and stores, not a call.
pub(crate) fn expand_fast_paths(module: &Module<'_>) {
    for function in module.functions_named(FAST) {
        module.remove_attribute(function, AttributeSlot::Function, Attribute::NoInline);
        module.add_attribute(function, AttributeSlot::Function, Attribute::AlwaysInline);
    }
}

/// T03 Dup under the compiled fast path contract (runtime ABI 4, `hirsel_rt_v2.h`): a count
/// below the context's dup bound gains a holder by a store of `count + 1`; anything else (the
/// immortal sentinel, saturation, and every count when diagnostic mode or a measurement build
/// keeps the bound zero) goes to `hirsel_rt_v2_dup`.
pub(crate) fn dup<'m>(
    builder: &crate::context::Builder<'m>,
    function: Function<'m>,
    runtime: Function<'m>,
    context: crate::context::Value<'m>,
    block: crate::context::Value<'m>,
) {
    use crate::context::{Access, BinOp, IntPredicate};
    use hirsel_lir::runtime_abi::{COUNT_OFFSET, CTX_DUP_MAX_OFFSET};
    let module = builder.module();
    let (i32_ty, i64_ty) = (module.int_type(32), module.int_type(64));
    let word = |n: u64| module.const_int(i64_ty, n.into());
    let bound = builder.load(i32_ty, builder.offset(context, word(CTX_DUP_MAX_OFFSET)), 4);
    let at = builder.offset(block, word(COUNT_OFFSET));
    let count = builder.load_access(i32_ty, at, 4, Access::Count);
    let [inline, slow, done] = std::array::from_fn(|_| module.append_block(function));
    builder.cold_cond_br(builder.icmp(IntPredicate::Uge, count, bound), slow, inline);
    builder.position(inline);
    let more = builder.binary(BinOp::Add, count, module.const_int(i32_ty, 1));
    builder.store_access(more, at, 4, Access::Count);
    builder.br(done);
    builder.position(slow);
    builder.call(runtime, &[context, block]);
    builder.br(done);
    builder.position(done);
}

/// T03 Dup of a `Shared` root under the root count contract (runtime ABI 25,
/// `hirsel_rt_v2.h`): a count below the context's dup bound gains a root by a relaxed CAS of
/// `count + 1`, retried while another holder changes it; anything else (the overflow state, the
/// finite maximum, and every count when the bound is zero) goes to `hirsel_rt_v2_root_dup`.
pub(crate) fn root_dup<'m>(
    builder: &crate::context::Builder<'m>,
    function: Function<'m>,
    runtime: Function<'m>,
    context: crate::context::Value<'m>,
    block: crate::context::Value<'m>,
) {
    use crate::context::{BinOp, IntPredicate};
    use hirsel_lir::runtime_abi::{COUNT_OFFSET, CTX_DUP_MAX_OFFSET};
    let module = builder.module();
    let (i32_ty, i64_ty) = (module.int_type(32), module.int_type(64));
    let word = |n: u64| module.const_int(i64_ty, n.into());
    let bound = builder.load(i32_ty, builder.offset(context, word(CTX_DUP_MAX_OFFSET)), 4);
    let at = builder.offset(block, word(COUNT_OFFSET));
    let [retry, inline, slow, done] = std::array::from_fn(|_| module.append_block(function));
    builder.br(retry);
    builder.position(retry);
    let count = builder.load_relaxed(i32_ty, at, 4);
    builder.cold_cond_br(builder.icmp(IntPredicate::Uge, count, bound), slow, inline);
    builder.position(inline);
    let more = builder.binary(BinOp::Add, count, module.const_int(i32_ty, 1));
    let stored = builder.cmpxchg_relaxed(at, count, more);
    builder.cond_br(stored, done, retry);
    builder.position(slow);
    builder.call(runtime, &[context, block]);
    builder.br(done);
    builder.position(done);
}

/// T01 Alloc of a small block of pool `pool` (`runtime_abi::small_pool`) under the compiled
/// fast path contract: pop the context's current page for the pool, count the block live (an
/// idle page stops being idle) and write its header (count 1, the descriptor index, storage
/// kind small). A missing page or an empty allocation list calls `hirsel_rt_v2_alloc` with
/// `arguments`, as every request does when diagnostic mode, a measurement build or the system
/// allocator keeps the pages null.
fn alloc_small<'m>(
    builder: &crate::context::Builder<'m>,
    function: Function<'m>,
    runtime: Function<'m>,
    arguments: &[crate::context::Value<'m>],
    pool: u64,
    descriptor: crate::context::Value<'m>,
) -> crate::context::Value<'m> {
    use crate::context::{Access, BinOp, IntPredicate};
    use hirsel_lir::runtime_abi::{
        COUNT_OFFSET, COUNT_UNIQUE, CTX_PAGES_OFFSET, LAYOUT_OFFSET, PAGE_FREE_OFFSET,
        PAGE_LIVE_OFFSET, POOL_COUNT,
    };
    debug_assert!(pool < POOL_COUNT);
    let module = builder.module();
    let (ptr, i32_ty, i64_ty) = (module.ptr_type(), module.int_type(32), module.int_type(64));
    let word = |n: u64| module.const_int(i64_ty, n.into());
    let context = arguments[0];
    let [paged, fast, idle, counted, slow, done] =
        std::array::from_fn(|_| module.append_block(function));
    let page = builder.load(
        ptr,
        builder.offset(context, word(CTX_PAGES_OFFSET + 8 * pool)),
        8,
    );
    builder.cold_cond_br(
        builder.icmp(IntPredicate::Eq, page, module.const_zero(ptr)),
        slow,
        paged,
    );
    builder.position(paged);
    let free_at = builder.offset(page, word(PAGE_FREE_OFFSET));
    let block = builder.load(ptr, free_at, 8);
    builder.cold_cond_br(
        builder.icmp(IntPredicate::Eq, block, module.const_zero(ptr)),
        slow,
        fast,
    );
    builder.position(fast);
    // A free block's first word is its allocation-list link.
    builder.store(builder.load(ptr, block, 8), free_at, 8);
    let live_at = builder.offset(page, word(PAGE_LIVE_OFFSET));
    let live = builder.load(i32_ty, live_at, 4);
    builder.store(
        builder.binary(BinOp::Add, live, module.const_int(i32_ty, 1)),
        live_at,
        4,
    );
    builder.cold_cond_br(
        builder.icmp(IntPredicate::Eq, live, module.const_int(i32_ty, 0)),
        idle,
        counted,
    );
    builder.position(idle);
    idle_slabs(builder, context, BinOp::Sub);
    builder.br(counted);
    builder.position(counted);
    // The header: one holder, and the descriptor index with storage kind small (zero bits).
    let count = module.const_int(i32_ty, COUNT_UNIQUE.into());
    builder.store_access(
        count,
        builder.offset(block, word(COUNT_OFFSET)),
        8,
        Access::Count,
    );
    builder.store(descriptor, builder.offset(block, word(LAYOUT_OFFSET)), 4);
    builder.br(done);
    builder.position(slow);
    let called = builder.call(runtime, arguments);
    builder.br(done);
    builder.position(done);
    let result = builder.phi(ptr);
    builder.add_incoming(result, block, counted);
    builder.add_incoming(result, called, slow);
    result
}

/// One page leaves (`Sub`) or joins (`Add`) the context's idle slab count.
fn idle_slabs<'m>(
    builder: &crate::context::Builder<'m>,
    context: crate::context::Value<'m>,
    op: crate::context::BinOp,
) {
    use hirsel_lir::runtime_abi::CTX_IDLE_SLABS_OFFSET;
    let module = builder.module();
    let i64_ty = module.int_type(64);
    let at = builder.offset(
        context,
        module.const_int(i64_ty, CTX_IDLE_SLABS_OFFSET.into()),
    );
    let idle = builder.load(i64_ty, at, 8);
    builder.store(builder.binary(op, idle, module.const_int(i64_ty, 1)), at, 8);
}

/// T16 Free of a dead block (its last credit consumed, its fields released) under the compiled
/// fast path contract: a small block of the running worker's heap goes on its page's
/// `local_free` list, and a page left with no live block joins the idle count. A first
/// deferred free of a page (its `local_free` list is empty, so the page may have to join its
/// class's available list), a block of another worker's heap (its page's owner is not the
/// context's heap id, runtime ABI 26: the runtime publishes it to its owner), a large block,
/// and every block while the context's inline-free flag is zero (diagnostic mode, measurement
/// builds, the system allocator) go to `hirsel_rt_v2_free`.
// The inline free reads the page owner and the context's heap id at their runtime ABI 26
// places: right after the fuel word, and after the page's last runtime-only word.
const _: () = assert!(
    hirsel_lir::runtime_abi::CTX_HEAP_ID_OFFSET == hirsel_lir::runtime_abi::CTX_FUEL_OFFSET + 8
);
const _: () = assert!(
    hirsel_lir::runtime_abi::PAGE_OWNER_OFFSET > hirsel_lir::runtime_abi::PAGE_LOCAL_FREE_OFFSET
);

fn free<'m>(
    builder: &crate::context::Builder<'m>,
    function: Function<'m>,
    runtime: Function<'m>,
    context: crate::context::Value<'m>,
    block: crate::context::Value<'m>,
) {
    use crate::context::{BinOp, Cast, IntPredicate};
    use hirsel_lir::runtime_abi::{
        CTX_FREE_INLINE_OFFSET, CTX_HEAP_ID_OFFSET, KIND_SHIFT, KIND_SMALL, LAYOUT_OFFSET,
        PAGE_LIVE_OFFSET, PAGE_LOCAL_FREE_OFFSET, PAGE_OWNER_OFFSET, SLAB_SIZE,
    };
    let module = builder.module();
    let (ptr, i32_ty, i64_ty) = (module.ptr_type(), module.int_type(32), module.int_type(64));
    let word = |n: u64| module.const_int(i64_ty, n.into());
    let half = |n: u32| module.const_int(i32_ty, n.into());
    let [small, fast, idle, slow, done] = std::array::from_fn(|_| module.append_block(function));
    let enabled = builder.load(
        i32_ty,
        builder.offset(context, word(CTX_FREE_INLINE_OFFSET)),
        4,
    );
    let descriptor = builder.load(i32_ty, builder.offset(block, word(LAYOUT_OFFSET)), 4);
    let kind = builder.binary(BinOp::And, descriptor, half(3 << KIND_SHIFT));
    let refused = builder.binary(
        BinOp::Or,
        builder.icmp(IntPredicate::Eq, enabled, half(0)),
        builder.icmp(IntPredicate::Ne, kind, half(KIND_SMALL << KIND_SHIFT)),
    );
    builder.cold_cond_br(refused, slow, small);
    builder.position(small);
    let bits = builder.cast(Cast::PtrToInt, block, i64_ty);
    let base = builder.binary(BinOp::And, bits, word(!(SLAB_SIZE - 1)));
    let page = builder.cast(Cast::IntToPtr, base, ptr);
    let local_at = builder.offset(page, word(PAGE_LOCAL_FREE_OFFSET));
    let head = builder.load(ptr, local_at, 8);
    let owner = builder.load(i32_ty, builder.offset(page, word(PAGE_OWNER_OFFSET)), 4);
    let heap = builder.load(i32_ty, builder.offset(context, word(CTX_HEAP_ID_OFFSET)), 4);
    builder.cold_cond_br(
        builder.binary(
            BinOp::Or,
            builder.icmp(IntPredicate::Eq, head, module.const_zero(ptr)),
            builder.icmp(IntPredicate::Ne, owner, heap),
        ),
        slow,
        fast,
    );
    builder.position(fast);
    // A free block's first word is its list link; it overlays the header.
    builder.store(head, block, 8);
    builder.store(block, local_at, 8);
    let live_at = builder.offset(page, word(PAGE_LIVE_OFFSET));
    let live = builder.binary(BinOp::Sub, builder.load(i32_ty, live_at, 4), half(1));
    builder.store(live, live_at, 4);
    builder.cold_cond_br(builder.icmp(IntPredicate::Eq, live, half(0)), idle, done);
    builder.position(idle);
    idle_slabs(builder, context, BinOp::Add);
    builder.br(done);
    builder.position(slow);
    builder.call(runtime, &[context, block]);
    builder.br(done);
    builder.position(done);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;

    #[test]
    fn whole_memory_promises_include_llvm_target_state() {
        for (effect, spelling) in [
            (MemoryEffects::Read, "memory(read)"),
            (MemoryEffects::ReadWrite, "memory(readwrite)"),
        ] {
            let context = Context::new();
            let module = Module::new(&context, "whole-memory-effects");
            let function = module
                .function("probe", module.function_type(module.void_type(), &[]))
                .unwrap();
            module.add_attribute(function, AttributeSlot::Function, Attribute::Memory(effect));
            module.verify().unwrap();
            let text = module.text();
            assert!(text.contains(spelling), "{text}");
            assert!(!text.contains("target_mem:"), "{text}");
        }
    }

    #[test]
    fn read_only_leaves_and_callback_entries_have_different_promises() {
        let context = Context::new();
        let module = Module::new(&context, "runtime-effects");
        let pointer = module.ptr_type();
        let leaf = module
            .function(
                "hirsel_rt_v2_hash_begin",
                module.function_type(module.int_type(64), &[pointer]),
            )
            .unwrap();
        substrate(&module, leaf, RuntimeEntry::HashBegin);
        let callback = module
            .function(
                "hirsel_rt_v2_close_handle",
                module.function_type(
                    module.int_type(8),
                    &[pointer, module.int_type(64), module.int_type(64)],
                ),
            )
            .unwrap();
        substrate(&module, callback, RuntimeEntry::CloseHandle);
        module.verify().unwrap();
        let text = module.text();
        assert!(text.contains("ptr captures(none)"), "{text}");
        assert!(
            text.contains("nounwind willreturn memory(argmem: read)"),
            "{text}"
        );
        assert!(text.contains("nounwind memory(readwrite)"), "{text}");
        let callback = text
            .lines()
            .find(|line| line.contains("@hirsel_rt_v2_close_handle"))
            .unwrap();
        assert!(!callback.contains("captures(none)"), "{text}");
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("attributes ") && line.contains("willreturn"))
                .count(),
            1,
            "{text}"
        );
    }

    /// A block whose only uses are stores into it and its free is deleted with its free,
    /// as clang deletes a malloc/free pair; one that escapes keeps its allocation. Without
    /// the allocator attributes neither call goes.
    #[test]
    fn a_block_that_never_escapes_is_deleted_with_its_free() {
        use crate::context::{Builder, TargetMachine};
        use hirsel_lir::layout_types::TargetSpec;
        for (annotated, escapes) in [(true, false), (true, true), (false, false)] {
            let context = Context::new();
            let module = Module::new(&context, "alloc-free-pair");
            let machine =
                TargetMachine::new(&TargetSpec::x86_64_unknown_linux_gnu(), true).unwrap();
            module.set_target(&machine);
            let pointer = module.ptr_type();
            let (i8_ty, i32_ty, i64_ty) =
                (module.int_type(8), module.int_type(32), module.int_type(64));
            let alloc = module
                .function(
                    "hirsel_rt_v2_alloc",
                    module
                        .function_type(pointer, &[pointer, i32_ty, i64_ty, i64_ty, i8_ty, i64_ty]),
                )
                .unwrap();
            let free = module
                .function(
                    "hirsel_rt_v2_free",
                    module.function_type(module.void_type(), &[pointer, pointer]),
                )
                .unwrap();
            if annotated {
                substrate(&module, alloc, RuntimeEntry::Alloc);
                substrate(&module, free, RuntimeEntry::Free);
            }
            let returned = if escapes { pointer } else { i64_ty };
            let function = module
                .function("probe", module.function_type(returned, &[pointer]))
                .unwrap();
            let builder = Builder::new(&module);
            builder.position(module.append_block(function));
            let ctx = module.parameter(function, 0);
            let int = |ty, n: u64| module.const_int(ty, n.into());
            let block = builder.call(
                alloc,
                &[
                    ctx,
                    int(i32_ty, 2),
                    int(i64_ty, 8),
                    int(i64_ty, 8),
                    int(i8_ty, 1),
                    int(i64_ty, 0),
                ],
            );
            let field = builder.offset(block, int(i64_ty, 8));
            builder.store(int(i64_ty, 7), field, 8);
            if escapes {
                builder.ret(block);
            } else {
                let value = builder.load(i64_ty, field, 8);
                builder.call(free, &[ctx, block]);
                builder.ret(value);
            }
            drop(builder);
            module.verify().unwrap();
            module.run_passes("default<O3>", &machine).unwrap();
            let text = module.text();
            let calls = |name: &str| {
                text.matches(&format!("call ptr @{name}")).count()
                    + text.matches(&format!("call void @{name}")).count()
            };
            let kept = !annotated || escapes;
            assert_eq!(calls("hirsel_rt_v2_alloc"), usize::from(kept), "{text}");
            assert_eq!(
                calls("hirsel_rt_v2_free"),
                usize::from(!annotated),
                "{text}"
            );
            assert_eq!(text.contains("ret i64 7"), !escapes, "{text}");
        }
    }

    #[test]
    fn a_dup_keeps_the_next_real_uniqueness_test() {
        use crate::context::{Builder, TargetMachine};
        use hirsel_lir::layout_types::TargetSpec;
        for duplicated in [false, true] {
            let context = Context::new();
            let module = Module::new(&context, "unique-after-dup");
            let machine =
                TargetMachine::new(&TargetSpec::x86_64_unknown_linux_gnu(), true).unwrap();
            module.set_target(&machine);
            let pointer = module.ptr_type();
            let function = module
                .function(
                    "probe",
                    module.function_type(module.int_type(1), &[pointer, pointer]),
                )
                .unwrap();
            let builder = Builder::new(&module);
            builder.position(module.append_block(function));
            let ctx = module.parameter(function, 0);
            let block = module.parameter(function, 1);
            builder.assume(is_unique(&builder, block)).unwrap();
            if duplicated {
                let dup = module
                    .function(
                        "hirsel_rt_v2_dup",
                        module.function_type(module.void_type(), &[pointer, pointer]),
                    )
                    .unwrap();
                substrate(&module, dup, RuntimeEntry::Dup);
                builder.call(dup, &[ctx, block]);
            }
            builder.ret(is_unique(&builder, block));
            drop(builder);
            module.verify().unwrap();
            module.run_passes("default<O3>", &machine).unwrap();
            module.verify().unwrap();
            let text = module.text();
            // LLVM 23 removes the precondition load along with llvm.assume.
            // The post-Dup query must still read the count and compare it.
            assert_eq!(
                text.matches("load i32").count(),
                usize::from(duplicated),
                "{text}"
            );
            if duplicated {
                assert!(text.contains("call void @hirsel_rt_v2_dup"), "{text}");
                assert!(text.contains("icmp eq i32"), "{text}");
            } else {
                assert!(text.contains("ret i1 true"), "{text}");
            }
            let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("unique-after-dup-{duplicated}"));
            std::fs::create_dir_all(&root).unwrap();
            let object = root.join("probe.o");
            std::fs::write(&object, module.emit_object(&machine).unwrap()).unwrap();
            let harness = root.join("harness.c");
            std::fs::write(
                &harness,
                format!(
                    "#include <stdbool.h>\n#include <stdint.h>\n\
                     extern bool probe(void *, uint32_t *);\n\
                     void hirsel_rt_v2_dup(void *ctx, uint32_t *count) {{ (void)ctx; ++*count; }}\n\
                     int main(void) {{ uint32_t count = 1; bool result = probe(0, &count); \
                     return result != {} || count != {}; }}\n",
                    u8::from(!duplicated),
                    if duplicated { 2 } else { 1 },
                ),
            )
            .unwrap();
            let llvm = std::path::PathBuf::from(
                std::env::var_os("LLVM_SYS_231_PREFIX").expect("pinned LLVM input"),
            );
            let executable = root.join("probe");
            let linked = std::process::Command::new(llvm.join("bin/clang"))
                .arg(format!("--ld-path={}", llvm.join("bin/ld.lld").display()))
                .args(["-std=c11", "-Wall", "-Werror"])
                .arg(&harness)
                .arg(&object)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                linked.status.success(),
                "{}",
                String::from_utf8_lossy(&linked.stderr)
            );
            assert!(
                std::process::Command::new(executable)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
}

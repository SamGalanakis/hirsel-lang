//! `hirsel_rt_v2` declarations as Rust data.
//!
//! This transcribes the C interface `hirsel-rt` publishes in
//! `crates/hirsel-rt/include/hirsel_rt_v2.h`: status convention,
//! counted block header, size-class table, domain and state tags, layout
//! descriptor constants and every declared entry. Emission and the heap
//! executor read these instead of restating them; `tests/it/schema.rs` renders each
//! declaration as a C prototype and requires it verbatim in the header, so the
//! two cannot drift. Sizes and alignments are explicit u64, status and booleans
//! are u8, pointers are opaque; no Rust `Vec`, enum layout, `Result`, panic
//! payload or host `usize` crosses the ABI.
use crate::layout_types::{ScalarKind, StorageKind};
use hirsel_codec::snapshot_enum;

pub const ABI_VERSION: u32 = 90;
/// TaskSpawn physical hint bits: ordered execution and replay ignore both.
pub const SPAWN_IMMEDIATE_AWAIT: u64 = 1;
pub const SPAWN_OP_FREE: u64 = 2;

/// Caller-owned streaming SipHash state (six u64 words).
pub const HASH_STATE_BYTES: u64 = 48;
pub const HASH_STATE_ALIGN: u32 = 8;

/// Caller-owned storage of one extern call's interval (five 64-bit words, runtime ABI 29):
/// its enclosing context and site, its own context, how it was dispatched and when it began.
pub const FOREIGN_SCOPE_BYTES: u64 = 40;
pub const FOREIGN_SCOPE_ALIGN: u32 = 8;

/// `hirsel_rt_v2_foreign_enter`'s dispatch word (S-13, runtime ABI 29): the callee package's
/// `threads` in its low two bits, whether the extern is `@blocking`, and from
/// [`FOREIGN_PACKAGE_SHIFT`] the package's ordinal among the program's `unsafe ffi`
/// packages, which names its `serial` gate.
pub const FOREIGN_THREADS_HOME: u64 = 0;
pub const FOREIGN_THREADS_SERIAL: u64 = 1;
pub const FOREIGN_THREADS_ANY: u64 = 2;
pub const FOREIGN_THREADS_MASK: u64 = 3;
pub const FOREIGN_BLOCKING: u64 = 4;
pub const FOREIGN_PACKAGE_SHIFT: u32 = 8;

/// Status byte of every fallible entry: 0 success, 1 language failure.
pub const STATUS_OK: u8 = 0;
pub const STATUS_FAILURE: u8 = 1;
/// A deep copy into a bounded arena whose budget does not cover it (MEM-6, runtime
/// ABI 7): nothing is copied or recorded; the caller records the arena's `OutOfSpace`.
pub const STATUS_EXHAUSTED: u8 = 2;
/// Private call status, emitted only by validated keyed-primitive adapters.
/// Actuals remain caller-owned; this is not an entered-callee failure.
pub const STATUS_PRETRANSFER_FAILURE: u8 = 3;
/// A resumable continuation alternate saved its frame at a boundary (runtime ABI 63):
/// its live state is materialized at the frame header's state and the boundary call
/// has not run. Only continuation alternates and their callers see it.
pub const STATUS_SUSPENDED: u8 = 4;
/// Set in `hirsel_rt_v2_resume_point`'s state argument when a task that is not the
/// Ordered head is logged for the boundary's operation (every op it performs is class D).
/// A physical hint, like [`RESUME_EARLY`]: without either the runtime assumes the
/// operation waits for the head, which changes no observation.
pub const RESUME_LOGGED: u32 = 1 << 31;
/// Set when every op the boundary's operation performs commits early (§4.1 E) or is
/// logged: a non-head proceeds when nothing ahead of it can refuse the early commit.
pub const RESUME_EARLY: u32 = 1 << 30;
/// Set when the boundary's operation may leave its stack from its first op
/// (`continuation::leaves_at_boundary`, runtime ABI 69): only then may that op's wait
/// return `STATUS_SUSPENDED` for the boundary call to reissue.
pub const RESUME_LEAVES: u32 = 1 << 29;
/// Every resume hint.
pub const RESUME_HINTS: u32 = RESUME_LOGGED | RESUME_EARLY | RESUME_LEAVES;
/// Bytes of a resumable frame's header: the u32 resume state and a reserved u32.
pub const RESUME_FRAME_HEADER_BYTES: u64 = 8;
/// Launch uses a separate channel from language failure.
pub const LAUNCH_OK: u8 = 0;
pub const LAUNCH_ABI_MISMATCH: u8 = 2;
pub const LAUNCH_INVALID_CONFIG: u8 = 3;
/// `hirsel_rt_v2_config.flags`: diagnostic mode (poisoning, header checks).
pub const CONFIG_DIAGNOSTIC: u32 = 1;

/// Bytes of the counted block header v2; the payload follows it.
pub const HEADER_SIZE: u64 = 8;
/// The count word contract compiled drop glue relies on (`hirsel_rt_v2.h`; T04 Drop,
/// DropLast, DropImmortal and T16 DropEdge): a `u32` at `COUNT_OFFSET`, the holders of a
/// live block, plain (never atomic) and task-local (OWN-6). At `COUNT_UNIQUE` the next
/// release kills the block; `COUNT_IMMORTAL` is sticky and never written; any other value
/// loses a holder by a store of `count - 1`. A holder is added by the runtime, or inline
/// below the context's dup bound ([`CTX_DUP_MAX_OFFSET`]).
///
/// **`Shared` root exception** (runtime ABI 25, MEM-11): a root block's count is its exact
/// root count, atomic and never saturating. A root is added inline only by a CAS of
/// `count + 1` below the dup bound, else by `hirsel_rt_v2_root_dup`; every root release calls
/// `hirsel_rt_v2_root_release` (or reaches a walk's [`STEP_RELEASE_ROOT`]), never the inline
/// edge. Past the run's finite maximum the header holds [`ROOT_OVERFLOW`].
pub const COUNT_OFFSET: u64 = 0;
pub const COUNT_UNIQUE: u64 = 1;
/// The count value of an immortal block: sticky, never wraps.
pub const COUNT_IMMORTAL: u64 = u32::MAX as u64;
/// The header count of a `Shared` root whose exact count the runtime keeps (runtime ABI 25):
/// at or above every dup bound, so an inline CAS dup never reaches it.
pub const ROOT_OVERFLOW: u64 = u32::MAX as u64;
/// The count of a tracked arena block (a List, Array, Map or Set in an arena, MEM-6,
/// runtime ABI 11, 12 for a Map's or Set's) that is unshared: a write may change it in place, though it is never unique.
/// A dup makes it [`COUNT_ARENA_SHARED`]. Like the immortal count, neither is ever
/// released: every count at or above [`COUNT_ARENA_SHARED`] is uncounted.
pub const COUNT_ARENA_UNIQUE: u64 = u32::MAX as u64 - 1;
/// The count of a tracked arena block that is shared: its next write copies it within
/// its arena. Sticky.
pub const COUNT_ARENA_SHARED: u64 = u32::MAX as u64 - 2;
/// Byte offset of the header's descriptor index and storage-kind word.
pub const LAYOUT_OFFSET: u64 = 4;

/// The task-local small-object class table: 16-byte steps to 256
/// bytes, then quarter-power-of-two steps to 4 KiB.
pub const SIZE_CLASSES: [u64; 32] = [
    16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 320, 384, 448, 512,
    640, 768, 896, 1024, 1280, 1536, 1792, 2048, 2560, 3072, 3584, 4096,
];
pub const SMALL_MAX: u64 = 4096;
/// The largest payload extent a heap request may name: a larger one lies beyond any
/// 64-bit host's address space, and the runtime terminates it environmentally
/// (`hirsel_rt`'s `check_request`). A reservation needing more is no reservation
/// (`list.reserve`'s library body).
pub const EXTENT_MAX: u64 = i64::MAX as u64 / 2;
/// Largest alignment served from a size class; larger alignments use the large path.
pub const SMALL_ALIGN_MAX: u64 = 16;
pub const SLAB_SIZE: u64 = 64 * 1024;
/// Read-only allocation metadata used by compiled capacity queries. Small
/// headers mask their address to SLAB_SIZE and read the u32 pool (modulo 32).
/// Large headers load a prefix pointer eight bytes before the header; immortal
/// static/region headers have an inline Extent immediately before the header, except
/// arena blocks of a [`LAYOUT_NO_EXTENT`] layout, whose capacity is the layout size.
pub const SLAB_POOL_OFFSET: u64 = 32;
pub const LARGE_PREFIX_POINTER_BACK: u64 = 8;
pub const LARGE_CAPACITY_OFFSET: u64 = 32;
pub const IMMORTAL_CAPACITY_BACK: u64 = 16;
/// A tracked arena block (runtime ABI 11) has one more prefix word before its extent (a
/// fixed layout's too, runtime ABI 12, whose extent words are unused):
/// the address of its arena's record, [`TRACKED_REGION_BACK`] bytes before the header,
/// so [`TRACKED_PREFIX_BYTES`] precede its payload in all, the header's eight included.
pub const TRACKED_REGION_BACK: u64 = 24;
pub const TRACKED_PREFIX_BYTES: u64 = 32;
/// `flags` of `hirsel_rt_v2_arena_alloc` and `hirsel_rt_v2_unique_copy`: the new block,
/// or a deep copy's root, is a tracked arena block (runtime ABI 11).
pub const ALLOC_TRACKED: u32 = 1;

/// Compiled fast paths (runtime ABI 4, `hirsel_rt_v2.h`). The run context begins with a
/// `u32` dup bound (a count below it gains a holder by a store of `count + 1`), a `u32`
/// inline-free flag, the `u64` count of idle slabs (pages with no live block) and
/// [`POOL_COUNT`] page pointers, the current allocation page of each small pool or null. A
/// page is a slab: its `free` allocation list, `u32` live count and `local_free` list are at
/// the `PAGE_*` offsets. A zero bound, a zero flag and null pages (diagnostic mode,
/// measurement builds, the system allocator) send every dup, free and allocation to its
/// runtime entry.
pub const CTX_DUP_MAX_OFFSET: u64 = 0;
pub const CTX_FREE_INLINE_OFFSET: u64 = 4;
pub const CTX_IDLE_SLABS_OFFSET: u64 = 8;
pub const CTX_PAGES_OFFSET: u64 = 16;
pub const POOL_COUNT: u64 = 64;
/// After the pages, the running task's `i64` fuel chunk (`budgets.limits@2`, runtime ABI 27): a
/// fuel build's code subtracts one at each fuel point and, when the result is negative, calls
/// `hirsel_rt_v2_fuel_refill`, which stores the next chunk in the word and returns (or stops the
/// run), so the code continues after the call.
pub const CTX_FUEL_OFFSET: u64 = CTX_PAGES_OFFSET + 8 * POOL_COUNT;
/// After the fuel word, the `u32` id of the running worker's heap (runtime ABI 26). A page's
/// `u32` owner at [`PAGE_OWNER_OFFSET`] is its heap's id: compiled code frees a small block
/// inline only on a page whose owner equals it, and sends any other to `hirsel_rt_v2_free`,
/// which publishes it on the page's [`PAGE_XTHREAD_FREE_OFFSET`] list for its owner.
pub const CTX_HEAP_ID_OFFSET: u64 = CTX_FUEL_OFFSET + 8;
/// After the view (the compiled words, then its `u32` heap and `u64` epoch stamp), the
/// task's LANG-10 counters (runtime ABI 65): the `u16` foreground and cleanup admission
/// counts, then the `u32` finalizer nesting. An admission in cleanup mode (nesting or
/// cleanup count nonzero) has room while the cleanup count is below
/// `depth::CLEANUP_LIMIT`, otherwise while the foreground count is below
/// `depth::FOREGROUND_LIMIT`. Validated native single-call scopes update their
/// selected counter directly; depth-leaf callees need only a read-only room
/// check (`invocations::room`). Runtime callback scopes retain their sites.
pub const CTX_FOREGROUND_COUNT_OFFSET: u64 = CTX_HEAP_ID_OFFSET + 24;
pub const CTX_CLEANUP_COUNT_OFFSET: u64 = CTX_FOREGROUND_COUNT_OFFSET + 2;
pub const CTX_CLEANUP_MODE_OFFSET: u64 = CTX_FOREGROUND_COUNT_OFFSET + 4;
pub const PAGE_FREE_OFFSET: u64 = 16;
pub const PAGE_LIVE_OFFSET: u64 = 24;
pub const PAGE_LOCAL_FREE_OFFSET: u64 = 72;
pub const PAGE_OWNER_OFFSET: u64 = 96;
pub const PAGE_XTHREAD_FREE_OFFSET: u64 = 104;
const _: () = assert!(CTX_HEAP_ID_OFFSET == 536 && CTX_FOREGROUND_COUNT_OFFSET == 560);

/// The small pool serving `extent` payload bytes at `align`: the size class (16-byte steps to
/// 256 bytes, then quarter powers of two to [`SMALL_MAX`]), offset by the class count when the
/// alignment is above 8. `None` for a large request.
pub const fn small_pool(extent: u64, align: u64) -> Option<u64> {
    if extent > SMALL_MAX || align > SMALL_ALIGN_MAX || !align.is_power_of_two() {
        return None;
    }
    let class = if extent <= 256 {
        extent.saturating_sub(1) / 16
    } else {
        let above = extent - 1;
        let p = 63 - above.leading_zeros() as u64;
        16 + (p - 8) * 4 + ((above - (1 << p)) >> (p - 2))
    };
    Some(
        class
            + if align > 8 {
                SIZE_CLASSES.len() as u64
            } else {
                0
            },
    )
}

/// `class_index` of a large-object block.
pub const LARGE_CLASS: u8 = 0xff;

/// Compiler-local region segments and the MEM-10 idle pool cap per task.
pub const SEGMENT_SIZE: u64 = 64 * 1024;
pub const REGION_POOL_IDLE_CAP: u64 = 8 << 20;
/// Arena bump ABI: cursor is mutable; end is the inline limit (null with diagnostics).
pub const REGION_CURSOR_OFFSET: u64 = 8;
pub const REGION_END_OFFSET: u64 = 16;
/// The logical bytes a bounded arena (MEM-6, runtime ABI 7) has left; an arena
/// construction subtracts its charge inline after testing it fits. Unbounded is
/// `u64::MAX`.
pub const REGION_REMAINING_OFFSET: u64 = 48;
/// Bytes of `hirsel_rt_v2_region`.
pub const REGION_RECORD_BYTES: u64 = 64;
/// Ordinary region prefix plus inventory and sealed-owner words. This reserves
/// storage only; the checked receipt entry must initialize its private journal.
pub const RECEIPT_REGION_RECORD_BYTES: u64 = 80;

/// Layout descriptor constants (`hirsel_rt_v2_layout`, `hirsel_rt_v2_step`);
/// `rt_program` lowers layout drop programs to these steps.
pub const LAYOUT_MAGIC: u32 = 0x484e_4c31;
pub const LAYOUT_DESCRIPTOR_VERSION: u32 = 18;
/// The finalizer belongs to an `unsafe ffi` package: its failure terminates (FAIL-3).
pub const LAYOUT_UNSAFE_FFI: u32 = 1;
/// The size is dynamic; the descriptor's size is the extent every value has.
pub const LAYOUT_DYNAMIC: u32 = 2;
/// A fixed-size layout that is not flat (descriptor version 5): its arena blocks carry no
/// extent before the header; their capacity is the descriptor size and their alignment
/// `max(align, 8)`.
pub const LAYOUT_NO_EXTENT: u32 = 8;
/// The descriptor's `logical` is per element (descriptor version 6): a block's charge
/// is it times the u64 count at the payload's start.
pub const LAYOUT_LOGICAL_PER_ELEMENT: u32 = 16;
/// `logical` of a layout with no logical size (a flat encoding or an environment): a
/// deep copy of such a block into a bounded arena is an invariant fault.
pub const LOGICAL_UNSUPPORTED: u64 = u64::MAX;
/// The payload of a `Shared` root block (MEM-11): the blocks its value reaches are frozen
/// while the root lives, and a deep copy reaching such a block adds a root instead.
pub const LAYOUT_FROZEN: u32 = 32;
/// The payload is a sequence's (descriptor version 7): a deep copy into an arena tracks
/// a block of it (MEM-6) unless it lies below a [`LAYOUT_UNTRACKED_BELOW`] block.
pub const LAYOUT_SEQUENCE: u32 = 64;
/// A block of this payload (a Slots', a closure environment's) keeps the
/// sequences it reaches untracked when a deep copy puts them in an arena (descriptor
/// version 7).
pub const LAYOUT_UNTRACKED_BELOW: u32 = 128;
/// A Map's, Set's or SortedMap's block payload (descriptor version 8, MEM-6): its entries List at
/// payload offset 0, then its index when it has one. Its own block charges nothing. A deep copy into a
/// bounded arena tracks the block and its entries List, charged as any List, and neither
/// tracks nor charges its index.
pub const LAYOUT_KEYED: u32 = 256;
/// The payload is an `@soa` List's columns (descriptor version 9, MEM-6): a deep copy into
/// an arena tracks a block of it as a [`LAYOUT_SEQUENCE`] one. A push to a tracked one is
/// charged and copies a shared one at its capacity; growth regrows it in its arena
/// (`hirsel_rt_v2_arena_regrow`), the compiled column copy moving its columns.
pub const LAYOUT_COLUMNS: u32 = 512;
/// The payload is a sequential sequence's, of `Packed` values (descriptor version 10, MEM-6,
/// MEM-13): a deep copy into an arena tracks a block of it as a [`LAYOUT_SEQUENCE`] one. A push
/// to a tracked one is charged and copies a shared one at its capacity; `hirsel_rt_v2_block_grow`
/// regrows a tracked one in its arena.
pub const LAYOUT_SEQUENTIAL: u32 = 1024;
/// An atomic root wrapping a mutex cell word (ABI 31). Releases apply immediately;
/// its final drop may run user code and fail, through RootDrop.
pub const LAYOUT_MUTEX_ROOT: u32 = 2048;
pub const STEP_RELEASE: u32 = 1;
pub const STEP_FINALIZE: u32 = 2;
pub const STEP_CLOSE_HANDLE: u32 = 3;
pub const STEP_TAG: u32 = 4;
pub const STEP_LOOP: u32 = 5;
pub const STEP_COLUMN: u32 = 6;
/// Unless the pointer word at `offset` is non-null, skip the next `span` steps:
/// the dataful arm of a null-pointer niche (layout version 2).
pub const STEP_NONNULL: u32 = 7;
pub const STEP_LIVE_BIT: u32 = 8;
/// Restore ordinary counts in the frozen value at `offset`, read through `layout`, before
/// the steps that release it: the first step of a `Shared` root block's program (MEM-11).
pub const STEP_THAW: u32 = 9;
/// A [`STEP_RELEASE`] after which its value runs nothing more (descriptor version 11): the
/// last step of the program, or the last of a variant arm that is itself last, never inside a
/// loop. When it kills its child the dying block is freed at once and the child takes its
/// place (a tail release); compiled drop glue frees in the same order.
pub const STEP_RELEASE_LAST: u32 = 10;
/// Release the `Shared` root reference stored at `offset` (descriptor version 13, MEM-11)
/// under the root count contract (`hirsel_rt_v2_root_release`): exact, never the inline edge,
/// and deferred where the run's mode defers root releases. Never a tail release.
pub const STEP_RELEASE_ROOT: u32 = 11;
/// Retire a language finalizer after all of its fields, including on failure.
pub const STEP_END_FINALIZE: u32 = 12;
/// Consume one private semantic metadata holder, independently of language credits.
pub const STEP_RELEASE_RECEIPT: u32 = 13;
/// Consume one opaque affine sealed owner, including its storage and Shared credits.
pub const STEP_RELEASE_SEALED: u32 = 14;
/// The finalizer is compiler implementation cleanup, adding no invocation itself.
pub const LAYOUT_IMPLEMENTATION_FINALIZER: u32 = 4096;
/// Step bits of the logical dead cursor; wide programs preserve forty mixed-radix
/// bits in the eight-byte header and a reserved four-byte shadow.
pub const STEP_CURSOR_BITS: u32 = 24;
/// A step field that is never masked out by a partial live mask.
pub const WHOLE_VALUE: u32 = u32::MAX;
/// `count_offset` of a loop with a static count.
pub const NO_OFFSET: u64 = u64::MAX;

/// Diagnostic-mode poison bytes.
pub const FREE_POISON: u8 = 0xdf;
pub const UNINIT_POISON: u8 = 0xa5;

/// Storage-domain discriminator bits of a header. Only blocks with a header
/// have one: region, arena and frozen interiors carry no count.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum DomainTag {
    Heap = 1,
    Static = 2,
    /// A compiler-reserved cleanup continuation for an inline value.
    Region = 3,
}

impl DomainTag {
    /// The header tag of a storage kind, if blocks of that kind have a header.
    pub const fn of(kind: StorageKind) -> Option<Self> {
        match kind {
            StorageKind::Heap => Some(Self::Heap),
            StorageKind::Static => Some(Self::Static),
            StorageKind::LocalRegion | StorageKind::Arena | StorageKind::Frozen => None,
        }
    }
}

/// Lifecycle state byte of a header.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum BlockState {
    Free = 0,
    Live = 1,
    /// Dead and on the worklist; its own finalizer has not started.
    FinalizePending = 2,
    /// DROP-2's `Finalizing`; cannot be entered again.
    Finalizing = 3,
    FieldsPending = 4,
    /// Raw storage held by a reuse token.
    Raw = 5,
}

/// Fields of the counted block header v2 (`hirsel_rt_v2_header`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum HeaderField {
    Count,
    Descriptor,
}

/// The two u32 words of the eight-byte header, whose payload always follows at byte eight.
pub const HEADER_V2: [(HeaderField, ScalarKind, u64); 2] = [
    (HeaderField::Count, ScalarKind::U32, 0),
    (HeaderField::Descriptor, ScalarKind::U32, 4),
];
pub const DESCRIPTOR_INDEX_MASK: u32 = 0x1fff_ffff;
pub const KIND_SHIFT: u32 = 29;
pub const KIND_SMALL: u32 = 0;
pub const KIND_LARGE: u32 = 1;
pub const KIND_STATIC: u32 = 2;
pub const KIND_REGION: u32 = 3;
pub const WIDE_DEAD: u32 = 1 << 31;
pub const LAYOUT_WIDE_CURSOR: u32 = 4;

/// Where an allocation request is served.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SizeClass {
    /// Class index into `SIZE_CLASSES`; slab metadata gives this capacity.
    Small { index: u8, capacity: u64 },
    /// The backing allocator directly; capacity is the request.
    Large { capacity: u64 },
}

/// The class serving `bytes` at `align`. A zero-byte request takes the first class.
pub fn size_class(bytes: u64, align: u64) -> SizeClass {
    if align <= SMALL_ALIGN_MAX
        && let Some(index) = SIZE_CLASSES.iter().position(|&c| bytes <= c)
    {
        return SizeClass::Small {
            index: index as u8,
            capacity: SIZE_CLASSES[index],
        };
    }
    SizeClass::Large {
        capacity: bytes.checked_add(15).map(|n| n & !15).unwrap_or(bytes),
    }
}

/// Named C types of the interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CName {
    Void,
    U8,
    U32,
    U64,
    /// `TaskCtx`: one task's context (its view of its worker's heap words, release worklist and
    /// failure state, its worker and its run's configuration and Io adapter). Not a source
    /// capability.
    Ctx,
    Layout,
    Header,
    Cleanup,
    Region,
    Builder,
    Config,
    Stats,
    Audit,
    Failure,
    Codec,
    ReportNode,
    TaskCode,
    SpawnAheadInfo,
    AheadPhysical,
    HostReadRecipe,
}

impl CName {
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Void => "void",
            Self::U8 => "uint8_t",
            Self::U32 => "uint32_t",
            Self::U64 => "uint64_t",
            Self::Ctx => "hirsel_rt_v2_ctx",
            Self::Layout => "hirsel_rt_v2_layout",
            Self::Header => "hirsel_rt_v2_header",
            Self::Cleanup => "hirsel_rt_v2_cleanup",
            Self::Region => "hirsel_rt_v2_region",
            Self::Builder => "hirsel_rt_v2_builder",
            Self::Config => "hirsel_rt_v2_config",
            Self::Stats => "hirsel_rt_v2_stats",
            Self::Audit => "hirsel_rt_v2_audit",
            Self::Failure => "hirsel_rt_v2_failure",
            Self::Codec => "hirsel_rt_v2_codec",
            Self::ReportNode => "hirsel_rt_v2_report_node",
            Self::TaskCode => "hirsel_rt_v2_task_code",
            Self::SpawnAheadInfo => "hirsel_rt_v2_spawn_ahead_info",
            Self::AheadPhysical => "hirsel_rt_v2_ahead_physical",
            Self::HostReadRecipe => "hirsel_rt_v2_host_read_recipe",
        }
    }
}

/// A C type: a named type behind `pointers` levels of indirection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CType {
    pub name: CName,
    pub constant: bool,
    pub pointers: u8,
}

impl CType {
    pub fn spelling(self) -> String {
        format!(
            "{}{}{}",
            if self.constant { "const " } else { "" },
            self.name.spelling(),
            if self.pointers == 0 {
                String::new()
            } else {
                format!(" {}", "*".repeat(self.pointers as usize))
            }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CParam {
    pub name: &'static str,
    pub ty: CType,
}

/// Every entry `hirsel_rt_v2.h` declares.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum RuntimeEntry {
    DepthEnter,
    DepthEnterAtSite,
    DepthLeave,
    DepthPreflight,
    StructuralOrder,
    StructuralHash,
    AbiVersion,
    CtxCreate,
    CtxDestroy,
    TaskExit,
    CtxStats,
    Trim,
    Alloc,
    Payload,
    Capacity,
    Dup,
    Release,
    Free,
    Reset,
    Reuse,
    Dispose,
    SeqReserve,
    BlockGrow,
    HashBegin,
    HashBytes,
    HashInit,
    HashUpdate,
    HashFinish,
    CodecEncode,
    CodecDecode,
    UniqueCopy,
    UniqueCopyValue,
    Detach,
    Freeze,
    Thaw,
    RootDup,
    RootRelease,
    RootDrop,
    Finalize,
    FinalizeBegin,
    FinalizeEnd,
    CloseHandle,
    Shielded,
    RegionEnter,
    RegionReserve,
    RegionOwner,
    RegionHold,
    RegionSchedule,
    RegionExit,
    ArenaAlloc,
    ArenaPush,
    ArenaReserve,
    ArenaRegrow,
    ArenaEnter,
    ArenaBound,
    ArenaExhausted,
    ArenaRecover,
    BuilderBegin,
    BuilderGrow,
    BuilderPublish,
    FailureBegin,
    FailureAppend,
    FailureActive,
    FailurePrimary,
    FailureCauseCount,
    FailureCause,
    FailureReanchor,
    FailureClear,
    IsolateCatch,
    InvariantFault,
    DiagPanic,
    /// Before an extern call: dispatch it to its effective thread (S-13) and push that
    /// thread's run context and source site.
    ForeignEnter,
    /// After an extern call: restore its enclosing context and source site, leave its
    /// `serial` gate and return the task from the thread it ran on.
    ForeignLeave,
    /// A null violated a foreign contract: report the site and end the process.
    ForeignViolation,
    /// In a C callback thunk: the run context of its thread's extern call.
    CallbackContext,
    /// A callback failed: the process ends (PROG-4).
    CallbackFailed,
    ExitReport,
    /// A fuel point found the running task's chunk spent: the runtime stores the next chunk
    /// and returns, or ends the run with BudgetExceeded (runtime ABI 27).
    FuelRefill,
    /// A context for a new worker of a run, on its own heap (runtime ABI 26).
    WorkerCreate,
    /// A worker retires; its heap stays in the run, orphaned.
    WorkerRetire,
    /// A worker's scheduling point: collect storage other workers freed to its heap.
    WorkerCollect,
    /// A C callback thunk returns to C: its Hirsel code gives back its compute permit.
    CallbackLeave,
    /// A failed unchecked-add contract: ends the run with a MEM-9 diagnostic.
    UncheckedAddOverflow,
    TaskSpawnAhead,
    AheadBarrier,
    AheadState,
    AheadHandoff,
    HostArm,
    HostEnter,
    HostActive,
    HostDefer,
    ReceiptRetain,
    ReceiptRelease,
    SealedRoot,
    SealedRelease,
    /// A resumable frame reached a boundary with its live state materialized: proceed
    /// with the boundary call (0) or return `STATUS_SUSPENDED` (1). The state argument
    /// carries [`RESUME_HINTS`] for an operation a non-head can issue.
    ResumePoint,
    /// A resumable task body: run its dispatcher over an owned compact frame, saving and
    /// resuming the frame at each `STATUS_SUSPENDED`, until it returns.
    ResumeRun,
    /// Bind a validated entered-tail site and retained nested frame.
    ResumeTail,
    ResumeCleanup,
    /// A constructor-tail block's payload is filled: its header takes the box's layout in
    /// place of the open stand-in (LANG-10, LIR schema revision 59, runtime ABI 70).
    HoleClose,
    ReceiptRegionEnter,
    ReceiptRegionExit,
    ReceiptBits,
    SealPrepareCommit,
    /// Enter/resume a certified Ordered join without re-taking its handle.
    OrderedJoin,
    OrderedSleepUntil,
    OrderedPrint,
    OrderedStdinRead,
    OrderedWatchNext,
    CpuMapScoped,
    ReceiptConstruct,
    ReceiptArenaInventory,
    CpuAheadArm,
    CpuAheadEnter,
    CpuAheadAlloc,
}

/// One C declaration of `hirsel_rt_v2`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RuntimeDeclaration {
    pub entry: RuntimeEntry,
    pub symbol: &'static str,
    pub returns: CType,
    pub parameters: &'static [CParam],
}

impl RuntimeDeclaration {
    /// The C prototype, spelled as in the header with single spaces.
    pub fn c_prototype(&self) -> String {
        let parameters = if self.parameters.is_empty() {
            "void".to_string()
        } else {
            self.parameters
                .iter()
                .map(|p| {
                    let ty = p.ty.spelling();
                    let gap = if ty.ends_with('*') { "" } else { " " };
                    format!("{ty}{gap}{}", p.name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let returns = self.returns.spelling();
        let gap = if returns.ends_with('*') { "" } else { " " };
        format!("{returns}{gap}{}({parameters});", self.symbol)
    }
}

const fn ty(name: CName) -> CType {
    CType {
        name,
        constant: false,
        pointers: 0,
    }
}
const fn ptr(name: CName, pointers: u8) -> CType {
    CType {
        name,
        constant: false,
        pointers,
    }
}
const fn cptr(name: CName) -> CType {
    CType {
        name,
        constant: true,
        pointers: 1,
    }
}
const fn p(name: &'static str, ty: CType) -> CParam {
    CParam { name, ty }
}

const VOID: CType = ty(CName::Void);
const U8: CType = ty(CName::U8);
const U32: CType = ty(CName::U32);
const U64: CType = ty(CName::U64);
const CTX: CParam = p("ctx", ptr(CName::Ctx, 1));
const BLOCK: CParam = p("block", ptr(CName::Header, 1));
const LAYOUT: CParam = p("index", U32);
const LIVE_MASK: CParam = p("live_mask", cptr(CName::U64));
const ANCHOR: CParam = p("anchor", U64);
const OPERATION_SITE: CParam = p("operation_site", U64);
const REGION: CParam = p("region", ptr(CName::Region, 1));

impl RuntimeEntry {
    pub const ALL: [RuntimeEntry; 115] = [
        Self::DepthEnter,
        Self::DepthEnterAtSite,
        Self::DepthLeave,
        Self::DepthPreflight,
        Self::StructuralOrder,
        Self::StructuralHash,
        Self::AbiVersion,
        Self::CtxCreate,
        Self::CtxDestroy,
        Self::TaskExit,
        Self::CtxStats,
        Self::Trim,
        Self::Alloc,
        Self::Payload,
        Self::Capacity,
        Self::Dup,
        Self::Release,
        Self::Free,
        Self::Reset,
        Self::Reuse,
        Self::Dispose,
        Self::SeqReserve,
        Self::BlockGrow,
        Self::HashBegin,
        Self::HashBytes,
        Self::HashInit,
        Self::HashUpdate,
        Self::HashFinish,
        Self::CodecEncode,
        Self::CodecDecode,
        Self::UniqueCopy,
        Self::UniqueCopyValue,
        Self::Detach,
        Self::Freeze,
        Self::Thaw,
        Self::RootDup,
        Self::RootRelease,
        Self::RootDrop,
        Self::Finalize,
        Self::FinalizeBegin,
        Self::FinalizeEnd,
        Self::CloseHandle,
        Self::Shielded,
        Self::RegionEnter,
        Self::RegionReserve,
        Self::RegionOwner,
        Self::RegionHold,
        Self::RegionSchedule,
        Self::RegionExit,
        Self::ArenaAlloc,
        Self::ArenaPush,
        Self::ArenaReserve,
        Self::ArenaRegrow,
        Self::ArenaEnter,
        Self::ArenaBound,
        Self::ArenaExhausted,
        Self::ArenaRecover,
        Self::BuilderBegin,
        Self::BuilderGrow,
        Self::BuilderPublish,
        Self::FailureBegin,
        Self::FailureAppend,
        Self::FailureActive,
        Self::FailurePrimary,
        Self::FailureCauseCount,
        Self::FailureCause,
        Self::FailureReanchor,
        Self::FailureClear,
        Self::IsolateCatch,
        Self::InvariantFault,
        Self::DiagPanic,
        Self::ForeignEnter,
        Self::ForeignLeave,
        Self::ForeignViolation,
        Self::CallbackContext,
        Self::CallbackFailed,
        Self::ExitReport,
        Self::FuelRefill,
        Self::WorkerCreate,
        Self::WorkerRetire,
        Self::WorkerCollect,
        Self::CallbackLeave,
        Self::UncheckedAddOverflow,
        Self::TaskSpawnAhead,
        Self::AheadBarrier,
        Self::AheadState,
        Self::AheadHandoff,
        Self::HostArm,
        Self::HostEnter,
        Self::HostActive,
        Self::HostDefer,
        Self::ResumePoint,
        Self::ResumeRun,
        Self::ResumeTail,
        Self::ResumeCleanup,
        Self::ReceiptRetain,
        Self::ReceiptRelease,
        Self::SealedRoot,
        Self::SealedRelease,
        Self::HoleClose,
        Self::ReceiptRegionEnter,
        Self::ReceiptRegionExit,
        Self::ReceiptBits,
        Self::SealPrepareCommit,
        Self::OrderedJoin,
        Self::OrderedSleepUntil,
        Self::OrderedPrint,
        Self::OrderedStdinRead,
        Self::OrderedWatchNext,
        Self::CpuMapScoped,
        Self::CpuAheadArm,
        Self::CpuAheadEnter,
        Self::CpuAheadAlloc,
        Self::ReceiptConstruct,
        Self::ReceiptArenaInventory,
    ];

    pub const fn declaration(self) -> RuntimeDeclaration {
        use CName as N;
        let (symbol, returns, parameters): (&'static str, CType, &'static [CParam]) = match self {
            Self::DepthEnter => ("hirsel_rt_v2_depth_enter", U8, const { &[CTX] }),
            Self::DepthEnterAtSite => (
                "hirsel_rt_v2_depth_enter_at_site",
                U8,
                const { &[CTX, ANCHOR, OPERATION_SITE] },
            ),
            Self::DepthLeave => ("hirsel_rt_v2_depth_leave", VOID, const { &[CTX] }),
            Self::DepthPreflight => (
                "hirsel_rt_v2_depth_preflight",
                U8,
                const {
                    &[
                        CTX,
                        p("program", cptr(N::Void)),
                        p("program_length", U64),
                        p("edges", cptr(N::Void)),
                        p("edge_count", U64),
                        p("value", cptr(N::Void)),
                    ]
                },
            ),
            Self::StructuralOrder => (
                "hirsel_rt_v2_structural_order",
                U8,
                const {
                    &[
                        CTX,
                        p("program", cptr(N::Void)),
                        p("program_length", U64),
                        p("left", cptr(N::Void)),
                        p("right", cptr(N::Void)),
                    ]
                },
            ),
            Self::StructuralHash => (
                "hirsel_rt_v2_structural_hash",
                U64,
                const {
                    &[
                        CTX,
                        p("program", cptr(N::Void)),
                        p("program_length", U64),
                        p("value", cptr(N::Void)),
                        p("seed", U64),
                    ]
                },
            ),
            Self::ReceiptRetain => (
                "hirsel_rt_v2_receipt_retain",
                VOID,
                const { &[p("receipt", ptr(N::Void, 1))] },
            ),
            Self::HoleClose => (
                "hirsel_rt_v2_hole_close",
                VOID,
                const { &[CTX, BLOCK, LAYOUT] },
            ),
            Self::ReceiptRelease => (
                "hirsel_rt_v2_receipt_release",
                VOID,
                const { &[p("receipt", ptr(N::Void, 1))] },
            ),
            Self::SealedRoot => (
                "hirsel_rt_v2_sealed_root",
                cptr(N::Void),
                const { &[CTX, p("owner", cptr(N::Void))] },
            ),
            Self::SealedRelease => (
                "hirsel_rt_v2_sealed_release",
                VOID,
                const { &[CTX, p("owner", ptr(N::Void, 1))] },
            ),
            Self::ReceiptRegionEnter => (
                "hirsel_rt_v2_receipt_region_enter",
                VOID,
                const { &[CTX, p("guard", ptr(N::Void, 1))] },
            ),
            Self::ReceiptRegionExit => (
                "hirsel_rt_v2_receipt_region_exit",
                U8,
                const { &[CTX, p("guard", ptr(N::Void, 1)), ANCHOR, OPERATION_SITE] },
            ),
            Self::ReceiptArenaInventory => (
                "hirsel_rt_v2_receipt_arena_inventory",
                VOID,
                const {
                    &[
                        CTX,
                        p("guard", ptr(N::Void, 1)),
                        LAYOUT,
                        p("value", cptr(N::Void)),
                    ]
                },
            ),
            Self::ReceiptConstruct => (
                "hirsel_rt_v2_receipt_construct",
                ptr(N::Void, 1),
                const {
                    &[
                        CTX,
                        LAYOUT,
                        p("value", cptr(N::Void)),
                        p("kind", U8),
                        p("inputs", ptr(N::Void, 2)),
                        p("input_count", U64),
                    ]
                },
            ),
            Self::ReceiptBits => (
                "hirsel_rt_v2_receipt_bits",
                ptr(N::Void, 1),
                const { &[CTX, LAYOUT, p("value", cptr(N::Void))] },
            ),
            Self::SealPrepareCommit => (
                "hirsel_rt_v2_seal_prepare_commit",
                U8,
                const {
                    &[
                        CTX,
                        p("guard", ptr(N::Void, 1)),
                        p("root", cptr(N::Void)),
                        LAYOUT,
                        p("receipt", cptr(N::Void)),
                        p("site", U64),
                        p("out_owner", ptr(N::Void, 2)),
                    ]
                },
            ),
            Self::AbiVersion => ("hirsel_rt_v2_abi_version", U32, const { &[] }),
            Self::CtxCreate => (
                "hirsel_rt_v2_ctx_create",
                U8,
                const { &[p("config", cptr(N::Config)), p("out", ptr(N::Ctx, 2))] },
            ),
            Self::CtxDestroy => ("hirsel_rt_v2_ctx_destroy", VOID, const { &[CTX] }),
            Self::TaskExit => (
                "hirsel_rt_v2_task_exit",
                VOID,
                const { &[CTX, p("out", ptr(N::Audit, 1))] },
            ),
            Self::CtxStats => (
                "hirsel_rt_v2_ctx_stats",
                VOID,
                const { &[CTX, p("out", ptr(N::Stats, 1))] },
            ),
            Self::Trim => ("hirsel_rt_v2_trim", VOID, const { &[CTX] }),
            Self::Alloc => (
                "hirsel_rt_v2_alloc",
                ptr(N::Header, 1),
                const {
                    &[
                        CTX,
                        LAYOUT,
                        p("extent", U64),
                        p("align", U64),
                        p("destination", U8),
                        p("site", U64),
                    ]
                },
            ),
            Self::Payload => ("hirsel_rt_v2_payload", ptr(N::Void, 1), const { &[BLOCK] }),
            Self::Capacity => ("hirsel_rt_v2_capacity", U64, const { &[BLOCK] }),
            Self::Dup => ("hirsel_rt_v2_dup", VOID, const { &[CTX, BLOCK] }),
            Self::Release => (
                "hirsel_rt_v2_release",
                U8,
                const { &[CTX, BLOCK, LAYOUT, LIVE_MASK, ANCHOR, OPERATION_SITE] },
            ),
            Self::Free => ("hirsel_rt_v2_free", VOID, const { &[CTX, BLOCK] }),
            Self::Reset => (
                "hirsel_rt_v2_reset",
                U8,
                const {
                    &[
                        CTX,
                        BLOCK,
                        LAYOUT,
                        LIVE_MASK,
                        ANCHOR,
                        OPERATION_SITE,
                        p("out_token", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::Reuse => (
                "hirsel_rt_v2_reuse",
                U8,
                const {
                    &[
                        CTX,
                        p("token", ptr(N::Header, 1)),
                        LAYOUT,
                        p("extent", U64),
                        p("align", U64),
                        p("out_block", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::Dispose => (
                "hirsel_rt_v2_dispose",
                VOID,
                const { &[CTX, p("token", ptr(N::Header, 1))] },
            ),
            Self::BlockGrow => (
                "hirsel_rt_v2_block_grow",
                ptr(N::Header, 1),
                const {
                    &[
                        CTX,
                        p("block", ptr(N::Header, 1)),
                        p("keep", U64),
                        p("extent", U64),
                        p("align", U64),
                    ]
                },
            ),
            Self::SeqReserve => (
                "hirsel_rt_v2_seq_grow",
                ptr(N::Header, 1),
                const {
                    &[
                        CTX,
                        p("block", ptr(N::Header, 1)),
                        p("element_size", U64),
                        p("element_align", U64),
                        p("capacity", U64),
                    ]
                },
            ),
            Self::HashBegin => ("hirsel_rt_v2_hash_begin", U64, const { &[CTX] }),
            Self::HashBytes => (
                "hirsel_rt_v2_hash_bytes",
                U64,
                const { &[p("state", U64), p("bytes", cptr(N::Void)), p("length", U64)] },
            ),
            Self::HashInit => (
                "hirsel_rt_v2_hash_init",
                VOID,
                const { &[p("out", ptr(N::Void, 1)), p("seed", U64)] },
            ),
            Self::HashUpdate => (
                "hirsel_rt_v2_hash_update",
                U64,
                const { &[p("state", U64), p("bytes", cptr(N::Void)), p("length", U64)] },
            ),
            Self::HashFinish => (
                "hirsel_rt_v2_hash_finish",
                U64,
                const { &[p("state", U64)] },
            ),
            Self::CodecEncode => (
                "hirsel_rt_v2_codec_encode",
                VOID,
                const {
                    &[
                        CTX,
                        p("codec", cptr(N::Codec)),
                        p("value", cptr(N::Void)),
                        p("out", ptr(N::Void, 1)),
                    ]
                },
            ),
            Self::CodecDecode => (
                "hirsel_rt_v2_codec_decode",
                VOID,
                const {
                    &[
                        CTX,
                        p("codec", cptr(N::Codec)),
                        p("bytes", cptr(N::Void)),
                        p("out", ptr(N::Void, 1)),
                    ]
                },
            ),
            Self::UniqueCopy => (
                "hirsel_rt_v2_unique_copy",
                U8,
                const {
                    &[
                        CTX,
                        BLOCK,
                        REGION,
                        p("flags", U32),
                        p("site", U64),
                        p("out_block", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::UniqueCopyValue => (
                "hirsel_rt_v2_unique_copy_value",
                U8,
                const {
                    &[
                        CTX,
                        LAYOUT,
                        p("value", ptr(CName::U8, 1)),
                        REGION,
                        p("site", U64),
                    ]
                },
            ),
            Self::Detach => (
                "hirsel_rt_v2_detach",
                U8,
                const {
                    &[
                        CTX,
                        BLOCK,
                        p("site", U64),
                        p("out_block", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::Freeze => ("hirsel_rt_v2_freeze", VOID, const { &[CTX, BLOCK] }),
            Self::Thaw => ("hirsel_rt_v2_thaw", VOID, const { &[CTX, BLOCK] }),
            Self::RootDup => ("hirsel_rt_v2_root_dup", VOID, const { &[CTX, BLOCK] }),
            Self::RootRelease => ("hirsel_rt_v2_root_release", VOID, const { &[CTX, BLOCK] }),
            Self::RootDrop => (
                "hirsel_rt_v2_root_drop",
                U8,
                const { &[CTX, BLOCK, ANCHOR, OPERATION_SITE] },
            ),
            Self::Finalize => (
                "hirsel_rt_v2_finalize",
                U8,
                const {
                    &[
                        CTX,
                        LAYOUT,
                        p("value", ptr(N::Void, 1)),
                        LIVE_MASK,
                        p("continuation", ptr(N::Cleanup, 1)),
                        ANCHOR,
                        OPERATION_SITE,
                    ]
                },
            ),
            Self::FinalizeBegin => (
                "hirsel_rt_v2_finalize_begin",
                VOID,
                const { &[CTX, ANCHOR] },
            ),
            Self::FinalizeEnd => ("hirsel_rt_v2_finalize_end", VOID, const { &[CTX, ANCHOR] }),
            Self::CloseHandle => (
                "hirsel_rt_v2_close_handle",
                U8,
                const { &[CTX, p("handle", U64), ANCHOR, OPERATION_SITE] },
            ),
            Self::Shielded => ("hirsel_rt_v2_shielded", U8, const { &[CTX] }),
            Self::RegionEnter => ("hirsel_rt_v2_region_enter", VOID, const { &[CTX, REGION] }),
            Self::RegionReserve => (
                "hirsel_rt_v2_region_reserve",
                U8,
                const {
                    &[
                        CTX,
                        REGION,
                        p("extent", U64),
                        p("align", U64),
                        p("out", ptr(N::Void, 2)),
                    ]
                },
            ),
            Self::RegionOwner => (
                "hirsel_rt_v2_region_owner",
                VOID,
                const {
                    &[
                        CTX,
                        REGION,
                        BLOCK,
                        LAYOUT,
                        p("capacity", U64),
                        p("align", U64),
                    ]
                },
            ),
            Self::RegionHold => (
                "hirsel_rt_v2_region_hold",
                U8,
                const { &[CTX, REGION, BLOCK] },
            ),
            Self::RegionSchedule => (
                "hirsel_rt_v2_region_schedule",
                U8,
                const { &[CTX, REGION, LAYOUT, p("value", ptr(N::Void, 1))] },
            ),
            Self::RegionExit => (
                "hirsel_rt_v2_region_exit",
                U8,
                const { &[CTX, REGION, ANCHOR, OPERATION_SITE] },
            ),
            Self::ArenaAlloc => (
                "hirsel_rt_v2_arena_alloc",
                ptr(N::Header, 1),
                const {
                    &[
                        CTX,
                        REGION,
                        LAYOUT,
                        p("extent", U64),
                        p("align", U64),
                        p("flags", U32),
                    ]
                },
            ),
            Self::ArenaPush => (
                "hirsel_rt_v2_arena_push",
                U8,
                const {
                    &[
                        CTX,
                        BLOCK,
                        p("element_size", U64),
                        p("element_align", U64),
                        p("out_block", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::ArenaReserve => (
                "hirsel_rt_v2_arena_reserve",
                U8,
                const {
                    &[
                        CTX,
                        BLOCK,
                        p("count", U64),
                        p("element_size", U64),
                        p("element_align", U64),
                        p("out_block", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::ArenaRegrow => (
                "hirsel_rt_v2_arena_regrow",
                ptr(N::Header, 1),
                const { &[CTX, BLOCK, p("extent", U64), p("align", U64)] },
            ),
            Self::ArenaEnter => ("hirsel_rt_v2_arena_enter", VOID, const { &[CTX, REGION] }),
            Self::ArenaBound => (
                "hirsel_rt_v2_arena_bound",
                VOID,
                const { &[CTX, REGION, p("capacity", U64)] },
            ),
            Self::ArenaExhausted => (
                "hirsel_rt_v2_arena_exhausted",
                U8,
                const {
                    &[
                        CTX,
                        REGION,
                        p("kind", U32),
                        p("rule", U32),
                        p("site", U64),
                        OPERATION_SITE,
                    ]
                },
            ),
            Self::ArenaRecover => ("hirsel_rt_v2_arena_recover", U8, const { &[CTX, REGION] }),
            Self::BuilderBegin => (
                "hirsel_rt_v2_builder_begin",
                U8,
                const {
                    &[
                        CTX,
                        REGION,
                        p("capacity", U64),
                        p("align", U64),
                        p("out", ptr(N::Builder, 1)),
                    ]
                },
            ),
            Self::BuilderGrow => (
                "hirsel_rt_v2_builder_grow",
                U8,
                const {
                    &[
                        CTX,
                        p("builder", ptr(N::Builder, 1)),
                        p("used", U64),
                        p("min_capacity", U64),
                    ]
                },
            ),
            Self::BuilderPublish => (
                "hirsel_rt_v2_builder_publish",
                VOID,
                const { &[CTX, p("builder", ptr(N::Builder, 1)), p("used", U64)] },
            ),
            Self::FailureBegin => (
                "hirsel_rt_v2_failure_begin",
                U8,
                const {
                    &[
                        CTX,
                        p("kind", U32),
                        p("rule", U32),
                        p("site", U64),
                        p("operation_site", U64),
                    ]
                },
            ),
            Self::FailureAppend => (
                "hirsel_rt_v2_failure_append",
                U8,
                const { &[CTX, p("failure", cptr(N::Failure))] },
            ),
            Self::FailureActive => ("hirsel_rt_v2_failure_active", U8, const { &[CTX] }),
            Self::FailurePrimary => (
                "hirsel_rt_v2_failure_primary",
                U8,
                const { &[CTX, p("out", ptr(N::Failure, 1))] },
            ),
            Self::FailureCauseCount => ("hirsel_rt_v2_failure_cause_count", U64, const { &[CTX] }),
            Self::FailureCause => (
                "hirsel_rt_v2_failure_cause",
                U8,
                const { &[CTX, p("index", U64), p("out", ptr(N::Failure, 1))] },
            ),
            Self::FailureReanchor => (
                "hirsel_rt_v2_failure_reanchor",
                VOID,
                const { &[CTX, p("site", U64), p("operation_site", U64)] },
            ),
            Self::FailureClear => ("hirsel_rt_v2_failure_clear", VOID, const { &[CTX] }),
            Self::IsolateCatch => (
                "hirsel_rt_v2_isolate_catch",
                VOID,
                const { &[CTX, p("start", U64)] },
            ),
            Self::UncheckedAddOverflow => (
                "hirsel_rt_v2_unchecked_add_overflow",
                VOID,
                const { &[CTX, p("site", U64)] },
            ),
            Self::InvariantFault => ("hirsel_rt_v2_invariant_fault", VOID, const { &[] }),
            Self::DiagPanic => ("hirsel_rt_v2_diag_panic", VOID, const { &[] }),
            Self::ForeignEnter => (
                "hirsel_rt_v2_foreign_enter",
                VOID,
                const {
                    &[
                        CTX,
                        p("site", U64),
                        p("previous", ptr(N::Void, 1)),
                        p("dispatch", U64),
                    ]
                },
            ),
            Self::ForeignLeave => (
                "hirsel_rt_v2_foreign_leave",
                VOID,
                const { &[p("previous", cptr(N::Void))] },
            ),
            Self::ForeignViolation => (
                "hirsel_rt_v2_foreign_violation",
                VOID,
                const { &[p("site", U64)] },
            ),
            Self::CallbackContext => (
                "hirsel_rt_v2_callback_context",
                ptr(N::Ctx, 1),
                const { &[] },
            ),
            Self::CallbackFailed => ("hirsel_rt_v2_callback_failed", VOID, const { &[CTX] }),
            Self::CallbackLeave => ("hirsel_rt_v2_callback_leave", VOID, const { &[CTX] }),
            Self::ExitReport => (
                "hirsel_rt_v2_exit_report",
                VOID,
                const {
                    &[
                        CTX,
                        p("value", ptr(N::U8, 1)),
                        p("layout", U32),
                        p("payload_offset", U64),
                        p("node", cptr(N::ReportNode)),
                    ]
                },
            ),
            Self::CpuMapScoped => (
                "hirsel_rt_v2_cpu_map_scoped",
                U8,
                const {
                    &[
                        CTX,
                        p("out", ptr(N::U32, 1)),
                        p("tasks", cptr(N::U64)),
                        p("width", U32),
                        p("action", U32),
                        p("count", U64),
                    ]
                },
            ),
            Self::TaskSpawnAhead => (
                "hirsel_rt_v2_task_spawn_ahead",
                U8,
                const {
                    &[
                        CTX,
                        p("caller_site", U64),
                        p("operation_site", U64),
                        p("destination", ptr(N::Void, 1)),
                        p("out", ptr(N::U64, 1)),
                        p("scope", cptr(N::U64)),
                        p(
                            "code",
                            CType {
                                name: N::TaskCode,
                                pointers: 0,
                                constant: false,
                            },
                        ),
                        p("environment", ptr(N::Header, 1)),
                        p("flags", U64),
                        p("info", cptr(N::SpawnAheadInfo)),
                    ]
                },
            ),
            Self::AheadState => (
                "hirsel_rt_v2_ahead_state",
                ptr(N::AheadPhysical, 1),
                const { &[CTX] },
            ),
            Self::AheadHandoff => (
                "hirsel_rt_v2_ahead_handoff",
                VOID,
                const { &[p("state", ptr(N::AheadPhysical, 1)), p("reason", U32)] },
            ),
            Self::CpuAheadArm => ("hirsel_rt_v2_cpu_ahead_arm", VOID, const { &[CTX] }),
            Self::CpuAheadEnter => (
                "hirsel_rt_v2_cpu_ahead_enter",
                VOID,
                const { &[p("state", ptr(N::AheadPhysical, 1))] },
            ),
            Self::CpuAheadAlloc => (
                "hirsel_rt_v2_cpu_ahead_alloc",
                ptr(N::Header, 1),
                const {
                    &[
                        p("state", ptr(N::AheadPhysical, 1)),
                        p("index", U32),
                        p("extent", U64),
                        p("align", U64),
                    ]
                },
            ),
            Self::HostArm => (
                "hirsel_rt_v2_host_arm",
                VOID,
                const {
                    &[
                        CTX,
                        p("recipe", cptr(N::HostReadRecipe)),
                        p("buffer", ptr(N::Header, 2)),
                    ]
                },
            ),
            Self::HostEnter => (
                "hirsel_rt_v2_host_enter",
                VOID,
                const {
                    &[
                        p("state", ptr(N::AheadPhysical, 1)),
                        p("recipe", cptr(N::HostReadRecipe)),
                    ]
                },
            ),
            Self::HostActive => (
                "hirsel_rt_v2_host_active",
                U8,
                const { &[p("state", ptr(N::AheadPhysical, 1))] },
            ),
            Self::HostDefer => (
                "hirsel_rt_v2_host_defer",
                VOID,
                const {
                    &[
                        p("state", ptr(N::AheadPhysical, 1)),
                        p("slot", U32),
                        p("block", ptr(N::Header, 1)),
                        p("fuel", U64),
                    ]
                },
            ),
            Self::AheadBarrier => ("hirsel_rt_v2_ahead_barrier", VOID, const { &[CTX] }),
            Self::ResumePoint => (
                "hirsel_rt_v2_resume_point",
                U8,
                const { &[CTX, p("frame", ptr(N::U8, 1)), p("state", U32)] },
            ),
            Self::OrderedJoin => (
                "hirsel_rt_v2_ordered_join",
                U8,
                const {
                    &[
                        CTX,
                        p("caller_site", U64),
                        OPERATION_SITE,
                        p("scope", ptr(N::U64, 1)),
                        p("child", ptr(N::U64, 1)),
                        p("slot", ptr(N::U8, 1)),
                        p("size", U64),
                        p("align", U32),
                    ]
                },
            ),
            Self::OrderedSleepUntil => (
                "hirsel_rt_v2_ordered_sleep_until",
                U8,
                const {
                    &[
                        CTX,
                        p("caller_site", U64),
                        OPERATION_SITE,
                        p("destination", ptr(N::U8, 1)),
                        p("out", ptr(N::U8, 1)),
                        p("clock", ptr(N::U64, 1)),
                        p("deadline", ptr(N::U64, 1)),
                    ]
                },
            ),
            Self::OrderedStdinRead => (
                "hirsel_rt_v2_ordered_stdin_read",
                U8,
                const {
                    &[
                        CTX,
                        p("caller_site", U64),
                        OPERATION_SITE,
                        p("destination", ptr(N::U8, 1)),
                        p("out", ptr(N::U8, 1)),
                        p("stdin", ptr(N::U64, 1)),
                        p("buffer", ptr(N::Header, 2)),
                        p("max", U64),
                    ]
                },
            ),
            Self::OrderedWatchNext => (
                "hirsel_rt_v2_ordered_watcher_next",
                U8,
                const {
                    &[
                        CTX,
                        p("caller_site", U64),
                        OPERATION_SITE,
                        p("destination", ptr(N::U8, 1)),
                        p("out", ptr(N::U8, 1)),
                        p("directory", ptr(N::U64, 1)),
                        p("watcher", ptr(N::U64, 1)),
                    ]
                },
            ),
            Self::OrderedPrint => (
                "hirsel_rt_v2_ordered_print",
                U8,
                const {
                    &[
                        CTX,
                        p("caller_site", U64),
                        OPERATION_SITE,
                        p("destination", ptr(N::U8, 1)),
                        p("out", ptr(N::U8, 1)),
                        p("stdout", ptr(N::U64, 1)),
                        p("owner", ptr(N::Header, 1)),
                        p("start", U64),
                        p("len", U64),
                    ]
                },
            ),
            Self::ResumeTail => (
                "hirsel_rt_v2_resume_tail",
                VOID,
                const {
                    &[
                        CTX,
                        p("frame", ptr(N::U8, 1)),
                        p("state", U32),
                        p("kind", U32),
                        p("bytes", U32),
                        p("anchor", U64),
                        OPERATION_SITE,
                    ]
                },
            ),
            Self::ResumeCleanup => (
                "hirsel_rt_v2_resume_cleanup",
                VOID,
                const {
                    &[
                        CTX,
                        p("frame", ptr(N::U8, 1)),
                        p("state", U32),
                        p("kind", U32),
                        p("bytes", U32),
                        p("anchor", U64),
                        OPERATION_SITE,
                        p("rows", cptr(N::U8)),
                        p("count", U32),
                    ]
                },
            ),
            Self::ResumeRun => (
                "hirsel_rt_v2_resume_run",
                U8,
                const {
                    &[
                        CTX,
                        p("environment", ptr(N::Header, 1)),
                        p("caller_site", U64),
                        OPERATION_SITE,
                        p("dispatcher", ptr(N::Void, 1)),
                        p("bytes", U32),
                    ]
                },
            ),
            Self::FuelRefill => ("hirsel_rt_v2_fuel_refill", VOID, const { &[CTX] }),
            Self::WorkerCreate => (
                "hirsel_rt_v2_worker_create",
                ptr(N::Ctx, 1),
                const { &[CTX] },
            ),
            Self::WorkerRetire => ("hirsel_rt_v2_worker_retire", VOID, const { &[CTX] }),
            Self::WorkerCollect => ("hirsel_rt_v2_worker_collect", VOID, const { &[CTX] }),
        };
        RuntimeDeclaration {
            entry: self,
            symbol,
            returns,
            parameters,
        }
    }
}

/// A user drop body: borrowed value and cleanup anchor; records its failure
/// before returning `STATUS_FAILURE` (`hirsel_rt_v2_drop_thunk`).
pub const DROP_THUNK: RuntimeThunk = RuntimeThunk {
    name: "hirsel_rt_v2_drop_thunk",
    returns: U8,
    parameters: &[CTX, p("value", ptr(CName::Void, 1)), ANCHOR, OPERATION_SITE],
};

/// The Io adapter's handle close, called for a `STEP_CLOSE_HANDLE`; records
/// its failure before returning `STATUS_FAILURE` (`hirsel_rt_v2_close_thunk`).
pub const CLOSE_THUNK: RuntimeThunk = RuntimeThunk {
    name: "hirsel_rt_v2_close_thunk",
    returns: U8,
    parameters: &[CTX, p("handle", U64), ANCHOR, OPERATION_SITE],
};

/// A compiler-generated callback type the runtime calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RuntimeThunk {
    pub name: &'static str,
    pub returns: CType,
    pub parameters: &'static [CParam],
}

impl RuntimeThunk {
    /// The C typedef, spelled as in the header with single spaces.
    pub fn c_typedef(&self) -> String {
        let prototype = RuntimeDeclaration {
            entry: RuntimeEntry::AbiVersion,
            symbol: "",
            returns: self.returns,
            parameters: self.parameters,
        }
        .c_prototype();
        let (returns, parameters) = prototype.split_once('(').expect("prototype");
        format!("typedef {}(*{})({parameters}", returns, self.name)
    }
}

snapshot_enum!(RuntimeEntry {
    DepthEnter,
    DepthEnterAtSite,
    DepthLeave,
    DepthPreflight,
    StructuralOrder,
    StructuralHash,
    AbiVersion,
    CtxCreate,
    CtxDestroy,
    TaskExit,
    CtxStats,
    Trim,
    Alloc,
    Payload,
    Capacity,
    Dup,
    Release,
    Free,
    Reset,
    Reuse,
    Dispose,
    SeqReserve,
    BlockGrow,
    HashBegin,
    HashBytes,
    HashInit,
    HashUpdate,
    HashFinish,
    CodecEncode,
    CodecDecode,
    UniqueCopy,
    UniqueCopyValue,
    Detach,
    Freeze,
    Thaw,
    RootDup,
    RootRelease,
    RootDrop,
    Finalize,
    FinalizeBegin,
    FinalizeEnd,
    CloseHandle,
    Shielded,
    RegionEnter,
    RegionReserve,
    RegionOwner,
    RegionHold,
    RegionSchedule,
    RegionExit,
    ArenaAlloc,
    ArenaPush,
    ArenaReserve,
    ArenaRegrow,
    ArenaEnter,
    ArenaBound,
    ArenaExhausted,
    ArenaRecover,
    BuilderBegin,
    BuilderGrow,
    BuilderPublish,
    FailureBegin,
    FailureAppend,
    FailureActive,
    FailurePrimary,
    FailureCauseCount,
    FailureCause,
    FailureReanchor,
    FailureClear,
    IsolateCatch,
    InvariantFault,
    DiagPanic,
    ForeignEnter,
    ForeignLeave,
    ForeignViolation,
    CallbackContext,
    CallbackFailed,
    ExitReport,
    FuelRefill,
    WorkerCreate,
    WorkerRetire,
    WorkerCollect,
    CallbackLeave,
    UncheckedAddOverflow,
    TaskSpawnAhead,
    AheadBarrier,
    AheadState,
    AheadHandoff,
    HostArm,
    HostEnter,
    HostActive,
    HostDefer,
    ReceiptRetain,
    ReceiptRelease,
    SealedRoot,
    SealedRelease,
    ResumePoint,
    ResumeRun,
    ResumeTail,
    ResumeCleanup,
    HoleClose,
    ReceiptRegionEnter,
    ReceiptRegionExit,
    ReceiptBits,
    SealPrepareCommit,
    OrderedJoin,
    OrderedSleepUntil,
    OrderedPrint,
    OrderedStdinRead,
    OrderedWatchNext,
    CpuMapScoped,
    ReceiptConstruct,
    ReceiptArenaInventory,
    CpuAheadArm,
    CpuAheadEnter,
    CpuAheadAlloc,
});

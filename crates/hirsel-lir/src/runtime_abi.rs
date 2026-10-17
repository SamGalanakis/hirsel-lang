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

pub const ABI_VERSION: u32 = 9;

/// Status byte of every fallible entry: 0 success, 1 language failure.
pub const STATUS_OK: u8 = 0;
pub const STATUS_FAILURE: u8 = 1;
/// A deep copy into a bounded arena whose budget does not cover it (MEM-6, runtime
/// ABI 7): nothing is copied or recorded; the caller records the arena's `OutOfSpace`.
pub const STATUS_EXHAUSTED: u8 = 2;
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
pub const COUNT_OFFSET: u64 = 0;
pub const COUNT_UNIQUE: u64 = 1;
/// The count value of an immortal block: sticky, never wraps.
pub const COUNT_IMMORTAL: u64 = u32::MAX as u64;
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
pub const PAGE_FREE_OFFSET: u64 = 16;
pub const PAGE_LIVE_OFFSET: u64 = 24;
pub const PAGE_LOCAL_FREE_OFFSET: u64 = 72;

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

/// Layout descriptor constants (`hirsel_rt_v2_layout`, `hirsel_rt_v2_step`);
/// `rt_program` lowers layout drop programs to these steps.
pub const LAYOUT_MAGIC: u32 = 0x484e_4c31;
pub const LAYOUT_DESCRIPTOR_VERSION: u32 = 6;
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
    /// `RunCtx`: task heap domain, data stack, release worklist, failure state,
    /// semantic configuration and Io adapter. Not a source capability.
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
    CodecEncode,
    CodecDecode,
    UniqueCopy,
    UniqueCopyValue,
    Detach,
    Freeze,
    Thaw,
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
    InvariantFault,
    DiagPanic,
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
const REGION: CParam = p("region", ptr(CName::Region, 1));

impl RuntimeEntry {
    pub const ALL: [RuntimeEntry; 55] = [
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
        Self::CodecEncode,
        Self::CodecDecode,
        Self::UniqueCopy,
        Self::UniqueCopyValue,
        Self::Detach,
        Self::Freeze,
        Self::Thaw,
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
        Self::InvariantFault,
        Self::DiagPanic,
    ];

    pub const fn declaration(self) -> RuntimeDeclaration {
        use CName as N;
        let (symbol, returns, parameters): (&'static str, CType, &'static [CParam]) = match self {
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
                const { &[CTX, BLOCK, LAYOUT, LIVE_MASK, ANCHOR] },
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
                const { &[CTX, p("handle", U64), ANCHOR] },
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
                const { &[CTX, REGION, ANCHOR] },
            ),
            Self::ArenaAlloc => (
                "hirsel_rt_v2_arena_alloc",
                ptr(N::Header, 1),
                const { &[CTX, REGION, LAYOUT, p("extent", U64), p("align", U64)] },
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
                const { &[CTX, REGION, p("kind", U32), p("rule", U32), p("site", U64)] },
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
                const { &[CTX, p("kind", U32), p("rule", U32), p("site", U64)] },
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
                const { &[CTX, p("site", U64)] },
            ),
            Self::FailureClear => ("hirsel_rt_v2_failure_clear", VOID, const { &[CTX] }),
            Self::InvariantFault => ("hirsel_rt_v2_invariant_fault", VOID, const { &[] }),
            Self::DiagPanic => ("hirsel_rt_v2_diag_panic", VOID, const { &[] }),
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
    parameters: &[CTX, p("value", ptr(CName::Void, 1)), ANCHOR],
};

/// The Io adapter's handle close, called for a `STEP_CLOSE_HANDLE`; records
/// its failure before returning `STATUS_FAILURE` (`hirsel_rt_v2_close_thunk`).
pub const CLOSE_THUNK: RuntimeThunk = RuntimeThunk {
    name: "hirsel_rt_v2_close_thunk",
    returns: U8,
    parameters: &[CTX, p("handle", U64), ANCHOR],
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
    CodecEncode,
    CodecDecode,
    UniqueCopy,
    UniqueCopyValue,
    Detach,
    Freeze,
    Thaw,
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
    InvariantFault,
    DiagPanic,
});

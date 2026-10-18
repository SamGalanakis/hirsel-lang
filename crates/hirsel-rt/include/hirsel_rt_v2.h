/*
 * hirsel_rt_v2: the Hirsel native runtime interface.
 *
 * Sizes and alignments are explicit uint64_t, statuses are uint8_t (HIRSEL_RT_V2_STATUS_OK or
 * HIRSEL_RT_V2_STATUS_FAILURE, a language failure recorded in the context), booleans are
 * uint8_t, and the run context is opaque and passed explicitly. No entry unwinds: a Rust panic
 * or a broken invariant aborts the process. The layouts below mirror crates/hirsel-rt/src and
 * are checked against it by tests/abi.rs and harness/harness.c.
 */
#ifndef HIRSEL_RT_V2_H
#define HIRSEL_RT_V2_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HIRSEL_RT_V2_ABI_VERSION 10
#define HIRSEL_RT_V2_STATUS_OK 0
#define HIRSEL_RT_V2_STATUS_FAILURE 1
/* A deep copy into a bounded arena its budget does not cover (MEM-6): nothing is copied or
 * recorded; the caller records the arena's OutOfSpace with hirsel_rt_v2_arena_exhausted. */
#define HIRSEL_RT_V2_STATUS_EXHAUSTED 2

#define HIRSEL_RT_V2_LAUNCH_OK 0
#define HIRSEL_RT_V2_LAUNCH_ABI_MISMATCH 2
#define HIRSEL_RT_V2_LAUNCH_INVALID_CONFIG 3
#define HIRSEL_RT_V2_CONFIG_DIAGNOSTIC 1

/* Counted block header v2. A block reference points at the header; the payload follows it. */
#define HIRSEL_RT_V2_HEADER_SIZE 8
/* The count word contract, which compiled drop glue relies on (T04 Drop, DropLast,
 * DropImmortal; T16 DropEdge): a uint32_t at COUNT_OFFSET, the number of holders of a live block.
 * It is plain (never atomic) and belongs to one task (OWN-6). COUNT_UNIQUE means the next release
 * kills the block: the glue then releases its fields and calls hirsel_rt_v2_free. COUNT_IMMORTAL is
 * sticky and is never written. Any other value loses one holder by a store of count - 1. A holder
 * is added by hirsel_rt_v2_dup, or inline under the compiled fast path contract below. The
 * descriptor word at LAYOUT_OFFSET stores its 29-bit program index,
 * storage kind and dead-wide-cursor flag. */
#define HIRSEL_RT_V2_COUNT_OFFSET 0
#define HIRSEL_RT_V2_COUNT_UNIQUE 1
#define HIRSEL_RT_V2_COUNT_IMMORTAL UINT32_MAX
#define HIRSEL_RT_V2_LAYOUT_OFFSET 4

/* Size classes: 16-byte steps to 256, then quarter-power-of-two steps to 4096. */
#define HIRSEL_RT_V2_CLASS_COUNT 32
#define HIRSEL_RT_V2_SMALL_MAX 4096
#define HIRSEL_RT_V2_SMALL_ALIGN_MAX 16
#define HIRSEL_RT_V2_SLAB_SIZE 65536
/* Capacity metadata for compiled read-only queries, on both 64-bit targets.
 * Small: slab base = header & ~(SLAB_SIZE - 1); u32 pool at SLAB_POOL_OFFSET,
 * class = pool % CLASS_COUNT. Large: prefix pointer before header, then u64
 * capacity at LARGE_CAPACITY_OFFSET. Static/region: u64 before header. A block of
 * a LAYOUT_NO_EXTENT layout may carry none (an arena block, or a small block of the
 * system-allocator archive, which is its header and class capacity alone): its
 * capacity is its descriptor's, and compiled code never queries it. */
#define HIRSEL_RT_V2_SLAB_POOL_OFFSET 32
#define HIRSEL_RT_V2_LARGE_PREFIX_POINTER_BACK 8
#define HIRSEL_RT_V2_LARGE_CAPACITY_OFFSET 32
#define HIRSEL_RT_V2_IMMORTAL_CAPACITY_BACK 16
#define HIRSEL_RT_V2_LARGE_CLASS 255
#define HIRSEL_RT_V2_CLASS_SIZES                                                                  \
    {16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256,                    \
     320, 384, 448, 512, 640, 768, 896, 1024, 1280, 1536, 1792, 2048, 2560, 3072, 3584, 4096}

/* Compiled fast paths (ABI 4). The run context begins with these words; a page is a slab.
 *   dup:   count = header->count; if (count < u32 at CTX_DUP_MAX_OFFSET) header->count = count + 1;
 *          else hirsel_rt_v2_dup(ctx, header).
 *   alloc: a small request of pool p = class(extent) + (align > 8 ? CLASS_COUNT : 0):
 *          page = pages[p] (CTX_PAGES_OFFSET, POOL_COUNT pointers); block = page->free; unless page
 *          and block are nonnull, hirsel_rt_v2_alloc; otherwise page->free = *(void **)block;
 *          live = page->live; if (live == 0) the u64 idle slab count at CTX_IDLE_SLABS_OFFSET
 *          loses one; page->live = live + 1; the header is count 1 and the descriptor index
 *          (storage kind small).
 *   free:  of a dead block (glue's count was COUNT_UNIQUE) when the u32 at CTX_FREE_INLINE_OFFSET is
 *          nonzero and the descriptor's kind bits are KIND_SMALL: page = header & ~(SLAB_SIZE - 1);
 *          head = page->local_free; unless head is nonnull, hirsel_rt_v2_free; otherwise
 *          *(void **)header = head; page->local_free = header; page->live -= 1, and if that
 *          leaves it 0 the idle slab count gains one.
 * A zero dup bound, a zero free flag and null pages (diagnostic mode, measurement builds, the
 * system allocator) send every operation to its entry. The memcheck archive keeps the dup bound
 * but zeroes the free flag and nulls the pages, so every allocation and free reaches its entry. */
#define HIRSEL_RT_V2_CTX_DUP_MAX_OFFSET 0
#define HIRSEL_RT_V2_CTX_FREE_INLINE_OFFSET 4
#define HIRSEL_RT_V2_CTX_IDLE_SLABS_OFFSET 8
#define HIRSEL_RT_V2_CTX_PAGES_OFFSET 16
#define HIRSEL_RT_V2_POOL_COUNT 64
#define HIRSEL_RT_V2_PAGE_FREE_OFFSET 16
#define HIRSEL_RT_V2_PAGE_LIVE_OFFSET 24
#define HIRSEL_RT_V2_PAGE_LOCAL_FREE_OFFSET 72

#define HIRSEL_RT_V2_DOMAIN_HEAP 1
#define HIRSEL_RT_V2_DOMAIN_STATIC 2
#define HIRSEL_RT_V2_DOMAIN_REGION 3

#define HIRSEL_RT_V2_STATE_FREE 0
#define HIRSEL_RT_V2_STATE_LIVE 1
#define HIRSEL_RT_V2_STATE_FINALIZE_PENDING 2
#define HIRSEL_RT_V2_STATE_FINALIZING 3
#define HIRSEL_RT_V2_STATE_FIELDS_PENDING 4
#define HIRSEL_RT_V2_STATE_RAW 5

#define HIRSEL_RT_V2_FREE_POISON 0xdf
#define HIRSEL_RT_V2_UNINIT_POISON 0xa5

/* Layout descriptors: a flat drop program per layout (hirsel-lir runtime_descriptor lowers the
 * LayoutRecord drop program to it). Step offsets are from the enclosing frame's base: the value,
 * a loop's element, or a column's element. */
#define HIRSEL_RT_V2_LAYOUT_MAGIC 0x484e4c31u
#define HIRSEL_RT_V2_LAYOUT_VERSION 6
#define HIRSEL_RT_V2_LAYOUT_UNSAFE_FFI 1
#define HIRSEL_RT_V2_LAYOUT_DYNAMIC 2
#define HIRSEL_RT_V2_LAYOUT_WIDE_CURSOR 4
/* A fixed-size, non-flat layout: its arena blocks have no extent prefix (capacity is size). */
#define HIRSEL_RT_V2_LAYOUT_NO_EXTENT 8
/* `logical` is per element: a block's MEM-6 charge is it times the u64 count at the payload's start. */
#define HIRSEL_RT_V2_LAYOUT_LOGICAL_PER_ELEMENT 16
/* `logical` of a layout with no logical size (a flat encoding, an environment). */
#define HIRSEL_RT_V2_LOGICAL_UNSUPPORTED UINT64_MAX
/* The payload of a Shared root block (MEM-11): the blocks its value reaches are frozen while the
 * root lives, and a deep copy reaching such a block adds a root instead. */
#define HIRSEL_RT_V2_LAYOUT_FROZEN 32
#define HIRSEL_RT_V2_INDEX_MASK 0x1fffffffu
#define HIRSEL_RT_V2_KIND_SHIFT 29
#define HIRSEL_RT_V2_KIND_SMALL 0
#define HIRSEL_RT_V2_WIDE_DEAD 0x80000000u
#define HIRSEL_RT_V2_STEP_RELEASE 1
#define HIRSEL_RT_V2_STEP_FINALIZE 2
#define HIRSEL_RT_V2_STEP_CLOSE_HANDLE 3
#define HIRSEL_RT_V2_STEP_TAG 4
#define HIRSEL_RT_V2_STEP_LOOP 5
#define HIRSEL_RT_V2_STEP_COLUMN 6
#define HIRSEL_RT_V2_STEP_NONNULL 7
#define HIRSEL_RT_V2_STEP_LIVE_BIT 8
/* Restore ordinary counts in the frozen value at offset (read through layout) before releasing
 * it: the first step of a Shared root block's program (MEM-11). */
#define HIRSEL_RT_V2_STEP_THAW 9
#define HIRSEL_RT_V2_STEP_CURSOR_BITS 24
#define HIRSEL_RT_V2_WHOLE_VALUE UINT32_MAX
#define HIRSEL_RT_V2_NO_OFFSET UINT64_MAX

/* Regions (MEM-10). */
#define HIRSEL_RT_V2_SEGMENT_SIZE 65536
#define HIRSEL_RT_V2_REGION_POOL_IDLE_CAP 8388608

typedef struct hirsel_rt_v2_ctx hirsel_rt_v2_ctx;
typedef struct hirsel_rt_v2_layout hirsel_rt_v2_layout;

typedef struct hirsel_rt_v2_header {
    _Alignas(8) uint32_t count;
    uint32_t descriptor;
} hirsel_rt_v2_header;

/* Prefix immediately before a static or region header, except an arena block of a
 * LAYOUT_NO_EXTENT layout, whose capacity is its layout size. */
typedef struct hirsel_rt_v2_extent { uint64_t capacity, align; } hirsel_rt_v2_extent;

/* A user drop body: borrowed value, cleanup anchor; records its failure before returning 1. */
typedef uint8_t (*hirsel_rt_v2_drop_thunk)(hirsel_rt_v2_ctx *ctx, void *value, uint64_t anchor);

/* Closes a run-context handle (the Io adapter's); records its failure before returning 1. */
typedef uint8_t (*hirsel_rt_v2_close_thunk)(hirsel_rt_v2_ctx *ctx, uint64_t handle, uint64_t anchor);

typedef struct hirsel_rt_v2_step {
    uint32_t kind;
    uint32_t field;   /* top-level field ordinal for live masks, or HIRSEL_RT_V2_WHOLE_VALUE */
    uint32_t variant; /* STEP_TAG: the ordinal that keeps the span */
    uint32_t span;    /* STEP_TAG, STEP_NONNULL, STEP_LOOP, STEP_COLUMN: the steps that follow and
                         belong to it */
    uint64_t offset;
    uint64_t stride;       /* STEP_LOOP, STEP_COLUMN: bytes between consecutive elements */
    uint64_t count;        /* STEP_LOOP: static element count; STEP_COLUMN: earlier columns' bytes
                              per element, so the column starts capacity * count further in */
    uint64_t count_offset; /* STEP_LOOP: offset of the u64 element count, or HIRSEL_RT_V2_NO_OFFSET
                              for a static count; STEP_COLUMN: offset of the u64 capacity */
    const hirsel_rt_v2_layout *layout; /* STEP_FINALIZE: the inline resource's layout */
} hirsel_rt_v2_step;

struct hirsel_rt_v2_layout {
    uint32_t magic;
    uint32_t version;
    uint64_t size; /* fixed size, or with LAYOUT_DYNAMIC the bytes every value has */
    uint64_t align;
    hirsel_rt_v2_drop_thunk finalizer; /* own user drop body or NULL; runs before the steps */
    const hirsel_rt_v2_step *steps;    /* drop program in drop order */
    uint64_t step_count;
    uint32_t field_count;
    uint32_t flags;
    uint64_t logical; /* MEM-6 charge of a block of this payload layout (runtime README) */
};

/* Compiler-reserved cleanup continuation for an inline value. */
typedef struct hirsel_rt_v2_cleanup {
    uint32_t index, phase;
    uint64_t cursor;
    void *value;
    hirsel_rt_v2_header *parent;
} hirsel_rt_v2_cleanup;

/* Arena bump ABI: compiled code updates cursor and reads end. The inline limit
 * end is null before the first segment and in diagnostic mode (runtime-only bumps).
 * A bounded arena (MEM-6) has `remaining` logical bytes, which compiled code tests and
 * lowers before each arena construction; it is UINT64_MAX when unbounded.
 * Remaining fields are private to the runtime. */
#define HIRSEL_RT_V2_REGION_CURSOR_OFFSET 8
#define HIRSEL_RT_V2_REGION_END_OFFSET 16
#define HIRSEL_RT_V2_REGION_REMAINING_OFFSET 48
typedef struct hirsel_rt_v2_region {
    void *segment;
    void *cursor;
    void *end;
    void *cleanup;
    struct hirsel_rt_v2_region *parent;
    uint64_t state;
    uint64_t remaining;
    uint32_t bound;     /* 0 unbounded, else 1 + the failure count when the bound was set */
    uint32_t exhausted; /* 0, else 1 + the index of the arena's own OutOfSpace record */
} hirsel_rt_v2_region;

typedef struct hirsel_rt_v2_builder {
    void *data;
    uint64_t capacity;
    uint64_t align;
    hirsel_rt_v2_region *region;
    uint64_t published;
} hirsel_rt_v2_builder;

typedef struct hirsel_rt_v2_config {
    uint32_t abi_version;
    uint32_t flags;
    uint64_t count_max; /* D's count bound: finite counts 1 .. count_max - 1; 0 = full width */
    uint64_t heap_idle_cap;               /* 0 selects 8 MiB */
    hirsel_rt_v2_close_thunk close_handle; /* NULL: a handle drop is an invariant fault */
    uint64_t hash_seed;
    const hirsel_rt_v2_layout *const *layouts;
    uint32_t layout_count, reserved;                    /* SEM-5: keys hash_begin; moves placement only */
} hirsel_rt_v2_config;

/* Present only in archives built with diagnostic or measurement. */
#ifdef HIRSEL_RT_ENABLE_STATS
typedef struct hirsel_rt_v2_stats {
    uint64_t backing_bytes;
    uint64_t backing_allocations;
    uint64_t slab_bytes;
    uint64_t idle_slab_bytes;
    uint64_t large_bytes;
    uint64_t live_blocks;
    uint64_t live_bytes;
    uint64_t region_active_bytes;
    uint64_t region_pool_bytes;
    uint64_t active_regions;
    uint64_t immortalized_blocks;
} hirsel_rt_v2_stats;
#endif

typedef struct hirsel_rt_v2_audit {
    uint64_t live_blocks;
    uint64_t live_bytes;
    uint64_t immortalized_blocks;
    uint64_t active_regions;
    uint64_t retained_bytes;
} hirsel_rt_v2_audit;

typedef struct hirsel_rt_v2_failure {
    uint32_t kind;
    uint32_t rule;
    uint64_t site;
} hirsel_rt_v2_failure;

#define HIRSEL_RT_V2_ASSERT_SIZE(type, bytes) _Static_assert(sizeof(type) == (bytes), #type " size")
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_header, HIRSEL_RT_V2_HEADER_SIZE);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_step, 56);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_layout, 64);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_cleanup, 32);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_region, 64);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_builder, 40);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_config, 56);
#ifdef HIRSEL_RT_ENABLE_STATS
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_stats, 88);
#endif
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_audit, 40);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_failure, 16);
_Static_assert(offsetof(hirsel_rt_v2_header, count) == HIRSEL_RT_V2_COUNT_OFFSET, "header count offset");
_Static_assert(sizeof(((hirsel_rt_v2_header *)0)->count) == 4, "header count width");
_Static_assert(offsetof(hirsel_rt_v2_header, descriptor) == HIRSEL_RT_V2_LAYOUT_OFFSET, "header layout offset");

/* Context and task lifetime. */
uint32_t hirsel_rt_v2_abi_version(void);
uint8_t hirsel_rt_v2_ctx_create(const hirsel_rt_v2_config *config, hirsel_rt_v2_ctx **out);
void hirsel_rt_v2_ctx_destroy(hirsel_rt_v2_ctx *ctx);
/* Only diagnostic/measurement archives populate the audit. Production writes zeros and
 * returns all storage without a live-storage scan; zeros are not a production leak proof. */
void hirsel_rt_v2_task_exit(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_audit *out);
#ifdef HIRSEL_RT_ENABLE_STATS
void hirsel_rt_v2_ctx_stats(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_stats *out);
#endif
void hirsel_rt_v2_trim(hirsel_rt_v2_ctx *ctx);

/* Launch (src/launch.rs): run a compiled `main(rt: Io, args: Args) -> Result[(), E]` entry at
 * the Hirsel ABI whose inline result is result_size bytes; returns the process exit code. */
typedef struct {
    uint64_t hash_seed, random_seed, test_seed;
    uint8_t run_identity[32];
} hirsel_rt_v2_launch_config;
int32_t hirsel_rt_v2_launch_zero(uint8_t (*entry)(hirsel_rt_v2_ctx *, uint8_t *, uint8_t *),
                                 uint64_t result_size, const hirsel_rt_v2_layout *const *layouts, uint32_t layout_count, const hirsel_rt_v2_launch_config *config);
int32_t hirsel_rt_v2_launch(uint8_t (*entry)(hirsel_rt_v2_ctx *, uint8_t *, uint8_t *,
                                             const uint64_t *, const uint64_t *),
                            uint64_t result_size, const hirsel_rt_v2_layout *const *layouts, uint32_t layout_count, const hirsel_rt_v2_launch_config *config);

/* Compiler root-order adapters receive [Io, Args, Env, Unsafe, Test].
 * roots uses bits 0..4 in that order. Standalone admission binds semantic code,
 * then acquires fresh grants selected by --hirsel-run before invoking entry. */
typedef uint8_t (*hirsel_rt_v2_roots_entry)(hirsel_rt_v2_ctx *, uint8_t *, uint8_t *, uint64_t *);
int32_t hirsel_rt_v2_launch_roots(hirsel_rt_v2_roots_entry entry, uint64_t result_size,
    const hirsel_rt_v2_layout *const *layouts, uint32_t layout_count,
    const hirsel_rt_v2_launch_config *config, uint8_t roots);
int32_t hirsel_rt_v2_launch_standalone(hirsel_rt_v2_roots_entry entry, uint64_t result_size,
    const hirsel_rt_v2_layout *const *layouts, uint32_t layout_count,
    const uint8_t program[32], const uint8_t *name, uint64_t name_len, uint8_t roots);

/* Allocation and counts. */
hirsel_rt_v2_header *hirsel_rt_v2_alloc(hirsel_rt_v2_ctx *ctx, uint32_t index, uint64_t extent,
                                        uint64_t align, uint8_t destination, uint64_t site);
void *hirsel_rt_v2_payload(hirsel_rt_v2_header *block);
uint64_t hirsel_rt_v2_capacity(hirsel_rt_v2_header *block);
void hirsel_rt_v2_dup(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
uint8_t hirsel_rt_v2_release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint32_t index,
                             const uint64_t *live_mask, uint64_t anchor);
/* For compiled drop glue (the count word contract above): return the storage of a block whose
 * count the glue read as COUNT_UNIQUE and whose fields it has released. */
void hirsel_rt_v2_free(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);

/* Sequence storage (LIR schema revision 7; runtime ABI 5). A sequence payload is a u64 count, a
 * u64 capacity in elements, then the elements from align_up(16, element_align). The capacity word
 * is never more than the storage's real slots. Make the sequence block hold at least `capacity`
 * elements of `element_size` bytes (the stride) at `element_align`. A unique heap block that
 * already holds them is left alone; otherwise its count and elements move bitwise into storage
 * of the size class that fits, the old storage is freed shallowly and the new block is
 * returned. A block that is not unique (shared, immortal or static) is detached once at the
 * requested capacity: its count and elements are copied, each counted element gains a holder
 * and the caller's credit of it is given back. Every path leaves the returned block's capacity
 * word exact. A dead block and an impossible size are invariant faults. */
hirsel_rt_v2_header *hirsel_rt_v2_seq_grow(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t element_size,
                              uint64_t element_align, uint64_t capacity);
/* Block growth (LIR schema revision 19): make the unique heap block hold at least `extent`
 * payload bytes at `align`; its first `keep` bytes move bitwise into storage of the size class
 * that fits, the old storage is freed shallowly and the new block is returned. A block that
 * already holds them is left alone. Faults are as for seq_grow. */
hirsel_rt_v2_header *hirsel_rt_v2_block_grow(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t keep,
                                uint64_t extent, uint64_t align);

/* Keyed hashing (LIR schema revision 7; SEM-5). hash_begin answers the run's keyed state and
 * hash_bytes folds `length` bytes into a state with SipHash-1-3. The bits are unobservable. */
uint64_t hirsel_rt_v2_hash_begin(hirsel_rt_v2_ctx *ctx);
uint64_t hirsel_rt_v2_hash_bytes(uint64_t state, const void *bytes, uint64_t length);

/* The canonical SER-2 codec (LIR schema revision 29; runtime ABI 9). A codec is compiler-emitted
 * static data: the schema's descriptor bytes, which an envelope must match exactly, and a program
 * of uint64_t words that pairs each schema node with the native layout of its values
 * (hirsel_lir::codec::CodecPlan::program). Program words: CODEC_VERSION, the node count, the byte
 * size of `out`, the root node, the out node and the Bytes node, then one word per node giving
 * the word offset of its body. A body is a CODEC_NODE_* kind, then:
 *   SCALAR    scalar code (bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64)
 *   TEXT      utf8, the slice's owner, start and length offsets, the byte sequence's index
 *   FIELDS    count, then (offset, node) per field in declaration order
 *   VARIANT   form (0 ordinal u32, 1 null-pointer niche), tag offset, the niche's nullary arm,
 *             arm count, then per arm in ordinal order: count, (offset, node) per field;
 *             finally one native active extent per arm
 *   BOX       payload index, size (NO_NODE for variable flat), align, payload node
 *   SEQUENCE  payload index, element align, first element offset, stride, element node
 *   REPEAT    count, stride, element node
 *   MAP, SET  container payload index, size, align, entries and index field offsets, the entries
 *             sequence's index, element align, first element offset and stride, the index
 *             sequence's index, element align and stride, key node, key offset, value node (or
 *             CODEC_NO_NODE), value offset
 *   FLAT_TEXT utf8; native storage is a u64 byte extent then bytes
 *   FRAMED    content node; native storage adds a u64 content extent
 *   FLAT_FIELDS count, then node per declaration-order field
 *   FLAT_VARIANT arm count, then count and field nodes per arm (native u32 tag)
 *   FLAT_SEQUENCE first element offset (8 fixed, 16 variable), fixed stride or 0, node
 *   SEQUENTIAL payload index, align, first element offset, element node
 *   COLUMNS   payload index, align, data offset, capacity quantum, element node,
 *             column count, then (capacity multiplier, stride, field node) per column
 * Every walk is iterative: no native stack depth follows the data. codec_encode reads the value at
 * `value` (the root node's layout) and writes a fresh Bytes slice of the envelope at `out`.
 * codec_decode reads the Bytes slice at `bytes`, validates the envelope and the whole payload
 * first, then writes Result[T, DecodeError] at `out`: Ok of fresh owned data, or Error of the
 * first malformation in reading order. Neither fails; operational budgets (the reference's
 * limits) terminate environmentally. */
#define HIRSEL_RT_V2_CODEC_VERSION 2
#define HIRSEL_RT_V2_CODEC_NODE_SCALAR 0
#define HIRSEL_RT_V2_CODEC_NODE_TEXT 1
#define HIRSEL_RT_V2_CODEC_NODE_FIELDS 2
#define HIRSEL_RT_V2_CODEC_NODE_VARIANT 3
#define HIRSEL_RT_V2_CODEC_NODE_BOX 4
#define HIRSEL_RT_V2_CODEC_NODE_SEQUENCE 5
#define HIRSEL_RT_V2_CODEC_NODE_REPEAT 6
#define HIRSEL_RT_V2_CODEC_NODE_MAP 7
#define HIRSEL_RT_V2_CODEC_NODE_SET 8
#define HIRSEL_RT_V2_CODEC_NODE_FLAT_TEXT 9
#define HIRSEL_RT_V2_CODEC_NODE_FRAMED 10
#define HIRSEL_RT_V2_CODEC_NODE_FLAT_FIELDS 11
#define HIRSEL_RT_V2_CODEC_NODE_FLAT_VARIANT 12
#define HIRSEL_RT_V2_CODEC_NODE_FLAT_SEQUENCE 13
#define HIRSEL_RT_V2_CODEC_NODE_SEQUENTIAL 14
#define HIRSEL_RT_V2_CODEC_NODE_COLUMNS 15
#define HIRSEL_RT_V2_CODEC_NO_NODE UINT64_MAX
typedef struct hirsel_rt_v2_codec {
    const uint8_t *descriptor;
    uint64_t descriptor_length;
    const uint64_t *program;
    uint64_t program_length;
} hirsel_rt_v2_codec;
void hirsel_rt_v2_codec_encode(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_codec *codec, const void *value, void *out);
void hirsel_rt_v2_codec_decode(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_codec *codec, const void *bytes, void *out);

void hirsel_rt_v2_region_owner(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, hirsel_rt_v2_header *block, uint32_t index, uint64_t capacity, uint64_t align);

/* Raw reuse tokens. */
uint8_t hirsel_rt_v2_reset(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint32_t index,
                           const uint64_t *live_mask, uint64_t anchor, hirsel_rt_v2_header **out_token);
uint8_t hirsel_rt_v2_reuse(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *token, uint32_t index,
                           uint64_t extent, uint64_t align, hirsel_rt_v2_header **out_block);
void hirsel_rt_v2_dispose(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *token);

/* Deep unique copy. Into a bounded arena (MEM-6) the whole tree is charged first, every block once
 * per reference; STATUS_EXHAUSTED when that exceeds the arena's remaining budget. */
uint8_t hirsel_rt_v2_unique_copy(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, hirsel_rt_v2_region *region, uint64_t site,
                                 hirsel_rt_v2_header **out_block);

/* Deep unique copy of an inline value: its holder slots rewritten to copies in place. */
uint8_t hirsel_rt_v2_unique_copy_value(hirsel_rt_v2_ctx *ctx, uint32_t index,
                                       uint8_t *value, hirsel_rt_v2_region *region, uint64_t site);

/* Copy-on-write detach: the block alone, its counted children shared. */
uint8_t hirsel_rt_v2_detach(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t site,
                            hirsel_rt_v2_header **out_block);

/* Shared roots (MEM-11). The root block's descriptor begins with STEP_THAW naming its frozen
 * value's layout. Freeze makes every counted block the value reaches immortal, stopping at
 * static blocks and nested roots; thaw (of the last root) restores each to one holder. */
void hirsel_rt_v2_freeze(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
void hirsel_rt_v2_thaw(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);

/* Finalizers. */
uint8_t hirsel_rt_v2_finalize(hirsel_rt_v2_ctx *ctx, uint32_t index, void *value,
                              const uint64_t *live_mask, hirsel_rt_v2_cleanup *continuation, uint64_t anchor);
void hirsel_rt_v2_finalize_begin(hirsel_rt_v2_ctx *ctx, uint64_t anchor);
void hirsel_rt_v2_finalize_end(hirsel_rt_v2_ctx *ctx, uint64_t anchor);
uint8_t hirsel_rt_v2_close_handle(hirsel_rt_v2_ctx *ctx, uint64_t handle, uint64_t anchor);
uint8_t hirsel_rt_v2_shielded(hirsel_rt_v2_ctx *ctx);

/* Regions. */
void hirsel_rt_v2_region_enter(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region);
uint8_t hirsel_rt_v2_region_reserve(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t extent,
                                    uint64_t align, void **out);
uint8_t hirsel_rt_v2_region_hold(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, hirsel_rt_v2_header *block);
uint8_t hirsel_rt_v2_region_schedule(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region,
                                     uint32_t index, void *value);
uint8_t hirsel_rt_v2_region_exit(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t anchor);
/* Arenas: an immortal region-kind block of layout `index` whose
 * payload the caller fills; the record is an arena's region. A LAYOUT_NO_EXTENT layout's block
 * is its eight-byte header and payload; every other block also has its extent before the header.
 * Every region reservation ends on a word, so the cursor is always eight-byte aligned. */
hirsel_rt_v2_header *hirsel_rt_v2_arena_alloc(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region,
                                              uint32_t index, uint64_t extent, uint64_t align);
/* Enter an arena's region, seeding its cursor and limit from an idle pooled segment if any. */
void hirsel_rt_v2_arena_enter(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region);
/* Bounded arenas (MEM-6). Bound an entered arena to `capacity` logical bytes. */
void hirsel_rt_v2_arena_bound(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t capacity);
/* Record the arena's own OutOfSpace failure (`kind`, `rule` at `site`); returns STATUS_FAILURE. */
uint8_t hirsel_rt_v2_arena_exhausted(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint32_t kind,
                                     uint32_t rule, uint64_t site);
/* 1 after forgetting the active failure when it is exactly the arena's own OutOfSpace, the one
 * failure recorded since its bound was set; else 0, the failure untouched. */
uint8_t hirsel_rt_v2_arena_recover(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region);
uint8_t hirsel_rt_v2_builder_begin(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t capacity,
                                   uint64_t align, hirsel_rt_v2_builder *out);
uint8_t hirsel_rt_v2_builder_grow(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_builder *builder, uint64_t used,
                                  uint64_t min_capacity);
void hirsel_rt_v2_builder_publish(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_builder *builder, uint64_t used);

/* Failures. */
uint8_t hirsel_rt_v2_failure_begin(hirsel_rt_v2_ctx *ctx, uint32_t kind, uint32_t rule, uint64_t site);
uint8_t hirsel_rt_v2_failure_append(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_failure *failure);
uint8_t hirsel_rt_v2_failure_active(hirsel_rt_v2_ctx *ctx);
uint8_t hirsel_rt_v2_failure_primary(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_failure *out);
uint64_t hirsel_rt_v2_failure_cause_count(hirsel_rt_v2_ctx *ctx);
uint8_t hirsel_rt_v2_failure_cause(hirsel_rt_v2_ctx *ctx, uint64_t index, hirsel_rt_v2_failure *out);
void hirsel_rt_v2_failure_reanchor(hirsel_rt_v2_ctx *ctx, uint64_t site);
void hirsel_rt_v2_failure_clear(hirsel_rt_v2_ctx *ctx);
void hirsel_rt_v2_isolate_catch(hirsel_rt_v2_ctx *ctx, uint64_t start);

/* PROG-3 host return-value descriptions. Immutable compiler data; no Io grant.
 * kind: 0 opaque text, 1 bool, 2 unsigned / 3 signed (a=bytes), 4 float
 * (a=bytes), 5 String / 6 Bytes slice, 7 aggregate (text=open; a=close:
 * 0 empty, 1 ')', 2 ' }', 3 ']'; b=1 means a one-tuple), 8 variant
 * (a=tag offset, b=0 u32 / 1 pointer niche / 2 live bit; c=niche ordinals
 * nullary in low32, dataful high32, or bit index), 9 box (a=payload offset),
 * 10 counted sequence (a=data offset from header, b=stride, c=count offset),
 * 11 inline sequence (a=count,b=stride), 12 transparent reference.
 * Variant fields' offsets select ordinals; other fields select byte offsets. */
typedef struct hirsel_rt_v2_report_node hirsel_rt_v2_report_node;
typedef struct hirsel_rt_v2_report_field {
    uint64_t offset;
    const hirsel_rt_v2_report_node *node;
    const uint8_t *label;
    uint64_t label_len;
} hirsel_rt_v2_report_field;
struct hirsel_rt_v2_report_node {
    uint64_t kind, a, b, c;
    const uint8_t *text;
    uint64_t text_len;
    const hirsel_rt_v2_report_field *fields;
    uint64_t field_count;
};
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_report_node, 64);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_report_field, 32);
void hirsel_rt_v2_exit_report(hirsel_rt_v2_ctx *ctx, uint8_t *value,
                              uint32_t layout, uint64_t payload_offset,
                              const hirsel_rt_v2_report_node *node);

/* Diagnostic probe: panics inside the runtime; the process aborts at the C boundary. */
void hirsel_rt_v2_invariant_fault(void);
void hirsel_rt_v2_diag_panic(void);

/* PROG-4: an extern call records its thread's run context for C callback thunks; a
   failing callback ends the process. */
void hirsel_rt_v2_foreign_enter(hirsel_rt_v2_ctx *ctx);
hirsel_rt_v2_ctx *hirsel_rt_v2_callback_context(void);
void hirsel_rt_v2_callback_failed(hirsel_rt_v2_ctx *ctx);

#ifdef __cplusplus
}
#endif

#endif /* HIRSEL_RT_V2_H */

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

#define HIRSEL_RT_V2_ABI_VERSION 94
/* TaskSpawn trailing flags: compiler proofs, ignored in ordered execution and replay. */
#define HIRSEL_RT_V2_SPAWN_IMMEDIATE_AWAIT 1
#define HIRSEL_RT_V2_SPAWN_OP_FREE 2
#define HIRSEL_RT_V2_FOREIGN_SCOPE_BYTES 40
#define HIRSEL_RT_V2_FOREIGN_SCOPE_ALIGN 8
#define HIRSEL_RT_V2_FOREIGN_THREADS_HOME 0
#define HIRSEL_RT_V2_FOREIGN_THREADS_SERIAL 1
#define HIRSEL_RT_V2_FOREIGN_THREADS_ANY 2
#define HIRSEL_RT_V2_FOREIGN_THREADS_MASK 3
#define HIRSEL_RT_V2_FOREIGN_BLOCKING 4
#define HIRSEL_RT_V2_FOREIGN_PACKAGE_SHIFT 8
#define HIRSEL_RT_V2_STATUS_OK 0
#define HIRSEL_RT_V2_STATUS_FAILURE 1
/* A deep copy into a bounded arena its budget does not cover (MEM-6): nothing is copied or
 * recorded; the caller records the arena's OutOfSpace with hirsel_rt_v2_arena_exhausted. */
#define HIRSEL_RT_V2_STATUS_EXHAUSTED 2
/* Private compiler adapter protocol: all actuals remain in caller custody. */
#define HIRSEL_RT_V2_STATUS_PRETRANSFER_FAILURE 3
/* A resumable continuation alternate saved its frame at a boundary whose call has not run. */
#define HIRSEL_RT_V2_STATUS_SUSPENDED 4

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
 * storage kind and dead-wide-cursor flag.
 * Shared root exception (ABI 25, MEM-11): a root block's count is its exact root count. It is
 * atomic, never saturates and is never released inline: a holder is added by a CAS of count + 1
 * while count < the u32 at CTX_DUP_MAX_OFFSET, else by hirsel_rt_v2_root_dup, and every release
 * calls hirsel_rt_v2_root_release (or reaches a walk's STEP_RELEASE_ROOT). Past the run's finite
 * maximum the header holds ROOT_OVERFLOW and the exact count lives in the runtime. */
#define HIRSEL_RT_V2_COUNT_OFFSET 0
#define HIRSEL_RT_V2_COUNT_UNIQUE 1
#define HIRSEL_RT_V2_COUNT_IMMORTAL UINT32_MAX
#define HIRSEL_RT_V2_ROOT_OVERFLOW UINT32_MAX
/* A tracked arena block (a List or Array in an arena, MEM-6, ABI 11, or a Map's or Set's block,
 * ABI 12) counts ARENA_UNIQUE while
 * unshared, which a write may change in place though it is never unique, and ARENA_SHARED once a
 * dup shares it, which its next write copies within its arena. No count at or above ARENA_SHARED
 * is ever released. */
#define HIRSEL_RT_V2_COUNT_ARENA_UNIQUE 4294967294u
#define HIRSEL_RT_V2_COUNT_ARENA_SHARED 4294967293u
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
/* A tracked arena block's arena record address, TRACKED_REGION_BACK bytes before its header:
 * TRACKED_PREFIX_BYTES precede its payload in all, the header included (ABI 11); a fixed
 * layout's extent words stay unused (ABI 12). */
#define HIRSEL_RT_V2_TRACKED_REGION_BACK 24
#define HIRSEL_RT_V2_TRACKED_PREFIX_BYTES 32
/* `flags` of arena_alloc and unique_copy: the block, or a deep copy's root, is tracked. */
#define HIRSEL_RT_V2_ALLOC_TRACKED 1
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
 *          head = page->local_free; unless head is nonnull and the u32 page->owner (at
 *          PAGE_OWNER_OFFSET) equals the u32 heap id at CTX_HEAP_ID_OFFSET, hirsel_rt_v2_free
 *          (a block of another worker's heap is published to its owner); otherwise
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
/* Run budgets (budgets.limits@2, runtime ABI 27): after the pages, the running task's i64 fuel
 * chunk. A fuel build's code subtracts one at each fuel point and, when the result is negative,
 * calls hirsel_rt_v2_fuel_refill and continues after the call. */
#define HIRSEL_RT_V2_CTX_FUEL_OFFSET 528
/* Runtime ABI 26: after the fuel word, the u32 id of the heap whose pages the context holds
 * (the running worker's). A slab's u32 owner is its heap's id; its xthread_free word is the
 * runtime's list of blocks other heaps freed, which compiled code never touches. */
#define HIRSEL_RT_V2_CTX_HEAP_ID_OFFSET 536
/* Runtime ABI 65: after the context's view (the words above, then a u32 heap and a u64
 * epoch), the task's LANG-10 counters, written only by the runtime: the u16 foreground
 * and cleanup admission counts and the u32 finalizer nesting. Compiled code reads them to
 * test a depth-leaf callee's room: in cleanup mode (nesting or cleanup count nonzero)
 * while the cleanup count is below 64, otherwise while the foreground count is below 1024. */
#define HIRSEL_RT_V2_CTX_FOREGROUND_COUNT_OFFSET 560
#define HIRSEL_RT_V2_CTX_CLEANUP_COUNT_OFFSET 562
#define HIRSEL_RT_V2_CTX_CLEANUP_MODE_OFFSET 564
#define HIRSEL_RT_V2_PAGE_FREE_OFFSET 16
#define HIRSEL_RT_V2_PAGE_LIVE_OFFSET 24
#define HIRSEL_RT_V2_PAGE_LOCAL_FREE_OFFSET 72
#define HIRSEL_RT_V2_PAGE_OWNER_OFFSET 96
#define HIRSEL_RT_V2_PAGE_XTHREAD_FREE_OFFSET 104
_Static_assert(HIRSEL_RT_V2_CTX_HEAP_ID_OFFSET == HIRSEL_RT_V2_CTX_FUEL_OFFSET + 8,
               "the heap id follows the fuel word");
_Static_assert(HIRSEL_RT_V2_PAGE_XTHREAD_FREE_OFFSET == HIRSEL_RT_V2_PAGE_OWNER_OFFSET + 8,
               "the xthread list follows the owner id and its padding");

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
#define HIRSEL_RT_V2_LAYOUT_VERSION 18
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
/* A sequence's payload: a deep copy into an arena tracks a block of it unless it lies below an
 * UNTRACKED_BELOW block (a SortedMap's, a Slots', an environment's), whose sequences stay
 * untracked. */
#define HIRSEL_RT_V2_LAYOUT_SEQUENCE 64
#define HIRSEL_RT_V2_LAYOUT_UNTRACKED_BELOW 128
/* A Map's or Set's payload (version 8): its entries List at offset 0, then its index. Its own
 * logical size is 0; a deep copy into a bounded arena tracks it and its entries and neither
 * tracks nor charges its index (MEM-6, ABI 12). */
#define HIRSEL_RT_V2_LAYOUT_KEYED 256
/* An @soa List's columns payload (version 9): tracked in a bounded arena as a sequence's; a push
 * copies a shared one at its capacity and growth regrows it there (MEM-6, ABI 14). */
#define HIRSEL_RT_V2_LAYOUT_COLUMNS 512
/* A sequential sequence's payload, of Packed values (version 10): tracked in a bounded arena as a
 * sequence's; a push copies a shared one at its capacity and block_grow regrows it there (MEM-6,
 * ABI 15). */
#define HIRSEL_RT_V2_LAYOUT_SEQUENTIAL 1024
#define HIRSEL_RT_V2_LAYOUT_MUTEX_ROOT 2048
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
/* A RELEASE after which its value runs nothing more (version 11): when it kills its child, the
 * dying block is freed at once and the child takes its place (a tail release). Never in a loop. */
#define HIRSEL_RT_V2_STEP_RELEASE_LAST 10
/* Release the Shared root reference at offset under the root count contract (version 13): exact,
 * and deferred where the run's mode defers root releases. Never a tail release. */
#define HIRSEL_RT_V2_STEP_RELEASE_ROOT 11
#define HIRSEL_RT_V2_STEP_END_FINALIZE 12
/* Consume one non-null private semantic metadata holder (descriptor version 17).
 * No language/Shared credit, allocation, user code, or suspension. */
#define HIRSEL_RT_V2_STEP_RELEASE_RECEIPT 13
/* Consume one affine opaque sealed owner; never a tail release. Descriptor version 18. */
#define HIRSEL_RT_V2_STEP_RELEASE_SEALED 14
#define HIRSEL_RT_V2_LAYOUT_IMPLEMENTATION_FINALIZER 4096
#define HIRSEL_RT_V2_STEP_CURSOR_BITS 24
#define HIRSEL_RT_V2_WHOLE_VALUE UINT32_MAX
#define HIRSEL_RT_V2_NO_OFFSET UINT64_MAX

/* Regions (MEM-10). */
#define HIRSEL_RT_V2_SEGMENT_SIZE 65536
#define HIRSEL_RT_V2_REGION_POOL_IDLE_CAP 8388608

typedef struct hirsel_rt_v2_ctx hirsel_rt_v2_ctx;
uint8_t hirsel_rt_v2_depth_enter(hirsel_rt_v2_ctx *ctx);
uint8_t hirsel_rt_v2_depth_enter_at_site(hirsel_rt_v2_ctx *ctx, uint64_t anchor, uint64_t operation_site);
void hirsel_rt_v2_depth_leave(hirsel_rt_v2_ctx *ctx);
uint8_t hirsel_rt_v2_depth_preflight(hirsel_rt_v2_ctx *ctx, const void *program, uint64_t program_length, const void *edges, uint64_t edge_count, const void *value);
/* Value walkers after complete logical admission: order is equal=0, less=1,
 * greater=2, unordered=3. No serialization or arena budget is involved. */
uint8_t hirsel_rt_v2_structural_order(hirsel_rt_v2_ctx *ctx, const void *program, uint64_t program_length, const void *left, const void *right);
uint64_t hirsel_rt_v2_structural_hash(hirsel_rt_v2_ctx *ctx, const void *program, uint64_t program_length, const void *value, uint64_t seed);
typedef struct hirsel_rt_v2_layout hirsel_rt_v2_layout;

typedef struct hirsel_rt_v2_header {
    _Alignas(8) uint32_t count;
    uint32_t descriptor;
} hirsel_rt_v2_header;

/* Prefix immediately before a static or region header, except an arena block of a
 * LAYOUT_NO_EXTENT layout, whose capacity is its layout size. */
typedef struct hirsel_rt_v2_extent { uint64_t capacity, align; } hirsel_rt_v2_extent;

/* A user drop body: borrowed value, cleanup anchor; records its failure before returning 1. */
typedef uint8_t (*hirsel_rt_v2_drop_thunk)(hirsel_rt_v2_ctx *ctx, void *value, uint64_t anchor, uint64_t operation_site);

/* Closes a run-context handle (the Io adapter's); records its failure before returning 1. */
typedef uint8_t (*hirsel_rt_v2_close_thunk)(hirsel_rt_v2_ctx *ctx, uint64_t handle, uint64_t anchor, uint64_t operation_site);

typedef struct hirsel_rt_v2_step {
    uint32_t kind;
    uint32_t field;   /* top-level field ordinal for live masks, or HIRSEL_RT_V2_WHOLE_VALUE */
    uint32_t variant; /* STEP_TAG: the ordinal that keeps the span */
    uint32_t span;    /* STEP_TAG, STEP_NONNULL, STEP_LOOP, STEP_COLUMN: the steps that follow and
                         belong to it */
    uint64_t offset;
    uint64_t stride;       /* STEP_LOOP, STEP_COLUMN: bytes between consecutive elements;
                              RELEASE/RELEASE_LAST: 24 denotes a String/Bytes slice (layout v15) */
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
    /* NULL: count all elements. Otherwise five words: first element offset, stride,
       tag offset, form (0 ordinal Some=1, 1 live bit, 2 non-null niche), bit number.
       Only initialized live entries contribute to a copy/detach charge. */
    const uint64_t *logical_count;
    /* Holder scan: eight-word instructions from hirsel-kernels::handle_scan.
       scan_words 0 certifies no holders; UINT64_MAX means unknown metadata. */
    const uint64_t *scan;
    uint64_t scan_words;
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
    uint64_t heap_idle_cap;               /* 0 selects 1 MiB */
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
    uint64_t operation_site;
} hirsel_rt_v2_failure;

#define HIRSEL_RT_V2_ASSERT_SIZE(type, bytes) _Static_assert(sizeof(type) == (bytes), #type " size")
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_header, HIRSEL_RT_V2_HEADER_SIZE);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_step, 56);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_layout, 88);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_cleanup, 32);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_region, 64);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_builder, 40);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_config, 56);
#ifdef HIRSEL_RT_ENABLE_STATS
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_stats, 88);
#endif
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_audit, 40);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_failure, 24);
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
/* Workers (runtime ABI 26, src/heaps.rs): a context for a new worker of ctx's run, on a heap of
 * its own; its retirement, after which its heap stays in the run, orphaned, until the run ends;
 * and a worker's scheduling point, which takes back the storage other workers freed to its heap
 * and collects orphaned heaps. A free of another heap's block publishes it to its owner. */
hirsel_rt_v2_ctx *hirsel_rt_v2_worker_create(hirsel_rt_v2_ctx *ctx);
void hirsel_rt_v2_worker_retire(hirsel_rt_v2_ctx *ctx);
void hirsel_rt_v2_worker_collect(hirsel_rt_v2_ctx *ctx);

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

/* A compiled program (src/program.rs), which the generated `main` passes to hirsel_rt_v2_main.
 * The root-order adapter receives [Io, Args, Env, Unsafe, Test]; roots uses bits 0..4 in that
 * order. A STANDALONE program binds its semantic digest and entry and acquires fresh grants
 * at each launch; otherwise config binds the one run the executable was built for. The
 * runtime keeps the failure table per program, so runs of several programs share a process.
 * The failure table (ABI 22) is little-endian: u32 site and rule counts, 32-byte site records
 * (u64 code, u32 line, u32 column, u32 offset and length of the definition path, then of the
 * file path) sorted by code, 8-byte rule records (u32 offset, length), then a UTF-8 string
 * pool; the runtime reads it in place. The entry rows (ABI 30, RUN-3) name each definition the
 * program's units make an Entry value of: its function-reference code and full UTF-8 path. */
typedef uint8_t (*hirsel_rt_v2_roots_entry)(hirsel_rt_v2_ctx *, uint8_t *, uint8_t *, uint64_t *);
#define HIRSEL_RT_V2_PROGRAM_UNREPLAYABLE 1u
#define HIRSEL_RT_V2_PROGRAM_STANDALONE 2u
#define HIRSEL_RT_V2_PROGRAM_ESCAPES 4u
#define HIRSEL_RT_V2_PROGRAM_FUELED 8u
typedef struct {
    const void *code;
    const uint8_t *path;
    uint64_t path_length;
} hirsel_rt_v2_entry_row;
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_entry_row, 24);
typedef struct {
    uint32_t abi_version, flags, roots, layout_count;
    hirsel_rt_v2_roots_entry entry;
    uint64_t result_size;
    const hirsel_rt_v2_layout *const *layouts;
    const uint8_t *failure_table;
    uint64_t failure_table_length;
    const uint8_t *entry_name;
    uint64_t entry_name_length;
    uint8_t digest[32];
    hirsel_rt_v2_launch_config config;
    const hirsel_rt_v2_entry_row *entries;
    uint64_t entry_count;
    /* ABI 45: 0 = Result[(), E], 1 = ExitCode, 2 = Result[ExitCode, E]. */
    uint64_t return_kind, success_offset;
    /* ABI 48: initialized callable values; separate from RUN-3 Entry admission. */
    const hirsel_rt_v2_entry_row *value_codes;
    uint64_t value_code_count;
    /* ABI 49: canonical runner CalleeFacts and verified RUN-3 code references. */
    const uint8_t *callee_facts;
    uint64_t callee_facts_length;
    const hirsel_rt_v2_entry_row *callee_codes;
    uint64_t callee_code_count;
    uint8_t callee_identity[32];
} hirsel_rt_v2_program_descriptor;
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_program_descriptor, 272);
/* Load the program and launch the run argv (argv[0] names the program) selects; returns the
 * exit status of its outcome. No other process arguments are read. */
int32_t hirsel_rt_v2_main(const hirsel_rt_v2_program_descriptor *program, int32_t argc, const char *const *argv);

/* Allocation and counts. */
hirsel_rt_v2_header *hirsel_rt_v2_alloc(hirsel_rt_v2_ctx *ctx, uint32_t index, uint64_t extent,
                                        uint64_t align, uint8_t destination, uint64_t site);
void *hirsel_rt_v2_payload(hirsel_rt_v2_header *block);
uint64_t hirsel_rt_v2_capacity(hirsel_rt_v2_header *block);
void hirsel_rt_v2_dup(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
uint8_t hirsel_rt_v2_release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint32_t index,
                             const uint64_t *live_mask, uint64_t anchor, uint64_t operation_site);
/* For compiled drop glue (the count word contract above): return the storage of a block whose
 * count the glue read as COUNT_UNIQUE and whose fields it has released. */
void hirsel_rt_v2_free(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
/* Constructor-tail destinations (LANG-10, runtime ABI 70). A block allocated at an open stand-in
 * layout (plain bytes of its box payload's size and alignment, nothing to release) has had its
 * payload written: its header takes the layout at `index`, of the same size and alignment. Until
 * then a release frees its storage alone. */
void hirsel_rt_v2_hole_close(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint32_t index);

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
 * hash_bytes hashes one scalar with SipHash-1-3. Composite keys initialize one caller-owned
 * 48-byte, 8-aligned stream, update it for every framed leaf, and finalize once. The state
 * address uses an opaque uint64_t carrier inside layout hash helpers. Bits are unobservable. */
uint64_t hirsel_rt_v2_hash_begin(hirsel_rt_v2_ctx *ctx);
uint64_t hirsel_rt_v2_hash_bytes(uint64_t state, const void *bytes, uint64_t length);
void hirsel_rt_v2_hash_init(void *out, uint64_t seed);
uint64_t hirsel_rt_v2_hash_update(uint64_t state, const void *bytes, uint64_t length);
uint64_t hirsel_rt_v2_hash_finish(uint64_t state);

/* The canonical SER-2 codec (LIR schema revision 29; runtime ABI 9). A codec is compiler-emitted
 * static data: the schema's descriptor bytes, which an envelope must match exactly, and a program
 * of uint64_t words that pairs each schema node with the native layout of its values
 * (hirsel_lir::codec::CodecPlan::program). Program words: CODEC_VERSION, the node count, the byte
 * size of `out`, the root node, the out node, the Bytes node and the root layout's descriptor
 * index (CODEC_NO_NODE without CHECKED nodes), then one word per node giving
 * the word offset of its body. A body is a CODEC_NODE_* kind, then:
 *   SCALAR    scalar code (bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64,
 *             char: 4 bytes that decode only as a Unicode scalar value)
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
 *             CODEC_NO_NODE), value offset; both additionally carry hashes and live-count
 *             field offsets, entry tag offset, form (0 ordinal, 1 live bit, 2 non-null niche)
 *             and bit number. Both encode only live entries; decode initializes Some entries,
 *             an empty hash cache/index and the live count. Wire bytes are unchanged.
 *   FLAT_TEXT utf8; native storage is a u64 byte extent then bytes
 *   FRAMED    content node; native storage adds a u64 content extent
 *   FLAT_FIELDS count, then node per declaration-order field
 *   FLAT_VARIANT arm count, then count and field nodes per arm (native u32 tag)
 *   FLAT_SEQUENCE first element offset (8 fixed, 16 variable), fixed stride or 0, node
 *   SEQUENTIAL payload index, align, first element offset, element node
 *   COLUMNS   payload index, align, data offset, capacity quantum, element node,
 *             column count, then (capacity multiplier, stride, field node) per column
 *   PATH      no operands; native storage is a u64 run-context Path identity, the wire its text
 *             as a String; decoding admits any UTF-8 text and interns it as path_of
 *   CHECKED   check index into the codec's checks, value node at the same address: a checked
 *             opaque type (SER-1). After materializing, decode calls each check on its value,
 *             inner occurrences first; at the first verdict 0 it releases the value through the
 *             root descriptor and writes Error(Invalid). A check's failure status ends the run
 *             with its recorded failure and no cleanup (FAIL-3).
 * Every walk is iterative: no native stack depth follows the data. codec_encode reads the value at
 * `value` (the root node's layout) and writes a fresh Bytes slice of the envelope at `out`.
 * codec_decode reads the Bytes slice at `bytes`, validates the envelope and the whole payload
 * first, then writes Result[T, DecodeError] at `out`: Ok of fresh owned data, or Error of the
 * first malformation in reading order. Neither fails; operational budgets (the reference's
 * limits) terminate environmentally. */
#define HIRSEL_RT_V2_CODEC_VERSION 5
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
#define HIRSEL_RT_V2_CODEC_NODE_PATH 16
#define HIRSEL_RT_V2_CODEC_NODE_CHECKED 17
#define HIRSEL_RT_V2_CODEC_NO_NODE UINT64_MAX
/* A checked opaque type's check on the borrowed value at `value`: writes its verdict (0 or 1)
 * at `verdict`; returns 0, or 1 with the check's failure recorded. */
typedef uint8_t (*hirsel_rt_v2_check_thunk)(hirsel_rt_v2_ctx *ctx, const void *value, uint8_t *verdict);
typedef struct hirsel_rt_v2_codec {
    const uint8_t *descriptor;
    uint64_t descriptor_length;
    const uint64_t *program;
    uint64_t program_length;
    const hirsel_rt_v2_check_thunk *checks;
    uint64_t check_count;
} hirsel_rt_v2_codec;
void hirsel_rt_v2_codec_encode(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_codec *codec, const void *value, void *out);
void hirsel_rt_v2_codec_decode(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_codec *codec, const void *bytes, void *out);

void hirsel_rt_v2_region_owner(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, hirsel_rt_v2_header *block, uint32_t index, uint64_t capacity, uint64_t align);

/* Raw reuse tokens. */
uint8_t hirsel_rt_v2_reset(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint32_t index,
                           const uint64_t *live_mask, uint64_t anchor, uint64_t operation_site, hirsel_rt_v2_header **out_token);
uint8_t hirsel_rt_v2_reuse(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *token, uint32_t index,
                           uint64_t extent, uint64_t align, hirsel_rt_v2_header **out_block);
void hirsel_rt_v2_dispose(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *token);

/* Deep unique copy. Into a bounded arena (MEM-6) the whole tree is charged first, every block once
 * per reference; STATUS_EXHAUSTED when that exceeds the arena's remaining budget. Into an arena,
 * its sequence blocks are tracked, the root when `flags` has ALLOC_TRACKED. */
uint8_t hirsel_rt_v2_unique_copy(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, hirsel_rt_v2_region *region,
                                 uint32_t flags, uint64_t site, hirsel_rt_v2_header **out_block);

/* Deep unique copy of an inline value: its holder slots rewritten to copies in place. */
uint8_t hirsel_rt_v2_unique_copy_value(hirsel_rt_v2_ctx *ctx, uint32_t index,
                                       uint8_t *value, hirsel_rt_v2_region *region, uint64_t site);

/* Copy-on-write detach: the block alone, its counted children shared. A tracked arena block is
 * copied within its arena, charged its count (MEM-6): STATUS_EXHAUSTED, with nothing copied,
 * when that exceeds the arena's remaining budget. */
uint8_t hirsel_rt_v2_detach(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t site,
                            hirsel_rt_v2_header **out_block);

/* Shared roots (MEM-11). The root block's descriptor begins with STEP_THAW naming its frozen
 * value's layout. Freeze makes every counted block the value reaches immortal, stopping at
 * static blocks and nested roots; thaw (of the last root) restores each to one holder. */
void hirsel_rt_v2_freeze(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
void hirsel_rt_v2_thaw(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
/* Root counts (ABI 25): add one root past the inline CAS's bound, exactly; give up one root by the
 * run's mode, walking the root when it was the last and the release applies now. */
void hirsel_rt_v2_root_dup(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
void hirsel_rt_v2_root_release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);
/* Private immutable semantic metadata only. Non-null handles carry explicit
 * metadata holders, independent of source/Shared credits and task lifetimes.
 * retain borrows a live handle and acquires one holder; release consumes one.
 * Both are mechanical NoAlloc helpers with no language failure or user code. */
void hirsel_rt_v2_receipt_retain(void *receipt);
void hirsel_rt_v2_receipt_release(void *receipt);
/* Affine sealed owners (ABI 71): read borrows an owner reserved by the
 * compiler-validated exclusive loan; release consumes its sole owner.
 * Both require the owner's live run. No language credit is duplicated. */
const void *hirsel_rt_v2_sealed_root(hirsel_rt_v2_ctx *ctx, const void *owner);
void hirsel_rt_v2_sealed_release(hirsel_rt_v2_ctx *ctx, void *owner);
/* Demand-only (ABI 75): guard storage is 80 bytes, aligned to 8, with the
 * unchanged region prefix at 0, private inventory at 64 and disarm word at 72.
 * Ordinary scopes retain their 64-byte record and do not call these helpers.
 * receipt_bits owns one metadata holder; seal borrows it. Initially only
 * actual-empty scopes with independently checked pointer-free roots are linked.
 * Seal returns OK or EXHAUSTED (output null, original custody unchanged);
 * invalid/incomplete source contracts trap. Source padding is initialized. */
void hirsel_rt_v2_receipt_region_enter(hirsel_rt_v2_ctx *ctx, void *guard);
uint8_t hirsel_rt_v2_receipt_region_exit(hirsel_rt_v2_ctx *ctx, void *guard,
                                        uint64_t anchor, uint64_t operation_site);
/* Construct original semantic metadata. kind 0 = inline, 1 = newly constructed
 * heap block, 2 = static byte literal. Inputs borrow ordered inline receipts.
 * The returned holder acquires metadata only; no source holder is acquired. */
/* Publish an original retained arena block with no outgoing owned holders.
 * C supplies the exact allocation correspondence. No source credit moves. */
void hirsel_rt_v2_receipt_arena_inventory(hirsel_rt_v2_ctx *ctx, void *guard,
    uint32_t index, const void *value);
void *hirsel_rt_v2_receipt_construct(hirsel_rt_v2_ctx *ctx, uint32_t index,
    const void *value, uint8_t kind, void **inputs, uint64_t input_count);
void *hirsel_rt_v2_receipt_bits(hirsel_rt_v2_ctx *ctx, uint32_t index, const void *value);
uint8_t hirsel_rt_v2_seal_prepare_commit(hirsel_rt_v2_ctx *ctx, void *guard, const void *root,
                                         uint32_t index, const void *receipt, uint64_t site, void **out_owner);
uint8_t hirsel_rt_v2_root_drop(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t anchor, uint64_t operation_site);

/* Finalizers. */
uint8_t hirsel_rt_v2_finalize(hirsel_rt_v2_ctx *ctx, uint32_t index, void *value,
                              const uint64_t *live_mask, hirsel_rt_v2_cleanup *continuation, uint64_t anchor, uint64_t operation_site);
void hirsel_rt_v2_finalize_begin(hirsel_rt_v2_ctx *ctx, uint64_t anchor);
void hirsel_rt_v2_finalize_end(hirsel_rt_v2_ctx *ctx, uint64_t anchor);
uint8_t hirsel_rt_v2_close_handle(hirsel_rt_v2_ctx *ctx, uint64_t handle, uint64_t anchor, uint64_t operation_site);
uint8_t hirsel_rt_v2_shielded(hirsel_rt_v2_ctx *ctx);

/* Regions. */
void hirsel_rt_v2_region_enter(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region);
uint8_t hirsel_rt_v2_region_reserve(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t extent,
                                    uint64_t align, void **out);
uint8_t hirsel_rt_v2_region_hold(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, hirsel_rt_v2_header *block);
uint8_t hirsel_rt_v2_region_schedule(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region,
                                     uint32_t index, void *value);
uint8_t hirsel_rt_v2_region_exit(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t anchor, uint64_t operation_site);
/* Arenas: an immortal region-kind block of layout `index` whose
 * payload the caller fills; the record is an arena's region. A LAYOUT_NO_EXTENT layout's block
 * is its eight-byte header and payload; every other block also has its extent before the header.
 * Every region reservation ends on a word, so the cursor is always eight-byte aligned. */
hirsel_rt_v2_header *hirsel_rt_v2_arena_alloc(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region,
                                              uint32_t index, uint64_t extent, uint64_t align, uint32_t flags);
/* Prepare one list.push to the List block `block`, of elements of `element_size` bytes at
 * `element_align` (MEM-6, ABI 11). A tracked arena block is
 * charged its element's logical size, copied within its arena when shared (charged its count),
 * and relocated there to max(4, 2 * count) elements when full; `out_block` then holds an unshared
 * block with room for one more element. Any other block is left alone and written back. Returns
 * STATUS_EXHAUSTED, with nothing changed or written, when a charge exceeds the arena's remaining
 * budget. */
uint8_t hirsel_rt_v2_arena_push(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t element_size,
                                uint64_t element_align, hirsel_rt_v2_header **out_block);
/* Prepare an Io read's `count` appended elements to the List block `block` at once (MEM-6,
 * ABI 13): as hirsel_rt_v2_arena_push, charged `count` elements, and relocated to
 * max(4, 2 * count of the block, its count plus `count`) elements when they do not fit. */
uint8_t hirsel_rt_v2_arena_reserve(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t count,
                                   uint64_t element_size, uint64_t element_align,
                                   hirsel_rt_v2_header **out_block);
/* Regrow the tracked arena @soa or sequential List block `block` (MEM-6, ABI 14): a fresh unshared tracked
 * block of its layout in its arena, of `extent` bytes at `align`, uncharged, its drop program
 * scheduled; the caller moves or copies the columns and empties an unshared `block`. */
hirsel_rt_v2_header *hirsel_rt_v2_arena_regrow(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block,
                                               uint64_t extent, uint64_t align);
/* Enter an arena's region, seeding its cursor and limit from an idle pooled segment if any. */
void hirsel_rt_v2_arena_enter(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region);
/* Bounded arenas (MEM-6). Bound an entered arena to `capacity` logical bytes. */
void hirsel_rt_v2_arena_bound(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t capacity);
/* Record the arena's own OutOfSpace failure (`kind`, `rule` at `site`); returns STATUS_FAILURE. */
uint8_t hirsel_rt_v2_arena_exhausted(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint32_t kind,
                                     uint32_t rule, uint64_t site, uint64_t operation_site);
/* 1 after forgetting the active failure when it is exactly the arena's own OutOfSpace, the one
 * failure recorded since its bound was set; else 0, the failure untouched. */
uint8_t hirsel_rt_v2_arena_recover(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region);
uint8_t hirsel_rt_v2_builder_begin(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_region *region, uint64_t capacity,
                                   uint64_t align, hirsel_rt_v2_builder *out);
uint8_t hirsel_rt_v2_builder_grow(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_builder *builder, uint64_t used,
                                  uint64_t min_capacity);
void hirsel_rt_v2_builder_publish(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_builder *builder, uint64_t used);

/* Failures. */
uint8_t hirsel_rt_v2_failure_begin(hirsel_rt_v2_ctx *ctx, uint32_t kind, uint32_t rule, uint64_t site, uint64_t operation_site);
uint8_t hirsel_rt_v2_failure_append(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_failure *failure);
uint8_t hirsel_rt_v2_failure_active(hirsel_rt_v2_ctx *ctx);
uint8_t hirsel_rt_v2_failure_primary(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_failure *out);
uint64_t hirsel_rt_v2_failure_cause_count(hirsel_rt_v2_ctx *ctx);
uint8_t hirsel_rt_v2_failure_cause(hirsel_rt_v2_ctx *ctx, uint64_t index, hirsel_rt_v2_failure *out);
void hirsel_rt_v2_failure_reanchor(hirsel_rt_v2_ctx *ctx, uint64_t site, uint64_t operation_site);
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

/* MEM-9: abandon the run with a located unchecked-contract diagnostic; no cleanup. */
void hirsel_rt_v2_unchecked_add_overflow(hirsel_rt_v2_ctx *ctx, uint64_t site);
void hirsel_rt_v2_invariant_fault(void);
/* Diagnostic probe: panics inside the runtime; the process aborts at the C boundary. */
void hirsel_rt_v2_diag_panic(void);

/* PROG-4: an extern call records its thread's run context for C callback thunks; a
   failing callback ends the process. */
/* previous is caller-owned storage of FOREIGN_SCOPE_BYTES at FOREIGN_SCOPE_ALIGN,
   live until the matching leave. dispatch is the callee package's threads (bits 0-1:
   0 home, 1 serial, 2 any), bit 2 for @blocking, and from bit 8 the package's ordinal,
   which names its serial gate (S-13). The call runs on the thread enter returns on:
   the enclosing interval's, home, a blocking-pool thread or the caller's; leave returns
   on the caller's worker. */
void hirsel_rt_v2_foreign_enter(hirsel_rt_v2_ctx *ctx, uint64_t site, void *previous,
                                uint64_t dispatch);
void hirsel_rt_v2_foreign_leave(const void *previous);
void hirsel_rt_v2_foreign_violation(uint64_t site);
hirsel_rt_v2_ctx *hirsel_rt_v2_callback_context(void);
void hirsel_rt_v2_callback_failed(hirsel_rt_v2_ctx *ctx);
/* A callback thunk returns to C: the compute permit its Hirsel code took at
   callback_context goes back. */
void hirsel_rt_v2_callback_leave(hirsel_rt_v2_ctx *ctx);

/* Private control of the typed CPU batch body. Start, bounded batch, drain. */
uint8_t hirsel_rt_v2_cpu_map_scoped(hirsel_rt_v2_ctx *ctx, uint32_t *out, const uint64_t *tasks, uint32_t width, uint32_t action, uint64_t count);

/* Additive spawn-ahead metadata (ABI 55). Physical entry/checkpoints only. */
typedef uint8_t (*hirsel_rt_v2_task_code)(hirsel_rt_v2_ctx *, hirsel_rt_v2_header *, uint64_t, uint64_t);
typedef struct hirsel_rt_v2_spawn_ahead_info {
    uint32_t version;
    uint32_t flags;
    hirsel_rt_v2_task_code entry;
} hirsel_rt_v2_spawn_ahead_info;
/* Version 2 extends the immutable v1 prefix; unsupported versions/flags are
 * refused before reading the entry or any pointed-to recipe. */
typedef struct hirsel_rt_v2_ahead_recipe {
    uint32_t wrapper_descriptor, source_descriptor;
    uint32_t wrapper_extent, callable_offset, live_word_offset, source_extent;
    uint64_t live_mask;
    const void *source_code;
} hirsel_rt_v2_ahead_recipe;
typedef struct hirsel_rt_v2_spawn_ahead_info_v2 {
    hirsel_rt_v2_spawn_ahead_info prefix;
    uint64_t stack_bytes;
    const hirsel_rt_v2_ahead_recipe *recipe;
} hirsel_rt_v2_spawn_ahead_info_v2;
typedef struct hirsel_rt_v2_ahead_physical {
    int64_t points_left;
    void *saved_sp, *idle_sp;
    uint32_t reason, reserved_zero;
} hirsel_rt_v2_ahead_physical;
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_spawn_ahead_info, 16);
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info, version) == 0, "hirsel_rt_v2_spawn_ahead_info.version offset");
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info, flags) == 4, "hirsel_rt_v2_spawn_ahead_info.flags offset");
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info, entry) == 8, "hirsel_rt_v2_spawn_ahead_info.entry offset");
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_ahead_recipe, 40);
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, wrapper_descriptor) == 0, "hirsel_rt_v2_ahead_recipe.wrapper_descriptor offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, source_descriptor) == 4, "hirsel_rt_v2_ahead_recipe.source_descriptor offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, wrapper_extent) == 8, "hirsel_rt_v2_ahead_recipe.wrapper_extent offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, callable_offset) == 12, "hirsel_rt_v2_ahead_recipe.callable_offset offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, live_word_offset) == 16, "hirsel_rt_v2_ahead_recipe.live_word_offset offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, source_extent) == 20, "hirsel_rt_v2_ahead_recipe.source_extent offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, live_mask) == 24, "hirsel_rt_v2_ahead_recipe.live_mask offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_recipe, source_code) == 32, "hirsel_rt_v2_ahead_recipe.source_code offset");
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_spawn_ahead_info_v2, 32);
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info_v2, prefix) == 0, "hirsel_rt_v2_spawn_ahead_info_v2.prefix offset");
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info_v2, stack_bytes) == 16, "hirsel_rt_v2_spawn_ahead_info_v2.stack_bytes offset");
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info_v2, recipe) == 24, "hirsel_rt_v2_spawn_ahead_info_v2.recipe offset");
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_ahead_physical, 32);
_Static_assert(offsetof(hirsel_rt_v2_ahead_physical, points_left) == 0, "hirsel_rt_v2_ahead_physical.points_left offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_physical, saved_sp) == 8, "hirsel_rt_v2_ahead_physical.saved_sp offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_physical, idle_sp) == 16, "hirsel_rt_v2_ahead_physical.idle_sp offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_physical, reason) == 24, "hirsel_rt_v2_ahead_physical.reason offset");
_Static_assert(offsetof(hirsel_rt_v2_ahead_physical, reserved_zero) == 28, "hirsel_rt_v2_ahead_physical.reserved_zero offset");
hirsel_rt_v2_ahead_physical *hirsel_rt_v2_ahead_state(hirsel_rt_v2_ctx *ctx);
void hirsel_rt_v2_ahead_handoff(hirsel_rt_v2_ahead_physical *state, uint32_t reason);
uint8_t hirsel_rt_v2_task_spawn_ahead(hirsel_rt_v2_ctx *ctx, uint64_t caller_site, uint64_t operation_site, void *destination, uint64_t *out, const uint64_t *scope, hirsel_rt_v2_task_code code, hirsel_rt_v2_header *environment, uint64_t flags, const hirsel_rt_v2_spawn_ahead_info *info);
void hirsel_rt_v2_ahead_barrier(hirsel_rt_v2_ctx *ctx);
/* Version 3 authorizes a selected-read continuation, never early spawn work. */
typedef struct hirsel_rt_v2_host_read_recipe {
    uint32_t buffer_descriptor, maximum, cleanup_slots, flags;
} hirsel_rt_v2_host_read_recipe;
typedef struct hirsel_rt_v2_host_root {
    uint32_t version, flags;
    uint64_t read_count;
    const hirsel_rt_v2_host_read_recipe *const *reads;
    uint8_t identity[32];
} hirsel_rt_v2_host_root;
typedef struct hirsel_rt_v2_spawn_ahead_info_v3 {
    hirsel_rt_v2_spawn_ahead_info_v2 prefix;
    const hirsel_rt_v2_host_root *host;
} hirsel_rt_v2_spawn_ahead_info_v3;
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_host_read_recipe, 16);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_host_root, 56);
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_spawn_ahead_info_v3, 40);
_Static_assert(offsetof(hirsel_rt_v2_spawn_ahead_info_v3, host) == 32, "host offset");
void hirsel_rt_v2_host_arm(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_host_read_recipe *recipe, hirsel_rt_v2_header **buffer);
void hirsel_rt_v2_host_enter(hirsel_rt_v2_ahead_physical *state, const hirsel_rt_v2_host_read_recipe *recipe);
typedef struct {
    hirsel_rt_v2_spawn_ahead_info prefix;
    uint64_t stack_bytes;
    const hirsel_rt_v2_ahead_recipe *recipe;
    uint32_t shared_descriptor, shared_payload_descriptor, jobs_descriptor, jobs_offset;
} hirsel_rt_v2_cpu_ahead_info;
_Static_assert(sizeof(hirsel_rt_v2_cpu_ahead_info) == 48, "hirsel_rt_v2_cpu_ahead_info size");
_Static_assert(offsetof(hirsel_rt_v2_cpu_ahead_info, shared_descriptor) == 32, "hirsel_rt_v2_cpu_ahead_info.shared_descriptor offset");
/* Metadata v6 owns one scalar job; its parent canonically preclaims the ordinal. */
typedef struct {
    hirsel_rt_v2_cpu_ahead_info prefix;
    uint32_t width_offset, ordinal_offset;
} hirsel_rt_v2_cpu_ahead_info_v6;
_Static_assert(sizeof(hirsel_rt_v2_cpu_ahead_info_v6) == 56, "hirsel_rt_v2_cpu_ahead_info_v6 size");
void hirsel_rt_v2_cpu_ahead_arm(hirsel_rt_v2_ctx *ctx);
void hirsel_rt_v2_cpu_ahead_enter(hirsel_rt_v2_ahead_physical *state);
/* CPU-only idle allocation; ends the private segment until job settlement is certified. */
hirsel_rt_v2_header *hirsel_rt_v2_cpu_ahead_alloc(hirsel_rt_v2_ahead_physical *state, uint32_t index, uint64_t extent, uint64_t align);
/* CPU storage and summary handoffs, record version 1. Until an allocating
 * certificate is admitted, return CPU_NOT_CERTIFIED without touching inputs.
 * A summary borrows its value and retained table from the saved mapper frame;
 * those references stay live until return. Its charges belong to the runtime,
 * not to this ABI record, and publication never increments counted holders. */
#define HIRSEL_RT_V2_CPU_HANDOFF_VERSION 1
#define HIRSEL_RT_V2_CPU_ACCEPTED 0
#define HIRSEL_RT_V2_CPU_NOT_CERTIFIED 1
#define HIRSEL_RT_V2_CPU_SUMMARY_PENDING 0
#define HIRSEL_RT_V2_CPU_SUMMARY_PUBLISHED 1
#define HIRSEL_RT_V2_CPU_SUMMARY_RETIRED 2
#define HIRSEL_RT_V2_CPU_SUMMARY_WITHDRAWN 3
#define HIRSEL_RT_V2_CPU_GROW_BLOCK 0
#define HIRSEL_RT_V2_CPU_GROW_SEQUENCE 1
#define HIRSEL_RT_V2_CPU_ALLOC_REGION 2
typedef struct hirsel_rt_v2_cpu_summary {
    uint32_t version, phase;
    uint64_t ordinal;
    const uint8_t *value;
    uint64_t value_bytes;
    uint32_t descriptor, retained_count;
    hirsel_rt_v2_header *const *retained;
} hirsel_rt_v2_cpu_summary;
typedef struct hirsel_rt_v2_cpu_growth {
    uint32_t version, kind;
    uint64_t ordinal;
    hirsel_rt_v2_header *block;
    hirsel_rt_v2_region *region;
    uint32_t index, flags;
    uint64_t extent, align, keep, element_size, element_align, capacity;
} hirsel_rt_v2_cpu_growth;
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_cpu_summary, 48);
_Static_assert(offsetof(hirsel_rt_v2_cpu_summary, retained) == 40, "cpu summary retained offset");
HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_cpu_growth, 88);
_Static_assert(offsetof(hirsel_rt_v2_cpu_growth, capacity) == 80, "cpu growth capacity offset");
/* Storage calls borrow one exclusively owned input block/region. On success,
 * growth replaces block; on refusal every field and input stays unchanged.
 * GROW_BLOCK uses keep/extent/align; GROW_SEQUENCE uses element_size,
 * element_align/capacity; ALLOC_REGION uses region/index/extent/align/flags.
 * Regions are ordinal-local, never shared between saved mappers. */
uint8_t hirsel_rt_v2_cpu_ahead_publish(hirsel_rt_v2_ahead_physical *state, hirsel_rt_v2_cpu_summary *summary);
uint8_t hirsel_rt_v2_cpu_ahead_grow(hirsel_rt_v2_ahead_physical *state, hirsel_rt_v2_cpu_growth *request);
uint8_t hirsel_rt_v2_host_active(hirsel_rt_v2_ahead_physical *state);
void hirsel_rt_v2_host_defer(hirsel_rt_v2_ahead_physical *state, uint32_t slot, hirsel_rt_v2_header *block, uint64_t fuel);
/* A resumable frame at a boundary: 0 proceeds with the boundary call, 1 suspends. */
uint8_t hirsel_rt_v2_resume_point(hirsel_rt_v2_ctx *ctx, uint8_t *frame, uint32_t state);
void hirsel_rt_v2_resume_tail(hirsel_rt_v2_ctx *ctx, uint8_t *frame, uint32_t state, uint32_t kind, uint32_t bytes, uint64_t anchor, uint64_t operation_site);
void hirsel_rt_v2_resume_cleanup(hirsel_rt_v2_ctx *ctx, uint8_t *frame, uint32_t state, uint32_t kind, uint32_t bytes, uint64_t anchor, uint64_t operation_site, const uint8_t *rows, uint32_t count);
/* A resumable task body: its dispatcher runs over an owned compact frame, saved at each suspension. */
uint8_t hirsel_rt_v2_resume_run(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *environment, uint64_t caller_site, uint64_t operation_site, void *dispatcher, uint32_t bytes);


/* A fuel point found the running task's chunk spent (its fuel word is negative): the runtime
   stores the next chunk, less the point the failed charge spent, in the word and returns, so
   the code continues; when the run's fuel is spent the run ends with BudgetExceeded (fuel), as
   a Blocked run ends, and the call does not return (runtime ABI 27). */
void hirsel_rt_v2_fuel_refill(hirsel_rt_v2_ctx *ctx);

uint8_t hirsel_rt_v2_ordered_join(hirsel_rt_v2_ctx *ctx, uint64_t caller_site, uint64_t operation_site, uint64_t *scope, uint64_t *child, uint8_t *slot, uint64_t size, uint32_t align);
uint8_t hirsel_rt_v2_ordered_sleep_until(hirsel_rt_v2_ctx *ctx, uint64_t caller_site, uint64_t operation_site, uint8_t *destination, uint8_t *out, uint64_t *clock, uint64_t *deadline);
uint8_t hirsel_rt_v2_ordered_stdin_read(hirsel_rt_v2_ctx *ctx, uint64_t caller_site, uint64_t operation_site, uint8_t *destination, uint8_t *out, uint64_t *stdin, hirsel_rt_v2_header **buffer, uint64_t max);
uint8_t hirsel_rt_v2_ordered_watcher_next(hirsel_rt_v2_ctx *ctx, uint64_t caller_site, uint64_t operation_site, uint8_t *destination, uint8_t *out, uint64_t *directory, uint64_t *watcher);
uint8_t hirsel_rt_v2_ordered_print(hirsel_rt_v2_ctx *ctx, uint64_t caller_site, uint64_t operation_site, uint8_t *destination, uint8_t *out, uint64_t *stdout, hirsel_rt_v2_header *owner, uint64_t start, uint64_t len);

void *hirsel_rt_v2_receipt_arena_construct(hirsel_rt_v2_ctx *ctx, void *guard, uint32_t index, const void *value, void **inputs, uint64_t input_count);
void hirsel_rt_v2_receipt_arena_inventory_paired(hirsel_rt_v2_ctx *ctx, void *guard, uint32_t index, const void *value, const void *receipt);

#ifdef __cplusplus
}
#endif

#endif /* HIRSEL_RT_V2_H */

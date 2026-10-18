/*
 * Standalone C harness for hirsel_rt_v2: drives the runtime only through its C interface.
 * Built with AddressSanitizer and UndefinedBehaviorSanitizer by run.sh (LeakSanitizer then
 * proves task exit returned every backing allocation), and cross-built for AArch64.
 */
#include "hirsel_rt_v2.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int failures;

#define CHECK(condition)                                                                           \
    do {                                                                                           \
        if (!(condition)) {                                                                        \
            fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #condition);          \
            failures++;                                                                            \
        }                                                                                          \
    } while (0)

static uint64_t closed[8];
static int closed_count;

/* The Io adapter's handle close: logs the handle id. */
static uint8_t close_handle(hirsel_rt_v2_ctx *ctx, uint64_t handle, uint64_t anchor) {
    (void)ctx;
    (void)anchor;
    if (closed_count < 8) closed[closed_count++] = handle;
    return HIRSEL_RT_V2_STATUS_OK;
}

static hirsel_rt_v2_stats stats(hirsel_rt_v2_ctx *ctx) {
    hirsel_rt_v2_stats out;
    hirsel_rt_v2_ctx_stats(ctx, &out);
    return out;
}

/* Finalizer events: value ids in call order. */
static uint64_t events[64];
static int event_count;

static uint8_t log_drop(hirsel_rt_v2_ctx *ctx, void *value, uint64_t anchor) {
    (void)ctx;
    (void)anchor;
    uint64_t id;
    memcpy(&id, value, sizeof id);
    if (event_count < 64) events[event_count++] = id;
    return HIRSEL_RT_V2_STATUS_OK;
}

static uint8_t failing_drop(hirsel_rt_v2_ctx *ctx, void *value, uint64_t anchor) {
    log_drop(ctx, value, anchor);
    uint64_t id;
    memcpy(&id, value, sizeof id);
    return hirsel_rt_v2_failure_begin(ctx, 1, (uint32_t)id, anchor, anchor);
}

#define STEP(kind_, field_, offset_)                                                               \
    {.kind = (kind_), .field = (field_), .offset = (offset_), .count_offset = HIRSEL_RT_V2_NO_OFFSET}
#define LAYOUT(size_, finalizer_, steps_, step_count_, field_count_)                               \
    {HIRSEL_RT_V2_LAYOUT_MAGIC, HIRSEL_RT_V2_LAYOUT_VERSION, (size_), 8, (finalizer_), (steps_), (step_count_),   \
     (field_count_), 0, 0}

static const hirsel_rt_v2_layout empty = LAYOUT(0, NULL, NULL, 0, 0);
static const hirsel_rt_v2_layout logged_leaf = LAYOUT(8, log_drop, NULL, 0, 1);
static const hirsel_rt_v2_layout failing_leaf = LAYOUT(8, failing_drop, NULL, 0, 1);
/* { id, a: ref, b: ref }: b then a. */
static const hirsel_rt_v2_step pair_steps[] = {STEP(HIRSEL_RT_V2_STEP_RELEASE, 2, 16), STEP(HIRSEL_RT_V2_STEP_RELEASE, 1, 8)};
static const hirsel_rt_v2_layout failing_pair = LAYOUT(24, failing_drop, pair_steps, 2, 3);
/* { tag: u32, next: ref } owning next when tag is 1. */
static const hirsel_rt_v2_step cell_steps[] = {
    {.kind = HIRSEL_RT_V2_STEP_TAG, .field = HIRSEL_RT_V2_WHOLE_VALUE, .variant = 1, .span = 1, .offset = 0,
     .count_offset = HIRSEL_RT_V2_NO_OFFSET},
    STEP(HIRSEL_RT_V2_STEP_RELEASE, 0, 8)};
static const hirsel_rt_v2_layout cell = LAYOUT(16, NULL, cell_steps, 2, 1);
/* The null-pointer niche of { Nil, Cons(next: ref) }: one word, owning next unless it is null. */
static const hirsel_rt_v2_step niche_steps[] = {
    {.kind = HIRSEL_RT_V2_STEP_NONNULL, .field = HIRSEL_RT_V2_WHOLE_VALUE, .span = 1, .offset = 0,
     .count_offset = HIRSEL_RT_V2_NO_OFFSET},
    STEP(HIRSEL_RT_V2_STEP_RELEASE, 0, 0)};
static const hirsel_rt_v2_layout niche_cell = LAYOUT(8, NULL, niche_steps, 2, 1);
/* A sequence payload { count: u64, elements: [{ handle: u64, item: ref }] }: per element, highest index first,
 * the item and then the handle. */
static const hirsel_rt_v2_step list_steps[] = {
    {.kind = HIRSEL_RT_V2_STEP_LOOP, .field = HIRSEL_RT_V2_WHOLE_VALUE, .span = 2, .offset = 8, .stride = 16,
     .count_offset = 0},
    STEP(HIRSEL_RT_V2_STEP_RELEASE, HIRSEL_RT_V2_WHOLE_VALUE, 8),
    STEP(HIRSEL_RT_V2_STEP_CLOSE_HANDLE, HIRSEL_RT_V2_WHOLE_VALUE, 0)};
static const hirsel_rt_v2_layout list = {HIRSEL_RT_V2_LAYOUT_MAGIC, HIRSEL_RT_V2_LAYOUT_VERSION, 8, 8, NULL, list_steps, 3, 0,
                                         HIRSEL_RT_V2_LAYOUT_DYNAMIC | HIRSEL_RT_V2_LAYOUT_WIDE_CURSOR, 0};

static const hirsel_rt_v2_layout ffi = {HIRSEL_RT_V2_LAYOUT_MAGIC, HIRSEL_RT_V2_LAYOUT_VERSION, 8, 8, failing_drop, NULL, 0,
                                            1, HIRSEL_RT_V2_LAYOUT_UNSAFE_FFI, 0};
static const hirsel_rt_v2_layout *const registry[] = {&empty, &logged_leaf, &failing_leaf, &failing_pair, &cell, &niche_cell, &list, &ffi};
static uint32_t descriptor_index(const hirsel_rt_v2_layout *layout) {
    for(uint32_t i=0; i<sizeof registry/sizeof *registry; i++) if(registry[i]==layout) return i+2;
    abort();
}
static hirsel_rt_v2_ctx *create(uint32_t flags, uint64_t count_max) {
    hirsel_rt_v2_config config = {.abi_version=HIRSEL_RT_V2_ABI_VERSION, .flags=flags, .count_max=count_max, .close_handle=close_handle, .layouts=registry, .layout_count=sizeof registry/sizeof *registry};
    hirsel_rt_v2_ctx *ctx = NULL;
    CHECK(hirsel_rt_v2_ctx_create(&config, &ctx) == HIRSEL_RT_V2_LAUNCH_OK);
    return ctx;
}

static hirsel_rt_v2_header *alloc(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_layout *layout, uint64_t extent, uint64_t align) {
    return hirsel_rt_v2_alloc(ctx, descriptor_index(layout), extent, align, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
}

static hirsel_rt_v2_header *node(hirsel_rt_v2_ctx *ctx, const hirsel_rt_v2_layout *layout, uint64_t id) {
    hirsel_rt_v2_header *block = alloc(ctx, layout, layout->size, 8);
    memcpy(hirsel_rt_v2_payload(block), &id, sizeof id);
    return block;
}

static void set_ref(hirsel_rt_v2_header *block, size_t offset, hirsel_rt_v2_header *child) {
    memcpy((char *)hirsel_rt_v2_payload(block) + offset, &child, sizeof child);
}

static uint64_t capacity(const hirsel_rt_v2_header *block) { return hirsel_rt_v2_capacity((hirsel_rt_v2_header *)block); }

static void test_classes_and_alignment(void) {
    hirsel_rt_v2_ctx *ctx = create(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, 0);
    static const uint32_t classes[] = HIRSEL_RT_V2_CLASS_SIZES;
    for (int index = 0; index < HIRSEL_RT_V2_CLASS_COUNT; index++) {
        hirsel_rt_v2_header *exact = alloc(ctx, &empty, classes[index], 8);
        hirsel_rt_v2_header *after = alloc(ctx, &empty, classes[index] + 1, 8);
        CHECK(capacity(exact) == classes[index]);
        CHECK((exact->descriptor & (3u<<29)) == 0);
        if (index + 1 < HIRSEL_RT_V2_CLASS_COUNT) {
            CHECK(capacity(after) == classes[index + 1]);
        } else {
            CHECK((after->descriptor & (3u<<29)) == (1u<<29));
        }
        memset(hirsel_rt_v2_payload(exact), 1, capacity(exact));
        memset(hirsel_rt_v2_payload(after), 1, capacity(after));
        CHECK(hirsel_rt_v2_release(ctx, exact, descriptor_index(&empty), NULL, 0) == 0);
        CHECK(hirsel_rt_v2_release(ctx, after, descriptor_index(&empty), NULL, 0) == 0);
    }
    for (uint64_t align = 1; align <= 8192; align *= 2) {
        hirsel_rt_v2_header *block = alloc(ctx, &empty, 100, align);
        CHECK(((uintptr_t)hirsel_rt_v2_payload(block) % align) == 0);
        CHECK(((block->descriptor & (3u<<29)) == (1u<<29)) == (align > HIRSEL_RT_V2_SMALL_ALIGN_MAX));
        memset(hirsel_rt_v2_payload(block), 2, 100);
        hirsel_rt_v2_release(ctx, block, descriptor_index(&empty), NULL, 0);
    }
    hirsel_rt_v2_ctx_destroy(ctx);
}

static uintptr_t page_of(const hirsel_rt_v2_header *block) {
    return (uintptr_t)block & ~(uintptr_t)(HIRSEL_RT_V2_SLAB_SIZE - 1);
}

/* A 40-byte block whose payload starts with `key`; a block handed out twice while live loses a key. */
static hirsel_rt_v2_header *keyed(hirsel_rt_v2_ctx *ctx, uint64_t key) {
    hirsel_rt_v2_header *block = alloc(ctx, &empty, 40, 8);
    memcpy(hirsel_rt_v2_payload(block), &key, sizeof key);
    return block;
}

static uint64_t key_of(hirsel_rt_v2_header *block) {
    uint64_t key;
    memcpy(&key, hirsel_rt_v2_payload(block), sizeof key);
    return key;
}

/* mimalloc page reuse (alloc.rs): a free pushes its page's `local_free` list. Allocation
 * extends sequentially through the page's untouched capacity, then collects `local_free` and hands those blocks
 * back last freed first, all before taking a new page. Keys checked before every free follow mimalloc's
 * test-stress.c. */
static void test_page_reuse(void) {
    enum { CAP = HIRSEL_RT_V2_SLAB_SIZE / 16 };
    static hirsel_rt_v2_header *live[CAP + 1];
    hirsel_rt_v2_ctx *ctx = create(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, 0);
    hirsel_rt_v2_header *a = alloc(ctx, &empty, 40, 8);
    hirsel_rt_v2_header *b = alloc(ctx, &empty, 40, 8);
    const uint64_t page_bytes = stats(ctx).backing_bytes;
    const uintptr_t page = page_of(a), stride = (uintptr_t)b - (uintptr_t)a;
    hirsel_rt_v2_release(ctx, a, descriptor_index(&empty), NULL, 0);
    hirsel_rt_v2_release(ctx, b, descriptor_index(&empty), NULL, 0);
    int count = 0;
    hirsel_rt_v2_header *block;
    do {
        block = keyed(ctx, count);
        live[count++] = block;
    } while (block != a && block != b && page_of(block) == page && count < CAP);
    CHECK(block == b);
    CHECK(count > 1);
    int sequential = 0;
    for (int index = 0; index + 1 < count; index++) {
        sequential += (uintptr_t)live[index] == (uintptr_t)b + (uintptr_t)(index + 1) * stride;
    }
    CHECK(sequential == count - 1);
    live[count] = keyed(ctx, count);
    CHECK(live[count++] == a);
    CHECK(stats(ctx).backing_bytes == page_bytes);
    int intact = 0;
    for (int index = 0; index < count; index++) {
        intact += key_of(live[index]) == (uint64_t)index;
        hirsel_rt_v2_release(ctx, live[index], descriptor_index(&empty), NULL, 0);
    }
    CHECK(intact == count);
    CHECK(stats(ctx).live_blocks == 0);
    /* A full page freed and allocated again is served from its collected list: no new backing, no double hand-out. */
    for (int index = 0; index < count; index++) live[index] = keyed(ctx, index);
    CHECK(stats(ctx).backing_bytes == page_bytes);
    intact = 0;
    for (int index = 0; index < count; index++) {
        intact += page_of(live[index]) == page && key_of(live[index]) == (uint64_t)index;
        hirsel_rt_v2_release(ctx, live[index], descriptor_index(&empty), NULL, 0);
    }
    CHECK(intact == count);
    CHECK(stats(ctx).live_blocks == 0);
    hirsel_rt_v2_audit audit;
    hirsel_rt_v2_task_exit(ctx, &audit);
    CHECK(audit.retained_bytes == 0 && stats(ctx).backing_bytes == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
}

static void test_counts_and_chain(void) {
    hirsel_rt_v2_ctx *ctx = create(0, 3);
    hirsel_rt_v2_header *block = alloc(ctx, &empty, 8, 8);
    for (int dup = 0; dup < 10; dup++) hirsel_rt_v2_dup(ctx, block);
    CHECK(block->count == HIRSEL_RT_V2_COUNT_IMMORTAL);
    for (int release = 0; release < 20; release++) hirsel_rt_v2_release(ctx, block, descriptor_index(&empty), NULL, 0);
    CHECK(block->count == HIRSEL_RT_V2_COUNT_IMMORTAL);
    CHECK(block->count != 1);
    CHECK(stats(ctx).immortalized_blocks == 1);
    hirsel_rt_v2_ctx_destroy(ctx);

    ctx = create(0, 0);
    hirsel_rt_v2_header *head = alloc(ctx, &cell, 16, 8);
    memset(hirsel_rt_v2_payload(head), 0, 16);
    for (int index = 1; index < 200000; index++) {
        hirsel_rt_v2_header *cons = alloc(ctx, &cell, 16, 8);
        uint32_t tag = 1;
        memcpy(hirsel_rt_v2_payload(cons), &tag, sizeof tag);
        set_ref(cons, 8, head);
        head = cons;
    }
    CHECK(hirsel_rt_v2_release(ctx, head, descriptor_index(&cell), NULL, 0) == 0);
    CHECK(stats(ctx).live_blocks == 0);

    head = alloc(ctx, &niche_cell, 8, 8);
    memset(hirsel_rt_v2_payload(head), 0, 8);
    for (int index = 1; index < 200000; index++) {
        hirsel_rt_v2_header *cons = alloc(ctx, &niche_cell, 8, 8);
        set_ref(cons, 0, head);
        head = cons;
    }
    CHECK(hirsel_rt_v2_release(ctx, head, descriptor_index(&niche_cell), NULL, 0) == 0);
    CHECK(stats(ctx).live_blocks == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
}

static void test_sequence_loop(void) {
    hirsel_rt_v2_ctx *ctx = create(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, 0);
    const uint64_t length = 3;
    hirsel_rt_v2_header *block = alloc(ctx, &list, 8 + length * 16, 8);
    char *payload = hirsel_rt_v2_payload(block);
    memcpy(payload, &length, sizeof length);
    for (uint64_t index = 0; index < length; index++) {
        uint64_t handle = 100 + index;
        memcpy(payload + 8 + index * 16, &handle, sizeof handle);
        set_ref(block, 8 + index * 16 + 8, node(ctx, &logged_leaf, 10 + index));
    }
    event_count = 0;
    closed_count = 0;
    CHECK(hirsel_rt_v2_release(ctx, block, descriptor_index(&list), NULL, 0) == HIRSEL_RT_V2_STATUS_OK);
    CHECK(event_count == 3 && events[0] == 12 && events[1] == 11 && events[2] == 10);
    CHECK(closed_count == 3 && closed[0] == 102 && closed[1] == 101 && closed[2] == 100);
    CHECK(stats(ctx).live_blocks == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
}

static void test_finalizer_failures(void) {
    hirsel_rt_v2_ctx *ctx = create(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, 0);
    hirsel_rt_v2_header *root = node(ctx, &failing_pair, 1);
    set_ref(root, 8, node(ctx, &failing_leaf, 2));
    set_ref(root, 16, node(ctx, &logged_leaf, 3));
    event_count = 0;
    CHECK(hirsel_rt_v2_release(ctx, root, descriptor_index(&failing_pair), NULL, 9) == HIRSEL_RT_V2_STATUS_FAILURE);
    CHECK(event_count == 3 && events[0] == 1 && events[1] == 3 && events[2] == 2);
    hirsel_rt_v2_failure primary;
    CHECK(hirsel_rt_v2_failure_primary(ctx, &primary) == 1 && primary.rule == 1 && primary.site == 9);
    hirsel_rt_v2_failure cause;
    CHECK(hirsel_rt_v2_failure_cause_count(ctx) == 1);
    CHECK(hirsel_rt_v2_failure_cause(ctx, 0, &cause) == 1 && cause.rule == 2);
    CHECK(stats(ctx).live_blocks == 0);

    /* An inline frame resource through compiler-reserved continuation storage. */
    uint64_t value[3] = {4, 0, 0};
    hirsel_rt_v2_header *a = node(ctx, &logged_leaf, 5);
    hirsel_rt_v2_header *b = node(ctx, &logged_leaf, 6);
    memcpy(&value[1], &a, sizeof a);
    memcpy(&value[2], &b, sizeof b);
    hirsel_rt_v2_cleanup continuation;
    event_count = 0;
    CHECK(hirsel_rt_v2_finalize(ctx, descriptor_index(&failing_pair), value, NULL, &continuation, 0) == HIRSEL_RT_V2_STATUS_FAILURE);
    CHECK(event_count == 3 && events[0] == 4 && events[1] == 6 && events[2] == 5);
    CHECK(hirsel_rt_v2_failure_primary(ctx, &primary) == 1 && primary.rule == 1);
    CHECK(hirsel_rt_v2_failure_cause_count(ctx) == 2);
    hirsel_rt_v2_failure_clear(ctx);
    CHECK(!hirsel_rt_v2_failure_active(ctx));
    hirsel_rt_v2_ctx_destroy(ctx);
}

static void test_regions(void) {
    hirsel_rt_v2_ctx *ctx = create(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, 0);
    hirsel_rt_v2_region region;
    hirsel_rt_v2_region_enter(ctx, &region);
    for (int index = 0; index < 400; index++) {
        void *storage = NULL;
        CHECK(hirsel_rt_v2_region_reserve(ctx, &region, 60000, 16, &storage) == 0);
        memset(storage, 3, 60000);
    }
    hirsel_rt_v2_builder builder;
    CHECK(hirsel_rt_v2_builder_begin(ctx, &region, 8, 8, &builder) == 0);
    memset(builder.data, 4, 8);
    CHECK(hirsel_rt_v2_builder_grow(ctx, &builder, 8, 200000) == 0);
    CHECK(((unsigned char *)builder.data)[7] == 4);
    hirsel_rt_v2_builder_publish(ctx, &builder, 8);
    for (uint64_t id = 1; id <= 4; id++) {
        hirsel_rt_v2_region_hold(ctx, &region, node(ctx, id % 2 ? &logged_leaf : &failing_leaf, id));
    }
    event_count = 0;
    CHECK(hirsel_rt_v2_region_exit(ctx, &region, 0) == HIRSEL_RT_V2_STATUS_FAILURE);
    CHECK(event_count == 4 && events[0] == 4 && events[3] == 1);
    hirsel_rt_v2_stats after = stats(ctx);
    CHECK(after.live_blocks == 0);
    CHECK(after.region_active_bytes == 0);
    CHECK(after.region_pool_bytes <= HIRSEL_RT_V2_REGION_POOL_IDLE_CAP);
    hirsel_rt_v2_failure_clear(ctx);

    hirsel_rt_v2_audit audit;
    hirsel_rt_v2_task_exit(ctx, &audit);
    CHECK(audit.retained_bytes == 0 && stats(ctx).backing_bytes == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
}

/* Run `probe` in a child and require SIGABRT. */
static void expect_abort(const char *name, void (*probe)(void)) {
    fflush(NULL);
    pid_t child = fork();
    if (child == 0) {
        /* Keep the child's expected abort report out of the harness output. */
        if (!freopen("/dev/null", "w", stderr)) _exit(2);
        probe();
        _exit(0);
    }
    int status = 0;
    waitpid(child, &status, 0);
    if (!(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT)) {
        fprintf(stderr, "%s: expected an abort, got status %d\n", name, status);
        failures++;
    }
}

static void panic_probe(void) { hirsel_rt_v2_diag_panic(); }

static void mismatch_probe(void) {
    hirsel_rt_v2_ctx *ctx = create(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, 0);
    hirsel_rt_v2_header *block = alloc(ctx, &logged_leaf, 8, 8);
    hirsel_rt_v2_release(ctx, block, descriptor_index(&empty), NULL, 0);
}

static void unsafe_ffi_probe(void) {
    hirsel_rt_v2_ctx *ctx = create(0, 0);
    hirsel_rt_v2_release(ctx, node(ctx, &ffi, 1), descriptor_index(&ffi), NULL, 0);
}

int main(void) {
    CHECK(hirsel_rt_v2_abi_version() == HIRSEL_RT_V2_ABI_VERSION);
    hirsel_rt_v2_config wrong = {.abi_version=HIRSEL_RT_V2_ABI_VERSION+1};
    hirsel_rt_v2_ctx *ctx = NULL;
    CHECK(hirsel_rt_v2_ctx_create(&wrong, &ctx) == HIRSEL_RT_V2_LAUNCH_ABI_MISMATCH && ctx == NULL);
    test_classes_and_alignment();
    test_page_reuse();
    test_counts_and_chain();
    test_sequence_loop();
    test_finalizer_failures();
    test_regions();
    expect_abort("panic probe", panic_probe);
    expect_abort("header/layout mismatch", mismatch_probe);
    expect_abort("unsafe ffi finalizer failure", unsafe_ffi_probe);
    if (failures) {
        fprintf(stderr, "harness: %d checks failed\n", failures);
        return 1;
    }
    printf("harness: passed\n");
    return 0;
}

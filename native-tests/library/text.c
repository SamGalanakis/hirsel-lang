#include "../../stdlib/native/library.h"
#include <assert.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

_Static_assert(sizeof(hirsel_rt_v2_slice) == 24, "slice size");
_Static_assert(sizeof(hirsel_rt_v2_string_result) == 32, "result size");
_Static_assert(offsetof(hirsel_rt_v2_string_result, value) == 8, "result payload");
static hirsel_rt_v2_header *block(hirsel_rt_v2_ctx *ctx, const char *bytes, uint64_t length) {
    hirsel_rt_v2_header *owner = hirsel_rt_v2_alloc(ctx, 0, 8 + length, 8,
        HIRSEL_RT_V2_DOMAIN_HEAP, 0);
    unsigned char *payload = (unsigned char*)owner + HIRSEL_RT_V2_HEADER_SIZE;
    memcpy(payload, &length, 8); memcpy(payload + 8, bytes, length);
    return owner;
}
static void release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *owner) {
    assert(hirsel_rt_v2_release(ctx, owner, owner->descriptor & 0x1fffffffu, NULL, 0) == 0);
}
static void shown(hirsel_rt_v2_slice value, const char *bytes, uint64_t length) {
    assert(value.length == length);
    assert(memcmp((unsigned char*)value.owner + HIRSEL_RT_V2_HEADER_SIZE + 8 + value.start, bytes, length) == 0);
}
static void mode(uint32_t flags) {
    hirsel_rt_v2_config config = { .abi_version = HIRSEL_RT_V2_ABI_VERSION, .flags = flags };
    hirsel_rt_v2_ctx *ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    for (int round = 0; round < 1000; round++) {
        hirsel_rt_v2_header *source = block(ctx, "prefix\xc3\xa9\xf0\x9f\x98\x80xTAIL", 17);
        uint64_t from = 2, to = 6;
        hirsel_rt_v2_slice slice;
        assert(hirsel_rt_v2_string_slice(ctx, NULL, &slice, source, 6, 7, &from, &to) == 0);
        shown(slice, "\xf0\x9f\x98\x80", 4);
        assert(slice.owner == source && slice.start == 8 && source->count == 2);
        release(ctx, slice.owner);
        slice = (hirsel_rt_v2_slice){ NULL, 123, 456 };
        assert(hirsel_rt_v2_string_slice(ctx, NULL, &slice, source, 6, 7, &to, &from) == 1);
        assert(slice.owner == NULL && slice.start == 123 && slice.length == 456 && source->count == 1);
        hirsel_rt_v2_string_result result;
        assert(hirsel_rt_v2_string_from_bytes(ctx, NULL, &result, source, 6, 7) == 0);
        assert(result.tag == 0 && result.value.owner == source && result.value.start == 6 && source->count == 1);
        release(ctx, result.value.owner);
        source = block(ctx, "prefix\xffTAIL", 11);
        assert(hirsel_rt_v2_string_from_bytes(ctx, NULL, &result, source, 6, 1) == 0);
        assert(result.tag == 1 && result.value.owner == source && result.value.start == 6 && result.value.length == 1 && source->count == 1);
        release(ctx, result.value.owner);
        hirsel_rt_v2_stats stats;
        hirsel_rt_v2_ctx_stats(ctx, &stats); assert(stats.live_blocks == 0);
    }
    hirsel_rt_v2_audit audit; hirsel_rt_v2_task_exit(ctx, &audit);
    assert(audit.live_blocks == 0 && audit.retained_bytes == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
}
int main(void) {
    mode(0); mode(HIRSEL_RT_V2_CONFIG_DIAGNOSTIC);
    puts("text ABI: both modes, 1000 rounds, zero live blocks");
}

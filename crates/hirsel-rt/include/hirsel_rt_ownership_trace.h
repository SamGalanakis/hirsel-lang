/* Optional diagnostic ABI extension. Exported only by ownership-trace archives.
 * Base context/layout ABI and ordinary native objects remain unchanged. */
#ifndef HIRSEL_RT_OWNERSHIP_TRACE_H
#define HIRSEL_RT_OWNERSHIP_TRACE_H
#include "hirsel_rt_v2.h"
#define HIRSEL_RT_V2_OWNERSHIP_TRACE_ABI_VERSION 102
#ifdef __cplusplus
extern "C" {
#endif
/* unit points to the checked LIR unit's 32-byte digest. The returned frame id
 * is diagnostic shadow state and must never enter a language computation. */
uint64_t hirsel_rt_v2_ownership_enter(hirsel_rt_v2_ctx *ctx, uint32_t abi,
                                    const uint8_t *unit);
/* token/owner encode a whole-frame location: high u32 = 0 (SSA value) or 1
 * (local slot), low u32 = index. They are locations, never resource identities. */
void hirsel_rt_v2_ownership_event(hirsel_rt_v2_ctx *ctx, uint64_t frame,
                                uint32_t marker, uint32_t action,
                                uint64_t token, uint64_t owner,
                                uint64_t site, uint32_t layout);
/* Internal alias relation. This neither moves nor creates a resource. */
void hirsel_rt_v2_ownership_alias(hirsel_rt_v2_ctx *ctx, uint64_t frame,
                                uint32_t marker, uint64_t view, uint64_t owner,
                                uint64_t site, uint32_t layout);
/* Stack-borrowed synchronous cleanup scope; six u64 words, aligned to 8.
 * bindings holds count five-word rows: offset, descriptor index, binding ordinal,
 * local layout, declaration field. */
void hirsel_rt_v2_ownership_cleanup_begin(hirsel_rt_v2_ctx *ctx, void *scope,
    uint64_t frame, uint32_t marker, void *environment, const uint64_t *bindings,
    uint64_t count);
void hirsel_rt_v2_ownership_cleanup_end(hirsel_rt_v2_ctx *ctx, void *scope);
void hirsel_rt_v2_ownership_leave(hirsel_rt_v2_ctx *ctx, uint64_t frame);
#ifdef __cplusplus
}
#endif
#endif

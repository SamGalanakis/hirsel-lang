/* Instrumentation hooks exported only by measurement archives. */
#ifndef HIRSEL_RT_MEASUREMENT_H
#define HIRSEL_RT_MEASUREMENT_H

#include "hirsel_rt_v2.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Strings are NUL-terminated UTF-8 and remain immutable for the run. */
typedef struct hirsel_rt_v2_profile_site {
    uint64_t id;
    const char *package;
    const char *file;
    const char *function;
    const char *operation;
    uint32_t line;
    uint32_t column;
} hirsel_rt_v2_profile_site;

uint64_t hirsel_rt_v2_profile_enter(hirsel_rt_v2_ctx *ctx,
                                  const hirsel_rt_v2_profile_site *site);
void hirsel_rt_v2_profile_leave(hirsel_rt_v2_ctx *ctx, uint64_t previous);

void hirsel_rt_v2_measure_release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block);

#ifdef __cplusplus
}
#endif

#endif

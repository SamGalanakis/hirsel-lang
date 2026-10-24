/* A diagnostic embedding for the native leaf hook contract. Calls only write
 * fixed rows; stdout is read after both language executions have returned. */
#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "hirsel_rt_v2.h"
#include "hirsel_rt_ownership_trace.h"
#include "entries.h"

struct row {
  uint64_t frame, token, owner, site;
  uint32_t tag, marker, action, layout;
  uint8_t unit[32];
};
static struct row rows[32];
static uint32_t count;
static uint64_t next_frame;
static hirsel_rt_v2_ctx *expected_context;

uint64_t hirsel_rt_v2_ownership_enter(hirsel_rt_v2_ctx *ctx, uint32_t abi, const uint8_t *unit) {
  assert(ctx == expected_context && count < 32 && abi == HIRSEL_RT_V2_OWNERSHIP_TRACE_ABI_VERSION);
  struct row *row = &rows[count++];
  row->tag = 0;
  row->frame = ++next_frame;
  memcpy(row->unit, unit, 32);
  return row->frame;
}
void hirsel_rt_v2_ownership_event(hirsel_rt_v2_ctx *ctx, uint64_t frame,
    uint32_t marker, uint32_t action, uint64_t token, uint64_t owner,
    uint64_t site, uint32_t layout) {
  assert(ctx == expected_context && count < 32);
  rows[count++] = (struct row){.tag=1, .frame=frame, .marker=marker,
    .action=action, .token=token, .owner=owner, .site=site, .layout=layout};
}
void hirsel_rt_v2_ownership_leave(hirsel_rt_v2_ctx *ctx, uint64_t frame) {
  assert(ctx == expected_context && count < 32);
  rows[count++] = (struct row){.tag=2, .frame=frame};
}
int main(void) {
  hirsel_rt_v2_config config = {0};
  config.abi_version = HIRSEL_RT_V2_ABI_VERSION;
  config.flags = HIRSEL_RT_V2_CONFIG_DIAGNOSTIC;
  assert(hirsel_rt_v2_ctx_create(&config, &expected_context) == 0);
  for (uint32_t i=0; i<2; ++i) {
    int64_t value = 0;
    assert(call_leaf(expected_context, &value) == 0 && value == 7);
  }
  hirsel_rt_v2_ctx_destroy(expected_context);
  assert(next_frame == 2 && count == 16);
  for (uint32_t i=0; i<count; ++i) {
    const struct row *row = &rows[i];
    if (row->tag == 0) {
      printf("enter %" PRIu64 " ", row->frame);
      for (uint32_t b=0; b<32; ++b) printf("%02x", row->unit[b]);
      printf("\n");
    } else if (row->tag == 2) {
      printf("leave %" PRIu64 "\n", row->frame);
    } else {
      printf("event %" PRIu64 " %" PRIu32 " %" PRIu32 " %" PRIu64
        " %" PRIu64 " %" PRIu64 " %" PRIu32 "\n", row->frame,
        row->marker, row->action, row->token, row->owner, row->site, row->layout);
    }
  }
}

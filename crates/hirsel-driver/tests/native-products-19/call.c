/* Uses the same generated C entry bridge and actual runtime registry as the
 * existing rc/maps/seed.c native probes. All code and static data come from
 * the source demand products; the host only calls and observes the entry. */
#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdio.h>
@CALL@
struct program_binding { const hirsel_rt_v2_layout *const *layouts; uint64_t count; };
extern const struct program_binding program __asm__("hirsel.program.registry");
int main(void) {
  hirsel_rt_v2_config config = {
    .abi_version = HIRSEL_RT_V2_ABI_VERSION,
    .flags = HIRSEL_RT_V2_CONFIG_DIAGNOSTIC,
    .layouts = program.layouts,
    .layout_count = (uint32_t)program.count
  };
  hirsel_rt_v2_ctx *ctx = NULL;
  assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
  uint64_t value = 0;
  uint8_t status = call_main(ctx, &value);
  assert(status == 0);
  printf("%llu\n", (unsigned long long)value);
  hirsel_rt_v2_audit audit;
  hirsel_rt_v2_task_exit(ctx, &audit);
  assert(audit.live_blocks == 0 && audit.retained_bytes == 0 && audit.active_regions == 0);
  hirsel_rt_v2_ctx_destroy(ctx);
  return 0;
}

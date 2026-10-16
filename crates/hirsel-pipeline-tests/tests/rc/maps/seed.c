#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
@CALLS@
struct program_binding { const hirsel_rt_v2_layout *const *layouts; uint64_t count; };
extern const struct program_binding program __asm__("hirsel.program.registry");
int main(void) {
  uint32_t placements[3][64];
  const uint64_t seeds[] = {0, 1, UINT64_C(0x983bbfafffaa173c)};
  for (unsigned run = 0; run < 3; ++run) {
    hirsel_rt_v2_config config = {.abi_version=HIRSEL_RT_V2_ABI_VERSION, .flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, .hash_seed=seeds[run], .layouts=program.layouts, .layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx* ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    hirsel_rt_v2_header* map = NULL;
    assert(call_build(ctx, NULL, &map) == 0);
    uint8_t* payload = hirsel_rt_v2_payload(map);
    hirsel_rt_v2_header* entries;
    hirsel_rt_v2_header* index;
    memcpy(&entries, payload + @ENTRIES@, sizeof entries);
    memcpy(&index, payload + @INDEX@, sizeof index);
    uint8_t* es = hirsel_rt_v2_payload(entries);
    uint8_t* ix = hirsel_rt_v2_payload(index);
    uint64_t count, capacity;
    memcpy(&count, es + @COUNT@, 8);
    memcpy(&capacity, ix + @CAPACITY@, 8);
    assert(count == 32 && capacity == 64);
    for (uint64_t i = 0; i < count; ++i) {
      uint64_t key, value;
      memcpy(&key, es + @START@ + i * @STRIDE@ + @KEY@, 8);
      memcpy(&value, es + @START@ + i * @STRIDE@ + @VALUE@, 8);
      assert(key == i && value == i + 100);
    }
    memcpy(placements[run], ix + @SLOTS@, sizeof placements[run]);
    // A shallow detached root retains shared children. Consuming the clone
    // must copy entries and leave the original map intact.
    hirsel_rt_v2_header* clone = hirsel_rt_v2_alloc(ctx, map->descriptor & HIRSEL_RT_V2_INDEX_MASK, @MAPSIZE@, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 1);
    memcpy(hirsel_rt_v2_payload(clone), payload, @MAPSIZE@);
    hirsel_rt_v2_dup(ctx, entries);
    hirsel_rt_v2_dup(ctx, index);
    uint64_t first = 0;
    assert(call_first(ctx, &first, clone) == 0 && first == 100);
    memcpy(&count, es + @COUNT@, 8);
    assert(count == 32);
    // Without another root, the same cursor moves its entries.
    assert(call_first(ctx, &first, map) == 0 && first == 100);

    hirsel_rt_v2_audit audit;
    hirsel_rt_v2_task_exit(ctx, &audit);
    assert(audit.live_blocks == 0 && audit.retained_bytes == 0 && audit.active_regions == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
  }
  assert(memcmp(placements[0], placements[1], sizeof placements[0]) != 0);
  assert(memcmp(placements[0], placements[2], sizeof placements[0]) != 0);
  puts("seed changes buckets; insertion order stays 0..31");
}

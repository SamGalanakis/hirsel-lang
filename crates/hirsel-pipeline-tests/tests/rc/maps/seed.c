#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
@CALLS@
struct program_binding { const hirsel_rt_v2_layout *const *layouts; uint64_t count; };
extern const struct program_binding program __asm__("hirsel.program.registry");
#ifdef HIRSEL_MAP_MODEL
// The same private config injection as the placement control below.
int main(void) {
  const uint64_t seeds[] = {0, 1, 2, UINT64_C(0x9e3779b97f4a7c15)};
  for (unsigned run = 0; run < 4; ++run) {
    @MODEL_CALLS@
  }
  puts("mixed traces agree across four seeds; zero live heap");
}
#else
int main(void) {
  uint64_t placements[4][64];
  const uint64_t seeds[] = {0, 1, 2, UINT64_C(0x9e3779b97f4a7c15)};
  for (unsigned run = 0; run < 4; ++run) {
    hirsel_rt_v2_config config = {.abi_version=HIRSEL_RT_V2_ABI_VERSION, .flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, .hash_seed=seeds[run], .layouts=program.layouts, .layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx* ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    hirsel_rt_v2_header* map = NULL;
    assert(call_build(ctx, NULL, &map) == 0);
    uint8_t* payload = hirsel_rt_v2_payload(map);
    hirsel_rt_v2_header* entries;
    hirsel_rt_v2_header* index;
    hirsel_rt_v2_header* hashes;
    memcpy(&entries, payload + @ENTRIES@, sizeof entries);
    memcpy(&index, payload + @INDEX@, sizeof index);
    memcpy(&hashes, payload + @HASHES@, sizeof hashes);
    uint8_t* hs = hirsel_rt_v2_payload(hashes);
    uint8_t* es = hirsel_rt_v2_payload(entries);
    uint8_t* ix = hirsel_rt_v2_payload(index);
    uint64_t count, capacity;
    memcpy(&count, es + @COUNT@, 8);
    memcpy(&capacity, ix + @CAPACITY@, 8);
    assert(count == 32 && capacity == 64);
    uint64_t live, cached;
    memcpy(&live, payload + @LIVE@, 8);
    memcpy(&cached, hs + @HASHCOUNT@, 8);
    assert(live == count && cached == count);
    for (uint64_t i = 0; i < count; ++i) {
      uint64_t key, value;
      memcpy(&key, es + @START@ + i * @STRIDE@ + @KEY@, 8);
      memcpy(&value, es + @START@ + i * @STRIDE@ + @VALUE@, 8);
      assert((@SLICE@ || key == i) && value == i + 100);
      uint64_t hash;
      memcpy(&hash, hs + @HASHSTART@ + i * 8, 8);
      if (@SLICE@) {
        // Independently frame byte lengths 0..31 across the two-word fast path.
        uint64_t state[6], length = i;
        uint8_t text[32];
        memset(text, 'x', sizeof text);
        hirsel_rt_v2_hash_init(state, seeds[run]);
        uint64_t stream = (uint64_t)(uintptr_t)state;
        hirsel_rt_v2_hash_update(stream, &length, 8);
        hirsel_rt_v2_hash_update(stream, text, length);
        assert(hash == hirsel_rt_v2_hash_finish(stream));
      } else if (@COMPOSITE@) {
        // Independent framing: tuple(u64, String, Set[u64]), with canonical
        // set order 1,256 despite construction order 256,1. One finalization.
        uint64_t state[6], length = 4, members = 2, one = 1, big = 256;
        hirsel_rt_v2_hash_init(state, seeds[run]);
        uint64_t stream = (uint64_t)(uintptr_t)state;
        hirsel_rt_v2_hash_update(stream, &key, 8);
        hirsel_rt_v2_hash_update(stream, &length, 8);
        hirsel_rt_v2_hash_update(stream, "word", 4);
        hirsel_rt_v2_hash_update(stream, &members, 8);
        hirsel_rt_v2_hash_update(stream, &one, 8);
        hirsel_rt_v2_hash_update(stream, &big, 8);
        assert(hash == hirsel_rt_v2_hash_finish(stream));
      } else {
        assert(hash == hirsel_rt_v2_hash_bytes(seeds[run], (const uint8_t*)&key, 8));
      }
    }
    memcpy(placements[run], ix + @SLOTS@, sizeof placements[run]);
    for (uint64_t i = 0; i < capacity; ++i) {
      uint64_t bucket = placements[run][i];
      if (bucket == 0) continue;
      uint32_t ordinal = (uint32_t)bucket;
      assert(ordinal != 0 && ordinal <= count);
      uint64_t hash;
      memcpy(&hash, hs + @HASHSTART@ + ((uint64_t)ordinal - 1) * 8, 8);
      assert((bucket >> 32) == (hash >> 32));
    }
    // A shallow detached root retains shared children. Consuming the clone
    // must copy entries and leave the original map intact.
    hirsel_rt_v2_header* clone = hirsel_rt_v2_alloc(ctx, map->descriptor & HIRSEL_RT_V2_INDEX_MASK, @MAPSIZE@, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 1);
    memcpy(hirsel_rt_v2_payload(clone), payload, @MAPSIZE@);
    hirsel_rt_v2_dup(ctx, entries);
    hirsel_rt_v2_dup(ctx, index);
    hirsel_rt_v2_dup(ctx, hashes);
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
  for (unsigned run = 1; run < 4; ++run) {
    assert(memcmp(placements[0], placements[run], sizeof placements[0]) != 0);
  }
  puts("seed changes buckets; insertion order stays 0..31");
}
#endif

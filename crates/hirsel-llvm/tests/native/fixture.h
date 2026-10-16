/* Bind C probes to the immutable table carried by their emitted program. This is a fixture
 * adapter for ABI v2 only; every compiled object uses the driver's same index assignment. */
#ifndef HIRSEL_NATIVE_FIXTURE_V2_H
#define HIRSEL_NATIVE_FIXTURE_V2_H
#include "hirsel_rt_v2.h"
#include <assert.h>
struct hirsel_fixture_binding { const hirsel_rt_v2_layout *const *layouts; uint64_t count; };
extern const struct hirsel_fixture_binding hirsel_fixture_program __asm__("hirsel.program.registry");
static inline uint8_t hirsel_fixture_ctx_create(const hirsel_rt_v2_config *input, hirsel_rt_v2_ctx **out) {
    hirsel_rt_v2_config config = *input;
    config.layouts = hirsel_fixture_program.layouts;
    config.layout_count = (uint32_t)hirsel_fixture_program.count;
    return hirsel_rt_v2_ctx_create(&config, out);
}
static inline uint32_t hirsel_fixture_index(const hirsel_rt_v2_layout *layout) {
    for(uint32_t i=0; i<hirsel_fixture_program.count; i++) if(hirsel_fixture_program.layouts[i] == layout) return i+2;
    assert(!"a fixture descriptor outside its linked program");
    return 0;
}
#define hirsel_rt_v2_ctx_create hirsel_fixture_ctx_create
#endif

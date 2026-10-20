/* Measurement host: reuse the sealed application's objects with the runtime's
 * existing standalone admission mode (fresh grants and process stdin).
 * Only the launch descriptor is copied; application code and ABI stay intact. */
#include "hirsel_rt_v2.h"
#include <string.h>

extern int32_t __real_hirsel_rt_v2_main(const hirsel_rt_v2_program_descriptor *,
                                      int32_t, const char *const *);
int32_t __wrap_hirsel_rt_v2_main(const hirsel_rt_v2_program_descriptor *program,
                               int32_t argc, const char *const *argv) {
  hirsel_rt_v2_program_descriptor standalone = *program;
  standalone.flags |= HIRSEL_RT_V2_PROGRAM_STANDALONE;
  memset(&standalone.config, 0, sizeof standalone.config);
  return __real_hirsel_rt_v2_main(&standalone, argc, argv);
}

// Measurement-only LLVM interposition. Never linked into a timed executable.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static _Atomic uint64_t totals[4];
static uint64_t now(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) abort();
    return (uint64_t)t.tv_sec * UINT64_C(1000000000) + (uint64_t)t.tv_nsec;
}
void hirsel_scalar_perf_reset(void) {
    for (unsigned i = 0; i < 4; i++) atomic_store(&totals[i], 0);
}
uint64_t hirsel_scalar_perf_read(unsigned i) {
    if (i >= 4) abort();
    return atomic_load(&totals[i]);
}
static int speed(void) {
    const char *level = getenv("HIRSEL_PERF_LEVEL");
    if (!level || (strcmp(level, "O2") && strcmp(level, "O3"))) abort();
    return !strcmp(level, "O3");
}
void *LLVMCreateTargetMachine(void *target, const char *triple, const char *cpu,
                              const char *features, int level, int reloc, int model) {
    typedef void *(*Create)(void *, const char *, const char *, const char *, int, int, int);
    Create original = (Create)dlsym(RTLD_NEXT, "LLVMCreateTargetMachine");
    if (!original) abort();
    // Preserve the unoptimized launch shim and layout-check machines. LLVM's
    // pinned C enum is None=0, Less=1, Default=2, Aggressive=3.
    if (level) level = speed() ? 3 : 2;
    return original(target, triple, cpu, features, level, reloc, model);
}
void *LLVMRunPasses(void *module, const char *pipeline, void *machine, void *options) {
    typedef void *(*Run)(void *, const char *, void *, void *);
    Run original = (Run)dlsym(RTLD_NEXT, "LLVMRunPasses");
    if (!original) abort();
    if (!strcmp(pipeline, "default<O2>") || !strcmp(pipeline, "default<O3>")) {
        pipeline = speed() ? "default<O3>" : "default<O2>";
    }
    uint64_t start = now();
    void *result = original(module, pipeline, machine, options);
    atomic_fetch_add(&totals[0], now() - start);
    atomic_fetch_add(&totals[2], 1);
    return result;
}
int LLVMTargetMachineEmitToMemoryBuffer(void *machine, void *module, int kind,
                                       char **error, void **buffer) {
    typedef int (*Emit)(void *, void *, int, char **, void **);
    Emit original = (Emit)dlsym(RTLD_NEXT, "LLVMTargetMachineEmitToMemoryBuffer");
    if (!original) abort();
    uint64_t start = now();
    int result = original(machine, module, kind, error, buffer);
    atomic_fetch_add(&totals[1], now() - start);
    atomic_fetch_add(&totals[3], 1);
    return result;
}

// Optional one-process summary for the existing benchmark build helper. Its
// runtime/link children have no LLVM calls; the parent writes the final row.
#include <stdio.h>
__attribute__((destructor)) static void write_totals(void) {
    const char *path = getenv("HIRSEL_PERF_PHASES_OUTPUT");
    if (!path) return;
    FILE *file = fopen(path, "w");
    if (!file) abort();
    fprintf(file, "{\"optimization_ns\":%llu,\"emission_ns\":%llu,\"optimization_calls\":%llu,\"emission_calls\":%llu}\n",
            (unsigned long long)atomic_load(&totals[0]),
            (unsigned long long)atomic_load(&totals[1]),
            (unsigned long long)atomic_load(&totals[2]),
            (unsigned long long)atomic_load(&totals[3]));
    if (fclose(file)) abort();
}

// Diagnostic interposition at LLVM's object-emission boundary, after optimizations.
// Delegates to the original function unchanged; never loaded by timed programs.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>
static _Atomic unsigned modules;
int LLVMTargetMachineEmitToMemoryBuffer(void *machine, void *module, int kind,
                                       char **error, void **buffer) {
    typedef int (*Emit)(void *, void *, int, char **, void **);
    typedef int (*Print)(void *, const char *, char **);
    typedef void (*Dispose)(char *);
    Emit emit = (Emit)dlsym(RTLD_NEXT, "LLVMTargetMachineEmitToMemoryBuffer");
    Print print = (Print)dlsym(RTLD_NEXT, "LLVMPrintModuleToFile");
    Dispose dispose = (Dispose)dlsym(RTLD_NEXT, "LLVMDisposeMessage");
    const char *prefix = getenv("HIRSEL_BENCH_IR_PREFIX");
    if (prefix && print) {
        char path[4096], *message = NULL;
        snprintf(path, sizeof(path), "%s-%u.ll", prefix, atomic_fetch_add(&modules, 1));
        if (print(module, path, &message)) fprintf(stderr, "IR capture: %s\n", message);
        if (message && dispose) dispose(message);
    }
    if (!emit) abort();
    return emit(machine, module, kind, error, buffer);
}

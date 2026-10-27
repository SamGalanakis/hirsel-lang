"""Detect a race solely through memory accesses in an emitted object on stdin."""
import os
from pathlib import Path
import resource
import subprocess
import sys
import tempfile

SOURCE = r'''
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
extern void tsan_mapper(uint64_t *);
static uint64_t cells[8];
static void *worker(void *cell) {
  for (unsigned i = 0; i < 10000; ++i) tsan_mapper(cell);
  return 0;
}
int main(int argc, char **argv) {
  unsigned same = argc == 2;
  pthread_t workers[8];
  for (unsigned i = 0; i < 8; ++i)
    if (pthread_create(&workers[i], 0, worker, &cells[same ? 0 : i])) abort();
  for (unsigned i = 0; i < 8; ++i)
    if (pthread_join(workers[i], 0)) abort();
  if (!same) for (unsigned i = 0; i < 8; ++i)
    if (cells[i] != 10000) abort();
  return 0;
}
'''

def shadow_space():
    _, hard = resource.getrlimit(resource.RLIMIT_DATA)
    resource.setrlimit(resource.RLIMIT_DATA, (hard, hard))
    resource.setrlimit(resource.RLIMIT_CPU, (10, 10))

with tempfile.TemporaryDirectory(prefix="hirsel-native-tsan-") as scratch:
    root = Path(scratch)
    (root / "mapper.o").write_bytes(sys.stdin.buffer.read())
    (root / "control.c").write_text(SOURCE)
    clang = Path(os.environ["LLVM_SYS_231_PREFIX"]) / "bin/clang"
    # The C control has no instrumentation. Only the emitted mapper can
    # report the conflict; the sanitizer's pthread interceptors supply starts.
    subprocess.run([clang, "-c", "-O1", root / "control.c", "-o", root / "control.o"], check=True)
    subprocess.run([clang, "-fsanitize=thread", root / "control.o", root / "mapper.o", "-pthread", "-o", root / "control"], check=True)
    for arguments, status in [([], 0), (["same-cell"], 66)]:
        result = subprocess.run([root / "control", *arguments], capture_output=True, text=True,
                                preexec_fn=shadow_space, timeout=15,
                                env=os.environ | {"TSAN_OPTIONS": "symbolize=0:halt_on_error=1:exitcode=66"})
        assert result.returncode == status, (result.returncode, result.stdout, result.stderr)
        assert ("ThreadSanitizer: data race" in result.stderr) == bool(arguments), result.stderr
    print("2 native TSan controls passed")

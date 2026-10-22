#!/usr/bin/env python3
"""Run the instrumented eight-worker adapter under the normal pool RSS reservation.

Only the child's DATA address-space guard is lifted, as for hirsel_guard::sanitized;
ThreadSanitizer reserves terabytes of shadow mappings, not resident storage.
"""
import os
import resource
import subprocess

def shadow_space():
    _, hard = resource.getrlimit(resource.RLIMIT_DATA)
    resource.setrlimit(resource.RLIMIT_DATA, (hard, hard))

binary = os.environ['HIRSEL_IO_REAL_TSAN']
result = subprocess.run([binary, '8', '16', '--require-tsan'], preexec_fn=shadow_space, timeout=60,
                        env=os.environ | {'TSAN_OPTIONS': 'halt_on_error=1'})
raise SystemExit(result.returncode)

# A nonzero child status is always a failed sanitizer control.

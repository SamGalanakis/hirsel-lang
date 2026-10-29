#!/usr/bin/env python3
"""Exercise layout identity and frozen startup inputs through the native linker."""
import os
from pathlib import Path
import subprocess
import sys

if "HIRSEL_STARTUP_DRIVER_TESTS" not in os.environ:
    # main-runs invokes tool tests directly. Buck supplies the driver harness,
    # frozen native inputs and the normal test limits for the same recipe.
    root = Path(__file__).resolve().parents[2]
    command = [str(root / ".buck2/bin/buck2"), "test", "//:test-native-startup-layout"]
    if "--ignored" in sys.argv[1:]:
        command += ["--", "--test-arg=--ignored"]
    sys.exit(subprocess.run(command, cwd=root).returncode)

harness = os.environ["HIRSEL_STARTUP_DRIVER_TESTS"]
for ignored, cases in [
    (False, [
        "native::kit::tests::startup_layout_identity_uses_profile_content_instead_of_its_path",
        "native::kit::tests::kit_links_depend_on_neither_linker_threads_nor_directories",
        "native::kit::tests::static_kit_bootstraps_threads_tls_and_retained_runtime_on_both_targets",
    ]),
    (True, [
        "requests::native_bootstrap::tests::actual_native_bootstrap_links_both_backends_and_restores_after_runtime_drop",
        "native::object_queries::platform::tests::selected_platform_closes_runtime_before_keys_and_replays_without_originals",
    ]),
]:
    if "--ignored" in sys.argv[1:] and not ignored:
        continue
    result = subprocess.run([harness, "--exact", "--nocapture", *(["--ignored"] if ignored else []),
                    *cases], capture_output=True, text=True)
    print(result.stdout, end="", flush=True)
    print(result.stderr, end="", file=sys.stderr, flush=True)
    result.check_returncode()
    if f"test result: ok. {len(cases)} passed;" not in result.stdout:
        raise RuntimeError("the selected native layout test bodies did not all execute")

#!/bin/sh
# Run only the immutable-source native compiler-to-Link control, without CLI
# or standard-product setup unrelated to its actual dependencies.
set -eu
llvm=$(realpath "$1")
kit=$(realpath "$2")
archives=$(realpath "$3")
harness=$(realpath "$4")
export LLVM_SYS_231_PREFIX="$llvm" LD_LIBRARY_PATH="$llvm/lib"
export HIRSEL_LINK_KIT="$kit" HIRSEL_RUNTIME_ARCHIVES="$archives"
export HIRSEL_JOBS=1 RUST_TEST_THREADS=1
"$harness" --exact native_compilation_tasks_19::immutable_root_observation_uses_only_admitted_portable_summaries --nocapture --test-threads=1

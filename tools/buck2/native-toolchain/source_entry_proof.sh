#!/bin/sh
# Only the actual source-entry refusal and valid assembly bodies are needed.
set -eu
llvm=$(realpath "$1")
harness=$(realpath "$2")
export LD_LIBRARY_PATH="$llvm/lib" HIRSEL_JOBS=1 RUST_TEST_THREADS=1
"$harness" --exact native::root_source::tests::immutable_constructor_refuses_a_verified_nonentry_body_within_bounds --nocapture --test-threads=1
"$harness" --exact native::root_source::tests::settled_source_assembles_without_engine_or_body_reads_and_binds_actual_ready --nocapture --test-threads=1

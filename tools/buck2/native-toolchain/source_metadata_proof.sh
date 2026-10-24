#!/bin/sh
set -eu
llvm=$(realpath "$1")
kit=$(realpath "$2")
archives=$(realpath "$3")
unit=$(realpath "$4")
integration=$(realpath "$5")
export LLVM_SYS_231_PREFIX="$llvm" LD_LIBRARY_PATH="$llvm/lib"
export HIRSEL_LINK_KIT="$kit" HIRSEL_RUNTIME_ARCHIVES="$archives"
export HIRSEL_JOBS=1 RUST_TEST_THREADS=1
"$unit" --exact native::root_source::metadata_tests::copied_source_metadata_rejects_excess_and_keeps_one_lifetime_charge --nocapture --test-threads=1
"$integration" --exact native_compilation_tasks_19::immutable_root_observation_uses_only_admitted_portable_summaries --nocapture --test-threads=1

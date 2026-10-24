#!/bin/sh
set -eu
llvm=$(realpath "$1")
kit=$(realpath "$2")
archives=$(realpath "$3")
driver=$(realpath "$4")
export LD_LIBRARY_PATH="$llvm/lib" LLVM_SYS_231_PREFIX="$llvm" HIRSEL_JOBS=1
export HIRSEL_LINK_KIT="$kit" HIRSEL_RUNTIME_ARCHIVES="$archives"
"$driver" --exact native::object_queries::tests::legacy_blob_schema_one_remains_readable_with_streamed_schema_two --nocapture --test-threads=1
"$driver" --exact native::object_queries::tests::actual_ready_fixture_emits_links_restores_and_exits_on_both_backends --nocapture --test-threads=1

#!/bin/sh
set -eu
llvm=$(realpath "$1")
auditors=$(realpath "$2")
floor=$(realpath "$3")
qemu=$(realpath "$4")
harness=$(realpath "$5")
export LLVM_SYS_231_PREFIX="$llvm" LD_LIBRARY_PATH="$llvm/lib"
export HIRSEL_AUDITOR_PACKAGE="$auditors" HIRSEL_GLIBC_FLOOR="$floor" HIRSEL_QEMU_AARCH64="$qemu/qemu-aarch64"
export HIRSEL_JOBS=1 RUST_TEST_THREADS=1
"$harness" --ignored --exact native::object_queries::runtime::tests::equal_bytes_keep_distinct_loaded_libraries_and_shared_aliases --nocapture --test-threads=1
"$harness" --ignored --exact native::object_queries::runtime::tests::actual_runtime_products_reopen_changed_leaf_versions_and_refuse_missing_closure --nocapture --test-threads=1

#!/bin/sh
set -eu
llvm=$(realpath "$1")
engine=$(realpath "$2")
driver=$(realpath "$3")
export LD_LIBRARY_PATH="$llvm/lib" LLVM_SYS_231_PREFIX="$llvm" HIRSEL_JOBS=1
"$engine" store::request:: --nocapture --test-threads=1
"$driver" native::object_queries::tests:: --nocapture --test-threads=1

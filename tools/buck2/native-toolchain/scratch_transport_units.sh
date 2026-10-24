#!/bin/sh
set -eu
llvm=$(realpath "$1")
kit=$(realpath "$2")
archives=$(realpath "$3")
engine=$(realpath "$4")
driver=$(realpath "$5")
export HIRSEL_LINK_KIT="$kit" HIRSEL_RUNTIME_ARCHIVES="$archives"
export LD_LIBRARY_PATH="$llvm/lib" LLVM_SYS_231_PREFIX="$llvm" HIRSEL_JOBS=1
"$engine" store::request:: --nocapture --test-threads=1
"$driver" native::object_queries::tests:: --nocapture --test-threads=1

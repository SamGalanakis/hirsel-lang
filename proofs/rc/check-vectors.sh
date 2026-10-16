#!/usr/bin/env bash
# Run every protocol vector in native-tests/model through `lake exe rc-trace` and compare
# its output with the checked `.expected` file next to it. Exit 1 on any difference.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
vectors="$here/../../native-tests/model"
export PATH="$HOME/.elan/bin:$PATH"
if ! command -v lake >/dev/null; then
  echo "$(basename "$0"): lake not found; install elan in user space (proofs/rc/README.md)" >&2
  exit 1
fi
cd "$here"
lake build rc-trace >/dev/null
fail=0
count=0
for v in "$vectors"/*.trace; do
  count=$((count + 1))
  expected="${v%.trace}.expected"
  if [ ! -f "$expected" ]; then
    echo "missing expected output: $expected" >&2
    fail=1
    continue
  fi
  if ! diff -u "$expected" <(lake exe rc-trace "$v"); then
    echo "vector mismatch: $v" >&2
    fail=1
  fi
done
if [ "$count" -eq 0 ]; then
  echo "no vectors found in $vectors" >&2
  exit 1
fi
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "rc-trace: $count vectors match"

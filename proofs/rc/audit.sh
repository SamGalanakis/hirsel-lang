#!/usr/bin/env bash
# Audit the Lean RC model: no placeholder or escape hatch in the sources, a clean build,
# and every published theorem (Audit.lean) depending only on Lean's standard axioms.
# Prints the published theorem set. Exit 1 on any finding.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
export PATH="$HOME/.elan/bin:$PATH"
if ! command -v lake >/dev/null; then
  echo "$(basename "$0"): lake not found; install elan in user space (proofs/rc/README.md)" >&2
  exit 1
fi
cd "$here"

sources=(Rc.lean Main.lean Audit.lean Rc/*.lean)
fail=0

# 1. No placeholder, axiom declaration or trust escape hatch in any source.
forbidden='\b(sorry|admit|axiom|native_decide|implemented_by|extern|unsafe|ofReduceBool|debug\.skipKernelTC)\b'
if grep -nE "$forbidden" "${sources[@]}" | grep -v '^Audit.lean:.*#print axioms'; then
  echo "audit: forbidden construct in the sources (above)" >&2
  fail=1
fi

# 2. A clean build: no `sorry` warning from the elaborator either.
build_log="$(lake build 2>&1)" || { echo "$build_log" >&2; echo "audit: lake build failed" >&2; exit 1; }
if grep -q "declaration uses 'sorry'" <<<"$build_log"; then
  echo "audit: the build reports a declaration using sorry" >&2
  fail=1
fi

# 3. Every published theorem depends only on the standard axioms.
out="$(lake env lean Audit.lean 2>&1)" || { echo "$out" >&2; echo "audit: Audit.lean failed" >&2; exit 1; }
allowed='^(propext|Classical\.choice|Quot\.sound)$'
count=0
echo "published theorems:"
while IFS= read -r line; do
  [ -z "$line" ] && continue
  name="$(sed -E "s/^'([^']+)'.*/\1/" <<<"$line")"
  if [[ "$line" == *"does not depend on any axioms"* ]]; then
    axioms="(none)"
  elif [[ "$line" =~ depends\ on\ axioms:\ \[(.*)\]$ ]]; then
    axioms="${BASH_REMATCH[1]}"
    while IFS= read -r ax; do
      ax="$(xargs <<<"$ax")"
      if ! [[ "$ax" =~ $allowed ]]; then
        echo "audit: $name depends on non-standard axiom $ax" >&2
        fail=1
      fi
    done < <(tr ',' '\n' <<<"$axioms")
  else
    echo "audit: unexpected output: $line" >&2
    fail=1
    continue
  fi
  count=$((count + 1))
  printf '  %-34s %s\n' "$name" "$axioms"
done <<<"$out"

expected="$(grep -c '^#print axioms' Audit.lean)"
if [ "$count" -ne "$expected" ]; then
  echo "audit: expected $expected theorems, checked $count" >&2
  fail=1
fi
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "audit: $count published theorems, standard axioms only, no placeholders"

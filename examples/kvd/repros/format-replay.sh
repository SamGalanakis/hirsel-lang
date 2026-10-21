#!/bin/sh
set -eu
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp "$(dirname "$0")/format-before.txt" "$tmp/main.hir"
hirsel run "$tmp/main.hir" --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]}}' --record "$tmp/run.tape"
hirsel fmt "$tmp/main.hir"
hirsel run "$tmp/main.hir" --replay "$tmp/run.tape"

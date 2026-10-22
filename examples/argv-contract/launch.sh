#!/bin/sh
# Install beside argv-contract and grants.json (see docs/native-argv.md).
set -eu
recipe_dir=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
exec "$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" "$@"

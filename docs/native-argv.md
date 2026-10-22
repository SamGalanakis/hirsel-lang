# Check the arguments a native program receives

The [argv-contract example](../examples/argv-contract/main.hir) prints one compact
JSON array followed by a newline. It prints application arguments only: no
executable name or launcher options. `[]` and `[""]` are different vectors.
The existing JSON writer escapes quotes, backslashes and control characters and
preserves Unicode. This recipe uses UTF-8 arguments; process arguments cannot
contain NUL. Hirsel refuses non-UTF-8 launch snapshots.

The example's [grant fixture](../examples/argv-contract/grants.json) grants only
stdout and Args, with recognizable template arguments:

```json
{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":["TEMPLATE ONLY","template-tail"]}
```

No directory, environment, network, clock or task authority is needed.

## Prepare a temporary launch directory

Run from the repository root with a matching `hirsel` toolchain on PATH:

```sh
recipe_dir=$(mktemp -d)
cp examples/argv-contract/grants.json examples/argv-contract/launch.sh "$recipe_dir/"
hirsel build examples/argv-contract/hirsel.json --backend llvm -o "$recipe_dir/argv-contract"
```

To check the other backend, repeat the build with `--backend cranelift` and rerun
the commands below. The shell commands work with either executable. The package
manifest selects this checkout's std; no installed-toolchain std selection is
needed for this manifest-based recipe.

## Compare the routes

With no separator, `hirsel run --grants` retains the template vector:

```sh
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json"
# ["TEMPLATE ONLY","template-tail"]
```

An explicit `--` replaces it, including when there are zero following arguments.
Run each command; each succeeds with exit 0 and prints exactly its comment:

```sh
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" --
# []
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- 'foo bar'
# ["foo bar"]
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- ''
# [""]
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- --follow
# ["--follow"]
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- '羊' 'café'
# ["羊","café"]
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- 'foo bar' baz
# ["foo bar","baz"]
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- 'foo bar' '' --follow '羊' baz
# ["foo bar","","--follow","羊","baz"]
```

Add `--native --backend llvm` (or `cranelift`) before `--` to run through that
backend using `hirsel run`. The vectors and exit codes stay the same.

A built executable consumes `--hirsel-grants` and the next operand as launcher
configuration. Every remaining operand is an application argument. The template
Args are **always replaced**, including when there are zero trailing operands:

```sh
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json"
# []
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" 'foo bar'
# ["foo bar"]
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" ''
# [""]
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" --follow
# ["--follow"]
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" '羊' 'café'
# ["羊","café"]
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" 'foo bar' baz
# ["foo bar","baz"]
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" 'foo bar' '' --follow '羊' baz
# ["foo bar","","--follow","羊","baz"]
```

The native template path is already the argument boundary. Adding `--` after it
passes a literal `"--"` to the app. The separate native form `program -- args...`
grants only Args; this example also requests Io, so that form exits 3 with no
application output. It cannot print without an explicit stdout grant.

A granted Io root with no stdout projection is a different failure: the app
starts, then returns `Grant(NotGranted)` (exit 1, empty stdout):

```sh
printf '%s\n' '{"io":{"dirs":[]},"args":[]}' > "$recipe_dir/denied.json"
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/denied.json"
# stderr: error: Grant(NotGranted)
```

To pass the spelling of a launcher option as data, place it after the boundary:

```sh
hirsel run examples/argv-contract/hirsel.json --grants "$recipe_dir/grants.json" -- --hirsel-grants literal.json
"$recipe_dir/argv-contract" --hirsel-grants "$recipe_dir/grants.json" --hirsel-grants literal.json
# Both: ["--hirsel-grants","literal.json"] (exit 0)
```

`logdigest` follows the same rule: a native launch with only `--hirsel-grants`
returns `error: Usage` and exit 1 even if the template contains `app.log` and
`--follow`. Supply `app.log --follow` explicitly, together with that app's
required directory, signal and task grants. Empty process argv never means
"use the template defaults".

## Preserve the caller's vector with a wrapper

The shipped [launch.sh](../examples/argv-contract/launch.sh) uses `exec` and quoted
`"$@"`. Installed beside the executable and explicit fixture `grants.json`, it
supplies that grant path and forwards exactly the caller's operands:

```sh
"$recipe_dir/launch.sh"
# []
"$recipe_dir/launch.sh" 'foo bar' '' --follow '羊' baz
# ["foo bar","","--follow","羊","baz"]
"$recipe_dir/launch.sh" --hirsel-grants literal.json
# ["--hirsel-grants","literal.json"]
```

All the native vectors above also apply to the wrapper. It synthesizes no
application args, evaluates no command string and preserves the executable's
exit status. It resolves siblings relative to the script, so it can be invoked
from another directory. Invoke its actual path; symlink relocation is not
supported. This is a POSIX `/bin/sh` recipe for Unix hosts; the native tests cover
x86-64 Linux on LLVM and Cranelift. Windows needs a platform-specific launcher;
PowerShell and `cmd.exe` quoting are outside this script's contract. Use a UTF-8
terminal when copying the Unicode operands.

## Record and replay without new grants

```sh
"$recipe_dir/argv-contract" --hirsel-record "$recipe_dir/argv.tape" \
  --hirsel-grants "$recipe_dir/grants.json" 'foo bar' '' --follow '羊' baz
# ["foo bar","","--follow","羊","baz"]
"$recipe_dir/argv-contract" --hirsel-replay "$recipe_dir/argv.tape"
# ["foo bar","","--follow","羊","baz"]
hirsel run examples/argv-contract/hirsel.json --replay "$recipe_dir/argv.tape"
# ["foo bar","","--follow","羊","baz"]
rm -r "$recipe_dir"
```

Each exits 0. Replay takes Args and recorded stdout from the tape and needs no
live grants file. The tests remove that file before replaying. Keep the tape
outside granted directories; this example grants none.

## Checked recipe and prior art

`crates/hirsel-cli/tests/support/native_argv_recipe_6.rs` reuses the examples policy
fixture, copies this app and script into a temporary directory, builds actual
standalone apps and compares exact stdout, stderr and exit status. Its three
cases cover interpreted run, native run and built/wrapped LLVM and Cranelift
launches, escaped controls, missing authority, required arguments and replay.

The command/argv separation follows Go's `os/exec/exec.go:393–405`; the independent
`["foo bar","baz"]` vector follows `os/exec/exec_test.go:285–295`, both at
`ebe2feefc400f813aefdc8a036e66d2619da6649` (BSD-3-Clause). No Go implementation
is copied. Hirsel-specific differences: Args omits the executable name, grants
are explicit launcher configuration, snapshots must be UTF-8, and replay uses
the admitted snapshot. Output uses Hirsel's existing JSON writer rather than
Go's space-joined test output so empty args and boundaries are visible.

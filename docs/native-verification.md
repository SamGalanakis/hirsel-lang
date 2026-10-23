# Build once, verify several launches

Build a standalone executable once per backend, then invoke that exact path for
every observation. The [tiny fixture](../examples/native-verification-9/main.hir)
reads `data/input.txt` and prints `<argument>:<file bytes>`. It requires one
argument, a read-only directory mount and stdout. It requests no environment,
network, clock or writable directory authority.

This is a reproducible verification recipe. It adds no launcher or cache, and
makes no speed claim. The [building guide](building.md#build-and-launch-a-program)
defines native toolchain setup, launch admission and record/replay behavior.
The commands below target x86-64 Linux with a matching `hirsel` on PATH and its
pinned LLVM, link kit and runtime archives configured as described there.

Use `hirsel build --help` or `hirsel help run` to discover compiler options
without preparing inputs or a native toolchain. Put command help before the
source operand; operands after `hirsel run ... --` are application Args.
Standalone launch prefixes follow the [recording recipe](#record-remove-host-access-replay):
`--hirsel-record` comes before `--hirsel-grants`.

## Prepare and build

Run from the repository root. `mktemp` gives this session private scratch; keep
it through verification, including failures. Every path below derives from its
absolute path.

```sh
recipe_dir=$(mktemp -d)
recipe_dir=$(cd "$recipe_dir" && pwd -P)
manifest="$PWD/examples/native-verification-9/hirsel.json"
export HIRSEL_JOBS=1 HIRSEL_DAEMON=off
mkdir "$recipe_dir/data" "$recipe_dir/other-data"
printf 'hello\n' > "$recipe_dir/data/input.txt"
printf 'other\n' > "$recipe_dir/other-data/input.txt"
printf 'HOST-DENIAL-SENTINEL\n' > "$recipe_dir/input.txt"
cat > "$recipe_dir/grants.json" <<'JSON'
{"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":"data","access":"read"}]},"args":[]}
JSON
cat > "$recipe_dir/other.json" <<'JSON'
{"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":"other-data","access":"read"}]},"args":[]}
JSON
printf '%s\n' '{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}' > "$recipe_dir/denied.json"

hirsel build "$manifest" --backend llvm -o "$recipe_dir/recipe-llvm"
hirsel build "$manifest" --backend cranelift -o "$recipe_dir/recipe-cranelift"
sha256sum "$manifest" examples/native-verification-9/main.hir \
  "$recipe_dir/recipe-llvm" "$recipe_dir/recipe-cranelift" > "$recipe_dir/identities.sha256"
cat "$recipe_dir/identities.sha256"
```

These are the only two native build commands. Source, manifest/dependency
selection, compiler, target, backend, codegen flags and runtime/link inputs are
compilation inputs. Rebuild after changing any of them. In particular, editing a
function body requires a new binary and invalidates its old recordings.
Arguments, grants, seeds and tape paths are launch inputs: changing them needs no
rebuild. Keep the compiler and all source/dependency files fixed through the run.

The manifest's `main.hir` and `../../stdlib/std@1/hirsel.json` resolve relative to
the manifest, independently of the shell's working directory. A file-based grants
template's relative `host` resolves relative to that template. An inline JSON
template must instead use absolute hosts. Neither selecting std nor finding a
host file grants authority. The sentinel above is deliberately outside the mount.

## Resolve directory hosts beside the grants file

Relative host paths resolve relative to the grants file. With
`/tmp/session/grants.json`, `"host":"data"` selects `/tmp/session/data`, even
when the launcher runs from another directory. The mount's `"path":"data"`
is the program's granted name; it does not determine the host base. The same
rule applies to directory hosts in nested runner grants. Inline JSON has no
file base and requires absolute hosts; canonical run records also require
canonical absolute hosts.

Keep the templates and fixture directories above unchanged. Preserve a separate
bad template and launch both from another working directory:

```sh
cat > "$recipe_dir/bad-relative.json" <<'JSON'
{"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":"data/data","access":"read"}]},"args":[]}
JSON
mkdir "$recipe_dir/launch-cwd"
(
  cd "$recipe_dir/launch-cwd"
  hirsel run "$manifest" --grants "$recipe_dir/bad-relative.json" -- alpha
  # exit 3, empty stdout: invalid io.dirs.host, before program entry
  hirsel run "$manifest" --grants "$recipe_dir/grants.json" -- alpha
  # exit 0, stdout: alpha:hello plus LF
)
```

The refusal retains its `kind:"invalid"`, `field:"io.dirs.host"` and OS cause,
and adds the original host, grants-file directory and attempted resolved
candidate. For the bad template those are `data/data`, `$recipe_dir` and
`$recipe_dir/data/data`. Read the decoded JSON `detail` to inspect quoted,
Unicode or whitespace-containing paths. The launcher refuses a missing relative
host without retrying another base or creating a directory. Choose the intended
host in the template; for an example's grants file beside its `fixtures`
directory, keep `"host":"fixtures"`, rather than prefixing it with the path
from the repository root. An absolute host keeps its existing validation and
admission behavior. Missing authority inside an admitted program remains a
separate application error, as the denied control below demonstrates.

## Compare identical observations

First use the ordinary interpreter as the control:

```sh
hirsel run "$manifest" --grants "$recipe_dir/grants.json" -- alpha
# stdout: alpha:hello\n; stderr empty; exit 0
hirsel run "$manifest" --grants "$recipe_dir/grants.json" -- 'two words'
# stdout: two words:hello\n; stderr empty; exit 0
hirsel run "$manifest" --grants "$recipe_dir/other.json" -- beta
# stdout: beta:other\n; stderr empty; exit 0
hirsel run "$manifest" --grants "$recipe_dir/denied.json" -- alpha
# stdout empty; stderr: error: Files(NotGranted)\n; exit 1
hirsel run "$manifest" --grants "$recipe_dir/grants.json" --
# stdout empty; stderr: error: Usage\n; exit 1
```

Then make the same assertions against each already-built executable:

```sh
for backend in llvm cranelift; do
  exe="$recipe_dir/recipe-$backend"
  "$exe" --hirsel-grants "$recipe_dir/grants.json" alpha
  "$exe" --hirsel-grants "$recipe_dir/grants.json" 'two words'
  "$exe" --hirsel-grants "$recipe_dir/other.json" beta
  "$exe" --hirsel-grants "$recipe_dir/denied.json" alpha
  "$exe" --hirsel-grants "$recipe_dir/grants.json"
done
```

Each launch must match its corresponding control's **stdout bytes, stderr bytes
and exit status**. `\n` in comments means one LF byte. Expected failures above
exit 1; run them in a shell without `set -e`, or capture their statuses explicitly.
These failures occur inside the program, after entry admission. They differ from
launching this Io/Args entry with no grants, which refuses admission with exit 3.

`hirsel run --grants ... --` replaces the template's Args, including with an empty
vector. A direct executable always replaces template Args with its trailing
operands. Do not add `--` after its `--hirsel-grants` path: that would become a
literal application operand. See [the argv recipe](native-argv.md).

## Pin a canonical run record

A grants template is rebound to the selected program on each launch. Export a
bound run record to expose its semantic program identity and pin it explicitly:

```sh
hirsel export --run "$manifest" -o "$recipe_dir/run.json"
python3 - "$recipe_dir" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
path = root / 'run.json'
run = json.loads(path.read_text())
grants = json.loads((root / 'grants.json').read_text())
grants['io']['dirs'][0]['host'] = str(root / 'data')
run.update(grants)
run['args'] = ['alpha']
path.write_text(json.dumps(run, indent=2) + '\n')
print(path)
print('program:', run['program'])
PY
hirsel run "$manifest" --run "$recipe_dir/run.json"
for backend in llvm cranelift; do
  "$recipe_dir/recipe-$backend" --hirsel-run "$recipe_dir/run.json" alpha
done
```

Each prints `alpha:hello` plus LF, with empty stderr and exit 0. A run record's
path and directory hosts must be canonical absolute paths. Interpreted `--run`
owns its Args and rejects trailing arguments; direct `--hirsel-run` replaces Args
with the process operands. This example supplies `alpha` explicitly to both.

## Record, remove host access, replay

Tapes must be outside every granted directory. Record once through each engine:

```sh
hirsel run "$manifest" --grants "$recipe_dir/grants.json" \
  --record "$recipe_dir/interpreted.tape" -- alpha
for backend in llvm cranelift; do
  "$recipe_dir/recipe-$backend" --hirsel-record "$recipe_dir/$backend.tape" \
    --hirsel-grants "$recipe_dir/grants.json" alpha
done
```

Every recording has the same successful observation as `alpha` above. Now move
the host directories away from the exact paths bound into the tapes. Renaming
works even when the checker runs as root; a permission-bit denial would not.

```sh
mv "$recipe_dir/data" "$recipe_dir/retired-data"
mv "$recipe_dir/other-data" "$recipe_dir/retired-other-data"
hirsel run "$manifest" --grants "$recipe_dir/denied.json" -- alpha
for backend in llvm cranelift; do
  "$recipe_dir/recipe-$backend" --hirsel-grants "$recipe_dir/denied.json" alpha
done
# All three: empty stdout; error: Files(NotGranted) plus LF on stderr; exit 1.
# The existing HOST-DENIAL-SENTINEL input.txt supplies no fallback authority.
for name in grants other denied run; do
  mv "$recipe_dir/$name.json" "$recipe_dir/$name.retired.json"
done

for producer in interpreted llvm cranelift; do
  tape="$recipe_dir/$producer.tape"
  hirsel run "$manifest" --replay "$tape"
  for backend in llvm cranelift; do
    "$recipe_dir/recipe-$backend" --hirsel-replay "$tape"
  done
done
sha256sum -c "$recipe_dir/identities.sha256"
```

All nine replays must print `alpha:hello` plus LF with empty stderr and exit 0,
although the original host directories and grant/record paths no longer exist.
Replay consumes the tape's admitted run and recorded effects, needs no live
grants and performs no host file read. A changed request, missing event or
unconsumed suffix fails replay. Keep tapes private: they contain the run record
and all bytes read/written. Preserve failed attempts in distinct directories;
remove only your own evidence directory when you no longer need it.

Binary v8 recordings now finish with frame kind 6: the event count and the
journal's terminal-outcome encoding. Replay checks the count and outcome; a
recorded Blocked ending permits an exhausted replay to stop without another
host input only after its terminal wait observes that ending. A missing event
alone does not establish Blocked. Legacy kind 5 count-only trailers remain
readable with their existing strict input rules. Durable journals retain the
same terminal evidence. Runtime revision binding still applies to old tapes.

## Run the executable check

From a source checkout, discover the registered target, then run this one check:

```sh
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test root//:test-native-verification-9
# To retain scratch on the invoking host, use this instead:
.buck2/bin/buck2 test -c hirsel.tests=host root//:test-native-verification-9
```

The target supplies the matching compiler, std image, runtime archives and
native tools. Alternatively, with that same toolchain environment configured:

```sh
python3 tools/tests/native_verification_9.py --hirsel /absolute/path/to/hirsel
```

Expected summary: **1 recipe test, 33 observations, builds={'llvm': 1,
'cranelift': 1}**. There are six ordinary cases (including the bound run), three
recordings, three host-denial sentinels and nine cross-engine replays. The same
expected triples are used for all engines. No `run --native` observation occurs.

The checker retains an owned `native-verification-9-*` scratch directory on its
execution host and prints its absolute path. A remote worker's scratch may be
discarded with its sandbox; use the host command above when retaining artifacts
locally is required. It records every argv, working directory, stdout,
stderr and exit status, build invocation counts, executable SHA-256/mtime,
canonical program identity, tapes and a result JSON. It freezes source,
dependency, compiler and supplied native tool/runtime input hashes before any
consumer, checks source/executable identity around observations, and verifies
all input hashes again after consumption, including failed attempts. Generated
launch policies, host input bytes, bound records and tapes are also hashed before
consumption; intentional renames preserve those hashes. Any second build, executable rewrite,
changed source, incomplete observation count or semantic mismatch fails.
Neither runtime tape nor sidecar formats are changed.

Running the Python file without `--hirsel` reports **SETUP ONLY, 0 recipe tests,
0 observations**; this supports tool discovery and is not a green integration
check. Setup errors and timeouts retain a separate result and partial command
evidence. A Buck2 error before script execution is a setup failure; zero selected
tests is not verification. Retain those outer logs too, and require the nonzero
summary above before claiming success.

Authority for this recipe is the current [native launch](building.md#build-and-launch-a-program)
and [record/replay](building.md#record-and-replay-a-run) documentation, checked
against CLI help by the executable check. It applies the build-once workflow
identified in dogfood-7's verification report (lines 94–114). No external codec,
protocol or algorithm implementation is ported; no external source license or
notice is introduced.

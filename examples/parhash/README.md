# parhash

Hash explicit relative files concurrently and print one JSON report in manifest
order. Repeated paths are separate indexed jobs. Uses the existing std SHA-256.

Build a matching CLI and std image:

```sh
python3 tools/buck2/prepare.py
.buck2/bin/buck2 build //:hirsel-cli-bin-hirsel //:hirsel-std-image --show-output
```

Set `HIRSEL` to the printed CLI path and export `HIRSEL_IMAGE` with the
printed image path. Native linking uses the pinned toolchain environment from
[docs/building.md](../../docs/building.md#what-a-native-build-needs) (the
Buck2 harness sets it automatically). The required absolute variables are
`LLVM_SYS_231_PREFIX`, `HIRSEL_LINK_KIT`, and preferably
`HIRSEL_RUNTIME_ARCHIVES` from the matching source build. Instantiate the grants template; its host designator must be absolute:

```sh
python3 - <<'PY'
import json
from pathlib import Path
p = Path('examples/parhash')
g = json.loads((p / 'grants.json').read_text())
g['io']['dirs'][0]['host'] = str((p / 'fixtures').resolve())
Path('/tmp/parhash-grants.json').write_text(json.dumps(g))
PY
HIRSEL_JOBS=4 tools/capped --timeout 60 "$HIRSEL" run --native --backend llvm \
  examples/parhash/hirsel.json --grants /tmp/parhash-grants.json --cores 2 \
  --record /tmp/parhash.tape
```

The two application arguments are the virtual granted directory and a JSON
manifest, for example `fixtures '{"paths":["abc","empty","abc"],"jobs":2,"byte_cap":1024}'`.
They can be supplied after `--` or snapshotted in grants as above. Only the
read-only directory, Tasks, stdout and stderr are needed. No traversal,
network, clock, process or mutation grant is used. `HIRSEL_JOBS=4` here supplies
the cap wrapper with enough CPU quota for the selected physical width. The
compiler still runs once outside repeated executable runs.

The manifest has exactly `paths`, `jobs` and `byte_cap`. Limits: 64 paths,
1..8 jobs, 0..262144 bytes per file, 65536 manifest bytes, 512 UTF-8 bytes per
path, 16 components. Paths must be canonical relative spellings, without dot
components, NUL, backslash or colon. Admission reserves `len(paths)*byte_cap`,
which must be <=4194304 bytes. Thus a large cap with many tiny files can be
rejected; choose the smallest useful cap. Duplicates count toward every bound.
Each bounded read may consume one extra byte to detect excess; at most 64 such
probe bytes are additional to the reserved payload.

Each batch spawns at most `jobs` children, drains its task set in completion
order and places successes by add index. It joins the complete batch before
admitting the next. Placement cores 1/2/4 choose physical execution and do not
change logical jobs. An error cancels and joins remaining members and abandons
all accumulated results. The first completed error in the failing batch is
reported; it is a scheduling input reproduced by replay. No partial success
report is emitted. Exit 0 means one complete report, exit 2 an application error,
exit 3 missing output grants. Output I/O failures remain typed entry errors.

Example output for the single `abc` file:

```json
{"entries":[{"index":0,"path":"abc","bytes":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}]}
```

Metadata rejects symlinks at every component and nonregular leaves before open.
Metadata then open is not an atomic snapshot against concurrent replacements;
the provider's pinned Dir confinement still applies. SHA-256 computes one bounded
complete digest after the read; pure hashing has no cancellation point. A slow
regular-file provider has no application deadline. The waiting-file cancellation
acceptance cell is explicitly incomplete until a supported provider can exercise it.

Canonical event tracing is a separate acceptance lane; see FRICTION.md for the
retained observer failure and its owner. Ordinary tape recording/replay works
without that diagnostic observer.

Build once for repeated runs: `"$HIRSEL" build examples/parhash/hirsel.json
--backend llvm -o /tmp/parhash`. Run `/tmp/parhash --hirsel-record /tmp/run.tape
--hirsel-grants /tmp/parhash-grants.json fixtures
'{"paths":["abc"],"jobs":2,"byte_cap":1024}'`. A standalone executable takes
its argument snapshot from those trailing arguments; supply both explicitly. Replay with `/tmp/parhash
--hirsel-replay /tmp/run.tape`, or `"$HIRSEL" run examples/parhash/hirsel.json
--replay /tmp/run.tape`; replay needs no original files. Record destinations must
be outside the granted mount. Use new paths for every recording.

Prior art: Tokio `join_set.rs:374–395` and `task_join_set.rs:267–295`,
`ce685ee6916549ed7e52dbfd5430858bbdc734d3` (MIT); Go `crypto/sha256/sha256.go:30–63`
and `sha256_test.go:26–31,94–118`, `ebe2feefc400f813aefdc8a036e66d2619da6649`
(BSD-3-Clause), licenses in `third-party/NOTICES.md`. Adaptations: Hirsel scopes
own cleanup, typed errors replace JoinError/panic, results use add order instead
of JoinSet completion order, batches bound admission. Model controls port six
literal vectors and half-update equivalence; no digest implementation is copied.

# packtree

Bounded deterministic uncompressed TAR pack, list and unpack. The six explicit
modules in hirsel.json separate CLI projections, declared catalog policy, ordered
writer publication, extraction preflight, exact streams/manifests and E1 jobs.

The source tree is declared in JSON, rather than discovered with an unbounded
read_dir. Schema 1 has entries with path, type (file/directory), size; every parent
must be a declared directory. Only those members are packed. Extra source files
are ignored. Metadata and exact EOF reads verify the declaration. Example:
fixtures/tree.json ports Go AddFS hello/world/emptyfolder, rejecting symlinks.
Create emptyfolder with `mkdir -p examples/packtree/fixtures/emptyfolder`.

Bounds: 64 members; 65536 bytes/file; 1048576 payload bytes; 2097152 archive
bytes; 512 UTF-8 bytes/path; depth8; jobs1..4; declaration65536 bytes.
Aggregate admission precedes reads; each worker holds at most one admitted body.
Results are joined in completion order and moved into path order, then the sole
mutable std.tar writer emits sorted UTF-8 names. Numeric metadata is fixed at
zero; modes are 0644/0755, owner/group names empty. std.tar selects USTAR when
fields fit, deterministic PAX otherwise (Unicode and long paths). No compression.

Use three distinct host directories: source, archives, target. Target must be
initially empty; grants can alias host roots, so the caller must supply distinct
roots. The app never weakens Dir confinement. Parent/final symlinks are refused
at source metadata checks; metadata and open are separate operations and provide
no adversarial symlink-race guarantee beyond the provider contract.

Create a private set of directories and concrete grants (run from the repo):

```sh
mkdir -p /tmp/packtree-demo/source/emptyfolder /tmp/packtree-demo/source/subfolder /tmp/packtree-demo/archives /tmp/packtree-demo/target
cp examples/packtree/fixtures/tree.json examples/packtree/fixtures/file.go /tmp/packtree-demo/source/
cp examples/packtree/fixtures/subfolder/another.go /tmp/packtree-demo/source/subfolder/
python3 - <<'PY'
import json
from pathlib import Path
base=Path('/tmp/packtree-demo').resolve()
g=json.loads(Path('examples/packtree/grants.json').read_text())
for mount in g['io']['dirs']: mount['host']=str(base/mount['path'])
for cmd,argv in [('pack',['pack','tree.json','out.tar','4']),('list',['list','out.tar']),('unpack',['unpack','out.tar'])]:
    g['args']=argv
    (base/(cmd+'-grants.json')).write_text(json.dumps(g))
PY
hirsel run examples/packtree/hirsel.json --grants /tmp/packtree-demo/pack-grants.json
hirsel build examples/packtree/hirsel.json --backend llvm -o /tmp/packtree-llvm
/tmp/packtree-llvm --hirsel-record /tmp/packtree-pack.tape --hirsel-grants /tmp/packtree-demo/pack-grants.json pack tree.json out.tar 4
/tmp/packtree-llvm --hirsel-grants /tmp/packtree-demo/list-grants.json list out.tar
/tmp/packtree-llvm --hirsel-grants /tmp/packtree-demo/unpack-grants.json unpack out.tar
/tmp/packtree-llvm --hirsel-replay /tmp/packtree-pack.tape
```

Use a fresh initially empty target for each unpack. Resetting or removing an old
output tree is an explicit caller action. The example pack can replace out.tar
atomically; the declaration never includes its own tree.json.

Observer/record options precede --hirsel-grants; application arguments follow it.
Keep recordings outside granted roots. Placement cores1/2/4 uses Ordered only;
jobs is a separate fixed logical admission bound. Requested width alone proves
no physical placement. For read-only listing use archives access read.

List and successful pack/unpack print schema1 JSON entries with canonical path,
type,size,SHA256 and aggregate bytes. Error JSON {kind,path} goes to stderr,
exit2; unavailable output projections exit3. Resource/tooling refusal is distinct
from language failure. Unpack reads and validates every path, kind, duplicate,
ancestor collision, limit, header and body before destination writes. External
archives can omit directory headers; safe parents are created explicitly.
Only regular files and directories are accepted. Global/sparse/link/device/FIFO
entries are refused. Nonzero trailing data and missing two-block terminators are
refused even though std.tar intentionally accepts some EOF-terminated archives.

Archive publication and each extracted file use AtomicFile. Failed writes abort
the temporary. Extraction is not a multi-file transaction: it publishes
.packtree-incomplete first, .packtree-manifest.json last, then removes the marker.
A failure leaves the incomplete tree for inspection; never resume into it.
The completion manifest is authoritative if marker cleanup itself fails. These
two names (and descendants) are reserved. No permissions/times are restored.

References: Go ebe2feef archive/tar/writer.go:401-466, writer_test.go:1354-1410;
path/filepath/path.go:319-346 and path_test.go:532-562,630-685 (BSD-3-Clause,
third-party/NOTICES.md). Deviations: explicit bounded declaration replaces ambient
WalkDir; supplied Dir replaces ambient fs; no symlinks; fixed metadata; bounded
owned batches replace io.Copy; errors are typed values; completion manifest and
preflight are application policy. std supplies all TAR and SHA256 codecs.
Python tarfile/hashlib supplies independent fixtures and output checks.
See FRICTION.md for exact verification dispositions and known missing cells.

Verification at the implementation generation: 86 small reference/checked-H
observations and22 additional distinct cap observations pass; native production
120/120 outcomes and240/240 native tape replays pass. Reference replay is106
passes plus14 incomplete boundary cells. Additional placement/semantic-event and
binding controls are detailed in FRICTION.md. Pending-worker cancellation and
several scripted/replay adapter obligations remain INCOMPLETE, so this is not an
all-engine/replay completion claim. The permanent harness has three named tests:
packtree_29_reference_checked_workflows, packtree_29_reference_checked_caps and
packtree_29_native_workflows_replay. App lines: main71, catalog213, pack80,
unpack154, manifest121, jobs104 (743 total).

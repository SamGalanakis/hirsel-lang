# pnmtiles

Inspect or filter one binary 8-bit Netpbm image. P5 grayscale and P6 RGB are
supported. `invert` subtracts each channel from 255. `blur` takes the 3×3
clamp-to-edge neighborhood, independently sums each channel, and emits
`floor(sum / 9)`, including at corners and tile seams. Dimensions are preserved.

```text
info ROOT INPUT [--interruptible]
filter ROOT INPUT OUTPUT invert|blur JOBS [--interruptible]
```

`ROOT` selects an explicitly granted virtual directory; input/output paths are
relative to it. `JOBS` is 1–4. Jobs 1 runs the same kernels serially; jobs 2–4
produce unique 16-row bands against one frozen immutable raster, drain completed
children, and assemble their results in row order. No tile or output is published
until every child succeeds. Publication uses `file.create_atomic`, two
`file.write_all` calls, then `file.commit`; errors drop and remove the temporary.
An existing output survives an error before commit. Input and output may name the
same file: the complete source is read before replacement.

From a checkout, with a matching `hirsel` on PATH:

```sh
mkdir -p /tmp/pnmtiles-data /tmp/pnmtiles-evidence
cp examples/pnmtiles/fixtures/*.pnm /tmp/pnmtiles-data/
python3 - <<'PY'
import json
from pathlib import Path
p = json.loads(Path('examples/pnmtiles/grants.template.json').read_text())
p['io']['dirs'][0]['host'] = '/tmp/pnmtiles-data'
Path('/tmp/pnmtiles-evidence/grants.json').write_text(json.dumps(p) + '\n')
PY
hirsel run examples/pnmtiles/hirsel.json --grants /tmp/pnmtiles-evidence/grants.json -- info data sample-2x3-c3-blur.pnm
hirsel build examples/pnmtiles/hirsel.json --backend llvm -o /tmp/pnmtiles-evidence/pnmtiles
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 /tmp/pnmtiles-evidence/pnmtiles --hirsel-record /tmp/pnmtiles-evidence/blur.tape --hirsel-grants /tmp/pnmtiles-evidence/grants.json filter data sample-2x3-c3-blur.pnm output.ppm blur 4
/tmp/pnmtiles-evidence/pnmtiles --hirsel-replay /tmp/pnmtiles-evidence/blur.tape
```

The template grants stdout, stderr, tasks and one writable directory. For `info`,
use read access; filters need write access to publish. No ambient paths, clocks,
processes, network or environment are read. To use `--interruptible`, explicitly
add `"signals":{"provider":"signals.host@1","kinds":["interrupt"]}` to `io`.
The whole operation then races the interrupt wait and cancels/joins the loser.
Cancellation is checked per output row. A concurrent cancellation after a
successful commit can leave the complete output; atomic publication does not
promise rollback of an already committed effect.

Successful stdout is one line: `P5 WIDTH HEIGHT BYTES` for info, or
`invert P5 WIDTH HEIGHT BYTES` / `blur P6 WIDTH HEIGHT BYTES` for a filter.
Raster bytes go only to the output file. Exit 0 means success; exit 2 is an
application error with exactly `pnmtiles: CODE\n` on stderr. Codes distinguish
`usage`, `unsupported`, `header`, `decimal`, `overflow`, `dimensions`, `maxval`,
`short_raster`, `trailing_raster`, `header_limit`, `raster_limit`, `jobs`,
`file_denied`, `file_open`, `file_read`, `file_create`, `file_write`, `file_commit`,
`grant`, `canceled`, and internal `assembly`. Missing stdout/stderr exits 3.
Language failures and tooling stops retain the launcher's own classification.

Limits are width/height 1–256, raster ≤196608 bytes, header ≤4096 bytes,
4 live tile tasks, 16 rows per tile, jobs ≤4. Checked products and offsets precede
reads and allocations. Header fields accept unsigned decimal digits, including
leading zeroes. Comments begin with `#` between fields and end at CR or LF.
Exactly one ASCII whitespace byte after maxval is the raster separator. Every
following byte is data, including newline, space and `#`; a CRLF there consumes
CR and treats LF as data. Raster length must match exactly; concatenated images,
trailing whitespace, ASCII/PBM/PAM, 16-bit samples and maxval other than 255 are
rejected. A one-byte EOF probe distinguishes exact length from a longer stream.

The codec is private to this example. `pnm.hir` owns incremental decoding and
canonical encoding; `image.hir` owns checked layouts, borrowed views and the gray/RGB
pixel dictionaries; `filter.hir` implements generic integer kernels; `tiles.hir`
owns bounded admission and ordered assembly; `report.hir` owns errors and operation
metadata; `main.hir` orchestrates CLI, grants, input and publication. All six are
explicit manifest modules. They total 940 formatted handwritten Hirsel lines
(main245, pnm158, image150, filter124, tiles160, report103).

Decoder/tap structure and upstream PGM/gray/RGB arrays come from image-rs commit
`075e65d618a323e5cb48c1d8710a2eca25d690c6`:
`src/codecs/pnm/decoder.rs:303–473,1415–1581`,
`src/codecs/pnm/mod.rs:119–184`, `src/imageops/sample.rs:901–973`.
The MIT option is retained in `LICENSE-image-rs`. Deviations are the bounded
binary-only subset, strict field-boundary comments and exact raster length,
integer averaging instead of float weights, capability-based bounded reads,
explicit frozen/unique ownership, structured cancellation, and atomic publication.
Independent Python integer-oracle bytes are retained as permanent test vectors.

Run focused tests with:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --test-arg=pnm_tiles_30
```

The harness executes the actual entry, modules and capability projections.
`FRICTION.md` records original failures, per-engine/replay results and incomplete
obligations. A tooling Steps stop has no completed semantic observation. The
header-cap/one-over reference cells retain 1M steps and cap-invert retains 16M;
these bounds have not been increased. Raw tapes, seals and receipts live outside
the repository in `/workspace/code/hirsel-briefs/pnm-tiles-30/`.

The optional production width driver in
`crates/hirsel-cli/tests/support/pnm_tiles_30/qualify.py` accepts `--app-root`
(the retained native test directory containing both binaries, sources, capped
helper and cases) and a fresh external `--root` for receipts. It runs ordinary
Ordered success/domain/cancellation cells at widths1/2/4, samples settled control
readiness, and waits for an armed SIGINT handler before sending a cancellation.
It uses no scheduling hooks or sleeps. These are capacity receipts, not proof
of simultaneous hardware execution. Each signal tape replays against its own run;
externally delivered signals need not occur at identical logical points.

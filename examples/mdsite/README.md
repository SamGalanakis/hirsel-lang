mdsite renders safe Markdown with the shipped `std.markdown` renderer. No arguments keep the original `data/content/` to `data/site/` tree and HTML. The explicit commands add configuration, navigation, copied assets and a verified incremental manifest:

```sh
cd examples/mdsite
hirsel run hirsel.json --grants grants-plan.json
hirsel run hirsel.json --grants grants-build.json
hirsel run hirsel.json --grants grants.json
hirsel test hirsel.json
```

The templates use a `data` host directory relative to the working directory. `grants-plan.json` grants read access; `grants-build.json` grants read/write access. For a repository-root launch, make a private copy of grants.json with `io.dirs[0].host` set to the absolute path of `examples/mdsite/data`. The virtual mount remains `data`. Both commands resolve the configuration path within that mount. The included `data/site.toml` is a complete configuration:

```toml
content = "content"
assets = "assets"
output = "site"
title = "Sheep & goats"
jobs = 4
```

All five fields are required; unknown fields are rejected. Roots are canonical relative paths, disjoint by ancestor relationship. Pages retain their content-root prefix and append `.html`; assets use paths relative to the asset root. The first `# ` line supplies a page title, otherwise the source path is used. Titles and link attributes are HTML-escaped, link segments are percent-encoded, and raw HTML and unsafe Markdown destinations remain omitted. Navigation uses paths relative to each page.

The application admits at most 64 pages/assets combined, each at most 64 KiB, total admitted config/page/asset bytes at most 1 MiB, input and complete published paths at most 256 UTF-8 bytes and 8 components, titles at most 256 bytes, config at most 4096 bytes, and jobs 1–4. Discovery also caps directory listings at 64 returned entries and 128 visited entries per input tree. It rejects symlinks/special entries. Exact staged output including manifest must fit 2 MiB before publication. These are domain errors (exit 2), distinct from engine tooling limits. Current API limitations on allocation before returned listings/Markdown strings are recorded in FRICTION.md.

`plan` performs discovery, rendering and hash verification without writes. `build` reads/hashes all admitted inputs, processes bounded task batches, returns owned results, sorts outputs by UTF-8 path and commits them individually with AtomicFile. The manifest `.mdsite.json` is last. It stores config SHA-256 and input/output SHA-256 pairs in a path-keyed JSON object. Outputs are skipped only when both the new hashes and the on-disk output hash match. Layout/navigation changes therefore invalidate affected pages. Two builds report skips; a page edit changes its content and navigation where applicable. Stale paths from the previous manifest are reported and retained; unrelated files are never deleted.

AtomicFile protects each file independently. A publication failure may leave a prefix of updated files with the old manifest; the next build verifies and repairs that state. There is no multi-file atomicity or crash-durable transaction claim. No-argument compatibility output has no incremental manifest.

Production recording from the repository root, with a matching installed toolchain:

```sh
python3 - <<'PY_GRANTS'
import json
from pathlib import Path
p = json.loads(Path("examples/mdsite/grants-build.json").read_text())
p["io"]["dirs"][0]["host"] = str(Path("examples/mdsite/data").resolve())
Path("/tmp/mdsite-grants.json").write_text(json.dumps(p))
PY_GRANTS
hirsel build examples/mdsite/hirsel.json --backend llvm -o /tmp/mdsite-llvm
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 /tmp/mdsite-llvm --hirsel-record /tmp/mdsite.tape --hirsel-grants /tmp/mdsite-grants.json build site.toml
/tmp/mdsite-llvm --hirsel-replay /tmp/mdsite.tape
hirsel run examples/mdsite/hirsel.json --replay /tmp/mdsite.tape
```

The private grants args must be `["build","site.toml"]`; use Ordered `tasks.cooperative@1`, stdout, one concrete data mount, and placement `{ "cores": 1, "commit": "shared", "reorder": "off" }`. Widths 2/4 select the same logical jobs setting. Keep tapes outside mutable mounts. Requested placement width alone does not prove effective parallelism.

Modules: main coordinates capability effects and task ownership; catalog discovers/admit inputs and titles; render composes layouts through std.markdown/html; assets reads bounded inputs and verifies digests; publish validates/encodes manifests and owns atomic writes; model defines typed policy/results. The generic WorkResult is instantiated for pages, assets and rendered output, with nested unique payloads crossing tasks.

References: Go filepath.walkDir path.go:319–346 and path_test.go:532–562,630–685 at ebe2feefc400f813aefdc8a036e66d2619da6649 (BSD-3-Clause, third-party/NOTICES.md): visit sorted direct children before descent and propagate errors. Deviation: explicit bounded stack, application caps, rejected symlinks and post-discovery global sort. CommonMark.js html.js:95–136 and spec.txt:8780–8798 at 898886961b49c1df1175666d03c3ac66c9cc9040 (BSD-2-Clause): reuse shipped renderer; fixed autolink ampersand vector and safe-default policy. No Markdown parser or renderer is copied into the application.

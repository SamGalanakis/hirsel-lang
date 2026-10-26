# filemanifest authoring findings

Scope: first implementation on frozen origin/main `263bd5d80a8a6d908d5d890fa340aaa2a2322da9`.
No compiler, runtime, std or shared test runner changes are made. Source check
and reachable TIR export are separate from execution. Evidence, original sources,
JSON diagnostics, pinned reference hashes and attempts are retained outside the
repository at `/workspace/code/hirsel-briefs/filemanifest-23/`.

## Eager traversal behind an iterator (about 15 minutes; owner-routed)

The intended composition was `glob.compile/is_match` over `file.walk`, counting
all yielded paths before selecting and retaining matches. `glob.files` eagerly
collects matches, so it cannot enforce the application's visited count first.
Reading the actual current provider showed a second eager boundary:
`crates/hirsel-rt/src/io.rs` `Request::Walk` calls `storage.walk`, and
`io_real.rs` `DirFd.entries` builds and sorts complete file/directory vectors
before the iterator exists. It follows confined in-root directory symlinks and
rejects active-ancestor cycles. This is not a claimed compiler diagnostic.

Minimal witness (complete package retained at
`/workspace/code/hirsel-briefs/filemanifest-23/walk-witness/`):

```hirsel
pub type AppError = Grant(GrantError) | File(FileError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let d = io.dir(rt, path.of("fixtures"))?
  let paths = file.walk(d)?
  for step in take paths {
    let _ = step?
    break
  }
  Ok(())
}
```

Even this first-item consumer traverses before entering the loop. A dangling,
escaping or cyclic link elsewhere can therefore fail before the first item.
The acceptance harness preserves these cases, plus 128/129 nonmatching paths,
which would expose a counter incorrectly applied only to matches. `file.read_dir`
is also eager per directory, so replacing the walk with it would not establish a
strict host bound. Directories are not returned and cannot be charged here.

Working application policy: cap yielded paths, selected records, path bytes,
depth, pattern bytes and digest/input bytes; validate every yielded path, check
all components and leaf metadata before each selected open, and preserve Dir
confinement. The README explicitly states the provider's traversal/memory are
outside these caps. Metadata followed by open is not atomic against concurrent
replacement. Empty aliased directories may never be yielded, and unselected
in-root links can be tolerated. No universal no-follow guarantee is claimed.

Owner reply to message 630092 approved proceeding with the honest limitation;
streaming walk/read_dir and explicit link/cycle policy belong to a separate lane.
No language-rule change or shared runtime patch was attempted.

A cheap 1×/2×/4× first-item witness used 16/32/64 empty regular files under
`tools/capped --timeout 60`, 2 GiB, one CPU: observed scope peak RSS was
107/101/102 MiB, status 0 for all. Source/compiler setup dominates those tiny
trees, so this establishes no memory-scaling claim. A tree with `a` and `z -> .`
returned status 1, `error: File(FileFault(1))`, before it could yield `a`;
peak was 102 MiB. All four private trees were checked unchanged then removed,
with cleanup asserted. Logs are `walk-witness/run-{16,32,64,cycle}.log` in the
external evidence directory. This preserves the original failing cycle cell;
regular-tree success does not make that cell pass.

## Unique records through a helper (about 5 minutes; intentional rule)

First attempt `main.hir:67:3`, complete `list.sort(mut entries,
model.entry_order)` call, failed MODE-2: “`entries` is not proved deeply unique
on every path, but a unique value is required”. The helper's signature was
`hash_file(...) -> Result[model.Entry, model.InvalidInput]`. Although its body
constructed an owned path/digest, that signature did not promise uniqueness.

Minimal form:

```hirsel
fn make() -> model.Entry {
  model.Entry(path: "a", size: 0, sha256: "")
}
// let mut entries: List[model.Entry] = []
// list.push(mut entries, make())
// list.sort(mut entries, model.entry_order) // requires deep uniqueness
```

Fixed by private summaries (b552ff23d839 / f71ba1242e00): the private helper body proves its fresh Result payload without a result annotation. The original MODE-2 guidance pointed to the push input; its diagnostics remain in `attempt-01/check.log`.

## A duplicate-path set shares the decoded name (about 8 minutes; intentional rule)

First `model.hir:88:3` sort and `:89:3` return failed MODE-2 after
`set.insert(mut seen, copy name)` shared a name also moved into the record.
Trying `set.insert(mut seen, unique name)` moved the local: attempt 02
reported OWN-2 at `model.hir:77:101`, the later error-path use of `name`,
with the move at `:76:33` and folded notes for other uses.
Trying `unique copy name` avoided that move but still did not prove the complete
record pushed through `list.push` deeply unique (attempt 03 MODE-2, same sort
and return sites). That attempted repair is retained; it is not presented as a
successful improvement.

Working repair:

```hirsel
set.insert(mut seen, copy name)
// ... finish validation ...
list.push(mut entries,
  unique Entry(path: take name, size: size, sha256: take digest))
```

The final `unique Entry` explicitly copies bounded counted fields independently
of the set and JSON DOM. It is extra allocation and stays in the final source.
No attempt was made to loosen sort's signature or alter the checker.

## Capability, path and rendering plumbing (about 20 minutes; application policy)

The first complete attempt already used these documented recipes, so there was
no compiler diagnostic to repair:

- Pass `d: Dir` and `mut handle: File of d` together; keep mutable total byte
  state explicit. Helpers never return a branded handle or erase its brand.
- `path.text` is a borrowed path spelling. `hash_file` constructs `unique name`
  for an independently owned report path; `invalid` likewise copies kind/path
  so error reporting does not retain the source or a temporary path.
- Build parent names explicitly in a bounded List[u8] because metadata checks
  must precede selected opens. `text.finish_valid(copy prefix)` prepares a
  spelling; the bounded copy-on-write builder can continue after that call.
- Explicit `Result` matches map distinct file/grant/pattern errors into a typed
  `InvalidInput` with stable kind/path. Expected absence alone is `missing`;
  metadata/permission errors never masquerade as a missing file. No generic
  error wrapper/framework was added.
- Render fixed-key JSON with `json.write_text_into`; this avoids handwritten
  string escaping and makes stdout deterministic. `process.ExitCode` expresses
  mismatch 1 and invalid-input 2 without relying on an Error return's stderr
  presentation. Missing output grants return 3.
- Bound manifest bytes before `json.parse`, then validate/count records before
  constructing the application list. The parser's DOM is bounded by bytes,
  rather than claimed to have the application's 64-record ceiling.

These are useful app-level compositions, not evidence of missing std APIs.

## Build and registration setup (about 5 minutes; tooling)

Two shell-background setup attempts exited before execution and produced empty
logs; neither is counted as a build or test. The subsequent tracked command
completed prepare/sync and built the CLI and matching image. New integration
inputs use explicit include_str calls, following archiveaudit, so the remote
harness carries model, manifest, vectors, fixture files and capped wrapper.

The initial modules.rs registration inserted a blank line. The shepherd flagged
its independent-line-list violation before land; removed only that blank line
after both active tests passed, then ran tools/check-hot-files. This metadata-only
repair does not alter tested executable inputs.

## Verification status

The final evidence summary below records actual execution counts and outcomes.
A clean enumerated scenario does not establish universal ergonomic coverage.

The final source check passed and reachable TIR export succeeded. Focused Buck2
execution ran 2/2 tests (0 failed, 0 ignored): checked H exercised all 30 Go
match vectors, empty/abc SHA-256 and model controls; the actual CLI/native
harness passed 83 cases × H/LLVM/Cranelift = 249 observations, including exact
stdout/stderr/status, fixture modes/content/link preservation, outside-grant
sentinel preservation and explicit temporary cleanup. Test times were 1.819 s
and 204.376 s; only those two cost rows were added. The original first source
check failed with 3 MODE-2 diagnostics; the accepted source check has 0.
No failing engine cell was replaced with a reduced success.

## Streaming walk resolution (2026-10-23)

The historical eager-walk observations above describe the original provider.
`2574f29ab368107406a88289e897ea3b2cb6edbb` replaced it with a demand-pulled
cursor. Root construction reads one directory; descendants are read only as
iteration reaches them. Files and links are yielded in global UTF-8 byte order;
links are never resolved or descended. The app's existing loop already counts
results before selection and needs no relocation or eager wrapper repair.

The follow-up `filemanifest_stream_25` tests freeze 0/127/128/129-path controls,
an unread later failing subtree, and errors discovered before/at the lookahead.
The 129th result produces `visited_limit` before inspecting that result, even
when it is a traversal error; an earlier error is `walk`. Directory reads needed
to find the extra result are permitted. Exact output and directory-read/open
projections are checked against independent fixtures on the reference interpreter,
checked H, LLVM and Cranelift. The test fixture manifest records walkdir's
root/directory/order adaptations and the simulator/host fault encodings.

Retained workarounds: `glob.files` still collects matches, so the app uses
`glob.compile/is_match` over the cursor; the cap counts yielded results rather
than directories. Cursor pages are bounded by active depth and maximum directory
width, while consumed Path identities and recorded inputs may remain retained.
Selected paths still use separate metadata and open operations, without an atomic
no-follow snapshot guarantee. The unique-record, bounded-copy and capability
plumbing findings above remain applicable.

Streaming documentation and permanent cap/effect coverage landed in
`dfb8bfab3ddac7f415d1a4939b9cc475388d7c95`: six new bounded partitions prove
32 app observations across four engines; the existing 83-case app harness and
two cursor controls also pass. The cap loop and provider remain unchanged.

## Reusable overflow admission (2026-10-24)

`stream.bound(stream.from_results(take paths), model.VISITED)` replaces only
the manual visited counter and overflow check. It stays before path validation
and matching, maps Source to `walk` and Limit to `visited_limit`, and consumes
one demanded extra result even when that result is an error. Successful scans
drain through EOF; early application errors do no hidden lookahead. The lazy
compile/is_match composition, path checks, selected-record cap and separate
metadata/open snapshot limitation remain as described above.

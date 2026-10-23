# depplan first-contact findings

Frozen starting main: `f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`.
Scope: a named, bounded JSON dependency planner, typed records and error reports,
borrowed map lookups, mutable map/set/list builders, qualified std toposort,
reachable TIR export and interpreter/LLVM/Cranelift CLI runs. Raw sources,
manifests, hashes and JSON diagnostics for attempts 01–04 are retained outside
the repository under the lane's evidence directory. Times below are approximate
active investigation, not performance measurements.

The first attempt was saved before any correction. No checker, language rule,
shared runner or std implementation was changed. These findings describe this
program's enumerated scenarios, not universal ergonomic coverage.

| Attempt and minimal form | Actual diagnostic / span | Repair and disposition | Time |
| --- | --- | --- | --- |
| 01: `let mut result = Report(...)` | LANG-14: “`result` is reserved for the returned value in a postcondition; choose another name”; `model.hir:94:11`, bytes 4249–4255, identifier `result`. | Rename to `report_value`, including its field writes. Intentional reserved-name rule; final code retains this rename. | 1 min |
| 02: `out.print(output, encoded + "\n")` | LANG-7: “operator is not defined for this type”; `main.hir:40:38`, bytes 2019–2033, `encoded + "\n"`; repeated at `48:36`, bytes 2472–2486. | Use `f"{encoded}\n"`. String interpolation follows the documented formatting API. Intentional rule; final code retains interpolation and its temporary output string. | 1 min |
| 02: `string.from_utf8(data)` | LANG-4: “unresolved module member `string.from_utf8`; looked for core@1/string/from_utf8, std@1/string/from_utf8”; `model.hir:26:18`, bytes 1499–1515. | Use the shipped `string.from_bytes(take data)`. Mistaken API spelling, not a std gap. | 1 min |
| 02: `json.member(value, "nodes")`; `json.element(nodes_value, i)` | LANG-4: unresolved `json.member` at `model.hir:28:27`, bytes 1648–1659, and `34:22`, bytes 1993–2004; unresolved `json.element` at `33:22`, bytes 1912–1924. | Use the separate documented `json_access` module. Caller error; no new accessor module or shared helper. | 1 min |
| 02: `json.array_len(nodes_value)` | LANG-4: unresolved `json.array_len`; suggested `array_next`; `model.hir:29:21`, bytes 1727–1741, repeated at `35:23`, bytes 2081–2095. | Use `json_access.as_array` then `list.len`, with explicit shape rejection. Caller error; the spelling-based suggestion is unsuitable for this intent, but is not treated as a demonstrated compiler defect. | 1 min |
| 02: `let mut ids: Map[String,u64] = ...; ... Error(Blocked(ids)) => ...` | LANG-4: “a local cannot shadow another local”; pattern `ids` at `model.hir:84:19`, bytes 3974–3977; earlier local at `51:11`, bytes 2684–2687. | Rename the blocked pattern to `blocked_ids`. Intentional no-shadowing rule, including match patterns. Final code retains the rename. | 1 min |
| 02: derive `Manifest { nodes: List[Node] }` without a `node_from_json` helper | LANG-4: “unresolved name `depplan@0/model/node_from_json`”; generated decoder `14:29`, bytes 546–576, with secondary span at the generating `comptime` body (`model.hir:18:87`, bytes 911–953). | Add the explicit `node_from_json` derive body. Documented nominal hook convention; final code retains this extra helper. The error does point back to its generator. | 2 min |
| 03: `let Ok(sink) = io.stderr(rt) ...; out.print(sink, f"{encoded}\n")` | CAP-1: “the operation requires its explicit typed capability argument”; `main.hir:48:20`, bytes 2456–2487, complete `out.print` call. | Use `err.print` for `Stderr`. Intentional capability distinction; the diagnostic could be more specific about the wrong output capability, but no shared diagnostic patch is justified here. | 1 min |
| First host launch: grant `host: "examples/depplan/fixtures"` | Launcher exit 3: `{"kind":"invalid","field":"io.dirs.host","detail":"examples/depplan/examples/depplan/fixtures: No such file or directory (os error 2)"}`. There is no source rule/span for this launch-config error. | Change host to `fixtures`; it is resolved relative to the grants template directory. Final read-only grant template retains that spelling. Intentional launch policy. | 1 min |

Other explicit adaptations retained in the final program:

- Empty builders carry `List[u8]`, `List[toposort.Edge]`, `List[String]`,
  `Map[String, u64]` and `Set[u64]` annotations, and counters use `0u64` or an
  annotated constant. This is the documented prefix-inference rule (LANG-2),
  not a failed inference experiment. Expected enum types at function returns
  and matches select bare constructors. Approximately 2 minutes to apply.
- Borrowed names cannot become independent map keys, error payloads or output
  lists by accident. Explicit `unique name`, `unique prerequisite`,
  `unique manifest.nodes[id].name` and the report payload copies detach those
  strings/lists. The input byte builder is moved into `bytes.from_list`, then
  requested `unique` for the decoder's declared boundary. These ownership
  choices are visible in the final code; no unsafe escape or rule change.
  Approximately 2 minutes to write and check.
- Derived typed JSON decoding has explicit node/manifest hooks and plan/report
  encoding hooks. A byte-bounded JSON DOM plus collection-length preflight
  precedes typed decoding; the planner repeats caps for direct typed callers.
  This application policy avoids accumulating an unbounded typed list, while
  accepting the bounded DOM allocation and the decode/helper boilerplate.
  Approximately 4 minutes of design and implementation.
- `Invalid` wraps `toposort.GraphError` for unexpected std failures and has an
  application `Blocked(List[String])` arm. Dense blocked IDs are translated
  into owned names; numeric endpoints never escape as user identities.
  `Input(String)` covers I/O/usage codes and is used at `?` propagation
  boundaries. These wrappers are application policy, not std gaps.
  Approximately 2 minutes.
- The CLI explicitly forwards `Io`, derives `Dir`, passes that directory with
  its branded file handle, and separately obtains `Stdout` and `Stderr`.
  It returns `process.ExitCode` after writing its own typed error report,
  avoiding an additional implicit launcher error. Help only needs stdout;
  missing output authority returns 2. This is documented capability/exit
  plumbing, retained rather than adding process authority. Approximately
  3 minutes.
- Python's implicit node insertion would change ready ties when a prerequisite
  is declared later. The independent oracle first declares every input name,
  then adds prerequisites. A separate precedence check and a transitive-closure
  blocked oracle keep the expected answers independent of the shipped Kahn
  implementation. This is a deliberate reference adaptation, not a Hirsel
  workaround. Approximately 2 minutes.

No language or std gap has been demonstrated in these source-check scenarios.
Engine and acceptance results are recorded separately after the focused tests;
source checking alone is not evidence that a native engine executed the program.

Validation: the final focused selection executed **2 tests, 2 passed, 0 failed**:
39 cases on each of interpreter/LLVM/Cranelift plus one repeated release per
engine (120 runtime processes), and a pure checked-heap entry covering success,
blocked downstream names and unknown references with zero live heap bytes.
The first focused run also passed both tests, but local harness/template edits
overlapped its remote execution; it is retained as setup evidence. The final
run began after all consumed Rust/Python/source/fixture/grants edits; only this
file and README prose changed during it. Edit times, final executable-input
hashes and both unmodified logs are retained externally. Cost metadata was
merged from the final run without repeating unchanged execution cells.

## API discovery resolution

The [tested std API recipes](../../docs/std-api-recipes.md) address the API-search
detour recorded above: `json_access.member`/`element`, contextual `Text(String)`,
core `string.from_bytes(take data)` and manifest-registered `time.format`.
The original attempts remain historical evidence. The production calls, required
annotations, ownership copies and application adapters remain valid. This is
documentation navigation for existing APIs, with no alias or resolver change.

Addressed by the executable recipe documentation landed in `36879d8e307bb032944a618df036327bff837fe5`.


## Relative-host diagnostic guidance resolved (round 24)

Landed in `231d4880e4e6acd59b4da5d1f95ea7c02156f363`. The historical relative-host refusal above is retained.
A missing relative host now keeps `kind:"invalid"`, `field:"io.dirs.host"` and
the OS cause while naming the original host, grants-file directory and attempted
resolved candidate. It explains that relative host paths resolve relative to
the grants file. The checked-in `host:"fixtures"` and all grant/confinement
semantics stay unchanged. See the
[shared grants-base recipe](../../docs/native-verification.md#resolve-directory-hosts-beside-the-grants-file)
for a private bad/correct pair launched from another working directory.

Focused shared-parser/CLI tests cover the diagnostic, cwd independence,
relocation, nested runner mounts, escaped paths and authority controls. This
resolution does not claim that this example's native application matrix was
rerun.

## Nested JSON diagnostic resolution (2026-10-23)

Landed in `66a5b0288e76496d525e829f2db8fefc0cc614a8`: ordinary JSON decoding now labels
the enclosing `Manifest.nodes` source field when `node_from_json` is missing
or has the wrong contract. The label gives the canonical helper path and
directional borrowed-argument signature, alongside the original rule, generated
failure span and comptime origin. The explicit `node_from_json` helper remains
necessary and is retained. See the [complete nested JSON helper recipe](../../docs/spec/std-library.md#nested-json-helpers).
The earlier failed attempt above records the historical diagnostic unchanged.

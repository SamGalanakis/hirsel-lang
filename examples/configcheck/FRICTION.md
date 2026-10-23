# configcheck authoring friction

Scope: this complete TOML/YAML service validator, its upstream parsing vectors,
CLI/grant controls and bounded three-engine fixtures. Frozen launch main:
`f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`. First sources and manifests, each
failed attempt, exported JSON diagnostics, toolchain/source hashes and engine logs
are retained externally in the configcheck-23 evidence directory. No compiler,
language-rule, shared runner or std implementation was changed.

## Reserved local (about 1 minute; intentional rule)

Attempt: `let mut result: List[yaml.Value] = []` in the TOML array adapter.
Actual LANG-14 at original `model.hir:49:15-21`:

> `result` is reserved for the returned value in a postcondition; choose another name

Repair: rename the list/map accumulator and all uses to `converted_items`.
The diagnostic suggested `value_2`; the more descriptive name is application-owned.
No diagnostic issue was inferred from this rejection.

## A copied borrow is still local (about 3 minutes; intentional MODE-1)

Five independently folded MODE-1 diagnostics followed the rename. Original
attempt-02 sites and minimal forms:

| Site | Attempt | Working repair |
| --- | --- | --- |
| `model.hir:20:26` | `fn field(at: String, kind: String) -> ConfigError { Field(FieldError(path: copy at, kind: copy kind)) }` | `unique at` and `unique kind` give the report independent owned strings. Both copies remain in final code. |
| `model.hir:40:24` | `fn from_toml(value: toml.Value, ...) -> Result[yaml.Value, ConfigError] { ... Text(s) => Ok(Text(copy s)) ... }` | Declare `take value`, pass `take parsed`, explicitly copy recursive child values into taken parameters. No promise that `copy` makes a borrowed value global. |
| `model.hir:100:23` | `fn required(entries: Map[String, yaml.Value], ...) -> Result[yaml.Value, ConfigError] { ... Some(value) => Ok(take value) ... }` | Declare the result `in entries`, keeping its borrowed origin visible. |
| `model.hir:112:6` | `fn name_value(value: yaml.Value, ...) -> Result[String, ConfigError] { ... Ok(copy s) }` | Declare the result `in value`; the owner stays live while the checked name is used. |
| `model.hir:133:33` (also 140:36) | `fn validate(value: yaml.Value) ... option.unwrap_or(map.get(entries, "retries"), Integer(3))` | Take the root value and use `unique map.get(...)` before the global-taking Option helper, for both retries and upstream defaults. The explicit deep copies remain. |

Actual message for each returned value:

> this operation requires a global value: `<parameter>` is local to the caller, but the result must be global

For the last site:

> this operation requires a global value: `value` is local to the caller, but `option.unwrap_or` requires a global argument

The adapter diagnostic also folded a `map.insert` global-argument obligation into
its notes. The diagnostics explain that `copy` shares local blocks and suggest
taken parameters or independent `unique` copies. Repairs follow those documented
rules. The result relations, root `take`, recursive `copy` calls and Option copies
are all retained; they are not hidden behind a helper library.

## Grant path base (about 1 minute; documented launcher behavior)

The first checked-in template used host `examples/configcheck/fixtures`.
Execution refused before entry with exit 3:

> {"kind":"invalid","field":"io.dirs.host","detail":"examples/configcheck/examples/configcheck/fixtures: No such file or directory (os error 2)"}

Repair: host `fixtures`, relative to the grants file. The README explains this
base. Integration tests use private absolute host paths. This setup failure was
not called an application rejection or an accepted interpreter cell.

## Standalone native environment (about 2 minutes; setup recipe)

The initial external LLVM build omitted the pinned native environment. Exit 2:

> hirsel: native link: LLVM_SYS_231_PREFIX must name an absolute toolchain input

The Buck2 test launcher supplies the pinned absolute toolchain and runtime archive
environment; the focused harness uses that recipe. The failed external build is
retained as a setup failure, not a failed configuration fixture or native pass.

## Upstream fixture expectation (about 3 minutes; test-oracle correction)

The first executed focused run ran two tests and both failed, with the pure entry
returning `error: empty child`. The full quoted-dot fixture has eleven lines; the
planning excerpt ended at line eight, `[table.withdot]`. The attempted assertion
`map.len(withdot) == 0` was wrong for the retained full file. Its unchanged upstream
JSON oracle expects three children: `plain = 5`, `key.with.dots = 6`, and
`escaped.dot = 7`. The original source and failed run are retained externally.
Repair: assert those exact upstream values; also inspect every nested YAML record,
coordinate and power value. No parser workaround or shared implementation fix.
The original failed cells remain failures; acceptance requires the corrected
complete entry to execute on every engine.

## Additional explicit plumbing retained

- Prefix typing: empty Lists/Maps, YAML parser-boundary values, node counters and
  chunk sizes have explicit types. Integer narrowing occurs only after range
  validation. These were written explicitly in the first attempt; no separate
  annotation rejection was observed (about 2 minutes; intentional language rule).
- `ConfigError` has TOML/YAML payload arms for one-level error injection; the I/O
  layer wraps it once as `InputError.Config`. `AppError.Output` preserves failed
  writes. This is the documented error-union recipe (about 2 minutes; intentional).
- Three named unary mappers in the pure reference entry intentionally replace
  parser/config errors with a fixed test failure string. The app itself preserves
  parser variants and positions (about 1 minute; intentional LANG-8).
- TOML's and YAML's value algebras differ. A bounded application-local adapter
  converts TOML to the YAML parser-boundary algebra before shared typed validation;
  datetime has an explicit unsupported-value error. This creates an intermediate
  tree. No std conversion API or speculative abstraction was added (about
  5 minutes; application adaptation).
- The app acquires stdout explicitly and returns the existing `process.ExitCode`.
  Missing directory access becomes a typed report; missing stdout returns 3.
  No write/process/socket/clock capability is requested (about 2 minutes;
  intentional capability plumbing).
- The harness runs a tiny subprocess wrapper inside the unchanged `tools/capped`
  so cap telemetry and application stderr have separate files. It checks real
  status, exact streams and input hashes, then removes its private inputs
  (about 5 minutes; test plumbing).

The scenarios above establish only the documented local experience. They do not
claim universal ergonomic coverage or infer engine acceptance from source check.


## Executed acceptance

The corrected focused run executed both registered tests: 2 passed, 0 failed,
0 ignored. Its CLI matrix checked 70 cases × 3 engines = 210 observations, plus
three complete upstream-vector executions. Plain and checked observations agree;
the existing H heap runner also agrees and ends with zero live heap bytes.
Source check, reachable TIR export, exact output/status/stderr, grants, bounds,
input hashes and private-directory cleanup all passed. Before correction, the
first focused run executed the same two test identities: 0 passed, 2 failed on
the fixture expectation described above. New test count before this package: 0.

No std/compiler defect or routed issue remains from these enumerated scenarios.
The documented copies, result relations, error wrappers, renamed accumulators and
capability/environment plumbing remain part of the working recipe.


Landing metadata (under 1 minute; intentional tooling rule): the independent
`modules.rs` list checker rejected the blank line before the new `mod` row with
`expected one independent mod declaration`. Removed that blank line; registration
is still in `modules.rs`. No behavior or test selection changed.


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

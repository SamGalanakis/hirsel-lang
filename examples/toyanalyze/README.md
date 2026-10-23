# Toy analyzer

A bounded, static language analyzer written in Hirsel. Its toy language is separate
from Hirsel: `let name = expression;`, `{ statements }`, `import module;`, decimal
integer and boolean literals, names, `module.export`, parentheses, `* / + -`,
ordered comparisons and equality. Precedence follows that order; binary operators
associate left. Arithmetic/ordering require integers; equality requires equal
types. No evaluation, integer arithmetic overflow, functions or classes. Identifiers
are ASCII letters/underscore then letters/digits/underscore. `//` comments end at
newline. Each block allows shadowing; declarations are visible during their own
initializer, producing a self-initializer error. Every module top-level let exports
its inferred type. Imports use only manifest-declared names. Unknown types suppress
cascading type errors. Failed syntax recovers at statement boundaries.

```
hirsel run examples/toyanalyze/hirsel.json --grants examples/toyanalyze/grants.json
hirsel run examples/toyanalyze/hirsel.json --grants examples/toyanalyze/grants.json -- session modules.json 2 session.json
```

Manifest: `{"modules":[{"name":"base","path":"base.toy"}]}`. Paths resolve beneath
the explicit `input` Dir. No directory scanning. Source reads and pure analyses use
structured batches of at most JOBS (1–4); diagnostics are independent of task completion
order. `session` first checks, then applies `replace`/`check` commands. Replacement is
in-memory and takes a newly parsed AST. Changed content hashes invalidate the module
and every transitive importer; unchanged replacements retain summaries.

Limits: 16 modules, 8192 bytes/source, 131072 aggregate source bytes, 256 bindings/module,
24 AST/block depth, 32 replacements/session, 65 commands, 128 retained diagnostics
with uncapped total. Manifest is bounded at 8192 bytes and session/cache at 300000.
File readers request at most the remaining cap plus one. Module/path duplicates
are rejected. These bounds are application policy, distinct from toy diagnostics.
Diagnostic spans are zero-based half-open byte offsets. Within each module reports
sort by start/end/code; modules follow manifest order. Exit 0: clean; 1: toy-language
diagnostics; 2: resource/schema/I/O/usage refusal; 3: missing stdout.

Optional final CACHE argument selects a path under a separately granted read-write
`output` Dir. Schema 1 stores module content SHA-256 and exported type summaries.
Existing cache syntax/version/shape is validated; summaries are recomputed before
atomic replacement, so stale or forged content cannot authorize semantic reuse.
Cross-launch cache acceleration is deliberately absent. Incremental session reuse
is real and separate from this advisory persistent format.

`--ready` before the command prints `ready` and consumes one byte from explicitly
granted stdin before processing; hosts can use this admission handshake to inspect
settled worker capacity without introducing timing assumptions.

Modules: main (capability and CLI orchestration), syntax (bounded token views/parser),
symbols (declare/define scopes), analyze (pure type/diagnostic decisions), incremental
(dependencies, hashes and summaries), report (stable JSON and cache encoding).

Prior art: Crafting Interpreters 4a840f70 Resolver.java:320–406 and negative variable
vectors (MIT); CPython 010bebbf Lib/graphlib.py:86–136 and test_graphlib.py:41–130
(PSF-2.0). See LICENSES.md. Static undefined names replace Lox runtime failures;
functions/classes are omitted; bounded dense ready scans replace graphlib's hash
queue; dependency names must be explicitly declared. Shipped std SHA-256 is reused.

Verification status and current limitations are in FRICTION.md; static acceptance
alone does not qualify execution engines or replay.

An optional manifest field `"ready":"admitted"` writes `ready` to that relative
path under an explicitly granted writable `output` Dir after the first bounded
source batch is admitted. It announces admission, not a clean language check.
The loader validates the batch's declared metadata/size before writing. This
explicit application receipt can release an external producer; no output grant is
needed when the field is absent. `AfterWrite` fixture gates exercise a read that
actually parks before this receipt is written. Receipt denial cancels and joins
the pending batch. Absolute/traversal paths and nontext fields refuse before load.

Focused verification:

```
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=toy_analyzer_31 --test-arg=--test-threads=1
```

Physical worker qualification uses the separately ignored host identity:

```
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --test-arg=toy_analyzer_31_production_ordered --test-arg=--ignored
```

That run needs four admitted CPUs. Every native application process stays under
60 CPU seconds/2GiB with the same four-CPU wrapper allowance. Logical jobs stays2;
requested widths1/2/4 are qualified by a `running` control reply with effective N
and pending0 after the stdin readiness handshake. This is executor capacity, not
proof of simultaneous hardware execution. The normal pool test cannot promise
four admitted CPUs and intentionally does not run that physical qualification.

For a standalone build and run (instantiate a private grants file with absolute
input and optional output roots first):

```
hirsel build examples/toyanalyze/hirsel.json --backend llvm -o /tmp/toyanalyze-llvm
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 /tmp/toyanalyze-llvm --hirsel-record /tmp/toyanalyze.tape --hirsel-grants /tmp/toyanalyze-grants.json check modules.json 2
/tmp/toyanalyze-llvm --hirsel-replay /tmp/toyanalyze.tape
```

Keep recordings outside every granted writable directory. Cache-enabled invocations
append `cache.json`; session invocations use `session modules.json 2 session.json`.
Runtime/native tool inputs must match the built CLI (the Buck2 launcher supplies them).

# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Used `distinct` and `result` as ordinary names. | [Exact output](repros/incident-1.txt) | Renamed them `sorted_unique` and `output_lines`; LANG-14. [Minimal program](repros/incident-1.hir). | 1 | language friction |
| Considered the LANG-14 replacement suggestion. | [Exact output](repros/incident-2.txt) | Help suggests reserved `result`. Routed bug to @agentlang-24, message 557193; left compiler unchanged. [Minimal program](repros/incident-2.hir). | 0.5 | bug |
| Inferred a list from its elements or the other `if` arm. | [Exact output](repros/incident-3.txt) | Added `List[cli.Flag]` / `List[String]`; LANG-2. [Minimal program](repros/incident-3.hir). | 0.5 | language friction |
| Sorted a list after a mutable helper call. | [Exact output](repros/incident-4.txt) | Declared `@preserves_unique(lines)` on the append-only helper; MODE-2/CLO-2. [Minimal program](repros/incident-4.hir). | 1 | language friction |
| Returned `out.print(..)?` from a Result function. | [Exact output](repros/incident-5.txt) | Print with `?`, then return `Ok(())`; LANG-7/LANG-8. [Minimal program](repros/incident-5.hir). | 0.5 | language friction |
| Read buffered lines inside a collection loop. | [Exact output](repros/incident-6.txt) | Moved the file/stdin loops into separate helpers. [12-line repro](repros/invalid-origin.hir), runnable via its adjacent manifest. Removing the outer loop or reads accepts. Routed @agentlang-24 message 557423; compiler fix out of scope. | 4 | bug |
| Named a local `counts`, also the name of an @test. | [Exact output](repros/incident-7.txt) | Renamed test `count_words`; LANG-4 applies across the entire module. [Minimal program](repros/incident-7.hir). | 0.5 | language friction |
| Took a record field directly. | [Exact output](repros/incident-8.txt) | Destructured the whole Scan with `let Scan(..) = take scan`; OWN-8. [Minimal program](repros/incident-8.hir). | 0.5 | language friction |
| Pushed strings from a taken nested sequence into a unique accumulator. | [Exact output](repros/incident-9.txt) | Borrowed iteration and an explicit `unique line` preserve the accumulator contract; MODE-2. [Minimal program](repros/incident-9.hir). | 0.5 | language friction |
| Expected no implicit text when main returns Error, following the tour. | [Exact output](repros/returned-error.txt): `error: ()`, exit 1. | PROG-3 correctly specifies the launcher report without a stderr grant. Fixed the stale tour sentence; [minimal program](repros/returned-error.hir). | 0.5 | doc gap |
| Registered a new example with dynamically named `repository_manifest` paths, as tasks does. | `called `Result::unwrap()` on an `Err` value: Os { code: 2, kind: NotFound, message: "No such file or directory" }` in the remote harness (five selected tests). | Single-source examples now use `Example::new` with explicit `include_str!` inputs; added the registration note to examples/README.md. | 0.5 | doc gap |

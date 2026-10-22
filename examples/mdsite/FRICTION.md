# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.
Both incidents below are historical and fixed; their captured output is unchanged.

| Tried | Historical diagnostic | Current disposition / repair | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Expected raw HTML from the spec overview, unlike the API index. | [Exact output](repros/incident-1.txt) | The API correctly documents seven options and safe rendering. Fixed the stale overview wording and asserted safe default output. No Markdown rules changed. | 1 | doc gap |
| Compiled `markdown.to_html` natively. | [Exact output](repros/incident-2.txt) | [Minimal program](repros/markdown-native.hir) and adjacent manifest now pass native LLVM. Fixed by [native importer storage/default handling, 0a32703c](https://github.com/SamGalanakis/hirsel-lang/commit/0a32703ca4b6bc31776a024e01d5b2c10d356383); [import regression](../../crates/hirsel-pipeline-tests/tests/rc/rc_import.rs) and [application registration](../../crates/hirsel-cli/tests/examples.rs) retain coverage. | 24 | bug (fixed) |

The native importer incident includes about 2 minutes of direct investigation plus
22 minutes waiting after the other example work was verified. At that time only
patched-toolchain controls passed the minimal native program, all 3 LLVM tests,
the full site run and LLVM replay. The fix has since landed; later dogfood
verification also ran the unchanged minimal program successfully on main's LLVM
backend. The original panic is not a current blocker.

The application retains borrowed views while traversing paths and rendering text,
`unique raw` for UTF-8 ownership, `take source`/`take index` to transfer owners,
explicit Result arms and `Ok(...)` completion. Safe Markdown rendering and HTML
escaping are intentional output contracts. No native workaround remains.

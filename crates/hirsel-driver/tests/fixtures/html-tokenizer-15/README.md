These fixtures port Go x/net HTML's lexical tokenizer and tests at
`7e562009b3884a0845019b5f886f0242a7c0b970`, under the BSD license in
`GO-LICENSE.txt`. `token_test.go:30-743` supplies all 117 `tokenTests` inputs.
The oracle records `Token()` values and the cumulative lengths of `Raw()`
**before** the destructive accessor, giving exact original byte spans.

Reproduce using a checkout at that revision and Go 1.25 or newer:

```
python3 crates/hirsel-driver/tests/fixtures/html-tokenizer-15/generate.py /path/to/x-net /path/to/go
```

With only the upstream path, the script regenerates the two static entity
constants in `html_tokenizer.hir`. With Go, it also regenerates `vectors.hir`:
369 exact cases (117 upstream, 252 additions), including all 2,231 named
references, both CDATA modes, rawtext override and every prefix of selected
state-transition inputs. `caps.hir` is hand-written and appended to the generated
fixture. Regeneration uses a temporary module outside the repository and
preserves no executable or build output. The temporary module lowers only the
upstream module's Go-version requirement to 1.25; no HTML sources are changed.

Semantic adaptations: whole owned UTF-8 input, inclusive byte/token/attribute/work
caps, absolute spans, owned accessor outputs, sorted static entity indexing,
iterative script dispatch, and omission of atoms/namespace metadata. These are
documented in `docs/spec/std-library.md`. All markup recovery, duplicate-key
selection, character references and rawtext behavior tested here come from the
pinned tokenizer; there is no DOM or sanitization claim.

The Rust harness keeps the complete 369-case H/LLVM/Cranelift run and heap
checks separate. Heap cases use fresh ledgers in bounded batches because the
independent oracle retains dead allocation history. The full all-2,231-reference
input is a separate unignored test with its exact original bytes and expected
token; it is not split into smaller entity inputs. Exhaustive work cutoffs for
all five sources run in bounded ranges through each source's first success,
with the same assertions as the full fixture. Every heap run requires zero
live heap bytes. Per-case resident/peak readings are diagnostic evidence only.

Whole case 251 currently exceeds the checked-heap pool memory reservation. Its
unignored `html_tokenizer_15_heap_full_entity_input` test is recorded in
`tools/known-failures.tsv`, owned by `heapcheck-dead-frames-16`; that lane must
remove the entry after repairing dead Frame/Temporary ledger retention while
preserving the independent audit. The complete fixture passes H, LLVM and
Cranelift; the whole-case heap obligation remains open.

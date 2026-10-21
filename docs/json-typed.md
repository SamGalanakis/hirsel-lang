# Direct typed JSON decoding

serde-rs/json 8c535a260ce561eaab4111aafc29038eb9f77ff3 supplies the pull-decoder structure.
- src/de.rs:255-271: pull whitespace scanner.
- src/de.rs:1820-1860: struct token dispatch.
- src/de.rs:1919-2045: stateful SeqAccess/MapAccess comma, key, colon progression.
- src/read.rs:521-566: borrowed unescaped string path, escaped scratch fallback.
- tests/test.rs:1252-1475: record, option and externally tagged enum vectors. The new serde-enum byte twin preserves the existing fixture's semantic expectations.

Deviations:
- Numbers remain exact byte spellings; existing Hirsel numeric parsers enforce the same fitting rules. Short token-only owned text is needed because core Bytes-to-String construction consumes owned bytes and the existing scalar parsers accept String; borrowing/copying the whole document would violate locality or trigger the deferred slice-copy runtime problem.
- TextView is Unescaped(Bytes) or Escaped(String), because converting a borrowed Bytes view through core string.from_bytes requires global owned input. Field matching reads the borrowed bytes directly; typed string fields copy only the token. No runtime ABI change.
- Duplicate decoded keys, including ignored members, are rejected; serde_json normally accepts/replaces duplicates in maps. Known record slots detect duplicates without a hash map; ignored-key sets enforce the dynamic JSON policy.
- Record sequences are rejected, as existing Hirsel Value decoders require objects. Nonfinite float strings and the preexisting leaf-error messages remain Hirsel's policy.
- The existing Value from_json signature stays available. The additive from_json_reader/from_json_bytes entries share numeric/nonfinite fitting and enum diagnostics, avoiding a stringify/reparse cost for dynamic callers.
- Syntax validation reruns only after a typed error, preserving whole-document parser-error precedence. Externally tagged objects check cardinality with a lookahead reader to preserve existing single-key error precedence.
- The byte entry explicitly detaches its typed Result to satisfy locality for recursive types. This currently adds one typed-result graph copy, confirmed in the restart profile; it never builds a JSON DOM. Returning the moved success payload remains MODE-1 for recursive Tree values, so removing the copy requires a separate ownership improvement.
- Native borrowed while-let condition lowering declines a borrowed counted read; generated record loops use an equivalent while true with a let-else inside the body. A minimal native Foo{x:u64} probe is red before this change and green after it; lexical pulls and slot extraction are independently green.
- UTF-8 validation uses Rust core/src/str/validations.rs:123-217 scalar-width and continuation ranges (a7c37f9f0ee5e93396106e9de20f4a86b4154e63), also the accepted-run logic of existing std@1/utf8.decode_lossy. The implementation omits Rust's unsafe aligned-word ASCII loads because Hirsel has byte indexing.

The new `SEM-3-json-reader-import-*` programs compare all 293 existing UTF-8
JSONTestSuite inputs against the unchanged `json.parse`, including exact error
variants and byte offsets. Invalid UTF-8 is tested at the byte entry; raw token
views deliberately stay bytes. The original JSON corpus and derived-Value
expectations are unchanged. `SEM-3-json-typed-{reader,numbers,serde-enum}-run`
cover direct records, every externally tagged variant style, options, maps,
sets, tuples, recursion, fitting boundaries, nonfinite strings, ownership and
syntax precedence.

`bench/micro/programs/extended/json_typed.hir` constructs a document and decodes
it ten times directly; `json_typed_dom.hir` makes the same typed values through
the original DOM path. Both calculate the same checksum as the standalone
serde-derived Rust reference. `bench/micro/typed-session` runs these alongside
the unchanged dynamic JSON row and 10k-task restart in a single seeded,
interleaved ten-round session. See [the measured report](../bench/micro/reports/json-typed-2026-10-21.md)
for counts and residual attribution.

# JSON-domain codec vectors

`tools/generate-json-vectors.py` imports the complete pinned tables below into
`conformance/SEM/SEM-3-{json,cbor,msgpack}-import-*-run.hir`. Each launch manifest
includes `packages/fixtures/json_vectors@1/harness.hir`. Regenerate with:

```sh
python3 tools/generate-json-vectors.py JSONTestSuite cbor-test-vectors msgpack-test-suite --hirsel /absolute/path/to/hirsel
```

The generator checks each checkout's commit. It follows
`tools/generate-yaml-test.py`'s program/manifest generation and Unicode literal
escaping. Eight inexpensive checks share a program; floating-point checks have
their own program to respect the interpreter's one-million-step limit. Inputs
are embedded verbatim, including invalid UTF-8 bytes and the 100,000/250,001-byte
nesting inputs. There is no skip list. Failure output identifies the source case.

| Reference | Revision and source | Imported checks |
| --- | --- | --- |
| [JSONTestSuite](https://github.com/nst/JSONTestSuite) | `1ef36fa01286573e846ac449e8683f8833c5b26a`, `README.md:15-21`, every `test_parsing/*.json:1` | 318 inputs: 95 UTF-8 y, 176 UTF-8 n, 22 UTF-8 i, 25 invalid UTF-8 |
| [CBOR test-vectors](https://github.com/cbor/test-vectors) | `aba89b653e484bc8573c22f3ff35641d79dfd8c1`, `README.md:6-25`, `appendix_a.json:1` | All 82 entries: 57 decode/re-encode, 24 Unsupported, 1 Syntax |
| [MessagePack test-suite](https://github.com/kawanet/msgpack-test-suite) | `e04f6edeaae589c768d6b70fcce80aa786b7800e`, `src/*.yaml:1`, `dist/msgpack-test-suite.json:1` | 56 JSON cases / 194 encodings; 29 unsupported cases / 39 encodings |
| [msgpack-rust](https://github.com/3Hren/msgpack-rust) | `cf880019f72e35a884659a9b29d9eb89b862670b`, `rmp/tests/func/decode/float.rs:17-43`, `rmp/tests/func/decode/{sint,string,map}.rs:1`, `rmp/tests/func/encode/{sint,uint}.rs:1` | 15 additional signed-positive, float-width, nonfinite, UTF-8, string-boundary and empty-map checks |

Each CBOR call names its file, line, table index and wire bytes; each MessagePack call names
the YAML row's file and line. `dist` and `src` contain the same 194 JSON-domain
encodings at this revision. The audit's count of 200 was an overcount. Its 23
out-of-domain CBOR entries excluded the two bignum tags, which are also rejected:
the actual split is 57 accepted / 25 rejected. The legacy RFC 7049
`f818` (simple value 24) entry is malformed under RFC 8949 section 3.3,
so it specifically requires `Syntax`; the other 24 require `Unsupported`. Importing the whole tables includes
all 36 CBOR vectors identified as missing by the audit, plus the other rows.

Accepted JSON inputs are parsed, stringified, reparsed and stringified again;
the two renderings must match. Binary inputs check the decoded JSON value and
byte-exact preferred re-encoding, including for every alternative encoding.
Floating-point values compare by f64 bits, including signed zero, because
Python's reference JSON exponent spelling and Hirsel's shortest spelling differ.
The byte assertion independently checks that a floating-point wire value remains
a floating-point value when re-encoded.

## Policies and deviations

- JSONTestSuite `README.md:19` says y inputs accept. Hirsel rejects the two
  `y_object_duplicated_key*` inputs with `DuplicateKey`: object keys are unique,
  avoiding a silent last-wins policy. These are explicit rejection checks.
- The i inputs permit either outcome (`README.md:21`). Hirsel accepts all ten
  UTF-8 `i_number_*` inputs with lossless number lexemes, rejects malformed
  surrogates and BOM as `Syntax`, and rejects 500 nested arrays as `Nesting`.
  Other negative UTF-8 inputs must yield `Syntax` or `Nesting`. The remaining
  25 files fail `text.finish`, since Hirsel strings must contain valid UTF-8.
- JSON number spellings stay lossless, object insertion order stays stable, and
  nesting is bounded at 128 open containers. These existing policies differ
  from serde_json's default numeric model, sorted objects and depth counter;
  they provide exact numbers and bounded input processing.
- CBOR accepts the JSON value domain. Byte strings, tags (including bignums),
  nonfinite floats, undefined/simple values and non-text map keys yield
  `Unsupported`. Reserved two-byte simple values below 32 are malformed (`Syntax`),
  following RFC 8949 section 3.3. Integer bounds are u64 / -2^64. The source table also includes
  RFC 7049 Appendix A's two bignum examples; they are explicitly checked.
- CBOR indefinite containers decode and re-encode as definite containers.
  Deterministic output uses shortest exact float widths and length/byte key
  order, following ciborium `ciborium-ll/src/hdr.rs:150-166` and
  `ciborium/src/value/canonical.rs:47`. The indefinite streaming string and
  Fun/Amt map have no definite counterpart in the source table, so the
  generator supplies their RFC-defined preferred encodings explicitly.
- MessagePack bin/ext/timestamp inputs yield `Unsupported`, because the JSON
  value model cannot represent them. All 39 encodings of these 29 cases are
  checked. Integers retain integer type even if a float alternative is shorter (the
  -281474976710656 row). They use shortest unsigned encodings when nonnegative, even
  where kawanet lists a same-width signed encoding first (the i64-max row).
- MessagePack floating-point output uses f64 (`cb`), following rmpv's F64
  model; the suite's f32 alternatives widen exactly to f64. The decoder retains
  the widened value, rather than a separate F32 type, because json.Value has
  one numeric lexeme representation. CBOR narrows only in deterministic mode.
- Integral decoded floats now retain `.0`, including negative zero, following
  serde_json `src/ser.rs:1715-1722` and `tests/test.rs:145-155`'s
  finite-float formatting contract. Hirsel's
  general f64 Display omits `.0`, so `binary_json.finite` adds it when its result
  has no decimal point or exponent. Existing exponent spellings already carry
  the floating-point type. The general-purpose float formatter is unchanged.
- The shared codec helpers still use bounded depth and validate numbers before
  encoding; they return `binary.DecodeError` for encoding errors too. Hirsel
  has no f16 type, so its existing half-float helper widens/narrows by bits,
  checked by the complete Appendix A float table. No codec API or wire grammar
  changes were needed for these imports.

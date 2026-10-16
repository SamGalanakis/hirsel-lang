# Unicode 16.0.0 source normalization and segmentation data

`../std@1/unicode.hir` implements `STD-OP-unicode.normalize_nfc` as ordinary
Hirsel source with no authority. It borrows valid UTF-8 and returns independently
owned NFC text. Equality, ordering and hashing remain bytewise. Compatibility
mappings are excluded. The source implements recursive canonical decomposition,
stable canonical ordering, composition blocking and algorithmic Hangul.

Vendored primary inputs (https://www.unicode.org/Public/16.0.0/ucd/):

- `UnicodeData.txt`: canonical mappings and canonical combining classes.
- `DerivedNormalizationProps.txt`: `Full_Composition_Exclusion`.
- `NormalizationTest.txt`: all 19,965 official vectors, five NFC invariants each.

The unmodified inputs include their provenance and copyright notices;
`LICENSE.txt` is the Unicode license. Generated table comments pin the input
SHA-256 digests. The oracle file SHA-256 is
`d811971453e7075e1ad56fb1b301eece5aa80757b81f6156e74a1bfb3ae5ceb1`.

Run `python3 stdlib/unicode16/generate.py` from the repository root to regenerate
`stdlib/std@1/unicode_tables.hir`; `--check` verifies byte-identical regeneration.
There are 934 nonzero combining-class rows, 2,081 canonical mapping rows and
961 nonexcluded composition pairs. Each scalar is three big-endian bytes; the
combining class is one byte. Sorted, fixed-width immutable Bytes constants avoid
runtime table construction and permit ordinary allocation-free binary searches.
Zero denotes a missing second decomposition scalar or no composition; no
canonical mapping contains U+0000. Hangul is represented algorithmically.

Both source files belong to the ordinary `std@1/unicode` module. Later source
Unicode algorithms can reuse the private combining-class lookup by joining that
module; no compiler registry or runtime normalization hook is added.

`hirsel-driver/tests/stdlib_nfc.rs` executes all 99,825 official NFC comparisons and 275,446 unlisted assigned-scalar identities
through the shipped source, using official expected columns rather than a shared
Rust normalization kernel. Its batching adapter only transports Strings and
retains validated TIR/constants. A second test proves Source origin and heap
agreement.

The complete oracle runs under `cargo test --release -p hirsel-driver --test
it stdlib_nfc::`; its debug build is explicitly ignored, while the source-origin/heap
control and directed conformance remain in the fast gate.

The exhaustive source oracle reads `HIRSEL_JOBS` and runs at most four independent
interpreters. Sixteen-case batches bound retained traces; exact checked-count
assertions prevent dropping a shard. Total coverage is 375,271 comparisons.

`../std@1/graphemes.hir` implements `STD-OP-unicode.graphemes` as an ordinary
`@loop_source` view. The public opaque GraphemeView and GraphemeCursor retain
String; association operations are private, checked ordinary source functions.
They allocate no blocks, carry no authority and yield owner-sharing slices
related to the original source. Segmentation follows default extended clusters
in [UAX #29 revision 45](https://www.unicode.org/reports/tr29/tr29-45.html).

Additional unmodified primary inputs:

- `GraphemeBreakProperty.txt`: grapheme break properties.
- `DerivedCoreProperties.txt`: Indic_Conjunct_Break properties for GB9c.
- `emoji-data.txt`: Extended_Pictographic for GB11.
- `GraphemeBreakTest.txt`: all 1,093 official Unicode16 boundary cases. SHA-256:
  `ee2b9354d270ac061b29f09662cafea06341d77e704b8cc6bd72aaeeda363cb5`.

`python3 stdlib/unicode16/generate_graphemes.py --check` verifies the pinned
ordinary source table: 683 sorted disjoint seven-byte ranges, with three-byte
inclusive endpoints and one property byte. Hangul syllable classes are computed
in source. The shared private `scalar_at` helper from normalization reads the
endpoints; the normalization tables and algorithm are unchanged. The cursor
holds a slice-relative byte position and a bounded suffix state for RI pairs,
emoji ZWJ sequences and Indic linking; it never rescans a prefix or looks outside
the selected slice.

`hirsel-driver/tests/stdlib_graphemes.rs` runs all 1,093 official cases twice,
with full strings and slices starting after a seven-byte hostile RI/ZWJ prefix,
plus empty controls. Exact cluster contents and counts come from the official
boundary markers. The full oracle is an explicit scheduled `full_gate_` test;
table regeneration and directed conformance remain in the fast gate; the
source/native unit and heap control uses the same full-gate convention because
its debug execution exceeds a minute on this host.

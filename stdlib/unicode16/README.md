# Unicode 16.0.0 source text data

`../std@1/unicode.hir` implements `unicode.normalize_nfc`, `normalize_nfd`,
`normalize_nfkc` and `normalize_nfkd` as ordinary
Hirsel source with no authority. It borrows valid UTF-8 and returns independently
owned normalized text. Equality, ordering and hashing remain bytewise.
Compatibility mappings enter only NFKC/NFKD. The source implements recursive
canonical decomposition,
stable canonical ordering, composition blocking and algorithmic Hangul.

Vendored primary inputs (https://www.unicode.org/Public/16.0.0/ucd/):

- `UnicodeData.txt`: canonical/compatibility mappings, canonical combining classes and simple casing.
- `DerivedNormalizationProps.txt`: `Full_Composition_Exclusion`.
- `NormalizationTest.txt`: all 19,965 official vectors, five invariants for each of four forms.

The unmodified inputs include their provenance and copyright notices;
`LICENSE.txt` is the Unicode license. Generated table comments pin the input
SHA-256 digests. The oracle file SHA-256 is
`d811971453e7075e1ad56fb1b301eece5aa80757b81f6156e74a1bfb3ae5ceb1`.

Run `python3 stdlib/unicode16/generate.py` from the repository root to regenerate
`stdlib/std@1/{unicode_tables,unicode_property_tables,unicode_compatibility_tables,string_case_tables}.hir`; `--check` verifies byte-identical regeneration.
There are 934 nonzero combining-class rows, 2,081 canonical mapping rows and
961 nonexcluded composition pairs. Each scalar is three big-endian bytes; the
combining class is one byte. Sorted, fixed-width immutable Bytes constants avoid
runtime table construction and permit ordinary allocation-free binary searches.
Zero denotes a missing second decomposition scalar or no composition; no
canonical mapping contains U+0000. Hangul is represented algorithmically.

The normalization source files belong to the ordinary `std@1/unicode` module. Later source
Unicode algorithms can reuse the private combining-class lookup by joining that
module; no compiler registry or runtime normalization hook is added.

`hirsel-driver/tests/it-stdlib/stdlib_nfc.rs` executes all 399,300 official
four-form comparisons and 1,101,784 unlisted assigned-scalar identities through
the shipped source. Expected results are official columns, not a shared Rust
normalization kernel. Sixteen-input batches bound retained traces; 64 deterministic catalogue
partitions and exact counts preserve every comparison within the pool timeout. The exhaustive oracle is release-only;
directed conformance and regeneration remain in the fast gate.

Compatibility decompositions use 3,832 seven-byte index rows (scalar, three-byte
payload byte offset, one-byte scalar count), with a packed scalar payload.
Recursive compatibility and canonical decomposition share stable ordering,
algorithmic Hangul, and NFC's composition/exclusion tables. NFD/NFKD skip
composition; all forms use the same UTF-8 builder.

Full case data adds the unmodified official `SpecialCasing.txt` and
`CaseFolding.txt` from the Unicode16 UCD, under `LICENSE.txt`. The generator pins
their SHA-256 digests and emits bounded seven-byte indices and packed scalar
payloads in the `string` module. Upper/lower combine UnicodeData simple mappings
with unconditional SpecialCasing overrides. Lowercase also applies Final_Sigma,
using Unicode16 `Cased` and `Case_Ignorable` from DerivedCoreProperties on the
original slice, as Rust does. Case folding selects precisely C+F (not S or T),
without normalization or locale tailoring. `eq_ignore_case` compares full folds.
The scalar simple-case and ASCII-only functions retain their existing behavior.
The same release oracle harness tests 2,981 scalars against official full
upper/lower/fold mappings (8,943 comparisons), plus six long-string checks
adapted from Rust, for 8,949 comparisons. Long inputs run separately under the
same step limit. Directed conformance ports Rust's complete Final_Sigma vectors
and checks folding without normalization, default non-Turkic behavior, Cherokee,
expansions and nonzero-origin slices.

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

Scalar property and simple-case helpers in `../std@1/unicode_properties.hir`
share this module and the `scalar_at` decoder. `generate.py` additionally emits
`unicode_property_tables.hir` from UnicodeData categories L*/Nd and simple
upper/lower columns, and the unmodified official `PropList-16.0.0.txt` input for
White_Space. PropList.txt SHA-256 is
`53d614508e2a0b2305a8aa21cd60d993de9326cdf65993660dfcce4503548583`.
Six-byte rows encode three-byte inclusive range endpoints or source/destination
scalars. The generator merges adjacent property ranges; no foreign-version
property or case tables are imported. Existing NFC and grapheme data and
regeneration checks are preserved. Invalid scalars match no property or case row.

`../std@1/display_width.hir` implements `text.display_width` and the formatting
cluster-width estimate using the existing Unicode16 segmentation. The first
scalar of each cluster counts 2 for East_Asian_Width W/F or C++23
[format.string.std] (LWG 3780/P2675) blocks 4DC0–4DFF, 1F300–1F5FF and
1F900–1F9FF, otherwise 1. This follows libc++
`libcxx/utils/generate_width_estimation_table.py:42–61` at
`89e3afe2b38064b136cfb37a9d07f8c28709a6c4`. The Unicode16 union narrows
exactly 86 scalars compared with the earlier P1868 table. Isolated marks and controls
count 1. Fill scalars are measured independently; each side uses whole fills
followed by spaces for leftover cells.

`EastAsianWidth.txt` is the unmodified Unicode 16.0.0 UCD input from
https://www.unicode.org/Public/16.0.0/ucd/EastAsianWidth.txt (Unicode license).
`python3 stdlib/unicode16/generate_display_width.py --check` verifies the
six-byte sorted range table and pins the input SHA-256 in its generated header.
The table shares the `text` module with its algorithm. Conformance covers the
P1868 multilingual column examples, CJK, combining accents, ZWJ families, emoji
modifiers, independent fills, dynamic counts and custom text padding.

The independent property oracle in `stdlib_nfc.rs` expands UnicodeData First/Last
ranges and reads category and simple-case columns plus PropList White_Space.
Its 64 scheduled `full_gate_unicode16_properties_shard_*` tests execute all five
public functions from ordinary std source for every one of 1,112,064 valid
scalars (5,560,320 comparisons), including unassigned scalars and noncharacters.
A small always-on control covers Latin-1, simple dotted-I case, range/surrogate
edges and Unicode16 additions. No generated property table or host Unicode
version supplies expected values.

Rust flt2dec's 97 exact rows are pinned in
`crates/hirsel-driver/tests/fixtures/flt2dec-exact.json`, with source line numbers,
and carried in ten `SEM-3-fstring-flt2dec-exact-*` kernel corpus cases, with
62 public fixed-format checks in eleven companion `*-public-*` cases. A Rust test
also ports `check_exact`'s varying precision and zero-extension loops (1,706
rounding comparisons). A shared internal rounding helper is extracted unchanged
from `fixed`. The kernel fixtures select just std's bigint and rounding source
files as the std package, keeping `internal` modules private while testing
decimal cutoffs above the radix point, which Hirsel fstrings do not expose.
The eight binary16 rows retain their exact dyadic
values, widened to binary32 for fstrings because Hirsel has no binary16 scalar.
The bigint coefficient at each requested decimal cutoff replaces Rust's
coefficient/exponent buffer pair; this is the same numeric result and checks
carry, sticky bits and ties to even without adding a public format mode.

The width generator retains the existing six-byte inclusive range layout and
Unicode16 input rather than libc++'s packed C++ entries/latest Unicode, keeping
ordinary Hirsel lookup and the language's pinned Unicode semantics. NFC
quick-check remains an optional performance follow-up; normalization is unchanged.

The corpus keeps up to 38 significant digits per sanity row to stay within its
1M-step budget; the Rust test retains every original varying-precision and
zero-extension request, including the complete smallest-subnormal digit string.

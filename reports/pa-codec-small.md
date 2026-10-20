# Small codec prior-art realignment

References were cloned into the lane's external reference directory and pinned to
the commits below. Paths and line numbers refer to those commits, not a moving
branch. This work changes ordinary source and corpus fixtures; no runtime ABI,
TIR/LIR representation, or serialized Value representation changes.

| Item | Reference | Imported checks |
|---|---|---|
| path | Go `ebe2feefc400f813aefdc8a036e66d2619da6649`, `src/path/path_test.go:18-65,152-158,168-180,191-204` | All 36 Clean, 13 Dir, 11 Base and 5 Ext rows (65). The audit reports 79; the four named tables at the pinned commit contain 65. |
| glob | Same Go commit, `src/path/match_test.go:20-77` | Complete 56-row matchTests, including the 27 missing rows (10 match, 17 bad-pattern rows). |
| tzif | jiff `fff640d4`, `crates/jiff/src/tz/timezone.rs:2658-3510` | All 47 missing rows of the 211-row ambiguity/instant tables: Troll TZif 13, Troll POSIX 13, EST5EDT POSIX instants 10, Honolulu instants 7, and four remaining POSIX ambiguity boundaries. |
| POSIX parser | Same jiff commit, `crates/jiff-core/src/tz/posix.rs:2573-3140` | 23 accepted and 38 rejected full-footer rules derived from abbreviation, offset, calendar and transition-time parser fragments. |
| yaml | yaml-test-suite `6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f`, each case's `test.event:1` | Replace all 13 count-only checks with exact event-derived values: 4ABK, 6M2F, CFD4, DFF7, FH7J, FRK4, NHX8, NKF9, PW8X, S3PD, SM9W/01, UKK6/00, UKK6/02. All 402 suite IDs remain represented. |
| semver | node-semver `6e05b7637396ac66522cff8731f07cfe0ef49a29`, `test/fixtures/valid-versions.js:2-31`, `equality.js:40-42` | All 22 version rows compare all five Version fields; all three option-free equality rows check precedence independently of build metadata. |
| uuid | RFC 9562 A.1/A.2/A.4/A.5/B.1/B.2, Figures 16/19/24/25/27/30; uuid-rs `276c2362`, `src/parser.rs:337-382` | Six appendix strings check parse, version, variant, formatting and raw bits; three missing parser spellings also parse. |
| toml | toml-test `ff49d109861c1ad25af53f687f2aef19ab650600`, `tests/files-toml-1.1.0:1`; toml-rs `ceac6e4412420d5a5c19e4f7293ad005cf1a31b2` | Replace 709 TOML 1.0 cases with all 712 TOML 1.1 cases, no skip list. Every valid case decodes to tagged JSON and round-trips, every invalid case rejects. |

TOML implementation follows toml-rs `crates/toml_parser/src/decoder/string.rs:312-370`
for `e` and two-digit Unicode `x` escapes,
`crates/toml_datetime/src/datetime.rs:451-499` for optional seconds, and
`crates/toml_parser/src/parser/document.rs:962-1256` for inline-table trivia and
trailing commas. Its existing recursive descent and table-origin model stay in
place. `moment` treats omitted seconds as zero. The fixture's datetime oracle
normalizes missing seconds to `:00`, as toml-test does. The existing multiline
array append mechanism now also handles multiline inline tables. A separate
corpus program checks insertion, removal, replacement, comment preservation and
all four optional-second time/date-time conversions. The old hand-written
assertion rejecting `1979-05-27T07:32` now expects a local date-time.

The YAML generator now remembers anchors on scalar mapping keys before returning
the key string, so 6M2F's alias has an independently constructed expected value.
The corpus continues to use its existing `harness.values` deep value comparison.

## Deviations and scope

- Path keeps Hirsel's existing slash-only lexical policy. Above-root `..` in
  `/abc/def/../../..` is `Invalid(15)`, while Go clamps to `/`; NUL and backslash
  are invalid and join rejects absolute children. This prevents lexical escape
  and platform-dependent separators and is explicit in the module header.
- Path `parent` drops the final nonempty raw component. Thus `x/` has parent `.`,
  while Go Dir returns `x`; no preliminary Clean is applied. Empty and root paths
  have no parent or filename (Go returns `.` or `/`). `extension` omits its dot,
  treats leading dots as filename text, and examines the filename after trailing
  slashes (`a.dir/` gives `Some("dir")`, while Go Ext gives the empty string).
  These preserve the existing Option-based API and Rust-like extension contract.
- Go's bad-pattern table carries candidate names; Hirsel compiles before matching,
  so each bad row asserts Syntax independently of that name. Hirsel's byte error
  offsets remain covered by the existing glob cases. Its existing two-row DP
  matcher has bounded work instead of Go's backtracking; no grammar change.
- Jiff exposes fragment parsers and partial parsers internally. Hirsel's public
  API accepts complete TZif footers, so parser fragments are embedded in complete
  rules. Partial-parser trailing text is rejected by the complete parser. The
  TZif API does not expose abbreviations, so instant vectors check offsets and
  civil time. Ambiguity vectors check Earlier, Later and Reject.
- YAML retains the existing core-schema, String-key and duplicate-key rules:
  15 complex-key and one duplicate-key suite input intentionally reject. No event
  streaming API or event-oracle comparison is introduced; the requested 13 cases
  now compare the entire value, including null and empty mapping keys.
- SemVer keeps strict dtolnay/SemVer version syntax. The one `v` prefix in
  valid-versions.js and the whitespace/`v` prefix on equality.js's final
  option-free row are removed before field/precedence comparisons, as in the
  existing npm fixture imports. A separate check pins rejection of `v1.2.3`.
  The 34 equality rows requiring npm's loose option do not apply to this API.
  Build metadata remains outside precedence (SemVer 2.0 section 10).
- The two dtolnay NUL vectors mentioned in the audit are already present on the
  starting main (`SEM-3-semver-version-run:36`, `-cargo-syntax-run:27`).
- UUID adds parsing checks, not v1/v3/v5/v6/v8 constructors. Its existing v7 design
  takes clock/random explicitly with no hidden submillisecond counter.
- TOML keeps bounded nesting (128), binary64/i64 values, source splicing followed
  by validation instead of toml_edit's mutable decorated DOM, and canonical
  output with explicit seconds. These are existing Hirsel ownership/bounded-input
  and byte-preservation choices. Adoption of the 1.1 input grammar was explicitly
  decided in this task; old 1.0-only rejection cases are replaced by the 1.1 list.

## Validation before rebasing the suite batching change

All 247 selected corpus programs passed their expected outcomes and canonical
formatting checks through Buck2's `gate-conformance-selected-shard-*` targets.
The selection includes every TOML corpus importer (130 programs), all 11 changed
YAML suite programs containing the 13 upgraded cases, timezone/semver/uuid/glob
programs, and path ownership twins. The first run passed 243; three new programs
needed distinct local names (Hirsel forbids shadowing), and the existing datetime
program needed the 1.1 expectation. Only those four programs ran again and passed.
The imported TOML names equal the 712 entries in `files-toml-1.1.0` exactly:
218 valid and 494 invalid, in 110 generated programs (725 decode/round-trip calls).
All 402 YAML IDs remain represented; no generated case calls `harness.parses`.
Registered corpus programs increase from 6177 to 6195: TOML 123 to 130, timezone
11 to 18, semver 13 to 14, UUID 5 to 6, plus one path and one glob program.


## Rebase onto the batched YAML corpus

Main's `dda057aa` batches the 402 YAML IDs into four programs with explicit
step budgets instead of 91 small programs. The conflict was resolved by keeping
that batching and regenerating from the merged generator: the same 13 exact
value expectations and scalar-key anchors are retained, with no resurrected
shards. All four resulting batch programs passed the selected Buck2 conformance
target after this resolution, with canonical formatting and all 402 IDs. Initial program-count figures above describe the original base;
the landed change preserves main's batching and adds no YAML program.

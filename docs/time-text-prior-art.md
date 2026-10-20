# Time text references and vector selection

Duration parsing follows Go's sign, repeated integer/fraction/unit components,
unit table and overflow checks in `src/time/format.go:1555-1731`, commit
`7f86f957633ee31a1c17a1eb8452ca7a3d9bc141`. Fraction accumulation stops at the
same 63-bit bound; conversion uses the same `f * (unit / scale)` binary64
expression and truncation. The 43 `parseDurationTests` rows at
`src/time/time_test.go:945-1005` are in `SEM-1-duration-go-run.hir`, including
the duplicate minimum-value row. The error table at lines 1016-1043 is in
`SEM-1-duration-go-errors-run.hir`, with additional syntax and i128 overflow checks.

RFC 3339 parsing follows jiff's date, bounded clock components, fraction and
offset stages in `crates/jiff/src/fmt/temporal/parser.rs:524-623,768-838`, commit
`fff640d47f72ca6d3a122678315bbd76488306fe`. Its `parse_second` rule at lines
813-827 is shared by both timestamp and pattern parsers: 60 becomes 59,
retaining the fractional nanoseconds. `SEM-1-time-jiff-temporal-run.hir` has
67 adapted snapshot rows (date/time/datetime plus applicable offset rows), each
with its upstream file and line, and five additional assertions for shared leap-second behavior.

The strtime filter accepts only unmodified `%Y %m %d %H %M %S %f %z %a %b %j %%`.
It selects all 31 strict printer snapshot rows in `crates/jiff/src/fmt/strtime/printer.rs`
and all 50 parser snapshot rows in `crates/jiff/src/fmt/strtime/parse.rs` using
only those directives. They are in `SEM-1-strtime-jiff-{print,parse}-run.hir`,
with file:line citations at every row. The existing strftime case retains its
composite, ordinal, literal, fraction, boundary and invalid-pattern checks.
Numeric and literal whitespace now follow `parse.rs:201-220,881-884`.

Deviations and adaptations:

- Duration's existing signed i128 nanoseconds replace Go's signed i64 bound.
  All seven Go overflow inputs therefore succeed with their wider values.
  Existing i128 extrema and new component/sum overflow cases retain coverage.
- Go's dynamically allocated error strings become Hirsel's existing typed
  `Invalid` and `OutOfRange`, retaining an allocation-free parser.
- Three invalid UTF-8 Go error inputs cannot inhabit Hirsel `String` and are
  omitted. Both representable replacement-character inputs are carried.
- Go's fraction conversion ends in a uint64 cast. Hirsel's core exposes only
  float-to-i32, so the finite, nonnegative result (at most 3.6e12) is truncated
  by decoding its IEEE significand. This preserves Go's binary64 rounding.
- Timestamp parsing uses the RFC 3339 subset requested by its public API:
  four-digit nonnegative year, extended date/clock, T/t, required seconds,
  dot fraction and Z/z or a whole-minute offset below 24 hours. Temporal's
  signed expanded years, basic forms, space separator, incomplete clocks,
  comma fractions, offset seconds and annotations remain unsupported.
  Non-subset snapshot forms become rejection checks. Partial date/clock
  vectors receive a fixed clock/date and UTC suffix; bare-time ambiguity
  entrypoint tests have no timestamp analogue and are excluded.
- Hirsel requires complete civil datetimes rather than jiff's optional fields.
  Clock-only and offset-only strtime vectors receive a fixed valid date;
  date-only vectors receive midnight. Zoned printer vectors use their tested
  fixed offsets, since the formatter receives an offset rather than a zone.
- Strtime flags, widths, precision modifiers, lenient printing and other directives are outside
  the existing 12-directive surface, so their rows are excluded, not expanded.

# Go slog JSONHandler port

Reference: [golang/go ebe2feefc400f813aefdc8a036e66d2619da6649](https://github.com/golang/go/tree/ebe2feefc400f813aefdc8a036e66d2619da6649).
The reference was cloned and checked out at that commit before porting.

The implementation follows `src/log/slog/handler.go:272-325` (built-ins in
`time`, `level`, `msg` order, then attrs, one complete write),
`handler.go:478-493` (empty attr elision),
`src/log/slog/json_handler.go:94-140` (time and typed values), and
`json_handler.go:185-262` (string escaping, no HTML escaping).
The top-level record uses sequential appends rather than a JSON map so repeated
keys survive, including keys matching built-ins.

| Upstream vectors | Corpus file | Before | After |
| --- | --- | ---: | ---: |
| `src/testing/slogtest/slogtest.go:36-81`: built-ins, attrs, empty-attr, zero-time | `conformance/SEM/SEM-3-log-slog-cases-run.hir` | 0 | 4 |
| `src/log/slog/json_handler_test.go:111-141`: TestAppendJSONValue | `conformance/SEM/SEM-3-log-slog-values-run.hir` | 0 | 15 |
| `src/log/slog/json_handler_test.go:154-170`: TestJSONAppendAttrValueSpecial | `conformance/SEM/SEM-3-log-slog-special-run.hir` | 0 | 4 |
| Additional boundaries and failures | `conformance/SEM/SEM-3-log-slog-edges-run.hir` | 0 | 9 |
| Existing logging checks (updated to the new schema) | `conformance/SEM/SEM-3-log-run.hir` | 5 | 5 |

Each imported row checks the literal complete JSON line. The original Rust
scripted-sink test still checks filtering before the first write, exactly one
write, exact bytes, and propagation of provider faults. The csvjson CLI error
fixture now checks ERROR, msg, and the flat reason attribute.

## Deviations and adaptations

- Explicit `log.Value` variants replace Go reflection, `Any`, and interface
  dispatch. Hirsel uses explicit data and does not have Go's dynamic interfaces.
  `Json` accepts the already marshaled value, `Failure` accepts the error's text,
  and `Severity` renders a level. The two json.Marshaler vectors retain their
  `xyz`/`pqr` array payloads via `Json(Array(...))`; marshal-versus-error method
  dispatch itself has no Hirsel equivalent. `io.EOF` becomes `Failure("EOF")`.
- Hirsel `String` is valid UTF-8. The malformed final `0xf6` byte in upstream
  value row 4 is fed through `utf8.decode_lossy` before logging, retaining the
  replacement character. It renders as literal U+FFFD rather than Go JSON v1's
  `\ufffd`; the decoded JSON value is identical. U+2028/U+2029 still use Go's
  escape spelling. All other imported expected bytes match the reference.
- `None()` represents an absent/zero record time. `Some(Timestamp(0))` is the
  Unix epoch and is emitted. Timestamps contain only Unix nanoseconds, so they
  render in UTC; Go's timezone-bearing `time.Time` can retain other offsets.
  An explicitly supplied year-1 timestamp is data, not an absence sentinel.
- `time.Duration` has i128 nanoseconds, wider than Go's int64 duration. It stays
  an exact integer, preserving Hirsel's existing duration range.
- `Json` uses Hirsel's validated JSON DOM and its nesting bound. Invalid number
  and nesting failures become `!ERROR:json: invalid number` and
  `!ERROR:json: nesting too deep`, respectively, rather than Go reflection or
  marshaler error descriptions. Serialization happens in a separate buffer so
  a failure cannot corrupt the enclosing record. Nonfinite Float and invalid
  timestamp errors retain Go's exact strings.
- The API remains an explicit sink with caller-supplied time and minimum level;
  there is no ambient logger, clock, source lookup, WithAttrs/WithGroup,
  ReplaceAttr, or LogValuer. Those features require a separate API design, so
  only the four flat-API slogtest cases are imported. A complete line goes to
  one err.print call; cross-task serialization is the caller's responsibility
  under Hirsel's capability/task semantics, rather than a hidden Go mutex.
- Finite floats reuse Hirsel's existing shortest-roundtrip formatter, with Go's
  positive-exponent `+` appended. This keeps scalar formatting in ordinary
  source instead of importing Go's runtime-backed encoding/json machinery.

The format change also updates the csvjson and tasks callers, the serializable
log fixture, the existing source and Rust fixtures, the library specification, and the third-party notice. Entry's input
member names remain `message`, `fields`, and `timestamp`; the output schema is
`msg`, flat attributes, and optional `time`.

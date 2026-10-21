# Explaining CLI parse errors

Keep the original argument snapshot and give a copy to `parse`. On failure,
print `error_message` and `usage` explicitly. The renderer is pure (`uses {}`)
and returns a unique String without a trailing newline. Only the caller's
printing needs an `Io` capability.

This self-contained example uses a fixed snapshot so it can run as a fixture.
A real application can collect its `Args` with `arg.len` and `arg.text` first.
Specs passed to `usage` must be valid; applications with dynamic specs should
handle `InvalidSpec` separately and skip usage for that error.

```hirsel
pub fn main(rt: Io) -> Result[(), GrantError] {
  let specs: List[std@1/cli.Flag] = [
    std@1/cli.Flag(
      name: "output",
      short: Some(b'o'),
      kind: Value(),
      help: "output path"
    )
  ]
  // Keep a snapshot: parse consumes its copy, the renderer borrows the original.
  let args: List[String] = ["-o"]
  match std@1/cli.parse(copy args, specs) {
    Ok(_) => (),
    Error(error) => {
      let sink = io.stderr(rt)?
      let _ = err.print(sink, std@1/cli.error_message(args, specs, error))
      let _ = err.print(sink, "\n")
      let _ = err.print(sink, std@1/cli.usage("example", specs))
    },
  }
  Ok(())
}
```

It writes to stderr:

```text
argument 0 "-o": a value is required for "--output <value>" but none was supplied
Usage: example [options] [--] [arguments]
  -o, --output <value>	output path
```

The index is zero-based. The token is double-quoted: `"` and `\` are escaped,
and every byte outside printable ASCII is written as lowercase `\xhh`.
For example `--é` becomes `"--\xc3\xa9"`; ESC becomes `\x1b`.
This is a byte-preserving diagnostic representation, not a shell command.
No control character from an argument is emitted literally.

Unknown flags identify the original token. Missing values identify the canonical
long name even when invoked through a short alias. Invalid switch values list
`true, false, 1, 0, t, f, T, F, TRUE, FALSE, True, False`.

Use the same arguments and specs that produced the error. The helper rechecks
parsing on a copy before naming a spec. Stale errors, successful snapshots,
errors after the first positional or `--`, consumed value tokens, invalid specs
and forged indices get useful fallback text. Out-of-range indices are never
read. `InvalidSpec` always returns a fixed explanation, without choosing among
ambiguous names. Parsing, repeat ordering and stopping behavior are unchanged;
there are no automatic suggestions, help, printing or exit effects.

## Reference and adaptations

Reused upstream clones are pinned to clap
[`ccc89b83875474399e9d9d70841163f6319d1a41`](https://github.com/clap-rs/clap/tree/ccc89b83875474399e9d9d70841163f6319d1a41)
and Go
[`ebe2feefc400f813aefdc8a036e66d2619da6649`](https://github.com/golang/go/tree/ebe2feefc400f813aefdc8a036e66d2619da6649).

The port keeps clap's error kind separate from its context:
`clap_builder/src/error/kind.rs:4-35` (`UnknownArgument`, `InvalidValue`),
`context.rs:8-15` (`InvalidArg`, `InvalidValue`, `ValidValue`), and
`error/mod.rs:434-480,720-747` (constructing empty/invalid value and unknown
argument errors). A private `ErrorContext` record carries those three fields;
the existing public `ParseError` supplies the kind. Rendering follows
`error/format.rs:207-233,351-365`. Go's `src/flag/flag.go:1077-1084,1136-1160`
supplies the unknown/missing/invalid boolean diagnostic branches and the
existing CLI grammar.

The exact unknown-argument wording comes from
`tests/builder/error.rs:84-117,119-139,183-206`. The lane's reference fixture
ports `--foo` from `cant_use_trailing` and `suggest_trailing`, and `--ignored`
from `suggest_trailing_last`/`unknown_argument_flag` (`:142-166,313-346`).
The `unused` positional vector (`:102-117`) is a successful parse here; rendering
a forged unknown-argument error therefore uses the fallback, as required by
Hirsel's existing positional semantics. The Go `-bool=x` vector
(`src/flag/flag_test.go:652-674`) is tested with a registered switch.

Semantic and presentation deviations:

- Existing typed errors are preserved: `UnknownFlag` maps to clap's
  `UnknownArgument`; `MissingValue` and `UnexpectedValue` map to its
  `InvalidValue` shape. `None` means a missing value; `Some("")` remains an
  explicitly invalid switch value, rather than conflating both empty cases.
  Numeric conversion and validation remain the caller's responsibility.
- Explicit immutable arguments/specs replace mutable parser state. A deep
  copy is parsed again to validate context for arbitrary public errors,
  respecting earlier errors, aliases, consumed values, first-positional
  stopping and `--`. Invalid/stale context gets kind-only fallback text;
  out-of-range indices are checked before access. Invalid specs use fixed text.
- A fixed private record replaces clap's extensible context map because only
  three context fields are needed by this additive helper; no public error
  taxonomy or parser feature is added.
- The zero-based index and original token are included. Canonical flag usage
  (`--name <value>`) replaces clap's configurable argument display names;
  possible values are Go's twelve accepted boolean spellings in parser order.
- Double-quoted ASCII byte escaping covers tokens, values and names instead
  of clap's single-quoted styling and Go's raw `%s` names/value-only `%q`.
  Non-ASCII UTF-8 bytes and controls use `\xhh`, so terminal controls cannot
  be emitted even from forged errors or Unicode spec names.
- One line without a trailing LF replaces clap's styled multiline error.
  Usage, error prefix, suggestions, help, I/O and exit remain explicit caller
  decisions. Upstream suggestion/usage/help output is omitted from the exact
  vectors; these features are outside this helper's contract.

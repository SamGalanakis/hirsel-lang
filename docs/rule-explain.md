# Explain a diagnostic rule

Use the installed binary from any directory, including one without a Hirsel
package:

```sh
hirsel explain MODE-4
hirsel explain OWN-2 --json
hirsel explain CAP-2
hirsel explain --list
hirsel explain --list --json
```

`explain` reads the guidance compiled into the binary. It does not load source,
initialize a compiler or engine, discover a workspace, contact a resident server
or the network, or open a std image or writable cache. Neither a package nor
`HOME` is required. The output reflects the installed binary's catalogue.

Rule IDs use their exact uppercase spelling, such as `OWN-2`. Malformed IDs,
unknown IDs, rules without catalogued explanations, conflicting options and
extra arguments produce a tool error on stderr and exit 2, with empty stdout.
Success exits 0 and writes the explanation or list to stdout. `--list` contains
one rule per line, deduplicated and sorted by rule ID bytes (`LANG-12` comes
before `LANG-2`). It lists established guidance, rather than every language rule.

Explanations contain the rule, title, meaning and typical repair, plus existing
related-rule or conditional explanations from the diagnostic reference. A rule
without a specific repair explicitly says so; JSON represents that repair as
`null`. These are conditional suggestions to consider at the diagnostic's site.
The command neither proposes a source-specific replacement nor edits files.
A diagnostic's more specific repair takes precedence over typical guidance.

For example:

- `MODE-4` explains sendability. For counted data the guidance calls for a
  `unique` value with sendable parts; `copy` does not prove sendability. The
  related `CONC-5` and `CONC-6` entries describe mutex and channel conditions.
- `OWN-2` covers moved, uninitialized or borrowed values where ownership is
  required. `take x` applies to an owned binding at its last use; `copy x`
  applies only when the value is copyable and must remain available.
- `CAP-2` covers explicit capability arguments and authority bounds. An opaque
  type's `uses {Dir}` describes authority held by its fields; it grants no
  capability. Removing authority-bearing captures or changing a receiver's
  bound depends on the intended contract.

## JSON schema 1

An explanation is one JSON object followed by a newline:

```json
{
  "schema_version": 1,
  "rule": "OWN-2",
  "title": "Moves and availability",
  "meaning": "The value was moved, was not initialized on every path, or was borrowed where ownership is required.",
  "repair": "Use the value before its `take`, or initialize it on every reaching path. To pass an owned binding to a `take` parameter at its last use, write `take x`; if a copyable value must remain available, pass `copy x` instead.",
  "related": []
}
```

`related` contains objects with `rule`, `title`, `meaning`, `repair` and `related`
fields, using the same value types as the parent. A related entry may name the
same rule to explain a particular condition, such as ambiguous plain `?` under
`LANG-8`. Strings use standard JSON escaping. Consumers should use fields rather
than object-key order. List JSON has `schema_version: 1` and a `rules` array of
rule ID strings in the same order as the text list. This additive command format
does not change diagnostic JSON or primary source spans.

## Catalogue and implementation references

Primary explanations and diagnostic fallback notes use
`crates/hirsel-diag/src/guidance.tsv`. The existing related-rule mapping from
`tools/generate-diagnostics.py` is transcribed in
`crates/hirsel-diag/src/rule_explain_related.tsv`; consistency tests compare every
primary and related explanation against `docs/diagnostics.md`. Related entries
are available to `explain` without adding fallback notes to their ordinary
diagnostics. No generated reference is modified.

The command follows Rust's catalogue lookup and early explain dispatch at
upstream commit `a7c37f9f0ee5e93396106e9de20f4a86b4154e63`:

- `compiler/rustc_driver_impl/src/lib.rs:208–211` dispatches before input loading;
  `:427–473` validates, looks up and prints an explanation, or reports an error.
- `compiler/rustc_error_codes/src/lib.rs:7–29` defines the error-code catalogue;
  `compiler/rustc_errors/src/codes.rs:29–58` embeds descriptions and rejects
  absent entries.

Deliberate deviations: Hirsel accepts only exact `RuleId` spelling, without
Rust's case folding or optional `E`; it serves existing Hirsel guidance and
conditions rather than Rust Markdown; it uses a subcommand and adds schema-1
JSON and a sorted list; it always prints directly, without a pager, color or
Markdown rewriting. Its dispatch occurs before compiler-stack setup, rather
than after compiler option/session setup. These choices keep the route usable
by agents without source or environment initialization. Tests adapt `tests/ui/explain/basic.rs:1–2` (a valid explanation),
`error-with-no-explanation.rs:1–3` (a valid ID without an explanation),
`invalid-error-code.rs:1–2` (malformed spelling) and
`overflow-error-code.rs:1–4` (an oversized code) to Hirsel IDs. Hirsel IDs have
no fixed numeric width, so even an arbitrarily long positive suffix is valid
syntax but has no explanation unless catalogued. `no-E-prefix.rs:1–2` is
intentionally inverted: Hirsel requires its full prefix. Additional argument,
reference consistency and JSON cases cover Hirsel's contract.

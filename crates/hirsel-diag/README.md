# hirsel-diag

Own language diagnostics and human/stable JSON rendering.

Allowed dependencies: hirsel-span, serde_json and unicode-width (human terminal-cell rendering).

Invariants and boundary:

- Every Diagnostic has exactly one syntactically validated RuleId (spec introduction, FAIL-1).
  Compiler literals use `const { RuleId::of("OWN-6") }`; `RuleId::new` validates
  IDs parsed from external data. Both share the same spelling check.
  Phase wrappers accept RuleId and construct through `Diagnostic::error`.
- Primary and labelled secondary locations are byte Spans, resolved using the owning source snapshot.
- Notes are attached text; a fix is one optional replacement range and replacement string.
  A note beginning `help: ` is a suggested repair and renders as `= help: ...`.
- `obligations` say what a diagnostic reports beyond its
  rule: a required multiplicity, a generic copy proof, a failed generic bound with the
  arguments it names, or a parameter mode imposed on one argument. The driver orders
  and suppresses on them (section 23, spec #38); it never matches message or label text.
- `root_cause` is an opaque, definition-local key: the same provided cause at the same
  site regardless of the failed requirement (section 23). Equal keys fold into the diagnostic
  section 23 selects first; the others become its notes and a label.
- Neither field is rendered or serialised: JSON schema v1 is unchanged.
- Human output always prints severity[rule]; invalid source locations degrade without panicking.
- JSON schema version 1 is described below; language diagnostics never masquerade as unavailable implementation.

JSON schema v1 (`Diagnostic::to_json`): an object with these always-present keys.
Object key order is not significant; array order is deterministic and preserved.

| Key | JSON type / meaning |
|---|---|
| `schema_version` | integer, exactly 1 |
| `rule` | string, validated `PREFIX-N`, never null |
| `severity` | `"error"`, `"warning"`, or `"note"` |
| `message` | string |
| `primary` | span object |
| `secondary` | array of `{ "span": span, "message": string }` |
| `notes` | array of strings |
| `fix` | null or `{ "span": span, "replacement": string, "message": string }` |

A span object always contains `file_id` (u32), `file` (path string or null), `start`
and `end` (u32 half-open UTF-8 byte offsets), `line`, `column`, `end_line`,
`end_column` (one-based integers or null if unavailable). File IDs are only valid
inside this source snapshot. Paths are display paths (non-UTF-8 OS paths use lossy
replacement); tools editing files should retain the source map's actual PathBuf.
Columns count Unicode scalars. Human carets preserve tabs and use Unicode display
width for intervening text and highlighted ranges; JSON columns stay scalar-based. An invalid span retains its raw offsets and ID but
has null locations. Renderers do not invent source locations. Fixes are suggestions,
not automatic edits: verify the source revision and span before applying.

Breaking field/meaning changes increment schema_version. Consumers should ignore
unknown fields. CLI framing (one object per diagnostic, or a surrounding report)
is separate from this diagnostic schema. IO/launch/tool errors do not fabricate
RuleIds; they are outside this language-diagnostic schema.

Contract revision 1 preserves Diagnostic and JSON v1. OperationalFailure in
hirsel-types and ToolingLimit in hirsel-tir have no fabricated RuleId. Static TIR
validator invariant failures are tooling errors. Optional diagnostic-kind and
violation-capsule proposals remain phase factories/tooling sidecars.

When a diagnostic has neither a replacement nor a `help: ` note, both renderers
append the rule's typical repair from `src/guidance.tsv`. This does not invent a
replacement span. Producer-supplied repairs take precedence. The same table
owns the generated repair reference in `docs/diagnostics.md`; regenerate it with
`python3 tools/generate-diagnostics.py` and check it with `--check`.

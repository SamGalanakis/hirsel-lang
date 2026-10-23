# Explicit ignored tests

`tools/ignored-tests --list` lists archival audits and host CPU qualification
identities with their required environment inputs. Invoke one identity to run
it, for example:

```sh
TOY_ANALYZER_31_REFUSAL_ARCHIVE=/path/to/retained/original \
  tools/ignored-tests toy_analyzer_31::toy_analyzer_31_recording_difference
```

The command selects the exact ignored identity on the host through Buck2 and
requires exactly one passing executed test. Missing required inputs, a failed
case and a zero-test selection all fail. Retain the complete original source,
tapes, executables and expected observations required by that identity. These
are explicit audits: provide original immutable evidence, not fresh replacement
tapes. Audit helpers can write control artifacts beside inputs; use an owned
copy while preserving the original recording bytes.
Positive archive audits require a compiler and runtime generation matching
their recordings. A program-binding refusal remains a failing positive audit.

Physical qualification cases keep their original widths and capacity checks.
The host command does not reserve CPUs or change admission. A capacity failure
is a failed qualification, never a reason to clamp its input or count a smaller
width as the full case. The policy evaluator's ignored cell is instead run by
its ordinary simulation and native campaigns, with their original bounded
subprocess execution and parent assertions.

`python3 tools/tests/ignored_tests.py` checks all ignored sources against their
scheduled native/full/release homes, bounded delegated parent, or exact explicit
command. Registering a file never covers an adjacent unregistered identity.

# hirsel-analysis-canary

Two toy analyses that run out of process over the `hirsel.tir.export.v7` text
and write a `hirsel.analysis.findings.v1` sidecar
(`docs/spec/proof-seam.md`, PS-17 to PS-19). They are canaries:
`crates/hirsel-pipeline-tests/tests/front/analysis_canaries.rs` exports a
passing and a failing fixture for each to a file, runs the analysis entry point
the commands run over it and checks the findings, so a change to the seam that
breaks an out-of-process analysis turns the gate red.

- `taint <export> -o <findings>`: a parameter annotated
  `@analysis(taint: secret)` must not reach, through assignments, matches,
  calls and returns, an argument of a call that also passes a `core@1/Stdout`.
  Explicit flows only.
- `units <export> -o <findings>`: parameters annotated `@analysis(unit: m)`
  carry their unit through copies; adding or subtracting two different units
  is reported.

`src/taint.rs` and `src/units.rs` are the analyses; `src/lib.rs` holds what
both share: reading and pinning the export, place roots, locations, the
sidecar and `run`, the command's entry point. Dependencies: `hirsel-proof-export` and the
data crates it decodes into, never the compiler.

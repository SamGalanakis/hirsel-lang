# Historical compiler speed scoreboard

This is a historical report. Publish each new measurement in a separate
`RESULTS-<lane-or-session>.md` beside it; do not append rows here. Discover
reports with `rg --files -g 'RESULTS*.md'` (no shared report index to edit).

**Historical, incomplete, superseded.** These rows measure compiler
`69f9038496da68c7dec184e006628547c85050e6`, not current main or arch. The owner
stopped the roughly ten-hour campaign after the compiler had been superseded.
No stopped work was restarted, and no corrected supplemental matrix was run.
These numbers do not decide current architecture acceptance.

The last durable checkpoint contains **428 cells / 3878 samples**. The seven
original example pairs completed ten rounds; later inputs yielded CPU31 between
rounds. Synthetic 10k retains rounds 0–6 from the checkpoint; later commands
only have their original log evidence. Synthetic 30k and the edit group were
not completed and have no invented rows. Native and scale timeouts/caps are
findings, not successful compile measurements.

Raw schema-1 evidence is retained outside the source tree in
`baseline-data/historical.json` and `baseline-data/historical-final-checkpoint.json`
beside the compiler-speed brief. Original per-command logs remain under
`baseline-data/full-work/logs/`; the stopped producer logs are
`/tmp/cc-baseline-full.log` and `/tmp/cc-baseline-full-resumed.log`.
Only fully checkpointed samples contribute to summaries. No compiler source was
changed by this lane.

Use `tools/compiler-bench/run --quick` against a bundle from current
`origin/main` or `origin/arch` for new measurements. Do not repeat this historical
multi-hour campaign. Quick validation on the historical compiler completed all
28 cold/warm cells with **98.54 seconds in measured commands**; this is a harness
smoke result, not a current-compiler performance claim. It is one round, so it
provides instruction counts and descriptive wall values, not wall ratios.

Failed input classifications:

- `tasks` debug: generated-body DWARF anchor failure, later fixed on main in
  `139046c7`; original failures retained.
- `hello-web` native: the original readiness connection closed before sending
  HTTP and triggered MEM-13. Idle and valid-HTTP controls survived; the failure
  preceded intentional shutdown. Those rows are excluded from accepted startup
  summaries. Future probes send and drain a valid HTTP GET.
- `nested-match-16/32`: rejected generated binder shadowing (LANG-4), not accepted
  compiler work. The generator now uses unique binder names.
- `derive-64`: incomplete manifest sidecar, rejected before any definitions or
  V runs. The generator now emits a complete manifest.

Generator fixes apply to future inputs only; they do not relabel historical
failures. Every failed cell is excluded from accepted-program ratios. Native
samples for silent stress inputs explicitly use exit time because no first
output exists. Std is whole-source precheck only; unsupported executable modes
and unavailable phase reports are not fabricated.

Compiler main `69f9038496da68c7dec184e006628547c85050e6`; 2026-10-20/21 (historical campaign, stopped incomplete); CPU 31; 10 seeded rounds, cold followed by unchanged warm.

Instructions are user-mode instructions including children. Wall seconds and peak RSS MiB come from GNU time; native wall stops at first stdout byte or listening. Native counters/RSS include bounded shutdown. Warm retains the preceding cold run’s artifacts. Cold removes program/native caches, retaining the shipped std precheck and prebuilt Rust dependencies; neither run flushes the host page cache. Rust runs offline at one job. Wall ratios require an uninterrupted ten-round session; round-scoped or resumed measurements retain descriptive wall values and instruction ratios.

A ratio above 1 means Hirsel costs more. Native compares startup (compile + launch) with `cargo build --release` (compile only). Times are session-local; instructions are the verdict. — means no paired port or unavailable telemetry. Tasks reuses its existing app benchmark Rust port. No Rust ports exist for hello-web, the tour, conformance or std; existing pairs are retained. Std is measured by `precheck` (whole std from source), which has no native entry and accepts no timings flag. Tour/conformance fixtures without an executable entry remain check-only.

## check

| input | lines | cache | status / n | wall s | instructions G | RSS MiB | Rust wall s / instructions G / RSS MiB | H/R wall / instructions / RSS | H/R instructions per line | top five phases by self time (s) |
|---|---:|---|---|---:|---:|---:|---|---|---:|---|
| hello | 7 | cold | ok / 10 | 0.975 | 5.855 | 386 | 0.130 / 0.248 / 70 | 7.50x / 23.62x / 5.49x | 10.12 | parse 0.737; resolve 0.007; modes 0.002; signatures 0.001; check 0.000 |
| hello | 7 | warm | ok / 10 | 0.970 | 5.855 | 386 | 0.100 / 0.228 / 30 | 9.70x / 25.70x / 12.86x | 11.01 | parse 0.735; resolve 0.007; modes 0.002; signatures 0.001; check 0.001 |
| todo-api | 499 | cold | ok / 10 | 2.335 | 14.744 | 579 | 0.190 / 0.488 / 123 | 12.29x / 30.23x / 4.70x | 30.17 | signatures 1.039; parse 0.719; modes 0.176; comptime 0.077; verify 0.048 |
| todo-api | 499 | warm | ok / 10 | 2.400 | 14.744 | 579 | 0.100 / 0.229 / 30 | 24.00x / 64.51x / 19.14x | 64.38 | signatures 1.047; parse 0.740; modes 0.182; comptime 0.083; verify 0.048 |
| kvstore | 371 | cold | ok / 10 | 3.010 | 14.116 | 544 | 0.230 / 0.394 / 114 | 13.09x / 35.82x / 4.77x | 36.78 | signatures 1.294; parse 1.097; modes 0.135; verify 0.084; comptime 0.051 |
| kvstore | 371 | warm | ok / 10 | 3.435 | 14.115 | 544 | 0.135 / 0.229 / 30 | 25.44x / 61.77x / 17.97x | 63.43 | signatures 1.518; parse 1.240; modes 0.145; verify 0.103; comptime 0.057 |
| logq | 301 | cold | ok / 10 | 3.310 | 14.020 | 544 | 0.215 / 0.363 / 106 | 15.40x / 38.63x / 5.15x | 32.85 | signatures 1.265; parse 1.176; verify 0.180; modes 0.155; comptime 0.052 |
| logq | 301 | warm | ok / 10 | 3.380 | 14.021 | 544 | 0.145 / 0.228 / 30 | 23.31x / 61.46x / 17.99x | 52.27 | signatures 1.360; parse 1.202; verify 0.182; modes 0.151; comptime 0.057 |
| csvjson | 285 | cold | ok / 10 | 1.780 | 7.929 | 407 | 0.230 / 0.416 / 114 | 7.74x / 19.06x / 3.57x | 20.73 | parse 1.055; modes 0.274; signatures 0.072; check 0.024; resolve 0.019 |
| csvjson | 285 | warm | ok / 10 | 1.780 | 7.929 | 407 | 0.130 / 0.229 / 30 | 13.69x / 34.67x / 13.52x | 37.71 | parse 1.095; modes 0.262; signatures 0.068; check 0.022; resolve 0.020 |
| mdtoc | 189 | cold | ok / 10 | 1.625 | 6.196 | 389 | 0.200 / 0.317 / 102 | 8.12x / 19.56x / 3.83x | 16.97 | parse 1.229; modes 0.055; resolve 0.016; signatures 0.011; check 0.008 |
| mdtoc | 189 | warm | ok / 10 | 1.570 | 6.196 | 389 | 0.140 / 0.228 / 30 | 11.21x / 27.19x / 12.91x | 23.60 | parse 1.177; modes 0.052; resolve 0.015; signatures 0.011; check 0.008 |
| bench-lite | 215 | cold | ok / 10 | 2.770 | 12.798 | 535 | 0.210 / 0.331 / 105 | 13.19x / 38.65x / 5.09x | 37.75 | signatures 1.233; parse 1.076; modes 0.054; comptime 0.051; verify 0.048 |
| bench-lite | 215 | warm | ok / 10 | 2.880 | 12.798 | 534 | 0.120 / 0.228 / 30 | 24.00x / 56.12x / 17.67x | 54.81 | signatures 1.188; parse 1.125; modes 0.049; verify 0.049; comptime 0.048 |
| tasks | 3036 | cold | ok / 10 | 5.110 | 32.720 | 761 | 0.610 / 1.767 / 188 | — / 18.52x / 4.05x | 7.39 | signatures 1.949; modes 1.175; parse 0.828; comptime 0.315; verify 0.269 |
| tasks | 3036 | warm | ok / 10 | 5.060 | 32.721 | 759 | 0.155 / 0.402 / 46 | — / 81.48x / 16.50x | 32.53 | signatures 1.933; modes 1.184; parse 0.822; comptime 0.315; verify 0.267 |
| hello-web | 34 | cold | ok / 10 | 0.930 | 6.268 | 388 | — / — / — | — / — / — | — | parse 0.649; signatures 0.043; modes 0.012; resolve 0.008; check 0.002 |
| hello-web | 34 | warm | ok / 10 | 0.945 | 6.268 | 388 | — / — / — | — / — / — | — | parse 0.660; signatures 0.043; modes 0.012; resolve 0.007; check 0.002 |
| conformance/CAP-1-arg-snapshot-run | 10 | cold | ok / 10 | 0.190 | 0.230 | 291 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| conformance/CAP-1-arg-snapshot-run | 10 | warm | ok / 10 | 0.185 | 0.230 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| conformance/CLO-1-nested-local-call-result-run | 24 | cold | ok / 10 | 0.890 | 5.864 | 386 | — / — / — | — / — / — | — | parse 0.659; resolve 0.007; modes 0.003; signatures 0.001; check 0.001 |
| conformance/CLO-1-nested-local-call-result-run | 24 | warm | ok / 10 | 0.890 | 5.864 | 386 | — / — / — | — / — / — | — | parse 0.663; resolve 0.007; modes 0.003; signatures 0.001; check 0.001 |
| conformance/CONC-2-taken-block-sender-capture-run | 24 | cold | ok / 10 | 0.190 | 0.240 | 292 | — / — / — | — / — / — | — | modes 0.004; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| conformance/CONC-2-taken-block-sender-capture-run | 24 | warm | ok / 10 | 0.185 | 0.240 | 292 | — / — / — | — / — / — | — | modes 0.004; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| conformance/CT-2-size-local-run | 23 | cold | ok / 10 | 0.200 | 0.291 | 300 | — / — / — | — / — / — | — | verify 0.005; lower 0.003; modes 0.003; resolve 0.001; signatures 0.001 |
| conformance/CT-2-size-local-run | 23 | warm | ok / 10 | 0.190 | 0.291 | 300 | — / — / — | — / — / — | — | verify 0.005; lower 0.003; modes 0.003; resolve 0.001; signatures 0.001 |
| conformance/DROP-2-destructure-opaque-run | 18 | cold | ok / 10 | 0.190 | 0.228 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/DROP-2-destructure-opaque-run | 18 | warm | ok / 10 | 0.180 | 0.228 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; signatures 0.001; check 0.000; parse 0.000 |
| conformance/FACT-3-match-tag-retested-each-pass-run | 21 | cold | ok / 10 | 0.180 | 0.234 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/FACT-3-match-tag-retested-each-pass-run | 21 | warm | ok / 10 | 0.180 | 0.234 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/GEN-1-capture-data-nested-run | 47 | cold | ok / 10 | 0.190 | 0.251 | 291 | — / — / — | — / — / — | — | modes 0.004; resolve 0.002; signatures 0.001; check 0.001; parse 0.000 |
| conformance/GEN-1-capture-data-nested-run | 47 | warm | ok / 10 | 0.190 | 0.251 | 291 | — / — / — | — / — / — | — | modes 0.004; resolve 0.002; signatures 0.001; check 0.001; parse 0.000 |
| conformance/LANG-11-block-value-borrow-run | 90 | cold | ok / 10 | 0.190 | 0.272 | 292 | — / — / — | — / — / — | — | modes 0.006; signatures 0.002; resolve 0.002; check 0.002; parse 0.000 |
| conformance/LANG-11-block-value-borrow-run | 90 | warm | ok / 10 | 0.190 | 0.272 | 292 | — / — / — | — / — / — | — | modes 0.006; signatures 0.002; resolve 0.002; check 0.001; parse 0.000 |
| conformance/MEM-11-thaw-after-root-dropped-run | 34 | cold | ok / 10 | 0.180 | 0.246 | 291 | — / — / — | — / — / — | — | modes 0.003; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/MEM-11-thaw-after-root-dropped-run | 34 | warm | ok / 10 | 0.180 | 0.246 | 291 | — / — / — | — / — / — | — | modes 0.003; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/MODE-1-contract6-owned-run | 45 | cold | ok / 10 | 0.180 | 0.267 | 291 | — / — / — | — / — / — | — | modes 0.005; signatures 0.003; check 0.001; resolve 0.001; parse 0.000 |
| conformance/MODE-1-contract6-owned-run | 45 | warm | ok / 10 | 0.180 | 0.267 | 291 | — / — / — | — / — / — | — | modes 0.005; signatures 0.003; check 0.001; resolve 0.001; parse 0.000 |
| conformance/OWN-3-element-sibling-field-block-argument-run | 41 | cold | ok / 10 | 0.180 | 0.244 | 291 | — / — / — | — / — / — | — | modes 0.003; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/OWN-3-element-sibling-field-block-argument-run | 41 | warm | ok / 10 | 0.180 | 0.244 | 291 | — / — / — | — / — / — | — | modes 0.003; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| conformance/PROV-1-copied-prefix-if-run | 61 | cold | ok / 10 | 0.195 | 0.302 | 292 | — / — / — | — / — / — | — | modes 0.007; signatures 0.003; resolve 0.002; check 0.002; parse 0.000 |
| conformance/PROV-1-copied-prefix-if-run | 61 | warm | ok / 10 | 0.190 | 0.302 | 292 | — / — / — | — / — / — | — | modes 0.007; signatures 0.003; resolve 0.002; check 0.002; parse 0.000 |
| conformance/SEM-1-bigint-go-bitwise-run | 96 | cold | ok / 10 | 0.900 | 5.965 | 387 | — / — / — | — / — / — | — | parse 0.648; modes 0.011; resolve 0.007; signatures 0.005; check 0.002 |
| conformance/SEM-1-bigint-go-bitwise-run | 96 | warm | ok / 10 | 0.875 | 5.965 | 387 | — / — / — | — / — / — | — | parse 0.644; modes 0.011; resolve 0.007; signatures 0.004; check 0.002 |
| conformance/SER-2-duplicate-map-run | 20 | cold | ok / 10 | 0.185 | 0.249 | 291 | — / — / — | — / — / — | — | modes 0.004; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| conformance/SER-2-duplicate-map-run | 20 | warm | ok / 10 | 0.180 | 0.249 | 291 | — / — / — | — / — / — | — | modes 0.004; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| tour/LANG-6-tour-00-statements-run | 27 | cold | ok / 10 | 0.875 | 5.849 | 386 | — / — / — | — / — / — | — | parse 0.651; resolve 0.007; modes 0.002; check 0.001; signatures 0.000 |
| tour/LANG-6-tour-00-statements-run | 27 | warm | ok / 10 | 0.880 | 5.849 | 386 | — / — / — | — / — / — | — | parse 0.666; resolve 0.007; modes 0.002; check 0.001; signatures 0.000 |
| tour/LANG-11-tour-loop-labels-run | 17 | cold | ok / 10 | 0.185 | 0.228 | 291 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| tour/LANG-11-tour-loop-labels-run | 17 | warm | ok / 10 | 0.180 | 0.228 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| tour/LANG-11-tour-labelled-block-run | 18 | cold | ok / 10 | 0.185 | 0.237 | 291 | — / — / — | — / — / — | — | modes 0.003; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| tour/LANG-11-tour-labelled-block-run | 18 | warm | ok / 10 | 0.180 | 0.237 | 291 | — / — / — | — / — / — | — | modes 0.003; resolve 0.001; check 0.001; signatures 0.000; parse 0.000 |
| tour/LANG-8-tour-00-number-run | 8 | cold | ok / 10 | 0.900 | 5.849 | 386 | — / — / — | — / — / — | — | parse 0.666; resolve 0.007; modes 0.002; signatures 0.001; check 0.000 |
| tour/LANG-8-tour-00-number-run | 8 | warm | ok / 10 | 0.880 | 5.849 | 386 | — / — / — | — / — / — | — | parse 0.661; resolve 0.007; modes 0.002; signatures 0.001; check 0.000 |
| tour/LANG-2-tour-01-values-run | 26 | cold | ok / 10 | 0.880 | 5.879 | 386 | — / — / — | — / — / — | — | parse 0.645; resolve 0.008; modes 0.003; signatures 0.003; check 0.001 |
| tour/LANG-2-tour-01-values-run | 26 | warm | ok / 10 | 0.880 | 5.879 | 386 | — / — / — | — / — / — | — | parse 0.651; resolve 0.008; modes 0.003; signatures 0.003; check 0.001 |
| tour/OWN-4-tour-02-modes-run | 31 | cold | ok / 10 | 0.930 | 5.881 | 385 | — / — / — | — / — / — | — | parse 0.695; resolve 0.007; modes 0.004; signatures 0.002; check 0.001 |
| tour/OWN-4-tour-02-modes-run | 31 | warm | ok / 10 | 0.920 | 5.881 | 385 | — / — / — | — / — / — | — | parse 0.679; resolve 0.007; modes 0.004; signatures 0.002; check 0.001 |
| tour/LANG-8-tour-03-errors-run | 28 | cold | ok / 10 | 0.890 | 5.893 | 385 | — / — / — | — / — / — | — | parse 0.651; resolve 0.007; modes 0.005; signatures 0.002; check 0.001 |
| tour/LANG-8-tour-03-errors-run | 28 | warm | ok / 10 | 0.880 | 5.893 | 384 | — / — / — | — / — / — | — | parse 0.651; resolve 0.007; modes 0.005; signatures 0.002; check 0.001 |
| tour/FAIL-1-tour-03-track-caller-fail | 17 | cold | ok / 10 | 0.180 | 0.232 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| tour/FAIL-1-tour-03-track-caller-fail | 17 | warm | ok / 10 | 0.180 | 0.232 | 290 | — / — / — | — / — / — | — | modes 0.002; resolve 0.001; signatures 0.001; check 0.001; parse 0.000 |
| tour/LANG-2-generic-callable-context-run | 45 | cold | ok / 10 | 0.190 | 0.263 | 292 | — / — / — | — / — / — | — | modes 0.006; resolve 0.002; check 0.001; signatures 0.001; parse 0.000 |
| tour/LANG-2-generic-callable-context-run | 45 | warm | ok / 10 | 0.190 | 0.263 | 292 | — / — / — | — / — / — | — | modes 0.006; resolve 0.002; check 0.001; signatures 0.001; parse 0.000 |
| tour/LANG-8-tour-option-result-run | 23 | cold | ok / 10 | 0.890 | 5.888 | 386 | — / — / — | — / — / — | — | parse 0.647; resolve 0.007; modes 0.005; signatures 0.002; check 0.001 |
| tour/LANG-8-tour-option-result-run | 23 | warm | ok / 10 | 0.870 | 5.888 | 386 | — / — / — | — / — / — | — | parse 0.643; resolve 0.007; modes 0.005; signatures 0.002; check 0.001 |
| tour/LANG-8-tour-12-bad-input-run | 11 | cold | ok / 10 | 0.905 | 5.852 | 386 | — / — / — | — / — / — | — | parse 0.670; resolve 0.007; modes 0.002; signatures 0.001; check 0.001 |
| tour/LANG-8-tour-12-bad-input-run | 11 | warm | ok / 10 | 0.885 | 5.852 | 386 | — / — / — | — / — / — | — | parse 0.660; resolve 0.007; modes 0.002; signatures 0.001; check 0.000 |
| tour/LANG-8-tour-12-valid-input-run | 10 | cold | ok / 10 | 0.905 | 5.852 | 386 | — / — / — | — / — / — | — | parse 0.670; resolve 0.007; modes 0.002; signatures 0.001; check 0.000 |
| tour/LANG-8-tour-12-valid-input-run | 10 | warm | ok / 10 | 0.880 | 5.852 | 386 | — / — / — | — / — / — | — | parse 0.659; resolve 0.007; modes 0.002; signatures 0.001; check 0.000 |
| tour/LANG-13-tour-patterns-run | 40 | cold | ok / 10 | 0.885 | 5.879 | 386 | — / — / — | — / — / — | — | parse 0.640; resolve 0.008; modes 0.005; signatures 0.001; check 0.001 |
| tour/LANG-13-tour-patterns-run | 40 | warm | ok / 10 | 0.870 | 5.879 | 386 | — / — / — | — / — / — | — | parse 0.647; resolve 0.008; modes 0.005; signatures 0.001; check 0.001 |
| tour/SEM-5-tour-04-collections-run | 25 | cold | ok / 10 | 0.905 | 5.887 | 385 | — / — / — | — / — / — | — | parse 0.660; resolve 0.007; modes 0.006; signatures 0.001; check 0.001 |
| tour/SEM-5-tour-04-collections-run | 25 | warm | ok / 10 | 0.865 | 5.887 | 385 | — / — / — | — / — / — | — | parse 0.642; resolve 0.007; modes 0.006; signatures 0.001; check 0.001 |
| tour/SEM-3-tour-05-text-run | 36 | cold | ok / 10 | 0.910 | 5.924 | 386 | — / — / — | — / — / — | — | parse 0.666; modes 0.010; resolve 0.007; signatures 0.002; check 0.002 |
| tour/SEM-3-tour-05-text-run | 36 | warm | ok / 10 | 0.880 | 5.924 | 386 | — / — / — | — / — / — | — | parse 0.649; modes 0.010; resolve 0.007; signatures 0.002; check 0.002 |
| tour/SEM-3-tour-05-chars-run | 38 | cold | ok / 10 | 0.900 | 5.928 | 388 | — / — / — | — / — / — | — | parse 0.653; modes 0.010; resolve 0.008; signatures 0.003; check 0.002 |
| tour/SEM-3-tour-05-chars-run | 38 | warm | ok / 10 | 0.880 | 5.928 | 388 | — / — / — | — / — / — | — | parse 0.643; modes 0.010; resolve 0.008; signatures 0.003; check 0.002 |
| tour/SEM-3-tour-14-fstrings-run | 37 | cold | ok / 10 | 0.890 | 5.924 | 387 | — / — / — | — / — / — | — | parse 0.651; modes 0.009; resolve 0.008; signatures 0.003; check 0.002 |
| tour/SEM-3-tour-14-fstrings-run | 37 | warm | ok / 10 | 0.875 | 5.924 | 387 | — / — / — | — / — / — | — | parse 0.641; modes 0.009; resolve 0.008; signatures 0.003; check 0.002 |
| tour/SEM-3-tour-15-format-run | 39 | cold | ok / 10 | 0.890 | 5.980 | 388 | — / — / — | — / — / — | — | parse 0.644; modes 0.019; signatures 0.010; check 0.002; resolve 0.001 |
| tour/SEM-3-tour-15-format-run | 39 | warm | ok / 10 | 0.885 | 5.980 | 388 | — / — / — | — / — / — | — | parse 0.643; modes 0.019; signatures 0.010; check 0.002; resolve 0.001 |
| tour/LANG-8-tour-06-json-run | 11 | cold | ok / 10 | 0.900 | 5.883 | 386 | — / — / — | — / — / — | — | parse 0.658; resolve 0.007; modes 0.004; signatures 0.003; check 0.001 |
| tour/LANG-8-tour-06-json-run | 11 | warm | ok / 10 | 0.885 | 5.883 | 386 | — / — / — | — / — / — | — | parse 0.656; resolve 0.008; modes 0.004; signatures 0.003; check 0.001 |
| tour/LANG-8-tour-07-path-time-run | 18 | cold | ok / 10 | 0.885 | 5.903 | 385 | — / — / — | — / — / — | — | parse 0.653; resolve 0.007; modes 0.005; signatures 0.005; check 0.001 |
| tour/LANG-8-tour-07-path-time-run | 18 | warm | ok / 10 | 0.880 | 5.903 | 385 | — / — / — | — / — / — | — | parse 0.645; resolve 0.007; modes 0.005; signatures 0.005; check 0.001 |
| tour/CT-2-closed-frame-size-run | 19 | cold | ok / 10 | 0.195 | 0.267 | 298 | — / — / — | — / — / — | — | verify 0.003; modes 0.003; lower 0.002; signatures 0.002; resolve 0.001 |
| tour/CT-2-closed-frame-size-run | 19 | warm | ok / 10 | 0.190 | 0.267 | 299 | — / — / — | — / — / — | — | verify 0.003; modes 0.003; lower 0.002; signatures 0.002; resolve 0.001 |
| tour/LANG-9-tour-08-http-run | 41 | cold | ok / 10 | 0.925 | 6.158 | 388 | — / — / — | — / — / — | — | parse 0.661; signatures 0.025; modes 0.015; resolve 0.008; check 0.002 |
| tour/LANG-9-tour-08-http-run | 41 | warm | ok / 10 | 0.905 | 6.158 | 388 | — / — / — | — / — / — | — | parse 0.646; signatures 0.025; modes 0.015; resolve 0.008; check 0.002 |
| tour/LANG-9-tour-11-file-errors-run | 17 | cold | ok / 10 | 0.895 | 5.881 | 384 | — / — / — | — / — / — | — | parse 0.660; resolve 0.007; modes 0.005; check 0.001; signatures 0.000 |
| tour/LANG-9-tour-11-file-errors-run | 17 | warm | ok / 10 | 0.890 | 5.881 | 384 | — / — / — | — / — / — | — | parse 0.663; resolve 0.007; modes 0.005; check 0.001; signatures 0.000 |
| tour/LANG-9-tour-09-files-bufio-run | 19 | cold | ok / 10 | 0.880 | 5.908 | 385 | — / — / — | — / — / — | — | parse 0.641; resolve 0.007; modes 0.007; signatures 0.003; check 0.001 |
| tour/LANG-9-tour-09-files-bufio-run | 19 | warm | ok / 10 | 0.860 | 5.908 | 385 | — / — / — | — / — / — | — | parse 0.641; resolve 0.007; modes 0.007; signatures 0.003; check 0.001 |
| tour/CAP-1-tour-10-inputs-run | 27 | cold | ok / 10 | 0.880 | 5.914 | 385 | — / — / — | — / — / — | — | parse 0.643; modes 0.008; resolve 0.007; signatures 0.002; check 0.001 |
| tour/CAP-1-tour-10-inputs-run | 27 | warm | ok / 10 | 0.875 | 5.914 | 385 | — / — / — | — / — / — | — | parse 0.645; modes 0.008; resolve 0.007; signatures 0.002; check 0.001 |
| tour/CONC-2-tour-11-concurrency-run | 37 | cold | ok / 10 | 0.900 | 5.912 | 388 | — / — / — | — / — / — | — | parse 0.653; modes 0.009; resolve 0.008; signatures 0.002; check 0.002 |
| tour/CONC-2-tour-11-concurrency-run | 37 | warm | ok / 10 | 0.880 | 5.912 | 388 | — / — / — | — / — / — | — | parse 0.649; modes 0.009; resolve 0.008; signatures 0.002; check 0.002 |
| tour/CONC-2-tour-11-task-all-run | 54 | cold | ok / 10 | 0.900 | 5.949 | 388 | — / — / — | — / — / — | — | parse 0.651; modes 0.011; resolve 0.008; signatures 0.003; check 0.002 |
| tour/CONC-2-tour-11-task-all-run | 54 | warm | ok / 10 | 0.870 | 5.949 | 388 | — / — / — | — / — / — | — | parse 0.638; modes 0.011; resolve 0.008; signatures 0.003; check 0.002 |
| tour/MEM-11-tour-11-shared-config-run | 76 | cold | ok / 10 | 0.900 | 5.931 | 388 | — / — / — | — / — / — | — | parse 0.654; modes 0.009; resolve 0.008; signatures 0.004; check 0.002 |
| tour/MEM-11-tour-11-shared-config-run | 76 | warm | ok / 10 | 0.880 | 5.931 | 387 | — / — / — | — / — / — | — | parse 0.649; modes 0.009; resolve 0.009; signatures 0.004; check 0.002 |
| tour/CONC-3-tour-11-deadline-run | 40 | cold | ok / 10 | 0.885 | 5.930 | 387 | — / — / — | — / — / — | — | parse 0.638; resolve 0.008; modes 0.008; signatures 0.004; check 0.002 |
| tour/CONC-3-tour-11-deadline-run | 40 | warm | ok / 10 | 0.865 | 5.930 | 387 | — / — / — | — / — / — | — | parse 0.631; resolve 0.008; modes 0.008; signatures 0.004; check 0.002 |
| tour/CAP-1-tour-16-processes-run | 47 | cold | ok / 10 | 0.890 | 5.967 | 387 | — / — / — | — / — / — | — | parse 0.639; modes 0.015; resolve 0.008; check 0.002; signatures 0.000 |
| tour/CAP-1-tour-16-processes-run | 47 | warm | ok / 10 | 0.885 | 5.967 | 387 | — / — / — | — / — / — | — | parse 0.642; modes 0.015; resolve 0.008; check 0.002; signatures 0.000 |
| tour/MEM-4-tour-12-arenas-run | 16 | cold | ok / 10 | 0.885 | 5.865 | 386 | — / — / — | — / — / — | — | parse 0.641; resolve 0.008; modes 0.003; signatures 0.001; check 0.001 |
| tour/MEM-4-tour-12-arenas-run | 16 | warm | ok / 10 | 0.860 | 5.865 | 386 | — / — / — | — / — / — | — | parse 0.640; resolve 0.007; modes 0.003; signatures 0.001; check 0.001 |
| tour/LANG-9-tour-13-testing-run | 22 | cold | ok / 10 | 0.900 | 5.891 | 385 | — / — / — | — / — / — | — | parse 0.649; resolve 0.007; signatures 0.004; modes 0.004; check 0.001 |
| tour/LANG-9-tour-13-testing-run | 22 | warm | ok / 10 | 0.875 | 5.891 | 385 | — / — / — | — / — / — | — | parse 0.645; resolve 0.007; signatures 0.004; modes 0.004; check 0.001 |
| tour/SEM-3-tour-14-wire-run | 19 | cold | ok / 10 | 0.890 | 5.885 | 386 | — / — / — | — / — / — | — | parse 0.653; resolve 0.007; signatures 0.004; modes 0.003; check 0.001 |
| tour/SEM-3-tour-14-wire-run | 19 | warm | ok / 10 | 0.870 | 5.885 | 386 | — / — / — | — / — / — | — | parse 0.648; resolve 0.006; signatures 0.004; modes 0.003; check 0.001 |
| tour/PROG-4-tour-17-ffi-ok | 20 | cold | ok / 10 | 0.880 | 5.877 | 386 | — / — / — | — / — / — | — | parse 0.639; resolve 0.007; modes 0.004; signatures 0.002; check 0.001 |
| tour/PROG-4-tour-17-ffi-ok | 20 | warm | ok / 10 | 0.865 | 5.877 | 387 | — / — / — | — / — / — | — | parse 0.642; resolve 0.007; modes 0.004; signatures 0.002; check 0.001 |
| stress/nest-8 | 52 | cold | ok / 10 | 0.240 | 0.739 | 292 | 0.130 / 0.254 / 68 | — / 2.91x / 4.31x | 3.02 | modes 0.044; signatures 0.006; check 0.005; resolve 0.003; parse 0.000 |
| stress/nest-8 | 52 | warm | ok / 10 | 0.240 | 0.739 | 292 | 0.100 / 0.228 / 30 | — / 3.24x / 9.70x | 3.37 | modes 0.044; signatures 0.006; check 0.005; resolve 0.002; parse 0.000 |
| stress/nest-16 | 92 | cold | ok / 10 | 0.480 | 3.271 | 297 | 0.120 / 0.263 / 68 | — / 12.42x / 4.36x | 12.69 | modes 0.254; check 0.024; signatures 0.022; resolve 0.005; parse 0.000 |
| stress/nest-16 | 92 | warm | ok / 10 | 0.485 | 3.271 | 297 | 0.100 / 0.228 / 30 | — / 14.36x / 9.80x | 14.67 | modes 0.255; check 0.024; signatures 0.022; resolve 0.005; parse 0.000 |
| stress/nest-32 | 172 | cold | ok / 10 | 2.135 | 20.744 | 311 | 0.120 / 0.292 / 69 | — / 71.03x / 4.53x | 71.85 | modes 1.682; check 0.144; signatures 0.111; resolve 0.013; parse 0.001 |
| stress/nest-32 | 172 | warm | ok / 10 | 2.140 | 20.744 | 311 | 0.100 / 0.228 / 30 | — / 91.07x / 10.38x | 92.13 | modes 1.687; check 0.145; signatures 0.112; resolve 0.013; parse 0.001 |
| stress/instances-32 | 205 | cold | ok / 10 | 0.210 | 0.473 | 297 | 0.135 / 0.266 / 75 | — / 1.78x / 3.96x | 2.06 | modes 0.021; signatures 0.006; resolve 0.005; check 0.004; parse 0.001 |
| stress/instances-32 | 205 | warm | ok / 10 | 0.215 | 0.473 | 297 | 0.090 / 0.228 / 30 | — / 2.08x / 9.85x | 2.41 | modes 0.021; signatures 0.006; resolve 0.005; check 0.004; parse 0.001 |
| stress/instances-128 | 781 | cold | ok / 10 | 0.330 | 1.280 | 318 | 0.130 / 0.315 / 77 | — / 4.07x / 4.14x | 4.74 | modes 0.087; signatures 0.023; resolve 0.017; check 0.017; parse 0.002 |
| stress/instances-128 | 781 | warm | ok / 10 | 0.325 | 1.280 | 318 | 0.100 / 0.228 / 30 | — / 5.62x / 10.62x | 6.55 | modes 0.087; signatures 0.023; check 0.017; resolve 0.017; parse 0.002 |
| stress/wide-2000 | 8044 | cold | ok / 10 | 1.130 | 5.807 | 407 | 0.335 / 1.388 / 98 | — / 4.18x / 4.17x | 4.18 | signatures 0.266; modes 0.234; resolve 0.226; check 0.089; parse 0.055 |
| stress/wide-2000 | 8044 | warm | ok / 10 | 1.125 | 5.807 | 407 | 0.130 / 0.343 / 30 | — / 16.94x / 13.46x | 16.94 | signatures 0.267; modes 0.236; resolve 0.227; check 0.090; parse 0.056 |
| stress/long-2000 | 2004 | cold | ok / 10 | 0.440 | 1.754 | 390 | 0.180 / 0.701 / 95 | — / 2.50x / 4.08x | 2.50 | modes 0.211; check 0.030; parse 0.005; resolve 0.002; signatures 0.000 |
| stress/long-2000 | 2004 | warm | ok / 10 | 0.440 | 1.754 | 390 | 0.100 / 0.228 / 30 | — / 7.70x / 12.99x | 7.70 | modes 0.211; check 0.030; parse 0.005; resolve 0.002; signatures 0.000 |
| stress/arms-200 | 211 | cold | ok / 10 | 0.200 | 0.350 | 294 | 0.120 / 0.270 / 68 | — / 1.30x / 4.31x | 2.53 | check 0.007; modes 0.007; resolve 0.002; signatures 0.001; parse 0.000 |
| stress/arms-200 | 211 | warm | ok / 10 | 0.200 | 0.350 | 294 | 0.090 / 0.228 / 30 | — / 1.54x / 9.73x | 3.00 | check 0.007; modes 0.007; resolve 0.002; signatures 0.001; parse 0.000 |
| stress/nested-match-16 | 104 | cold | failed / 0 | 0.180 | — | 290 | 0.120 / 0.257 / 74 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-16 | 104 | warm | failed / 0 | 0.200 | — | 291 | 0.100 / 0.228 / 30 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-32 | 200 | cold | failed / 0 | 0.180 | — | 291 | 0.125 / 0.266 / 75 | — / — / — | — | resolve 0.001; modes 0.001; parse 0.000; signatures 0.000; check 0.000 |
| stress/nested-match-32 | 200 | warm | failed / 0 | 0.180 | — | 290 | 0.100 / 0.228 / 30 | — / — / — | — | resolve 0.001; modes 0.001; parse 0.000; signatures 0.000; check 0.000 |
| stress/table-2000 | 2010 | cold | failed / 0 | 3.970 | — | 3988 | 0.145 / 0.391 / 88 | — / — / — | — | — |
| stress/table-2000 | 2010 | warm | failed / 0 | 3.950 | — | 3987 | 0.100 / 0.228 / 30 | — / — / — | — | — |
| stress/derive-64 | 579 | cold | failed / 0 | 0.160 | — | 272 | 0.225 / 0.727 / 100 | — / — / — | — | — |
| stress/derive-64 | 579 | warm | failed / 0 | 0.160 | — | 272 | 0.100 / 0.228 / 30 | — / — / — | — | — |
| stress/closures-200 | 805 | cold | ok / 10 | 0.410 | 1.499 | 320 | 0.170 / 0.513 / 81 | — / 2.92x / 3.92x | 1.47 | modes 0.187; check 0.009; resolve 0.004; parse 0.002; signatures 0.000 |
| stress/closures-200 | 805 | warm | ok / 10 | 0.405 | 1.499 | 320 | 0.105 / 0.228 / 30 | — / 6.58x / 10.66x | 3.31 | modes 0.191; check 0.009; resolve 0.004; parse 0.002; signatures 0.000 |
| syn3k | 3066 | cold | ok / 10 | 4.405 | 30.499 | 717 | 0.350 / 1.216 / 147 | — / 25.08x / 4.88x | 23.85 | signatures 1.561; modes 1.051; parse 0.717; verify 0.388; check 0.189 |
| syn3k | 3066 | warm | ok / 10 | 4.395 | 30.499 | 717 | 0.100 / 0.231 / 30 | — / 131.83x / 23.72x | 125.33 | signatures 1.556; modes 1.051; parse 0.724; verify 0.389; check 0.189 |
| syn10k | 10274 | cold | ok / 7 | 10.410 | 72.503 | 1083 | 0.780 / 3.176 / 190 | — / 22.82x / 5.70x | 21.82 | modes 3.642; signatures 3.102; verify 1.074; parse 0.938; check 0.634 |
| syn10k | 10274 | warm | ok / 7 | 10.360 | 72.502 | 1083 | 0.100 / 0.240 / 30 | — / 301.93x / 36.12x | 288.59 | modes 3.631; signatures 3.090; verify 1.066; parse 0.936; check 0.635 |
## debug

| input | lines | cache | status / n | wall s | instructions G | RSS MiB | Rust wall s / instructions G / RSS MiB | H/R wall / instructions / RSS | H/R instructions per line | top five phases by self time (s) |
|---|---:|---|---|---:|---:|---:|---|---|---:|---|
| hello | 7 | cold | ok / 10 | 2.030 | 10.931 | 638 | 0.180 / 0.362 / 95 | 11.28x / 30.17x / 6.71x | 12.93 | parse 0.746; signatures 0.669; plan 0.069; verify 0.027; lower 0.023 |
| hello | 7 | warm | ok / 10 | 1.995 | 10.931 | 638 | 0.100 / 0.228 / 30 | 19.95x / 47.91x / 21.09x | 20.53 | parse 0.763; signatures 0.662; plan 0.068; verify 0.027; lower 0.023 |
| todo-api | 499 | cold | ok / 10 | 27.505 | 244.251 | 1033 | 0.795 / 2.989 / 173 | 34.60x / 81.72x / 5.97x | 81.56 | lir validate 8.632; verify 7.261; codegen 4.999; signatures 1.852; import 1.081 |
| todo-api | 499 | warm | ok / 10 | 28.010 | 244.254 | 1041 | 0.110 / 0.229 / 30 | 254.64x / 1067.16x / 34.40x | 1065.02 | lir validate 8.709; verify 7.296; codegen 4.825; signatures 1.916; import 1.090 |
| kvstore | 371 | cold | ok / 10 | 26.855 | 202.271 | 962 | 1.240 / 2.993 / 170 | 21.66x / 67.57x / 5.64x | 69.39 | lir validate 8.555; verify 6.993; codegen 4.451; signatures 1.884; import 1.058 |
| kvstore | 371 | warm | ok / 10 | 31.610 | 202.266 | 962 | 0.140 / 0.229 / 30 | 225.79x / 883.69x / 31.81x | 907.51 | lir validate 10.519; verify 7.084; codegen 5.411; signatures 1.950; import 1.205 |
| logq | 301 | cold | ok / 10 | 11.025 | 62.367 | 755 | 0.805 / 1.758 / 154 | 13.70x / 35.49x / 4.90x | 30.18 | lir validate 2.432; verify 1.833; signatures 1.504; parse 1.382; codegen 1.360 |
| logq | 301 | warm | ok / 10 | 13.030 | 62.366 | 755 | 0.150 / 0.228 / 30 | 86.87x / 273.29x / 24.97x | 232.43 | lir validate 2.878; verify 2.127; signatures 1.797; parse 1.405; codegen 1.222 |
| csvjson | 285 | cold | ok / 10 | 14.760 | 91.072 | 812 | 0.845 / 2.473 / 169 | 17.47x / 36.83x / 4.80x | 40.06 | lir validate 3.600; verify 3.203; codegen 1.814; signatures 1.357; parse 1.171 |
| csvjson | 285 | warm | ok / 10 | 14.550 | 91.076 | 811 | 0.125 / 0.229 / 30 | 116.40x / 397.66x / 26.83x | 432.54 | lir validate 3.618; verify 3.238; codegen 1.914; signatures 1.371; parse 1.120 |
| mdtoc | 189 | cold | ok / 10 | 5.640 | 27.109 | 703 | 0.720 / 1.588 / 148 | 7.83x / 17.08x / 4.74x | 14.82 | parse 1.294; signatures 1.036; verify 0.840; lir validate 0.721; codegen 0.463 |
| mdtoc | 189 | warm | ok / 10 | 5.955 | 27.109 | 703 | 0.150 / 0.228 / 30 | 39.70x / 118.80x / 23.34x | 103.09 | parse 1.395; signatures 1.038; verify 0.852; lir validate 0.738; codegen 0.397 |
| bench-lite | 215 | cold | ok / 10 | 6.095 | 33.720 | 707 | 0.570 / 1.269 / 143 | 10.69x / 26.56x / 4.93x | 25.95 | signatures 1.414; parse 1.187; verify 0.867; lir validate 0.866; codegen 0.415 |
| bench-lite | 215 | warm | ok / 10 | 6.520 | 33.720 | 706 | 0.150 / 0.228 / 30 | 43.47x / 147.80x / 23.26x | 144.37 | signatures 1.439; parse 1.167; lir validate 0.943; verify 0.912; codegen 0.452 |
| tasks | 3036 | cold | failed / 0 | 160.240 | — | 1243 | 2.950 / 12.783 / 294 | — / — / — | — | lir validate 81.496; verify 26.086; signatures 12.189; codegen 8.799; import 7.924 |
| tasks | 3036 | warm | failed / 0 | 144.660 | — | 1244 | 0.160 / 0.404 / 46 | — / — / — | — | lir validate 73.861; verify 25.059; signatures 11.742; codegen 6.660; import 6.329 |
| hello-web | 34 | cold | ok / 10 | 16.620 | 149.711 | 897 | — / — / — | — / — / — | — | verify 5.065; lir validate 4.223; codegen 3.005; signatures 1.098; parse 0.670 |
| hello-web | 34 | warm | ok / 10 | 16.465 | 149.711 | 898 | — / — / — | — / — / — | — | verify 5.095; lir validate 4.187; codegen 2.975; signatures 1.087; ready 0.655 |
| stress/nest-8 | 52 | cold | ok / 10 | 0.580 | 2.701 | 354 | 0.175 / 0.370 / 92 | — / 7.29x / 3.82x | 7.57 | verify 0.089; modes 0.049; signatures 0.034; lir validate 0.024; codegen 0.011 |
| stress/nest-8 | 52 | warm | ok / 10 | 0.580 | 2.701 | 354 | 0.100 / 0.228 / 30 | — / 11.84x / 11.79x | 12.29 | verify 0.089; modes 0.048; signatures 0.034; lir validate 0.025; codegen 0.011 |
| stress/nest-16 | 92 | cold | ok / 10 | 1.175 | 8.586 | 359 | 0.170 / 0.383 / 93 | — / 22.42x / 3.87x | 22.91 | verify 0.339; modes 0.265; signatures 0.084; lir validate 0.050; check 0.031 |
| stress/nest-16 | 92 | warm | ok / 10 | 1.180 | 8.586 | 359 | 0.105 / 0.228 / 30 | — / 37.64x / 11.96x | 38.46 | verify 0.342; modes 0.264; signatures 0.084; lir validate 0.050; check 0.031 |
| stress/nest-32 | 172 | cold | ok / 10 | 4.105 | 39.250 | 378 | 0.175 / 0.419 / 93 | — / 93.70x / 4.05x | 94.79 | modes 1.710; verify 1.383; signatures 0.260; check 0.163; lir validate 0.113 |
| stress/nest-32 | 172 | warm | ok / 10 | 4.110 | 39.250 | 378 | 0.090 / 0.228 / 30 | — / 172.06x / 12.54x | 174.06 | modes 1.703; verify 1.394; signatures 0.260; check 0.162; lir validate 0.113 |
| stress/instances-32 | 205 | cold | ok / 10 | 0.720 | 4.354 | 363 | 0.185 / 0.405 / 99 | — / 10.74x / 3.65x | 12.47 | lir validate 0.146; verify 0.063; codegen 0.046; modes 0.028; signatures 0.027 |
| stress/instances-32 | 205 | warm | ok / 10 | 0.710 | 4.354 | 363 | 0.090 / 0.228 / 30 | — / 19.09x / 12.06x | 22.16 | lir validate 0.145; verify 0.062; codegen 0.045; modes 0.028; signatures 0.027 |
| stress/instances-128 | 781 | cold | ok / 10 | 2.650 | 27.904 | 390 | 0.190 / 0.529 / 104 | — / 52.73x / 3.74x | 61.44 | lir validate 1.252; verify 0.265; codegen 0.232; import 0.138; modes 0.112 |
| stress/instances-128 | 781 | warm | ok / 10 | 2.650 | 27.904 | 390 | 0.100 / 0.228 / 30 | — / 122.31x / 12.96x | 142.51 | lir validate 1.249; verify 0.262; codegen 0.234; import 0.137; modes 0.112 |
| stress/wide-2000 | 8044 | cold | ok / 10 | 10.390 | 97.447 | 802 | 0.405 / 1.571 / 121 | — / 62.01x / 6.65x | 62.01 | lir validate 4.682; codegen 1.517; signatures 0.976; verify 0.590; modes 0.373 |
| stress/wide-2000 | 8044 | warm | ok / 10 | 10.305 | 97.447 | 802 | 0.130 / 0.343 / 30 | — / 283.97x / 26.52x | 283.97 | lir validate 4.643; codegen 1.503; signatures 0.977; verify 0.583; modes 0.371 |
| stress/long-2000 | 2004 | cold | ok / 10 | 15.090 | 198.167 | 580 | 0.300 / 1.308 / 130 | — / 151.56x / 4.46x | 151.56 | verify 8.720; import 3.852; lir validate 0.649; ready 0.414; codegen 0.357 |
| stress/long-2000 | 2004 | warm | ok / 10 | 14.950 | 198.167 | 594 | 0.095 / 0.228 / 30 | — / 868.74x / 19.63x | 868.74 | verify 8.654; import 3.813; lir validate 0.659; ready 0.417; codegen 0.353 |
| stress/arms-200 | 211 | cold | ok / 10 | 0.650 | 3.646 | 389 | 0.170 / 0.392 / 94 | — / 9.29x / 4.15x | 18.14 | lir validate 0.123; codegen 0.039; ready 0.035; verify 0.033; rc 0.022 |
| stress/arms-200 | 211 | warm | ok / 10 | 0.650 | 3.646 | 389 | 0.095 / 0.228 / 30 | — / 15.98x / 12.92x | 31.21 | lir validate 0.123; codegen 0.040; ready 0.035; verify 0.033; rc 0.023 |
| stress/nested-match-16 | 104 | cold | failed / 0 | 0.180 | — | 290 | 0.190 / 0.376 / 98 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-16 | 104 | warm | failed / 0 | 0.180 | — | 290 | 0.090 / 0.228 / 30 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-32 | 200 | cold | failed / 0 | 0.190 | — | 290 | 0.180 / 0.391 / 99 | — / — / — | — | resolve 0.001; modes 0.001; parse 0.000; signatures 0.000; check 0.000 |
| stress/nested-match-32 | 200 | warm | failed / 0 | 0.190 | — | 290 | 0.100 / 0.228 / 30 | — / — / — | — | resolve 0.001; modes 0.001; parse 0.000; signatures 0.000; check 0.000 |
| stress/table-2000 | 2010 | cold | failed / 0 | 4.060 | — | 3987 | 0.200 / 0.578 / 114 | — / — / — | — | — |
| stress/table-2000 | 2010 | warm | failed / 0 | 3.930 | — | 3987 | 0.095 / 0.228 / 30 | — / — / — | — | — |
| stress/derive-64 | 579 | cold | failed / 0 | 0.160 | — | 272 | 0.375 / 1.446 / 133 | — / — / — | — | — |
| stress/derive-64 | 579 | warm | failed / 0 | 0.170 | — | 272 | 0.100 / 0.228 / 30 | — / — / — | — | — |
| stress/closures-200 | 805 | cold | ok / 10 | 3.730 | 34.617 | 425 | 0.255 / 0.850 / 109 | — / 40.73x / 3.90x | 20.49 | lir validate 1.506; codegen 0.464; verify 0.437; import 0.304; modes 0.199 |
| stress/closures-200 | 805 | warm | ok / 10 | 3.745 | 34.617 | 425 | 0.100 / 0.228 / 30 | — / 151.77x / 14.18x | 76.36 | lir validate 1.520; codegen 0.454; verify 0.443; import 0.318; modes 0.198 |
| syn3k | 3066 | cold | ok / 10 | 65.235 | 661.723 | 1455 | 1.795 / 8.695 / 229 | — / 76.11x / 6.36x | 72.36 | lir validate 28.060; verify 11.297; codegen 10.358; signatures 4.199; import 2.781 |
| syn3k | 3066 | warm | ok / 10 | 65.060 | 661.726 | 1455 | 0.100 / 0.232 / 30 | — / 2855.21x / 48.11x | 2714.59 | lir validate 28.043; verify 11.408; codegen 10.196; signatures 4.255; import 2.786 |
| syn10k | 10274 | cold | ok / 7 | 160.710 | 1714.612 | 2357 | 3.700 / 18.681 / 292 | — / 91.78x / 8.09x | 87.73 | lir validate 80.878; codegen 23.654; verify 18.758; signatures 11.009; import 6.964 |
| syn10k | 10274 | warm | ok / 7 | 161.580 | 1714.603 | 2358 | 0.100 / 0.241 / 30 | — / 7124.39x / 77.94x | 6809.57 | lir validate 81.419; codegen 23.609; verify 18.787; signatures 10.950; import 7.050 |
## release

| input | lines | cache | status / n | wall s | instructions G | RSS MiB | Rust wall s / instructions G / RSS MiB | H/R wall / instructions / RSS | H/R instructions per line | top five phases by self time (s) |
|---|---:|---|---|---:|---:|---:|---|---|---:|---|
| hello | 7 | cold | ok / 10 | 1.870 | 10.743 | 632 | 0.175 / 0.308 / 90 | 10.69x / 34.88x / 6.99x | 14.95 | parse 0.711; signatures 0.661; plan 0.069; verify 0.026; lower 0.022 |
| hello | 7 | warm | ok / 10 | 1.895 | 10.743 | 632 | 0.105 / 0.228 / 30 | 18.05x / 47.08x / 20.90x | 20.18 | parse 0.719; signatures 0.671; plan 0.069; verify 0.027; lower 0.023 |
| todo-api | 499 | cold | ok / 10 | 32.575 | 263.358 | 932 | 1.880 / 8.997 / 174 | 17.33x / 29.27x / 5.36x | 29.21 | codegen 9.978; lir validate 8.803; verify 7.229; signatures 1.909; import 1.087 |
| todo-api | 499 | warm | ok / 10 | 32.125 | 263.364 | 934 | 0.100 / 0.229 / 30 | 321.25x / 1150.47x / 30.89x | 1148.17 | codegen 9.274; lir validate 8.891; verify 7.145; signatures 1.881; import 1.108 |
| kvstore | 371 | cold | ok / 10 | 36.050 | 218.522 | 911 | 2.675 / 8.974 / 166 | 13.48x / 24.35x / 5.47x | 25.01 | codegen 9.934; lir validate 9.924; verify 7.579; signatures 2.221; import 1.183 |
| kvstore | 371 | warm | ok / 10 | 33.290 | 218.523 | 913 | 0.135 / 0.229 / 30 | 246.59x / 954.71x / 30.18x | 980.45 | lir validate 9.419; codegen 8.776; verify 7.306; signatures 2.018; import 1.167 |
| logq | 301 | cold | ok / 10 | 14.350 | 70.799 | 738 | 1.775 / 5.610 / 156 | 8.08x / 12.62x / 4.72x | 10.73 | codegen 3.679; lir validate 2.630; verify 2.008; signatures 1.623; parse 1.339 |
| logq | 301 | warm | ok / 10 | 14.365 | 70.800 | 738 | 0.155 / 0.228 / 30 | 92.68x / 310.20x / 24.61x | 263.83 | codegen 3.621; lir validate 2.684; verify 1.973; signatures 1.560; parse 1.393 |
| csvjson | 285 | cold | ok / 10 | 17.690 | 104.511 | 784 | 1.655 / 6.335 / 166 | 10.69x / 16.50x / 4.73x | 17.94 | codegen 5.073; lir validate 3.434; verify 3.409; signatures 1.443; parse 1.081 |
| csvjson | 285 | warm | ok / 10 | 19.090 | 104.510 | 784 | 0.140 / 0.229 / 30 | 136.36x / 456.25x / 26.14x | 496.28 | codegen 5.675; lir validate 3.767; verify 3.114; signatures 1.362; parse 1.085 |
| mdtoc | 189 | cold | ok / 10 | 6.510 | 29.768 | 691 | 0.930 / 2.912 / 145 | 7.00x / 10.22x / 4.77x | 8.87 | parse 1.341; codegen 1.111; signatures 1.048; verify 0.848; lir validate 0.730 |
| mdtoc | 189 | warm | ok / 10 | 6.250 | 29.767 | 691 | 0.155 / 0.228 / 30 | 40.32x / 130.44x / 22.84x | 113.18 | parse 1.277; codegen 1.084; signatures 1.027; verify 0.784; lir validate 0.748 |
| bench-lite | 215 | cold | ok / 10 | 7.055 | 38.038 | 695 | 0.730 / 2.159 / 141 | 9.66x / 17.62x / 4.93x | 17.21 | signatures 1.416; codegen 1.359; parse 1.159; lir validate 0.898; verify 0.892 |
| bench-lite | 215 | warm | ok / 10 | 6.855 | 38.036 | 695 | 0.160 / 0.229 / 30 | 42.84x / 166.40x / 23.08x | 162.53 | codegen 1.390; signatures 1.326; parse 1.117; lir validate 0.849; verify 0.837 |
| tasks | 3036 | cold | ok / 10 | 84.275 | 725.705 | 1393 | 15.535 / 82.190 / 427 | — / 8.83x / 3.26x | 3.52 | lir validate 31.206; codegen 22.306; verify 12.149; signatures 6.143; import 3.106 |
| tasks | 3036 | warm | ok / 10 | 86.230 | 725.674 | 1393 | 0.155 / 0.403 / 46 | — / 1798.54x / 30.28x | 717.99 | lir validate 32.601; codegen 23.072; verify 12.206; signatures 6.071; import 3.199 |
| hello-web | 34 | cold | ok / 10 | 18.970 | 161.184 | 861 | — / — / — | — / — / — | — | codegen 5.550; verify 5.076; lir validate 4.191; signatures 1.092; parse 0.656 |
| hello-web | 34 | warm | ok / 10 | 19.090 | 161.188 | 861 | — / — / — | — / — / — | — | codegen 5.627; verify 5.092; lir validate 4.210; signatures 1.086; parse 0.665 |
| stress/nest-8 | 52 | cold | ok / 10 | 0.510 | 2.523 | 347 | 0.165 / 0.314 / 89 | — / 8.05x / 3.91x | 8.36 | verify 0.090; modes 0.049; signatures 0.034; lir validate 0.025; codegen 0.015 |
| stress/nest-8 | 52 | warm | ok / 10 | 0.505 | 2.523 | 347 | 0.100 / 0.228 / 30 | — / 11.05x / 11.47x | 11.48 | verify 0.090; modes 0.048; signatures 0.034; lir validate 0.024; codegen 0.015 |
| stress/nest-16 | 92 | cold | ok / 10 | 1.115 | 8.421 | 352 | 0.160 / 0.322 / 89 | — / 26.17x / 3.95x | 26.73 | verify 0.342; modes 0.265; signatures 0.085; lir validate 0.050; check 0.031 |
| stress/nest-16 | 92 | warm | ok / 10 | 1.105 | 8.421 | 352 | 0.110 / 0.228 / 30 | — / 36.89x / 11.69x | 37.69 | verify 0.339; modes 0.265; signatures 0.084; lir validate 0.050; check 0.031 |
| stress/nest-32 | 172 | cold | ok / 10 | 4.070 | 39.111 | 370 | 0.160 / 0.347 / 89 | — / 112.58x / 4.14x | 113.88 | modes 1.722; verify 1.389; signatures 0.262; check 0.166; lir validate 0.114 |
| stress/nest-32 | 172 | warm | ok / 10 | 4.045 | 39.112 | 370 | 0.100 / 0.228 / 30 | — / 171.31x / 12.22x | 173.30 | modes 1.699; verify 1.395; signatures 0.260; check 0.161; lir validate 0.114 |
| stress/instances-32 | 205 | cold | ok / 10 | 0.650 | 4.194 | 355 | 0.160 / 0.332 / 94 | — / 12.64x / 3.78x | 14.67 | lir validate 0.147; verify 0.062; codegen 0.057; modes 0.028; signatures 0.027 |
| stress/instances-32 | 205 | warm | ok / 10 | 0.660 | 4.194 | 356 | 0.090 / 0.228 / 30 | — / 18.37x / 11.80x | 21.33 | lir validate 0.147; verify 0.063; codegen 0.056; modes 0.027; signatures 0.027 |
| stress/instances-128 | 781 | cold | ok / 10 | 2.620 | 27.871 | 378 | 0.170 / 0.401 / 97 | — / 69.49x / 3.89x | 80.97 | lir validate 1.248; codegen 0.278; verify 0.266; import 0.136; modes 0.114 |
| stress/instances-128 | 781 | warm | ok / 10 | 2.600 | 27.871 | 378 | 0.100 / 0.228 / 30 | — / 122.07x / 12.55x | 142.23 | lir validate 1.243; codegen 0.280; verify 0.263; import 0.136; modes 0.112 |
| stress/wide-2000 | 8044 | cold | ok / 10 | 12.245 | 107.117 | 576 | 0.340 / 1.301 / 114 | — / 82.31x / 5.04x | 82.31 | lir validate 4.674; codegen 3.629; signatures 0.978; verify 0.586; modes 0.373 |
| stress/wide-2000 | 8044 | warm | ok / 10 | 12.310 | 107.119 | 576 | 0.130 / 0.343 / 30 | — / 311.95x / 19.20x | 311.95 | lir validate 4.646; codegen 3.664; signatures 0.983; verify 0.584; modes 0.374 |
| stress/long-2000 | 2004 | cold | ok / 10 | 14.665 | 196.974 | 546 | 0.215 / 0.809 / 106 | — / 243.36x / 5.14x | 243.36 | verify 8.644; import 3.830; lir validate 0.657; ready 0.417; rc 0.239 |
| stress/long-2000 | 2004 | warm | ok / 10 | 14.650 | 196.974 | 546 | 0.100 / 0.228 / 30 | — / 862.82x / 18.12x | 862.82 | verify 8.595; import 3.819; lir validate 0.655; ready 0.426; rc 0.246 |
| stress/arms-200 | 211 | cold | ok / 10 | 4.835 | 70.977 | 355 | 0.150 / 0.331 / 90 | — / 214.34x / 3.96x | 418.52 | codegen 4.304; lir validate 0.122; ready 0.035; verify 0.034; rc 0.023 |
| stress/arms-200 | 211 | warm | ok / 10 | 4.820 | 70.977 | 355 | 0.100 / 0.228 / 30 | — / 310.89x / 11.77x | 607.04 | codegen 4.287; lir validate 0.122; ready 0.034; verify 0.033; rc 0.023 |
| stress/nested-match-16 | 104 | cold | failed / 0 | 0.180 | — | 290 | 0.160 / 0.317 / 94 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-16 | 104 | warm | failed / 0 | 0.180 | — | 290 | 0.090 / 0.228 / 30 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-32 | 200 | cold | failed / 0 | 0.190 | — | 291 | 0.165 / 0.325 / 94 | — / — / — | — | resolve 0.002; modes 0.001; parse 0.000; signatures 0.000; check 0.000 |
| stress/nested-match-32 | 200 | warm | failed / 0 | 0.190 | — | 290 | 0.100 / 0.228 / 30 | — / — / — | — | resolve 0.001; modes 0.001; signatures 0.000; parse 0.000; check 0.000 |
| stress/table-2000 | 2010 | cold | failed / 0 | 3.870 | — | 3987 | 0.195 / 0.570 / 110 | — / — / — | — | — |
| stress/table-2000 | 2010 | warm | failed / 0 | 3.890 | — | 3987 | 0.095 / 0.228 / 30 | — / — / — | — | — |
| stress/derive-64 | 579 | cold | failed / 0 | 0.160 | — | 272 | 0.655 / 3.080 / 146 | — / — / — | — | — |
| stress/derive-64 | 579 | warm | failed / 0 | 0.160 | — | 272 | 0.100 / 0.228 / 30 | — / — / — | — | — |
| stress/closures-200 | 805 | cold | ok / 10 | 4.745 | 42.744 | 427 | 0.190 / 0.519 / 98 | — / 82.44x / 4.35x | 41.47 | codegen 1.570; lir validate 1.507; verify 0.434; import 0.305; modes 0.200 |
| stress/closures-200 | 805 | warm | ok / 10 | 4.735 | 42.746 | 427 | 0.100 / 0.228 / 30 | — / 187.24x / 14.24x | 94.20 | codegen 1.535; lir validate 1.502; verify 0.442; import 0.304; modes 0.200 |
| syn3k | 3066 | cold | ok / 10 | 76.950 | 724.416 | 1299 | 5.915 / 32.046 / 221 | — / 22.61x / 5.87x | 21.49 | lir validate 27.720; codegen 22.687; verify 11.389; signatures 4.188; import 2.761 |
| syn3k | 3066 | warm | ok / 10 | 77.290 | 724.412 | 1310 | 0.100 / 0.232 / 30 | — / 3125.28x / 43.66x | 2971.37 | lir validate 27.858; codegen 22.693; verify 11.440; signatures 4.242; import 2.815 |
| syn10k | 10274 | cold | ok / 7 | 192.740 | 1850.447 | 2116 | 15.310 / 83.110 / 290 | — / 22.27x / 7.29x | 21.28 | lir validate 83.258; codegen 52.475; verify 18.992; signatures 11.001; import 6.979 |
| syn10k | 10274 | warm | ok / 7 | 191.660 | 1850.448 | 2117 | 0.100 / 0.241 / 30 | — / 7687.81x / 69.99x | 7348.09 | lir validate 82.415; codegen 52.353; verify 19.205; signatures 11.177; import 7.135 |
## native

| input | lines | cache | status / n | wall s | instructions G | RSS MiB | Rust wall s / instructions G / RSS MiB | H/R wall / instructions / RSS | H/R instructions per line | top five phases by self time (s) |
|---|---:|---|---|---:|---:|---:|---|---|---:|---|
| hello | 7 | cold | ok / 10 | 1.927 | 11.026 | 638 | 0.175 / 0.308 / 90 | 11.01x / 35.80x / 7.05x | 15.34 | parse 0.773; signatures 0.689; plan 0.069; verify 0.026; lower 0.023 |
| hello | 7 | warm | ok / 10 | 1.784 | 10.801 | 635 | 0.105 / 0.228 / 30 | 16.99x / 47.33x / 20.99x | 20.28 | parse 0.735; signatures 0.668; plan 0.070; verify 0.027; lower 0.023 |
| todo-api | 499 | cold | ok / 10 | 32.132 | 265.361 | 936 | 1.880 / 8.997 / 174 | 17.09x / 29.49x / 5.38x | 29.43 | —; stopped before CLI report |
| todo-api | 499 | warm | ok / 10 | 31.875 | 265.120 | 947 | 0.100 / 0.229 / 30 | 318.75x / 1158.14x / 31.32x | 1155.82 | —; stopped before CLI report |
| kvstore | 371 | cold | ok / 10 | 31.260 | 220.109 | 916 | 2.675 / 8.974 / 166 | 11.69x / 24.53x / 5.50x | 25.19 | —; stopped before CLI report |
| kvstore | 371 | warm | ok / 10 | 32.489 | 219.880 | 913 | 0.135 / 0.229 / 30 | 240.66x / 960.64x / 30.19x | 986.53 | —; stopped before CLI report |
| logq | 301 | cold | failed / 0 | 14.780 | — | 740 | 1.775 / 5.610 / 156 | — / — / — | — | codegen 3.767; lir validate 2.704; verify 2.057; signatures 1.833; parse 1.592 |
| logq | 301 | warm | failed / 0 | 14.240 | — | 739 | 0.155 / 0.228 / 30 | — / — / — | — | codegen 3.647; lir validate 2.636; verify 2.103; signatures 1.736; parse 1.470 |
| csvjson | 285 | cold | ok / 10 | 18.056 | 105.428 | 792 | 1.655 / 6.335 / 166 | 10.91x / 16.64x / 4.78x | 18.10 | codegen 5.260; lir validate 3.433; verify 3.147; signatures 1.438; parse 1.153 |
| csvjson | 285 | warm | ok / 10 | 16.983 | 105.192 | 785 | 0.140 / 0.229 / 30 | 121.31x / 459.23x / 26.18x | 499.51 | codegen 5.018; lir validate 3.327; verify 3.114; signatures 1.393; parse 1.083 |
| mdtoc | 189 | cold | failed / 0 | 6.720 | — | 698 | 0.930 / 2.912 / 145 | — / — / — | — | codegen 1.219; signatures 1.181; parse 1.079; verify 0.903; lir validate 0.846 |
| mdtoc | 189 | warm | failed / 0 | 7.590 | — | 695 | 0.155 / 0.228 / 30 | — / — / — | — | codegen 1.525; parse 1.491; signatures 1.319; verify 0.944; lir validate 0.867 |
| bench-lite | 215 | cold | ok / 10 | 7.821 | 38.536 | 704 | 0.730 / 2.159 / 141 | 10.71x / 17.85x / 4.98x | 17.44 | signatures 1.579; codegen 1.556; parse 1.294; verify 1.041; lir validate 0.906 |
| bench-lite | 215 | warm | ok / 10 | 7.430 | 38.309 | 699 | 0.160 / 0.229 / 30 | 46.44x / 167.59x / 23.19x | 163.69 | codegen 1.564; signatures 1.549; parse 1.225; verify 1.003; lir validate 0.916 |
| tasks | 3036 | cold | ok / 10 | 85.023 | 733.786 | 1398 | 15.535 / 82.190 / 427 | — / 8.93x / 3.28x | 3.56 | lir validate 31.752; codegen 21.744; verify 12.203; signatures 7.058; import 3.026 |
| tasks | 3036 | warm | ok / 10 | 83.953 | 733.517 | 1396 | 0.155 / 0.403 / 46 | — / 1817.97x / 30.35x | 725.75 | lir validate 31.019; codegen 21.856; verify 11.954; signatures 7.033; import 3.008 |
| hello-web | 34 | cold | failed / 0 | 19.888 | — | 868 | — / — / — | — / — / — | — | codegen 5.938; verify 5.252; lir validate 4.374; signatures 1.186; ready 0.680; readiness observed, then failed: excluded from startup summaries |
| hello-web | 34 | warm | failed / 0 | 19.680 | — | 866 | — / — / — | — / — / — | — | codegen 5.801; verify 5.256; lir validate 4.310; signatures 1.208; parse 0.733; readiness observed, then failed: excluded from startup summaries |
| stress/nest-8 | 52 | cold | ok / 10 | 0.640 | 2.914 | 376 | 0.165 / 0.314 / 89 | — / 9.29x / 4.24x | 9.65 | verify 0.091; modes 0.050; signatures 0.042; lir validate 0.025; codegen 0.015; silent program: wall to exit |
| stress/nest-8 | 52 | warm | ok / 10 | 0.560 | 2.689 | 376 | 0.100 / 0.228 / 30 | — / 11.78x / 12.44x | 12.23 | verify 0.090; modes 0.050; signatures 0.042; lir validate 0.025; codegen 0.015; silent program: wall to exit |
| stress/nest-16 | 92 | cold | ok / 10 | 1.235 | 8.911 | 355 | 0.160 / 0.322 / 89 | — / 27.69x / 3.99x | 28.29 | verify 0.338; modes 0.269; signatures 0.103; lir validate 0.050; check 0.034; silent program: wall to exit |
| stress/nest-16 | 92 | warm | ok / 10 | 1.155 | 8.685 | 355 | 0.110 / 0.228 / 30 | — / 38.04x / 11.79x | 38.87 | verify 0.343; modes 0.266; signatures 0.102; lir validate 0.050; check 0.033; silent program: wall to exit |
| stress/nest-32 | 172 | cold | ok / 10 | 4.230 | 39.904 | 376 | 0.160 / 0.347 / 89 | — / 114.86x / 4.21x | 116.19 | modes 1.716; verify 1.398; signatures 0.308; check 0.168; lir validate 0.114; silent program: wall to exit |
| stress/nest-32 | 172 | warm | ok / 10 | 4.170 | 39.682 | 372 | 0.100 / 0.228 / 30 | — / 173.80x / 12.30x | 175.82 | modes 1.729; verify 1.396; signatures 0.307; check 0.168; lir validate 0.114; silent program: wall to exit |
| stress/instances-32 | 205 | cold | ok / 10 | 0.760 | 4.588 | 356 | 0.160 / 0.332 / 94 | — / 13.82x / 3.79x | 16.05 | lir validate 0.147; verify 0.062; codegen 0.057; signatures 0.034; modes 0.030; silent program: wall to exit |
| stress/instances-32 | 205 | warm | ok / 10 | 0.685 | 4.362 | 356 | 0.090 / 0.228 / 30 | — / 19.11x / 11.83x | 22.19 | lir validate 0.147; verify 0.063; codegen 0.057; signatures 0.034; modes 0.030; silent program: wall to exit |
| stress/instances-128 | 781 | cold | ok / 10 | 2.790 | 28.482 | 383 | 0.170 / 0.401 / 97 | — / 71.02x / 3.95x | 82.75 | lir validate 1.254; codegen 0.283; verify 0.263; import 0.137; signatures 0.129; silent program: wall to exit |
| stress/instances-128 | 781 | warm | ok / 10 | 2.715 | 28.255 | 383 | 0.100 / 0.228 / 30 | — / 123.75x / 12.70x | 144.19 | lir validate 1.256; codegen 0.279; verify 0.269; import 0.138; signatures 0.129; silent program: wall to exit |
| stress/wide-2000 | 8044 | cold | ok / 10 | 12.465 | 107.619 | 580 | 0.340 / 1.301 / 114 | — / 82.70x / 5.07x | 82.70 | lir validate 4.690; codegen 3.666; signatures 0.993; verify 0.590; modes 0.378; silent program: wall to exit |
| stress/wide-2000 | 8044 | warm | ok / 10 | 12.360 | 107.378 | 579 | 0.130 / 0.343 / 30 | — / 312.70x / 19.32x | 312.70 | lir validate 4.661; codegen 3.675; signatures 0.990; verify 0.582; modes 0.377; silent program: wall to exit |
| stress/long-2000 | 2004 | cold | ok / 10 | 14.965 | 197.421 | 547 | 0.215 / 0.809 / 106 | — / 243.91x / 5.15x | 243.91 | verify 8.751; import 3.847; lir validate 0.671; ready 0.422; rc 0.248; silent program: wall to exit |
| stress/long-2000 | 2004 | warm | ok / 10 | 14.775 | 197.195 | 547 | 0.100 / 0.228 / 30 | — / 863.79x / 18.17x | 863.79 | verify 8.692; import 3.841; lir validate 0.668; ready 0.421; rc 0.245; silent program: wall to exit |
| stress/arms-200 | 211 | cold | ok / 10 | 4.940 | 71.320 | 361 | 0.150 / 0.331 / 90 | — / 215.38x / 4.03x | 420.55 | codegen 4.302; lir validate 0.122; ready 0.035; verify 0.033; rc 0.023; silent program: wall to exit |
| stress/arms-200 | 211 | warm | ok / 10 | 4.870 | 71.095 | 356 | 0.100 / 0.228 / 30 | — / 311.41x / 11.81x | 608.05 | codegen 4.300; lir validate 0.122; ready 0.035; verify 0.033; rc 0.023; silent program: wall to exit |
| stress/nested-match-16 | 104 | cold | failed / 0 | 0.240 | — | 290 | 0.160 / 0.317 / 94 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-16 | 104 | warm | failed / 0 | 0.240 | — | 291 | 0.090 / 0.228 / 30 | — / — / — | — | modes 0.001; resolve 0.001; signatures 0.000; check 0.000; parse 0.000 |
| stress/nested-match-32 | 200 | cold | failed / 0 | 0.240 | — | 290 | 0.165 / 0.325 / 94 | — / — / — | — | resolve 0.001; modes 0.001; signatures 0.000; parse 0.000; check 0.000 |
| stress/nested-match-32 | 200 | warm | failed / 0 | 0.260 | — | 290 | 0.100 / 0.228 / 30 | — / — / — | — | resolve 0.002; modes 0.002; parse 0.001; signatures 0.001; check 0.000 |
| stress/table-2000 | 2010 | cold | failed / 0 | 3.960 | — | 3987 | 0.195 / 0.570 / 110 | — / — / — | — | — |
| stress/table-2000 | 2010 | warm | failed / 0 | 3.960 | — | 3987 | 0.095 / 0.228 / 30 | — / — / — | — | — |
| stress/derive-64 | 579 | cold | failed / 0 | 0.230 | — | 272 | 0.655 / 3.080 / 146 | — / — / — | — | — |
| stress/derive-64 | 579 | warm | failed / 0 | 0.220 | — | 272 | 0.100 / 0.228 / 30 | — / — / — | — | — |
| stress/closures-200 | 805 | cold | ok / 10 | 4.880 | 43.175 | 437 | 0.190 / 0.519 / 98 | — / 83.27x / 4.45x | 41.89 | codegen 1.534; lir validate 1.519; verify 0.439; import 0.307; modes 0.200; silent program: wall to exit |
| stress/closures-200 | 805 | warm | ok / 10 | 4.790 | 42.944 | 432 | 0.100 / 0.228 / 30 | — / 188.11x / 14.41x | 94.64 | codegen 1.550; lir validate 1.508; verify 0.436; import 0.318; modes 0.200; silent program: wall to exit |
| syn3k | 3066 | cold | ok / 10 | 78.041 | 731.417 | 1305 | 5.915 / 32.046 / 221 | — / 22.82x / 5.90x | 21.70 | lir validate 27.772; codegen 22.727; verify 11.416; signatures 4.909; import 2.800 |
| syn3k | 3066 | warm | ok / 10 | 77.784 | 731.159 | 1304 | 0.100 / 0.232 / 30 | — / 3154.39x / 43.46x | 2999.04 | lir validate 27.763; codegen 22.751; verify 11.415; signatures 4.898; import 2.800 |
| syn10k | 10274 | cold | ok / 7 | 195.675 | 1871.747 | 2116 | 15.310 / 83.110 / 290 | — / 22.52x / 7.29x | 21.53 | lir validate 82.506; codegen 52.305; verify 19.068; signatures 13.329; import 7.084 |
| syn10k | 10274 | warm | ok / 7 | 192.264 | 1871.430 | 2117 | 0.100 / 0.241 / 30 | — / 7774.98x / 69.99x | 7431.41 | lir validate 81.472; codegen 52.071; verify 18.910; signatures 13.190; import 6.984 |
## precheck

| input | lines | cache | status / n | wall s | instructions G | RSS MiB | Rust wall s / instructions G / RSS MiB | H/R wall / instructions / RSS | H/R instructions per line | top five phases by self time (s) |
|---|---:|---|---|---:|---:|---:|---|---|---:|---|
| std@1 | 73572 | cold | ok / 10 | 106.050 | 805.605 | 1581 | — / — / — | — / — / — | — | — |
| std@1 | 73572 | warm | ok / 10 | 107.185 | 805.616 | 1582 | — / — / — | — / — / — | — | — |

## Findings from runs that did not complete

- `logq/hirsel/native/cold`: failed; capped: peak 736 MiB of 4G (systemd scope), 14.8 s, exit 1. error: unknown flag --help
- `logq/hirsel/native/warm`: failed; capped: peak 705 MiB of 4G (systemd scope), 14.3 s, exit 1. error: unknown flag --help
- `mdtoc/hirsel/native/cold`: failed; capped: peak 646 MiB of 4G (systemd scope), 6.7 s, exit 1. error: no file access granted
- `mdtoc/hirsel/native/warm`: failed; capped: peak 624 MiB of 4G (systemd scope), 7.6 s, exit 1. error: no file access granted
- `tasks/hirsel/debug/cold`: failed; capped: peak 1402 MiB of 4G (systemd scope), 160.3 s, exit 2. historical generated-body DWARF anchor bug; fixed later on main in 139046c7
- `tasks/hirsel/debug/warm`: failed; capped: peak 1251 MiB of 4G (systemd scope), 144.7 s, exit 2. historical generated-body DWARF anchor bug; fixed later on main in 139046c7
- `hello-web/hirsel/native/cold`: failed; capped: peak 813 MiB of 4G (systemd scope), 20.1 s, exit 2. runtime failure[MEM-13] at hello_web@1/serve (/workspace/code/hirsel-wt/cc-baseline/examples/hello-web/main.hir:9:3)
- `hello-web/hirsel/native/warm`: failed; capped: peak 795 MiB of 4G (systemd scope), 19.8 s, exit 2. runtime failure[MEM-13] at hello_web@1/serve (/workspace/code/hirsel-wt/cc-baseline/examples/hello-web/main.hir:9:3)
- `stress/nested-match-16/hirsel/check/cold`: failed; capped: peak 238 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/check/warm`: failed; capped: peak 238 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/debug/cold`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/debug/warm`: failed; capped: peak 238 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/release/cold`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/release/warm`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/native/cold`: failed; capped: peak 248 MiB of 4G (systemd scope), 0.3 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-16/hirsel/native/warm`: failed; capped: peak 248 MiB of 4G (systemd scope), 0.3 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/check/cold`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/check/warm`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/debug/cold`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/debug/warm`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/release/cold`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/release/warm`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.2 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/native/cold`: failed; capped: peak 248 MiB of 4G (systemd scope), 0.3 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/nested-match-32/hirsel/native/warm`: failed; capped: peak 248 MiB of 4G (systemd scope), 0.3 s, exit 1. generator-rejected: LANG-4 nested binder shadowing; excluded from accepted-program ratios
- `stress/table-2000/hirsel/check/cold`: failed; capped: peak 3944 MiB of 4G (systemd scope), 4.0 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/check/warm`: failed; capped: peak 3944 MiB of 4G (systemd scope), 4.0 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/debug/cold`: failed; capped: peak 3944 MiB of 4G (systemd scope), 4.1 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/debug/warm`: failed; capped: peak 3944 MiB of 4G (systemd scope), 4.0 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/release/cold`: failed; capped: peak 3944 MiB of 4G (systemd scope), 3.9 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/release/warm`: failed; capped: peak 3944 MiB of 4G (systemd scope), 3.9 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/native/cold`: failed; capped: peak 3953 MiB of 4G (systemd scope), 4.0 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/table-2000/hirsel/native/warm`: failed; capped: peak 3953 MiB of 4G (systemd scope), 4.0 s, exit 71. hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); allocating 2784 more bytes failed with a resident peak of 3987M. Under Buck2 the limit is the test's measured reservation (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.
- `stress/derive-64/hirsel/check/cold`: failed; capped: peak 230 MiB of 4G (systemd scope), 0.2 s, exit 1. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/check/warm`: failed; capped: peak 230 MiB of 4G (systemd scope), 0.2 s, exit 1. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/debug/cold`: failed; capped: peak 230 MiB of 4G (systemd scope), 0.2 s, exit 1. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/debug/warm`: failed; capped: peak 230 MiB of 4G (systemd scope), 0.2 s, exit 1. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/release/cold`: failed; capped: peak 230 MiB of 4G (systemd scope), 0.2 s, exit 1. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/release/warm`: failed; capped: peak 230 MiB of 4G (systemd scope), 0.2 s, exit 1. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/native/cold`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.3 s, exit 3. hirsel: package manifest: package schema must be 1
- `stress/derive-64/hirsel/native/warm`: failed; capped: peak 239 MiB of 4G (systemd scope), 0.3 s, exit 3. hirsel: package manifest: package schema must be 1

A failed/capped/timeout pair is attempted once and not repeated across later rounds. A smaller capped/timed-out scale program blocks larger sizes in that language/mode. No cap is raised and no failed cell is represented as a speedup.

# Spectral audit

`spectralaudit ROOT INPUT OUTPUT JOBS [--interruptible]` reads one JSON manifest,
subtracts each channel's complex mean when requested, computes a direct DFT,
then reconstructs it with the inverse. OUTPUT is committed atomically only after
all channels pass reconstruction and Parseval checks. Status 2 diagnoses input,
grant, file, verification or cancellation errors. Status 3 means a missing
stdout/stderr grant; stream write failures propagate as AppError.Output.

```json
{"channels":[{"samples":[{"real":1,"imaginary":0},{"real":-1,"imaginary":0}]}],"remove_mean":false,"top":2,"band":{"first":0,"end":2}}
```

All fields are required and unknown fields rejected. Channels are identified by
input ordinal. Bounds: 1–4 channels, equal lengths of 1–32 complex samples,
finite numeric components in [-1000,1000], 1–8 top bins (clipped to N), 1–4 jobs,
32768 input bytes, 16384 output bytes. The band is [first,end), nonempty within N.
No windows, padding, sample rates, audio codecs or acquisition are implied.

Reports contain every complex bin, squared magnitude, peak ordinals, removed
mean, band energy, time/frequency energy, maximum complex reconstruction error
and relative energy error (absolute for zero time energy). Energy sums frequency
squared magnitudes divided by N. Forward uses negative angles, inverse positive
angles and divides by N exactly once. Numbers have six fractional digits;
negative values rounding to zero are spelled positively. Peak comparisons use
unrounded energy; exact ties retain the lower bin ordinal. Tiny nonzero residual
energies can therefore order mathematically zero bins differently.

The concrete f64 kernel ports RustFFT's small O(N²) Dft with N reusable twiddles,
explicit dirty scratch, deterministic ascending accumulation and no planner.
This is a numerical/ownership example, with no claimed performance advantage.
Series[T] provides borrowed map/fold callbacks; independent channel tasks share
immutable input and twiddles and return owned results in input order. A task
scope cancels and joins siblings on failure. `--interruptible` races the entire
workflow against an explicitly granted Interrupt signal.

Use a read-write directory grant for ROOT, stdout/stderr and cooperative Tasks;
interruptible mode additionally needs Signals. See grants.json for a template.
The focused spectral_audit_35 tests use the actual Io+Args main through public
Fixture APIs. The standard-library-only Decimal fixture generator is evidence,
not application code. It fixes 1e-9*(1+|expected|) component intervals,
1e-9 reconstruction and 1e-9 relative/zero-absolute energy tolerances.
Six-decimal reports must agree byte for byte across engines independently of
those algorithm tolerances. See FRICTION.md for incomplete qualification cells.

## Upstream

RustFFT commit
`3bfe1afaecf5b3e5cc3d70f861a8aa5234aa50f1`, algorithm/dft.rs:27–72,
twiddles.rs:6–23, separate trigonometric oracle dft.rs:94–106, and exact vectors
at dft.rs:284–299,327–359. See THIRD_PARTY.md for notices and deviations.

The original length-32 report and 32768-byte padded input stop in the
interpreters at the retained one-million-step budget (decimal formatting and
JSON whitespace respectively). Their qualification identity remains explicit
in `spectral_audit_35_original_caps` and is ignored in ordinary suites; it
still fails with INCOMPLETE when explicitly run. The vectors and caps are
unchanged. Native and mathematical-kernel evidence are separate obligations.

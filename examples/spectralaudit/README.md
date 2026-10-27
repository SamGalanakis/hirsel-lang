# Spectral audit

`spectralaudit ROOT INPUT OUTPUT JOBS [--interruptible]` reads one JSON manifest,
subtracts each channel's complex mean when requested, computes a Fourier transform,
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

The concrete f64 kernel uses scalar radix-2 layers for power-of-two lengths and
RustFFT's direct DFT for other lengths, with N reusable twiddles and explicit
dirty scratch. The inverse normalizes once after all layers. No planner or
SIMD backend is included.
Series[T] provides borrowed map/fold callbacks; independent channel tasks share
immutable input and twiddles and return owned results in input order. A task
scope cancels and joins siblings on failure. `--interruptible` races the entire
workflow against an explicitly granted Interrupt signal.

Use a read-write directory grant for ROOT, stdout/stderr and cooperative Tasks;
interruptible mode additionally needs Signals. See grants.json for a template.
From the checkout, with a matching `hirsel` on PATH:

```sh
mkdir -p /tmp/spectral-data /tmp/spectral-run
cp examples/spectralaudit/fixtures/rustfft-4.json /tmp/spectral-data/samples.json
python3 - <<'PYTHON'
import json
from pathlib import Path
grant = json.loads(Path('examples/spectralaudit/grants.json').read_text())
grant['io']['dirs'][0]['host'] = '/tmp/spectral-data'
Path('/tmp/spectral-run/grants.json').write_text(json.dumps(grant) + '\n')
PYTHON
hirsel build examples/spectralaudit/hirsel.json --backend llvm -o /tmp/spectral-run/spectralaudit
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 /tmp/spectral-run/spectralaudit --hirsel-record /tmp/spectral-run/audit.tape --hirsel-grants /tmp/spectral-run/grants.json data samples.json audit.json 4
/tmp/spectral-run/spectralaudit --hirsel-replay /tmp/spectral-run/audit.tape
```

Set the template's absolute host directory before launch. For interruptible
mode add `"signals":{"provider":"signals.host@1","kinds":["interrupt"]}`
to `io` and append `--interruptible` to the application words. Cancellation
joins child tasks; an already committed complete output remains published.

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

The original length-32 report, 32768-byte padded input and aggregate unrounded
oracle pass within their retained one-million-step budgets. Their fixtures,
independent oracles and caps remain unchanged. The input-cap workflow records
with the current compiler and checks its own tapes across engines. Historical
performance measurements are separate from these correctness results.

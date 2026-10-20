# Integer metric reducers: prior art and adaptations

OpenTelemetry Rust SDK pin `fb6b53566815062f6be838d49cbea77019669d6f`
(<https://github.com/open-telemetry/opentelemetry-rust>), reused clone
`/tmp/ref-opentelemetry-rust`:

- `opentelemetry-sdk/src/metrics/internal/histogram.rs:259-278` selects the
  first bound greater than or equal to a measurement, with the final bucket
  covering values above the last bound. Hirsel ports this selection directly
  using integer comparisons.
- `histogram.rs:316-342`, `check_buckets_are_selected_correctly`: bounds
  `[1,3,6]`, observations `1..=10`, counts `[1,2,3,4]`, count 10 and sum 55.
  Ported in `SEM-3-metrics-reduce-run.hir`.
- `opentelemetry-sdk/src/metrics/internal/mod.rs:163-190`, `ValueMap::measure`:
  existing canonical keys reuse their aggregator; distinct keys under the
  cardinality limit get new aggregators; subsequent keys share overflow.
  Hirsel preserves that order and tests accounting in both direct and
  restarted folds. Unlike OTel's extra empty-attribute tracker, the empty
  set consumes one normal series slot; overflow is preallocated separately.
- `opentelemetry-sdk/src/metrics/internal/last_value.rs` and `sum.rs` provide
  last-observation and additive reduction respectively. Hirsel's explicit
  observation order determines last, without timestamps or SDK callbacks.

The OpenTelemetry specification, Metrics SDK §Cardinality limits, also
requires that each overflow observation contribute to exactly one aggregate.
There is no synthetic overflow label that could collide with an application
label: `Instrument.overflow` is separate state.

Adaptations required by Hirsel semantics:

1. Owned explicit Registry replaces process-wide SDK state, locks and atomics.
   Linear bounded lists preserve first-seen order without hash iteration.
2. u64 counts/sums and i64 signed sums reject overflow before mutation. No
   floating-point conversion, NaN handling, saturation or ignored negatives.
3. Descriptor registration is the configuration's bounded descriptor list.
   Equal scope/name descriptors coalesce; incompatible descriptors fail.
4. Canonical telemetry labels are filtered before validation. Fixed ceilings
   and configured limits bound registry memory; trace exemplars stay out of keys.
5. A contiguous evidence ordinal prefix makes duplicate ingestion idempotent.
   A checkpoint couples the evidence capture/digest, reducer version, full
   configuration, ordinal and aggregate state. Decoded state is revalidated.
   Applications remain responsible for binding EvidenceId to actual bytes via
   the offline view parser; the reducer never reads recording files or clocks.
6. Histograms retain only count, sum and interval counts; optional OTel min/max,
   temporality and collector windows are deferred. Explicit registry lifetimes
   and evidence ordinal boundaries define accumulation intervals.

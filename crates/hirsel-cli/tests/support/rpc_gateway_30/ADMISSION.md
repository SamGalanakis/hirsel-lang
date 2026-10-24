The admission oracle checks independent admission deadlines, sixteen outstanding
request credits, `jobs` upstream credits, and at most three attempts per request.
The 10/20ms retry waits share each request's original deadline. Host scheduling
may delay admissions or task startup; attempt counts alone cannot prove legality.

`admission.py` checks normalized transitions. `admission_vectors.json` contains
27 accepting and 29 rejecting traces. `test_admission.py` runs each individually,
checks positive and negative coverage of all seventeen invariants, and sabotages
three expectations to test the oracle itself. Logical milliseconds belong only
to these finite vectors; they are not decoded native tapes.

The separately named `controlled-admission-16` and `controlled-admission-17`
cases run with the simulation clock. They retain the original application,
limits and input frames. The original `in-flight-16` and `in-flight-17` cases
retain their identities and ordinary host clocks.

Live admission currently reports `unproven: witness missing`. Neither stdout,
exact replay, host timing, nor canonical task events independently identify the
pure reserve/drop and task-to-RPC associations. The semantic verdict, occupancy
coverage, recorded bytes, and replay verdict are separate receipt fields.
An unproven semantic verdict fails the admission test even if every replay
matches. No producer-supplied validity, count or retry-permission flag is proof.

The next observer must bind committed reserve, token lifetime/transfer, RPC
identity, admission clock ordinal, scope timer/cancel gate, provider attempts,
sleep roles, bounded exit, delivery/drop and joined cleanup to canonical source
operations. Missing, duplicated or unbound transitions must fail closed.

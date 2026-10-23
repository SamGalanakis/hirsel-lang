# Prior art

CPython `010bebbf9fd12bba1e3f40bdbddacd51a9c2bf20`,
Lib/graphlib.py:112–200 (get_ready/done completion-release structure),
Lib/test/test_graphlib.py:40–79 (chain/disconnected/diamond shapes).
Whole-file SHA256: 905c5e625e19dd472d4e960518f51e640b7ee44788cab81cad5bc4a9baaa011b,
b66a698ec242e23001ab98660d0ad1f261d16c210858e25fde8d933a8fe62631.
PSF-2.0, Copyright Python Software Foundation, full text in LICENSES.

Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
src/os/exec/exec.go:915–950 (Wait consumes/reaps and joins copying loops),
src/os/exec/exec_test.go:470–525 (both pipes + stdin close).
Whole-file SHA256: 3ec96eeef97a375aa5e39663961f8adf3177db9449dacd3c407fd258bc9a2498,
d00336ba7c8217adaee445936f17fef4ffd15306e9eb5e9f46abf9d1d9e28907.
BSD-3-Clause (Go), Copyright The Go Authors, full text in LICENSES.

Deviations: explicit declarations replace implicit vertex insertion; failures block
transitive descendants; declaration-order scans replace ready FIFO because this
app promises stable declaration ties; bounded task sets replace goroutines;
child.wait discards both streams and closes stdin instead of exposing pipe
readers. There are no ambient effects or exceptions. Cycle detection and SHA256
reuse shipped std. Python expectations use independent transitive closure and
hashlib rather than the app's graph routine.

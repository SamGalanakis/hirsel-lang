# BigInt Toom-3

The implementation ports num-bigint `41f0b035d12971c5c7f7501917e31f0f6439c26c`,
`src/biguint/multiplication.rs:285-431`: split at `len(y)/3+1`, evaluate at
0, 1, -1, -2 and infinity, Bodrato interpolation, and high-to-low recomposition.
The upstream product-table test is `tests/biguint.rs:845-869`; all 21
`tests/consts` MUL_TRIPLES already run in the bigint corpus. New driver tests
exercise threshold boundaries, uneven thirds, opposite signs at negative
evaluation points, zero chunks, carries, unbalanced factors, recursive Toom-3,
and four external sign/order combinations against a host schoolbook oracle.
There are 72 factor-pattern pairs and 288 independently checked products in
two Rust cases, including 127/128/129-limb cutoff boundaries.

Deviations:

- Hirsel uses canonical owned limb lists and value-returning helpers, rather
  than Rust's mutable borrowed slices, to preserve its ownership model.
- Interpolation's exact divisions by 2 and 3 use the existing word divider,
  check remainder zero, then restore the sign. This avoids general division
  and matches the upstream integer quotients.
- The existing unbalanced chunk dispatcher is retained; replacing it with
  half-Karatsuba is outside this Toom-3 change.
- NTT (a later upstream addition) is outside the requested Toom-3 scope.

Run `python3 bench/numeric/toom3.py --output /tmp/toom3` to build private std
copies with multiple thresholds and a Karatsuba-only baseline. The workload
uses dense seeded factors, varies a factor each iteration and observes the
product. `bench/micro/run` checks matching output and runs ten seeded,
interleaved rounds under perf cycles/instructions with CPU affinity. No
compiler source or shared std file is mutated by the benchmark.

The existing divide-by-zero expectation moves with the added source lines;
its failure rule and function remain unchanged. The other 32 selected numeric
corpus cases passed without expectation changes.

## Crossover measurement

Production uses schoolbook through 32 limbs, Karatsuba through 128 limbs,
and Toom-3 above 128 limbs for comparable-length inputs. num-bigint uses
Karatsuba through 256 limbs; this cutoff is lower because the measured Hirsel
instruction counts favor Toom-3 sooner. The smaller factor governs dispatch.

Measured on an AMD Ryzen 9 5950X, x86-64 LLVM release, CPU 31. Every row is a
separate session with ten seed-42 interleaved rounds, `perf stat` cycles and
instructions, matched output, and recorded SMT sibling load. Ratios include
process startup, factor construction and the product loop. The broad
128–2,048-limb sweep uses 100 products; the smaller-input controls and
129–256-limb refinement use 1,000 products to reduce startup's share.

| Limbs per factor | Toom cutoff | Instructions / Karatsuba-only | Cycles / Karatsuba-only |
|---:|---:|---:|---:|
| 65 | 64 | 1.0122 | 1.0353 |
| 96 | 64 | 1.0595 | 1.0742 |
| 112 | 96 | 1.0187 | 1.0227 |
| 128 | 96 | 0.9890 | 0.9980 |
| 129 | 128 | 0.9523 | 0.9284 |
| 144 | 128 | 0.9484 | 0.9503 |
| 160 | 128 | 0.9455 | 0.9488 |
| 192 | 128 | 0.9888 | 0.9812 |
| 256 | 128 | 0.9506 | 0.9424 |
| 320 | 128 | 0.9181 | 0.8850 |
| 512 | 128 | 0.8774 | 0.9666 |
| 1,024 | 128 | 0.8408 | 0.8346 |
| 1,536 | 128 | 0.7911 | 0.7689 |
| 2,048 | 128 | 0.7940 | 0.8759 |

The single-level crossover lies between the sampled 112 and 128 limbs;
128 is a conservative dispatch cutoff, avoiding the smaller-input regressions
and taking the 129-limb recursion boundary's 4.8% instruction improvement.
At 1,536 limbs the cutoff-256 candidate took 0.8383 of baseline instructions,
versus 0.7911 for cutoff 128. Cutoffs 512 and 1,024 were also measured and
lost more of the savings. Time is not the verdict on this shared host.

Reproduce the smaller-input control with
`--thresholds 64,96,18446744073709551615 --sizes 65,80,96,112,128,129,144,160,192,256 --epochs 1000`.
Reproduce the refinement with `--thresholds 128,18446744073709551615
--sizes 129,144,160,176,192,224,256 --epochs 1000`.
Raw session JSON remains in the invocation's scratch benchmark directories;
it is not checked in as generated data. The commands preserve counters,
executable hashes, checksums, order and CPU samples when rerun.

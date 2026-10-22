# LangArena full-size comparison

This report is fixed to the measured source snapshot below. Earlier campaign and porter notes are preserved in [the previous report](/workspace/code/hirsel-briefs/pre-full-20261022/repo-evidence/bench/langarena/RESULTS-pre-full-20261022.md). Publish future campaigns under `/workspace/code/hirsel-briefs/<lane>/`; keep only a short result row or archive link here.

Measured source: Hirsel origin/main `9f161d96ce059b57cf1a1b5b39e9e5e55b2fb130`; upstream LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`. Config: unchanged `run.js` (50 tasks). Session: `la-compare-full-20261022`, started 2026-10-21T22:27:04.042846+00:00.

LLVM release / Rust geometric means on 45 passing pairs: **2.270× runtime**, **1.625× peak RSS**, **2.370× instructions**, **2.236× cycles**. Every displayed local ratio is a ratio of group medians: median(H samples) / median(Rust samples), not a median of within-round ratios. The geometric summaries aggregate those per-task ratios of medians. Ratios above one favor Rust.

On the identical 45-task intersection, inserting Hirsel among published production configurations gives **27/31 by geometric runtime/Rust ratio**. Applying the site's runtime-sum weighting in Rust-normalized units gives **28/31 configurations**, or **22/25 languages** using the site's fastest-production-configuration selection. Its covered-task runtime score would be **53.761/100 (26/31 configurations)**. These are subset projections, not a full 50-task placement: five tasks lack ports. No missing task receives a zero, Rust's time, or an invented result.

## Measurement method

Host: AMD Ryzen 9 5950X, Linux x86-64; `Linux turbo2-sam 6.8.0-137-generic #137-Ubuntu SMP PREEMPT_DYNAMIC Fri Jul 17 20:28:23 UTC 2026 x86_64`. All measurements use `nice -n 10`, `tools/capped` (4 GiB, two-CPU limit), then CPU 31 affinity for execution. The CPU timing lock is held across all rounds; runs are seeded and interleaved (seed 20261021). The live host rules require ten rounds with `perf stat` cycles and instructions. The main table uses all ten medians; the requested three-round medians use the first three complete rounds of this same session below.

Each sample must print its own expected checksum before it can contribute. Timed runtime excludes preparation and warmup; RSS is the whole-process high-water mark including startup, preparation and warmup. Hardware counters also include the whole process. Compile observations are diagnostic singletons outside the runtime session, and are not used for cross-machine compile ratios or rank. Cold builds use fresh caches; fetched compiler toolchains and dependencies are excluded, while cold Rust includes compiling dependencies. Incremental builds replace one source-file marker, then restore it. Hirsel compiles 45 task implementations; Rust compiles 50. Cranelift debug is a separate Hirsel configuration, not included in the release rank. No other language runs locally.

Published source: [`2026-10-21-kostya-ms7c37.js`](https://github.com/kostya/LangArena/blob/3c9f84ddaeec834c0631747cb789e8cfd602f993/results/2026-10-21-kostya-ms7c37.js) (latest file in the upstream checkout), SHA-256 `534c503f0785fe7772afe0f8df85ae0cdd59576415c9221ec9827587f267e319`. Its host is AMD Ryzen 7 3800X 8-Core Processor 78GB. For each task and metric, H/language = (local H/Rust) / (published language/Rust). All aggregate placements use those ratios or the published Rust's weights; local and remote seconds are never compared directly. Published instruction/cycle counts are absent, so those metrics have only the local Rust comparison. Runtime-score thresholds follow upstream `docs/gen.rb:116-180`, runtime-sum ranking `:219-250`, and fastest-configuration selection `:1062-1085`.

## Per-task medians (ten rounds)

| Task | LLVM s | Rust s | LLVM/Rust | LLVM MiB | Rust MiB | RSS ratio | Instructions ratio | Cycles ratio | Cranelift debug s | Debug MiB | Debug/Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Binarytrees::Obj | 3.436 | 3.248 | 1.058 | 100.250 | 67.500 | 1.485 | 0.444 | 1.045 | 6.721 | 101.125 | 2.069 |
| Binarytrees::Arena | 2.290 | 0.271 | 8.450 | 62.549 | 27.562 | 2.269 | 2.565 | 8.199 | 4.135 | 63.156 | 15.259 |
| Brainfuck::Array | 3.358 | 3.510 | 0.957 | 7.250 | 3.625 | 2.000 | 2.521 | 0.939 | 18.646 | 8.250 | 5.312 |
| Brainfuck::Recursion | 3.884 | 1.904 | 2.040 | 6.750 | 3.375 | 2.000 | 2.059 | 2.054 | 12.652 | 8.000 | 6.645 |
| Matmul::Single | 5.577 | 5.700 | 0.978 | 42.074 | 28.258 | 1.489 | 0.777 | 0.997 | 64.679 | 43.154 | 11.347 |
| Matmul::T4 | — | — | — | — | — | — | — | — | — | — | — |
| Matmul::T8 | — | — | — | — | — | — | — | — | — | — | — |
| Matmul::T16 | — | — | — | — | — | — | — | — | — | — | — |
| Base64::Encode | 8.015 | 0.234 | 34.179 | 9.840 | 5.090 | 1.933 | 31.588 | 33.109 | 47.433 | 10.549 | 202.273 |
| Base64::Decode | 5.491 | 0.289 | 19.000 | 9.086 | 5.000 | 1.817 | 22.126 | 18.550 | 118.102 | 9.943 | 408.659 |
| Json::Generate | 1.790 | 0.325 | 5.516 | 22.873 | 13.062 | 1.751 | 5.732 | 5.543 | 14.147 | 25.883 | 43.598 |
| Json::ParseDom | 1.498 | 2.159 | 0.694 | 30.512 | 20.230 | 1.508 | 1.346 | 0.700 | 11.838 | 32.045 | 5.482 |
| Json::ParseMapping | 3.591 | 0.400 | 8.977 | 23.688 | 15.500 | 1.528 | 6.679 | 8.938 | 32.874 | 23.383 | 82.185 |
| CSV::Parse | 17.539 | 0.810 | 21.653 | 82.629 | 13.000 | 6.356 | 12.952 | 21.463 | 128.551 | 85.359 | 158.705 |
| Etc::Sieve | 2.862 | 1.304 | 2.194 | 12.107 | 6.469 | 1.872 | 1.560 | 2.169 | 17.695 | 12.938 | 13.570 |
| Etc::TextRaytracer | 1.190 | 0.915 | 1.300 | 6.250 | 3.500 | 1.786 | 1.260 | 1.292 | 33.873 | 7.500 | 37.020 |
| Etc::NeuralNet | 2.509 | 1.149 | 2.183 | 6.625 | 3.500 | 1.893 | 1.809 | 2.207 | 37.251 | 7.750 | 32.406 |
| Etc::CacheSimulation | 3.848 | 0.861 | 4.469 | 8.947 | 4.250 | 2.105 | 3.391 | 4.360 | 6.971 | 9.619 | 8.097 |
| Etc::GameOfLife | 0.888 | 1.641 | 0.541 | 23.500 | 45.250 | 0.519 | 0.548 | 0.537 | 6.777 | 24.500 | 4.128 |
| Etc::Words | 3.044 | 1.094 | 2.782 | 10.932 | 5.158 | 2.119 | 2.797 | 2.747 | 8.478 | 11.777 | 7.749 |
| Etc::LogParser | — | — | — | — | — | — | — | — | — | — | — |
| Template::Regex | — | — | — | — | — | — | — | — | — | — | — |
| Template::Parse | 1.671 | 1.386 | 1.205 | 40.436 | 38.588 | 1.048 | 1.767 | 1.128 | 6.318 | 41.668 | 4.557 |
| Sort::Quick | 1.194 | 1.131 | 1.056 | 9.432 | 6.746 | 1.398 | 1.326 | 1.097 | 4.739 | 10.455 | 4.192 |
| Sort::Merge | 1.911 | 1.575 | 1.214 | 11.031 | 8.000 | 1.379 | 1.768 | 1.240 | 8.310 | 12.014 | 5.278 |
| Sort::Self | 2.781 | 0.405 | 6.868 | 9.615 | 8.000 | 1.202 | 6.818 | 6.660 | 20.874 | 10.684 | 51.540 |
| Graph::BFS | 2.727 | 1.753 | 1.556 | 207.225 | 78.250 | 2.648 | 1.509 | 1.500 | 5.618 | 207.992 | 3.205 |
| Graph::DFS | 1.446 | 1.324 | 1.092 | 195.393 | 78.750 | 2.481 | 1.753 | 1.166 | 6.392 | 196.777 | 4.827 |
| Graph::AStar | 11.199 | 2.574 | 4.351 | 311.730 | 82.000 | 3.802 | 4.107 | 4.355 | 23.020 | 315.701 | 8.943 |
| Hash::SHA256 | 1.151 | 1.147 | 1.004 | 7.559 | 4.250 | 1.778 | 1.000 | 1.006 | 28.336 | 8.379 | 24.715 |
| Hash::CRC32 | 0.915 | 0.290 | 3.149 | 7.402 | 4.250 | 1.742 | 7.922 | 3.058 | 13.946 | 8.445 | 48.008 |
| Calculator::Ast | 0.689 | 0.665 | 1.037 | 33.598 | 18.500 | 1.816 | 0.638 | 1.026 | 5.950 | 32.875 | 8.948 |
| Calculator::Interpreter | 1.893 | 0.851 | 2.223 | 50.787 | 50.750 | 1.001 | 1.200 | 2.099 | 3.029 | 55.270 | 3.557 |
| Maze::Generator | 0.580 | 0.923 | 0.629 | 82.588 | 171.012 | 0.483 | 0.986 | 0.630 | 3.316 | 83.912 | 3.593 |
| Maze::BFS | 1.348 | 1.320 | 1.021 | 90.213 | 179.637 | 0.502 | 1.859 | 1.011 | 3.396 | 91.424 | 2.574 |
| Maze::AStar | 6.054 | 1.371 | 4.416 | 127.236 | 191.266 | 0.665 | 4.529 | 3.781 | 11.262 | 128.252 | 8.215 |
| CLBG::Fannkuchredux | 0.886 | 1.030 | 0.860 | 6.625 | 3.375 | 1.963 | 1.313 | 0.870 | 7.163 | 7.750 | 6.957 |
| CLBG::Mandelbrot | 0.934 | 0.939 | 0.996 | 8.822 | 5.750 | 1.534 | 0.937 | 0.994 | 14.438 | 10.059 | 15.384 |
| CLBG::Nbody | 1.242 | 1.128 | 1.101 | 6.500 | 3.500 | 1.857 | 1.733 | 1.116 | 40.256 | 7.625 | 35.688 |
| CLBG::Spectralnorm | 1.560 | 0.974 | 1.601 | 6.875 | 3.500 | 1.964 | 2.153 | 1.594 | 46.020 | 8.250 | 47.224 |
| Compress::BWTEncode | 1.393 | 0.401 | 3.477 | 12.035 | 7.555 | 1.593 | 2.567 | 3.477 | 6.640 | 13.490 | 16.579 |
| Compress::BWTDecode | 1.428 | 0.523 | 2.731 | 60.637 | 42.461 | 1.428 | 3.901 | 2.363 | 7.292 | 62.174 | 13.942 |
| Compress::HuffEncode | 0.971 | 0.684 | 1.419 | 8.000 | 5.000 | 1.600 | 1.499 | 1.354 | 9.972 | 9.000 | 14.568 |
| Compress::HuffDecode | 4.549 | 0.636 | 7.152 | 11.887 | 8.633 | 1.377 | 6.923 | 6.983 | 27.169 | 12.949 | 42.719 |
| Compress::ArithEncode | 1.086 | 0.894 | 1.214 | 7.250 | 3.750 | 1.933 | 1.797 | 1.169 | 5.141 | 8.500 | 5.751 |
| Compress::ArithDecode | 1.193 | 1.232 | 0.968 | 6.875 | 4.000 | 1.719 | 1.564 | 0.948 | 7.670 | 8.000 | 6.226 |
| Compress::LZWEncode | 2.134 | 0.711 | 3.004 | 8.875 | 4.500 | 1.972 | 3.481 | 3.086 | 9.155 | 9.750 | 12.885 |
| Compress::LZWDecode | 3.894 | 0.498 | 7.812 | 13.344 | 8.863 | 1.506 | 5.241 | 7.364 | 22.027 | 13.998 | 44.186 |
| Distance::Jaro | 1.003 | 0.872 | 1.151 | 6.625 | 3.750 | 1.767 | 1.446 | 1.158 | 3.652 | 7.750 | 4.188 |
| Distance::NGram | 1.936 | 0.473 | 4.088 | 8.895 | 4.000 | 2.224 | 3.359 | 4.137 | 3.299 | 9.895 | 6.967 |

A dash means blocked or failed, never zero. Status details appear below.

### Observed interference

The shepherd reported that modes-flow-2 pinned CPU31 without the timing lock at approximately 04:10–04:12 CEST on 2026-10-22 (message 592896). This overlaps the tenth round. No sample was discarded or rerun; ten-round medians remain the reported values. Wall-time and cycle rankings are approximate shared-host observations with this known interference; instruction ratios remain primary. The first-three-round appendix predates this overlap. File modification times bound the following possibly affected samples; they are approximate process intervals, not proof of simultaneous work on every listed run. SMT-sibling occupancy is recorded separately in every raw sample and does not detect same-logical-CPU interference. Observed sibling busy shares ranged from 0 to 97.1%; affinity and a timing lock do not establish an idle physical core.

| Sample (zero-based round) | Start CEST | End CEST |
| --- | --- | --- |
| 9-Binarytrees--Arena-cranelift | 2026-10-22T04:11:09.043276+02:00 | 2026-10-22T04:11:19.602359+02:00 |
| 9-Brainfuck--Array-cranelift | 2026-10-22T04:11:22.437381+02:00 | 2026-10-22T04:11:57.389654+02:00 |
| 9-Compress--BWTDecode-cranelift | 2026-10-22T04:09:44.536614+02:00 | 2026-10-22T04:10:08.464802+02:00 |
| 9-Compress--HuffEncode-hirsel | 2026-10-22T04:11:19.895361+02:00 | 2026-10-22T04:11:22.169379+02:00 |
| 9-Etc--TextRaytracer-cranelift | 2026-10-22T04:10:08.680804+02:00 | 2026-10-22T04:11:05.867251+02:00 |
| 9-Graph--AStar-cranelift | 2026-10-22T04:11:57.620656+02:00 | 2026-10-22T04:12:55.315106+02:00 |
| 9-Sort--Quick-rust | 2026-10-22T04:11:06.156254+02:00 | 2026-10-22T04:11:08.713274+02:00 |

## Compile observations

| Build | LLVM release s | Rust release s | Cranelift debug s |
| --- | ---: | ---: | ---: |
| cold | 44.780 | 29.880 | 31.040 |
| incremental | 20.960 | 4.510 | 22.280 |

## Geometric means against published configurations

Each row uses the passing task intersection with positive published Rust and language values. Main ranking configurations are labeled prod; other published configurations are shown for completeness, including Ruby, PHP and JavaScript, which this results file marks hack. Lower H/reference is better. No published instruction counts or eligible compile timing ratios exist.

| Published configuration | Group | Runtime H/reference | n | Peak RSS H/reference | n |
| --- | --- | ---: | ---: | ---: | ---: |
| C#/AOT | hack | 1.311 | 45 | 0.594 | 45 |
| C#/AOT-EXTREME | hack | 1.296 | 45 | 0.596 | 45 |
| C#/JIT | prod | 1.506 | 45 | 0.307 | 45 |
| C#/R2R | hack | 1.226 | 45 | 0.347 | 45 |
| C#/SC-AOT | hack | 1.303 | 45 | 0.592 | 45 |
| C#/SC-JIT | hack | 1.538 | 45 | 0.381 | 45 |
| C++/Clang++ | prod | 2.315 | 45 | 1.333 | 45 |
| C++/Clang++/Fast | hack | 2.361 | 45 | 1.338 | 45 |
| C++/Clang++/PGO | hack | 2.158 | 45 | 1.352 | 45 |
| C++/Clang++/Unsafe | hack | 2.561 | 45 | 1.344 | 45 |
| C++/G++ | prod | 2.227 | 45 | 1.336 | 45 |
| C++/G++/Fast | hack | 2.367 | 45 | 1.326 | 45 |
| C++/G++/PGO | hack | 2.263 | 45 | 1.325 | 45 |
| C++/G++/Unsafe | hack | 2.477 | 45 | 1.326 | 45 |
| C/Clang | prod | 2.371 | 45 | 2.121 | 45 |
| C/Clang/Fast | hack | 2.348 | 45 | 2.116 | 45 |
| C/Clang/PGO | hack | 2.239 | 45 | 2.138 | 45 |
| C/Clang/Unsafe | hack | 2.452 | 45 | 2.114 | 45 |
| C/Gcc | prod | 2.256 | 45 | 2.119 | 45 |
| C/Gcc/Fast | hack | 2.305 | 45 | 2.073 | 45 |
| C/Gcc/PGO | hack | 2.236 | 45 | 2.067 | 45 |
| C/Gcc/Unsafe | hack | 2.404 | 45 | 2.077 | 45 |
| C/cproc/QBE | hack | 1.532 | 45 | 2.070 | 45 |
| C/mycc/C-Clang | hack | 2.149 | 45 | 2.135 | 45 |
| C/mycc/C-Clang/Final | hack | 2.295 | 45 | 2.135 | 45 |
| C/mycc/LLVM/Default | hack | 1.957 | 45 | 2.130 | 45 |
| C/mycc/LLVM/Final | prod | 2.323 | 45 | 2.118 | 45 |
| C/mycc/QBE/Default | hack | 1.662 | 45 | 2.131 | 45 |
| C/mycc/QBE/Final | hack | 1.669 | 45 | 2.110 | 45 |
| C3 | prod | 1.572 | 45 | 1.879 | 45 |
| C3/Fast | hack | 1.740 | 45 | 1.933 | 45 |
| C3/Fast/SingleModule | hack | 1.737 | 45 | 1.939 | 45 |
| C3/SingleModule | hack | 1.586 | 45 | 1.875 | 45 |
| C3/Unsafe | hack | 1.842 | 45 | 1.952 | 45 |
| Crystal | prod | 1.513 | 45 | 0.982 | 45 |
| Crystal/GCRY | hack | 1.554 | 45 | 0.700 | 45 |
| Crystal/O3 | hack | 0.583 | 45 | 0.891 | 45 |
| D/DMD | hack | 1.045 | 45 | 0.644 | 45 |
| D/DMD/Perf | hack | 1.056 | 45 | 0.630 | 45 |
| D/LDC | prod | 1.623 | 45 | 0.777 | 45 |
| D/LDC/MaxPerf | hack | 1.627 | 45 | 0.771 | 45 |
| D/LDC/Perf | hack | 1.629 | 45 | 0.785 | 45 |
| Dart/AOT | hack | 0.683 | 45 | 0.400 | 45 |
| Dart/JIT | prod | 0.756 | 45 | 0.066 | 45 |
| F#/JIT | prod | 1.374 | 45 | 0.255 | 45 |
| Go | prod | 1.455 | 45 | 0.684 | 45 |
| Go/Opt | hack | 1.456 | 45 | 0.692 | 45 |
| Goose/Clang | prod | 3.526 | 45 | 2.286 | 45 |
| Goose/Clang/Fast | hack | 3.550 | 45 | 2.257 | 45 |
| Goose/Clang/Middle | hack | 2.763 | 45 | 2.304 | 45 |
| Goose/GCC | hack | 2.890 | 45 | 2.290 | 45 |
| Goose/GCC/Fast | hack | 3.020 | 45 | 2.239 | 45 |
| Goose/GCC/Middle | hack | 2.644 | 45 | 2.269 | 45 |
| Gossamer | prod | 1.926 | 45 | 1.476 | 45 |
| JS/Bun/JIT | hack | 1.129 | 45 | 0.156 | 45 |
| JS/Deno/Default | hack | 1.005 | 45 | 0.126 | 45 |
| JS/Node/Default | hack | 0.995 | 45 | 0.098 | 45 |
| Java/GraalVM/JIT | prod | 1.651 | 45 | 0.060 | 45 |
| Java/GraalVM/Serial | hack | 1.690 | 45 | 0.070 | 45 |
| Java/OpenJDK | prod | 1.751 | 45 | 0.092 | 45 |
| Java/OpenJDK/EA | hack | 1.749 | 45 | 0.093 | 45 |
| Java/OpenJDK/Opt | hack | 1.736 | 45 | 0.112 | 45 |
| Java/OpenJDK/Serial | hack | 1.717 | 45 | 0.163 | 45 |
| Julia/Default | prod | 1.319 | 45 | 0.045 | 45 |
| Julia/Max | hack | 1.355 | 45 | 0.045 | 45 |
| Kotlin/GraalVM/JIT | hack | 1.682 | 45 | 0.058 | 45 |
| Kotlin/JVM/Default | prod | 1.766 | 45 | 0.092 | 45 |
| Kotlin/JVM/Max | hack | 1.793 | 45 | 0.005 | 45 |
| Kotlin/JVM/Opt | hack | 1.824 | 45 | 0.009 | 45 |
| Mojo | prod | 1.277 | 45 | 0.407 | 45 |
| Nim++/Clang | hack | 1.292 | 45 | 1.178 | 45 |
| Nim++/Clang/Danger | hack | 1.705 | 45 | 1.186 | 45 |
| Nim++/GCC | hack | 1.317 | 45 | 1.141 | 45 |
| Nim++/GCC/Danger | hack | 1.584 | 45 | 1.158 | 45 |
| Nim/Clang | prod | 1.479 | 45 | 1.454 | 45 |
| Nim/Clang/ARC | hack | 1.540 | 45 | 1.594 | 45 |
| Nim/Clang/Danger | hack | 1.632 | 45 | 1.467 | 45 |
| Nim/Clang/Max | hack | 1.645 | 45 | 1.477 | 45 |
| Nim/GCC | hack | 1.433 | 45 | 1.416 | 45 |
| Nim/GCC/ARC | hack | 1.461 | 45 | 1.561 | 45 |
| Nim/GCC/Danger | hack | 1.601 | 45 | 1.431 | 45 |
| Nim/GCC/Max | hack | 1.638 | 45 | 1.437 | 45 |
| Nim/NLVM | hack | 1.699 | 45 | 1.459 | 45 |
| Nim/NLVM/Danger | hack | 1.820 | 45 | 1.463 | 45 |
| Odin/Default | prod | 1.501 | 45 | 1.693 | 45 |
| Odin/MaxPerf | hack | 1.516 | 45 | 1.689 | 45 |
| Odin/Opt | hack | 1.503 | 45 | 1.699 | 45 |
| PHP | hack | 0.371 | 45 | 0.313 | 45 |
| Python/PYPY | prod | 0.480 | 45 | 0.125 | 45 |
| Ruby/CRuby4/YJIT | hack | 0.150 | 45 | 0.277 | 45 |
| Ruby/Spinel/Clang | hack | 0.357 | 45 | 0.274 | 45 |
| Ruby/Truffle/JVM | hack | 0.609 | 45 | 0.016 | 45 |
| Rust | prod | 2.270 | 45 | 1.625 | 45 |
| Rust/MaxPerf/Unsafe | hack | 2.323 | 45 | 1.651 | 45 |
| Rust/PGO | hack | 2.359 | 45 | 1.611 | 45 |
| Rust/WASM/Node | prod | 2.021 | 45 | 0.244 | 45 |
| Rust/WASM/WasmEdge | hack | 2.016 | 45 | 0.282 | 45 |
| Rust/WASM/Wasmer | hack | 1.877 | 45 | 0.109 | 45 |
| Rust/WASM/Wasmtime | hack | 1.920 | 45 | 0.140 | 45 |
| Rust/WMO | hack | 2.311 | 45 | 1.667 | 45 |
| Rust/WMO/Unchecked | hack | 2.315 | 45 | 1.652 | 45 |
| Scala/GraalVM/JIT | hack | 1.476 | 45 | 0.054 | 45 |
| Scala/JVM/Default | prod | 1.544 | 45 | 0.084 | 45 |
| Scala/JVM/Max | hack | 1.568 | 45 | 0.004 | 45 |
| Scala/JVM/Opt | hack | 1.560 | 45 | 0.009 | 45 |
| Swift | prod | 0.921 | 45 | 0.502 | 45 |
| Swift/Unchecked | hack | 0.999 | 45 | 0.503 | 45 |
| TypeScript/Bun/Compiled | hack | 1.142 | 45 | 0.159 | 45 |
| TypeScript/Bun/JIT | prod | 1.142 | 45 | 0.158 | 45 |
| TypeScript/Deno/Default | hack | 0.941 | 45 | 0.122 | 45 |
| TypeScript/Deno/Turbo | hack | 0.942 | 45 | 0.122 | 45 |
| TypeScript/Node/Default | prod | 0.981 | 45 | 0.096 | 45 |
| TypeScript/Node/Opt | hack | 0.646 | 45 | 0.140 | 45 |
| V/Clang | prod | 1.362 | 45 | 0.893 | 45 |
| V/Clang/Perf | hack | 1.344 | 45 | 0.915 | 45 |
| V/GCC | hack | 1.309 | 45 | 0.959 | 45 |
| V/GCC/Perf | hack | 1.311 | 45 | 0.917 | 45 |
| Zig | prod | 1.739 | 45 | 2.006 | 45 |
| Zig/Unchecked | hack | 2.292 | 45 | 2.185 | 45 |

## Rust-normalized placement (covered tasks)

Hirsel geometric ratio: 2.270; published-sum-weighted ratio: 2.635. Every row uses exactly 45 tasks. Full-suite placement remains unavailable.

| Production configuration | Geometric runtime/Rust | Weighted runtime/Rust | Covered-task score / 100 |
| --- | ---: | ---: | ---: |
| Goose/Clang | 0.644 | 0.712 | 96.670 |
| C/Clang | 0.957 | 1.037 | 86.650 |
| C/mycc/LLVM/Final | 0.977 | 1.060 | 86.336 |
| C++/Clang++ | 0.981 | 1.045 | 88.158 |
| Rust | 1.000 | 1.000 | 89.917 |
| C/Gcc | 1.006 | 1.084 | 85.686 |
| C++/G++ | 1.019 | 1.064 | 87.722 |
| Rust/WASM/Node | 1.123 | 1.072 | 86.804 |
| Gossamer | 1.179 | 1.136 | 83.328 |
| Kotlin/JVM/Default | 1.286 | 1.288 | 76.676 |
| Java/OpenJDK | 1.297 | 1.304 | 76.521 |
| Zig | 1.306 | 1.236 | 77.955 |
| Java/GraalVM/JIT | 1.375 | 1.402 | 73.264 |
| D/LDC | 1.399 | 1.369 | 78.845 |
| C3 | 1.444 | 1.423 | 75.113 |
| Scala/JVM/Default | 1.470 | 1.459 | 68.676 |
| Crystal | 1.501 | 1.437 | 73.220 |
| C#/JIT | 1.507 | 1.477 | 73.864 |
| Odin/Default | 1.512 | 1.564 | 77.471 |
| Nim/Clang | 1.535 | 1.527 | 72.278 |
| Go | 1.560 | 1.495 | 74.282 |
| F#/JIT | 1.653 | 1.655 | 67.201 |
| V/Clang | 1.667 | 1.775 | 70.939 |
| Julia/Default | 1.721 | 1.732 | 63.582 |
| Mojo | 1.778 | 2.317 | 62.967 |
| TypeScript/Bun/JIT | 1.987 | 2.113 | 52.558 |
| Hirsel | 2.270 | 2.635 | 53.761 |
| TypeScript/Node/Default | 2.315 | 2.424 | 48.677 |
| Swift | 2.466 | 3.517 | 49.973 |
| Dart/JIT | 3.004 | 3.111 | 37.123 |
| Python/PYPY | 4.734 | 5.081 | 16.786 |

## Ten worst LLVM release / Rust runtime ratios

Profiles rerun the same full-size task with the identical measured release executable using user cycles at 199 Hz. Percentages are flat self samples over the whole process, including preparation and warmup, rather than attribution confined to the timed interval. Unresolved kernel/skid and executable-offset buckets are retained in the raw profiles; known executable offsets are associated with their containing symbol. The profile environment restores the measured toolchain/cache overrides and records them separately; executable SHA-256 is checked before and after profiling. Classification names the leading actionable cause; it is a profile-based finding, not proof that other layers have no cost.

| Task | Runtime H/Rust | Instructions H/Rust | Top profile symbols (self %) | Classification and finding |
| --- | ---: | ---: | --- | --- |
| Base64::Encode | 34.179 | 31.588 | `std.base64.encode` 69.01%; `core.str.from_utf8` 2.58%; `__memmove_avx_unaligned_erms` 0.58% | std — scalar RFC4648 packing grows byte lists and validates UTF-8; the Rust anchor uses base64_simd (base64.hir:27-85; benchmark base64.hir:3-4). |
| CSV::Parse | 21.653 | 12.952 | `std.csv.normalized` 12.29%; `langarena.csv.execute` 12.26%; `std.internal.float_decimal.parse` 12.13% | std — normalization, decimal parsing and per-field allocation/copy dominate the streaming CSV path (stdlib/std@1/csv.hir; bench csv.hir owns a fresh input copy per iteration). |
| Base64::Decode | 19.000 | 22.126 | `std.base64.decode` 86.61%; `__memmove_avx_unaligned_erms` 0.20%; `hirsel_rt.alloc.grow` 0.13% | std — scalar per-byte validation/decoding dominates versus the Rust SIMD codec (stdlib/std@1/base64.hir:108 onward). |
| Json::ParseMapping | 8.977 | 6.679 | `langarena.json.point_from_json_reader` 9.33%; `hirsel_rt_v2_seq_grow` 7.64%; `std.internal.float_lemire.compute` 6.23% | std — typed reader spends time on decimal parsing, string/key handling and list growth, despite skipping unneeded coordinate fields (json.hir; std json reader). |
| Binarytrees::Arena | 8.450 | 2.565 | `langarena.binarytrees.arena_sum` 27.38%; `langarena.binarytrees.build` 7.83%; `__memmove_avx_unaligned_erms` 1.85% | algorithm port — Hirsel Node stores i64/u64/u64 rather than Rust i32/i32/i32, doubling payload width; recursive list access and memory behavior deserve isolation before compiler attribution (binarytrees.hir:27-51; Rust binarytrees.rs:65-76). Unresolved kernel/skid buckets are 62.7%, so this is not a pure traversal attribution. |
| Compress::LZWDecode | 7.812 | 5.241 | `langarena.compress.lzw_decode` 38.02%; `langarena.compress.copy_bytes` 35.27%; `hirsel_rt_v2_seq_grow` 5.25% | algorithm port — explicit per-byte dictionary copying and appending consume the hot loop (compress.hir:576-621), replacing Rust vector cloning/extending. |
| Compress::HuffDecode | 7.152 | 6.923 | `langarena.compress.huff_decode` 64.31%; `langarena.compress.execute` 34.25%; `langarena.compress.huff_encode` 0.16% | algorithm port — boxed-tree traversal plus bit-sized initialized scratch and repeated pop for truncation; preserve checksum while isolating decode from preparation (compress.hir:334-365). |
| Sort::Self | 6.868 | 6.818 | `std.list.merge_internal` 50.00%; `std.list.block_swap` 20.68%; `std.list.merge_blocks` 15.39% | std — WikiSort merge/block movement dominates; the anchor uses Rust stable sort, so the gap includes different standard algorithms (stdlib/std@1/list; sort.hir). |
| Json::Generate | 5.516 | 5.732 | `std.internal.decimal.write_unsigned` 18.46%; `std.internal.float_decimal.shortest64` 10.37%; `hirsel_rt_v2_bytes_append` 7.87% | std — number formatting and byte/text appends dominate the derived JSON writer (stdlib/std@1/internal/{decimal,float_decimal}.hir; json.hir). |
| Etc::CacheSimulation | 4.469 | 3.391 | `hirsel_rt.copy.copy_graph` 9.75%; `RandomState.hash_one<CopyKey>` 9.24%; `core.map.get` 5.52% | runtime — uniqueness copying tracks copied nodes in a hashed Copies table; copy_graph and its bookkeeping occupy the profile despite detaching index/free lists per batch (cache_simulation.hir:108 onward; crates/hirsel-rt/src/copy). |

## Blocked and failed tasks at the measured snapshot

After this campaign, commit `07413cab` added the regex capture/flags ports and validated both `test.js` and `run.js` checksums for Template::Regex and Etc::LogParser. Their validation notes are preserved in [the historical report](/workspace/code/hirsel-briefs/pre-full-20261022/repo-evidence/bench/langarena/RESULTS-pre-full-20261022.md#regex-captureflags-port-validation-2026-10-22). They have no timing samples in this frozen campaign. The following five blockers describe `9f161d96`, not current main; only the three Matmul pool blockers remain after that regex port.

- **Matmul::T4**: blocked: CPU parallel row execution with a fixed four-thread pool; in-run tasks share one cooperative run token.
- **Matmul::T8**: blocked: CPU parallel row execution with a fixed eight-thread pool; in-run tasks share one cooperative run token.
- **Matmul::T16**: blocked: CPU parallel row execution with a fixed sixteen-thread pool; in-run tasks share one cooperative run token.
- **Etc::LogParser**: blocked: regex lacks case-insensitive flags and a cursor-based non-overlapping match iterator.
- **Template::Regex**: blocked: regex lacks a cursor-based non-overlapping match iterator and capture replacement.

Minimal missing operations: `regex.compile("(?i)bot", 1000)` rejects the case-insensitive flag required by LogParser; non-overlapping regex cursor iteration and capture replacement are needed for `{{(.*?)}}` substitution in Template::Regex. Matmul needs a public fixed 4/8/16-worker CPU-parallel pool: spawning ordinary `task.async` rows still serializes CPU work through the cooperative run token. See `template.hir`, `log_parser.hir`, `matmul.hir` and prior port findings for source locations.

## Requested three-round medians

First three complete seeded rounds, not a separate campaign. These are reported separately from the ten-round host-rule verdict.

| Task | LLVM s | Rust s | H/R time | LLVM MiB | Rust MiB | Cranelift debug s | Debug MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Binarytrees::Obj | 3.444 | 3.152 | 1.093 | 100.250 | 67.250 | 6.741 | 101.250 |
| Binarytrees::Arena | 2.453 | 0.279 | 8.794 | 62.488 | 27.500 | 4.511 | 63.184 |
| Brainfuck::Array | 3.468 | 3.362 | 1.032 | 7.250 | 3.750 | 19.109 | 8.000 |
| Brainfuck::Recursion | 3.535 | 1.885 | 1.875 | 6.750 | 3.250 | 12.651 | 7.750 |
| Matmul::Single | 5.587 | 5.760 | 0.970 | 42.070 | 28.258 | 64.655 | 42.906 |
| Matmul::T4 | — | — | — | — | — | — | — |
| Matmul::T8 | — | — | — | — | — | — | — |
| Matmul::T16 | — | — | — | — | — | — | — |
| Base64::Encode | 7.889 | 0.240 | 32.869 | 9.680 | 5.086 | 47.609 | 10.531 |
| Base64::Decode | 5.175 | 0.288 | 17.968 | 9.094 | 5.000 | 118.170 | 9.941 |
| Json::Generate | 1.763 | 0.330 | 5.342 | 22.871 | 13.062 | 13.782 | 24.566 |
| Json::ParseDom | 1.488 | 1.934 | 0.770 | 30.551 | 20.480 | 11.563 | 32.047 |
| Json::ParseMapping | 3.425 | 0.401 | 8.542 | 23.715 | 15.500 | 32.484 | 23.320 |
| CSV::Parse | 17.055 | 0.867 | 19.672 | 83.680 | 13.000 | 131.722 | 90.824 |
| Etc::Sieve | 2.850 | 1.300 | 2.192 | 12.105 | 6.469 | 17.607 | 12.738 |
| Etc::TextRaytracer | 1.181 | 0.918 | 1.287 | 6.500 | 3.250 | 33.961 | 7.500 |
| Etc::NeuralNet | 2.460 | 1.143 | 2.152 | 6.500 | 3.500 | 37.796 | 7.750 |
| Etc::CacheSimulation | 4.133 | 0.843 | 4.902 | 8.906 | 4.250 | 6.915 | 9.504 |
| Etc::GameOfLife | 0.722 | 1.547 | 0.467 | 23.500 | 45.250 | 6.573 | 24.500 |
| Etc::Words | 3.199 | 1.094 | 2.924 | 10.941 | 5.156 | 8.572 | 11.773 |
| Etc::LogParser | — | — | — | — | — | — | — |
| Template::Regex | — | — | — | — | — | — | — |
| Template::Parse | 1.572 | 1.332 | 1.180 | 40.430 | 38.586 | 6.136 | 41.664 |
| Sort::Quick | 1.274 | 1.070 | 1.191 | 9.426 | 6.746 | 5.119 | 10.441 |
| Sort::Merge | 1.973 | 1.526 | 1.293 | 11.023 | 8.000 | 8.182 | 11.832 |
| Sort::Self | 2.781 | 0.406 | 6.849 | 9.605 | 8.000 | 20.161 | 10.672 |
| Graph::BFS | 2.914 | 1.802 | 1.617 | 207.059 | 78.250 | 5.633 | 207.992 |
| Graph::DFS | 1.437 | 1.291 | 1.113 | 195.387 | 78.750 | 6.288 | 196.539 |
| Graph::AStar | 11.860 | 2.644 | 4.486 | 297.258 | 82.000 | 23.368 | 332.293 |
| Hash::SHA256 | 1.178 | 1.129 | 1.043 | 7.562 | 4.250 | 29.561 | 8.371 |
| Hash::CRC32 | 0.887 | 0.282 | 3.146 | 7.316 | 4.250 | 13.586 | 8.371 |
| Calculator::Ast | 0.782 | 0.632 | 1.238 | 31.688 | 18.500 | 6.048 | 35.000 |
| Calculator::Interpreter | 1.946 | 0.880 | 2.212 | 50.621 | 50.750 | 2.892 | 53.309 |
| Maze::Generator | 0.546 | 0.861 | 0.634 | 82.582 | 171.012 | 3.374 | 83.676 |
| Maze::BFS | 1.333 | 1.162 | 1.147 | 90.281 | 179.512 | 3.399 | 91.426 |
| Maze::AStar | 5.799 | 1.275 | 4.548 | 127.129 | 191.266 | 12.884 | 128.254 |
| CLBG::Fannkuchredux | 0.892 | 1.023 | 0.872 | 6.500 | 3.250 | 7.093 | 7.750 |
| CLBG::Mandelbrot | 0.917 | 0.932 | 0.983 | 8.828 | 5.750 | 14.839 | 9.801 |
| CLBG::Nbody | 1.223 | 1.124 | 1.088 | 6.500 | 3.500 | 40.957 | 7.750 |
| CLBG::Spectralnorm | 1.561 | 0.954 | 1.637 | 6.750 | 3.500 | 46.258 | 8.250 |
| Compress::BWTEncode | 1.400 | 0.401 | 3.492 | 12.023 | 7.555 | 6.223 | 13.398 |
| Compress::BWTDecode | 1.390 | 0.528 | 2.633 | 60.637 | 42.461 | 7.447 | 62.164 |
| Compress::HuffEncode | 0.932 | 0.691 | 1.349 | 8.000 | 4.750 | 10.854 | 9.000 |
| Compress::HuffDecode | 4.723 | 0.664 | 7.113 | 11.883 | 8.480 | 26.749 | 12.828 |
| Compress::ArithEncode | 1.108 | 0.897 | 1.235 | 7.250 | 3.750 | 5.170 | 8.500 |
| Compress::ArithDecode | 1.182 | 1.238 | 0.955 | 6.750 | 4.000 | 8.534 | 8.000 |
| Compress::LZWEncode | 2.066 | 0.674 | 3.065 | 9.000 | 4.500 | 8.462 | 9.750 |
| Compress::LZWDecode | 3.990 | 0.489 | 8.160 | 13.121 | 8.863 | 21.692 | 13.789 |
| Distance::Jaro | 1.004 | 0.886 | 1.133 | 6.750 | 3.750 | 3.600 | 7.750 |
| Distance::NGram | 1.921 | 0.482 | 3.985 | 8.895 | 4.000 | 3.081 | 10.039 |

## Evidence and reproduction

```sh
python3 bench/langarena/compare.py --debug --rounds 10 --session la-compare-full-20261022 --output /tmp/la-compare-generated.md
.buck2/bin/buck2 test //:gate-langarena
```

Raw samples, counters, SMT sibling activity, command logs, profiles and immutable binary/source identities are retained outside the worktree at `/workspace/tmp/langarena-tools/evidence/la-compare-full-20261022/`. Raw measurements SHA-256: `56d78beb0777e5334a0a34c5a4ccb054af155cc9cba8cd870f85950ee723916a`. Consumed-source SHA-256: `024a8a5f67dc63c831a85259cd3403d860aedf3fc2134b76d38e7bebbde15798`. Runtime session lock interval: 2026-10-21T22:33:19.781206+00:00 to 2026-10-22T02:29:55.270721+00:00. Generated artifacts are not committed.

| Participant | Compiler SHA-256 | Executable SHA-256 |
| --- | --- | --- |
| hirsel | `db4578be41f986a7054f5727bd6b790be6e7b37d88621becd007f8c92b640ca2` | `8d4c6998e62593f830afd8d2aca02d6b26eab9e43bbe03afaaed5df3a1e32d11` |
| rust | `f3834d26669b03f6855fa54bd2381443123e860167bc0c7a8dd137e5cf6e5e4f` | `34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3` |
| cranelift | `db4578be41f986a7054f5727bd6b790be6e7b37d88621becd007f8c92b640ca2` | `56303b5845d6e5d2ce771d5eda1c4d2dfe92750641946ecd0509b7c28aa86dbb` |

Validation: `//:gate-langarena` passed 11 tests (10 existing plus one debug-column regression), including all 45 registered `test.js` checksums; the full campaign passed all 1,350 `run.js` sample checksums. All ten final diagnostic profiles also passed their expected full-size checksum. No test was removed or weakened. No benchmark workload, algorithm, checksum or safety setting was changed for this comparison.

## Archived campaign notes

Campaign data and reproduction scripts live in `/workspace/code/hirsel-briefs/<lane>/repo-evidence/`.

| Campaign | Archived report |
| --- | --- |
| la-astar-9 | [RESULTS-la-astar-9.md](/workspace/code/hirsel-briefs/la-astar-9/repo-evidence/bench/langarena/RESULTS-la-astar-9.md) |
| la-brainfuck-recursion-10 | [RESULTS-la-brainfuck-recursion-10.md](/workspace/code/hirsel-briefs/la-brainfuck-recursion-10/repo-evidence/bench/langarena/RESULTS-la-brainfuck-recursion-10.md) |
| la-bwt-9 | [RESULTS-la-bwt-9.md](/workspace/code/hirsel-briefs/la-bwt-9/repo-evidence/bench/langarena/RESULTS-la-bwt-9.md) |
| la-calculator-interpreter-10 | [RESULTS-la-calculator-interpreter-10.md](/workspace/code/hirsel-briefs/la-calculator-interpreter-10/repo-evidence/bench/langarena/RESULTS-la-calculator-interpreter-10.md) |
| la-copy-graph-8 | [RESULTS-la-copy-graph-8.md](/workspace/code/hirsel-briefs/la-copy-graph-8/repo-evidence/bench/langarena/RESULTS-la-copy-graph-8.md) |
| la-crc32-9 | [RESULTS-la-crc32-9.md](/workspace/code/hirsel-briefs/la-crc32-9/repo-evidence/bench/langarena/RESULTS-la-crc32-9.md) |
| la-graph-rss-10 | [RESULTS-la-graph-rss-10.md](/workspace/code/hirsel-briefs/la-graph-rss-10/repo-evidence/bench/langarena/RESULTS-la-graph-rss-10.md) |
| la-json-perf-8 | [RESULTS-la-json-perf-8.md](/workspace/code/hirsel-briefs/la-json-perf-8/repo-evidence/bench/langarena/RESULTS-la-json-perf-8.md) |
| la-lzw-10 | [RESULTS-la-lzw-10.md](/workspace/code/hirsel-briefs/la-lzw-10/repo-evidence/bench/langarena/RESULTS-la-lzw-10.md) |
| la-neural-net-10 | [RESULTS-la-neural-net-10.md](/workspace/code/hirsel-briefs/la-neural-net-10/repo-evidence/bench/langarena/RESULTS-la-neural-net-10.md) |
| la-port-fidelity-8 | [RESULTS-la-port-fidelity-8.md](/workspace/code/hirsel-briefs/la-port-fidelity-8/repo-evidence/bench/langarena/RESULTS-la-port-fidelity-8.md) |
| la-regex-ports-8 | [RESULTS-la-regex-ports-8.md](/workspace/code/hirsel-briefs/la-regex-ports-8/repo-evidence/bench/langarena/RESULTS-la-regex-ports-8.md) |
| la-sieve-10 | [RESULTS-la-sieve-10.md](/workspace/code/hirsel-briefs/la-sieve-10/repo-evidence/bench/langarena/RESULTS-la-sieve-10.md) |
| la-words-9 | [RESULTS-la-words-9.md](/workspace/code/hirsel-briefs/la-words-9/repo-evidence/bench/langarena/RESULTS-la-words-9.md) |
| port-3 | [RESULTS-port-3.md](/workspace/code/hirsel-briefs/port-3/repo-evidence/bench/langarena/RESULTS-port-3.md) |
| pre-full-20261022 | [RESULTS-pre-full-20261022.md](/workspace/code/hirsel-briefs/pre-full-20261022/repo-evidence/bench/langarena/RESULTS-pre-full-20261022.md) |
| la-csv-perf-8 | [RESULTS-la-csv-perf-8.md](/workspace/code/hirsel-briefs/la-csv-perf-8/repo-evidence/bench/langarena/RESULTS-la-csv-perf-8.md) |
| la-base64-perf-8 | Encode/decode instructions -66.37%/-64.77%; H/R 10.777/8.042; runtime allocations 102,384→6,368 / 77,200→5,169; exact checksums. [Full report](/workspace/code/hirsel-briefs/la-base64-perf-8/RESULTS-la-base64-perf-8.md) |

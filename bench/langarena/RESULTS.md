# LangArena: Hirsel and a local Rust anchor

Config `run.js`; 10 seeded interleaved rounds, seed 20261021; 2026-10-21T19:54:42.688645+00:00.
Local host: Linux turbo2-sam 6.8.0-137-generic #137-Ubuntu SMP PREEMPT_DYNAMIC Fri Jul 17 20:28:23 UTC 2026 x86_64. Hirsel: native LLVM release; Rust: upstream `cargo build --release`.
Upstream `3c9f84ddaeec834c0631747cb789e8cfd602f993`. Published data: `2026-10-21-kostya-ms7c37.js`, SHA-256 `534c503f0785fe7772afe0f8df85ae0cdd59576415c9221ec9827587f267e319`.
Published host: AMD Ryzen 7 3800X 8-Core Processor 78GB. Only Hirsel and Rust ran on this host.

Runtime excludes warmup; process peak RSS includes preparation, warmup and runtime startup. Compiler and dependency downloads are excluded from compile measurements; cold Rust includes compiling fetched dependencies. Compile numbers describe each whole suite, with different implemented coverage. Incremental compilation changes RECOMPILE_MARKER_0 to RECOMPILE_MARKER_1 in one source file, as benchmarks.rb:2721-2733.
CPU 31, shared timing lock `/workspace/code/hirsel-briefs/locks/cpu31-timing.lock` held for all interleaved rounds. Immutable consumed-source, compiler, native toolchain/runtime and binary SHA-256 identities were checked before/after the session.
Consumed source identity: `2b2d59db6d8904c2572a2c604a5e1a3d036c51d075faaa6a30bed7949af7dec6`.
Instruction verdict: cycles + instructions. Counters include startup and warmup. All local runs use nice -n 10 and tools/capped.

## Local measurements

| Task | Hirsel status | Rust status | Hirsel s | Rust s | Time H/R | Hirsel MiB | Rust MiB | RSS H/R | Instructions H/R |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Binarytrees::Obj | OK (10/10) | OK (10/10) | 3.134407340 | 3.167500000 | 0.990 | 99.750 | 67.500 | 1.478 | 0.445 |
| Binarytrees::Arena | OK (10/10) | OK (10/10) | 2.280876430 | 0.273500000 | 8.340 | 61.990 | 27.500 | 2.254 | 2.576 |
| Brainfuck::Array | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Brainfuck::Recursion | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Matmul::Single | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Matmul::T4 | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Matmul::T8 | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Matmul::T16 | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Base64::Encode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Base64::Decode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Json::Generate | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Json::ParseDom | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Json::ParseMapping | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| CSV::Parse | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Etc::Sieve | OK (10/10) | OK (10/10) | 2.697060475 | 1.273500000 | 2.118 | 11.404 | 6.469 | 1.763 | 1.559 |
| Etc::TextRaytracer | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Etc::NeuralNet | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Etc::CacheSimulation | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Etc::GameOfLife | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Etc::Words | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Etc::LogParser | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Template::Regex | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Template::Parse | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Sort::Quick | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Sort::Merge | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Sort::Self | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Graph::BFS | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Graph::DFS | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Graph::AStar | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Hash::SHA256 | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Hash::CRC32 | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Calculator::Ast | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Calculator::Interpreter | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Maze::Generator | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Maze::BFS | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Maze::AStar | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| CLBG::Fannkuchredux | OK (10/10) | OK (10/10) | 0.865981419 | 1.007000000 | 0.860 | 6.000 | 3.250 | 1.846 | 1.313 |
| CLBG::Mandelbrot | OK (10/10) | OK (10/10) | 0.907150046 | 0.904500000 | 1.003 | 8.279 | 5.750 | 1.440 | 0.937 |
| CLBG::Nbody | OK (10/10) | OK (10/10) | 1.220290336 | 1.091500000 | 1.118 | 5.875 | 3.500 | 1.679 | 1.733 |
| CLBG::Spectralnorm | OK (10/10) | OK (10/10) | 1.529949793 | 0.927500000 | 1.650 | 6.000 | 3.250 | 1.846 | 2.153 |
| Compress::BWTEncode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::BWTDecode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::HuffEncode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::HuffDecode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::ArithEncode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::ArithDecode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::LZWEncode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Compress::LZWDecode | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Distance::Jaro | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |
| Distance::NGram | missing: unported task | not measured: no Hirsel port | — | — | — | — | — | — | — |

## Geometric means and compile times

| Metric | Hirsel / Rust | Paired tasks |
| --- | ---: | ---: |
| seconds | 1.608 | 7 / 50 |
| rss_kib | 1.740 | 7 / 50 |
| instructions | 1.351 | 7 / 50 |
| cycles | 1.596 | 7 / 50 |

| Whole-suite compilation | Hirsel s | Local Rust s | H/R | Estimated on published host s |
| --- | ---: | ---: | ---: | ---: |
| cold | 17.750 | 25.450 | 0.697 | 4.854 |
| incremental | 12.030 | 3.150 | 3.819 | 4.277 |

## Position against published languages

Estimate = local Hirsel/Rust × published Rust for the same metric and task. These are projections, not measurements on the published host. Runtime rank includes Hirsel and all available upstream production configurations; lower is faster. Hirsel/reference ratios below use only paired, passing tasks. No full-suite ranking is claimed while tasks are missing.

| Task | Estimated s | Estimated MiB | Runtime rank | H / C/Clang | H / Zig | H / Goose/Clang |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Binarytrees::Obj | 2.580 | 100.472 | 21 / 31 | 1.047 | 1.814 | 11.780 |
| Binarytrees::Arena | 2.510 | 65.222 | 28 / 31 | 7.994 | 4.142 | 10.459 |
| Brainfuck::Array | — | — | — | — | — | — |
| Brainfuck::Recursion | — | — | — | — | — | — |
| Matmul::Single | — | — | — | — | — | — |
| Matmul::T4 | — | — | — | — | — | — |
| Matmul::T8 | — | — | — | — | — | — |
| Matmul::T16 | — | — | — | — | — | — |
| Base64::Encode | — | — | — | — | — | — |
| Base64::Decode | — | — | — | — | — | — |
| Json::Generate | — | — | — | — | — | — |
| Json::ParseDom | — | — | — | — | — | — |
| Json::ParseMapping | — | — | — | — | — | — |
| CSV::Parse | — | — | — | — | — | — |
| Etc::Sieve | 2.370 | 11.576 | 28 / 31 | 1.941 | 1.884 | 2.593 |
| Etc::TextRaytracer | — | — | — | — | — | — |
| Etc::NeuralNet | — | — | — | — | — | — |
| Etc::CacheSimulation | — | — | — | — | — | — |
| Etc::GameOfLife | — | — | — | — | — | — |
| Etc::Words | — | — | — | — | — | — |
| Etc::LogParser | — | — | — | — | — | — |
| Template::Regex | — | — | — | — | — | — |
| Template::Parse | — | — | — | — | — | — |
| Sort::Quick | — | — | — | — | — | — |
| Sort::Merge | — | — | — | — | — | — |
| Sort::Self | — | — | — | — | — | — |
| Graph::BFS | — | — | — | — | — | — |
| Graph::DFS | — | — | — | — | — | — |
| Graph::AStar | — | — | — | — | — | — |
| Hash::SHA256 | — | — | — | — | — | — |
| Hash::CRC32 | — | — | — | — | — | — |
| Calculator::Ast | — | — | — | — | — | — |
| Calculator::Interpreter | — | — | — | — | — | — |
| Maze::Generator | — | — | — | — | — | — |
| Maze::BFS | — | — | — | — | — | — |
| Maze::AStar | — | — | — | — | — | — |
| CLBG::Fannkuchredux | 0.879 | 7.298 | 1 / 31 | 0.758 | 0.898 | 0.861 |
| CLBG::Mandelbrot | 0.952 | 8.651 | 28 / 31 | 1.052 | 1.053 | 1.038 |
| CLBG::Nbody | 1.547 | 6.682 | 17 / 31 | 1.030 | 1.035 | 1.104 |
| CLBG::Spectralnorm | 1.524 | 6.938 | 29 / 31 | 1.650 | 1.493 | 1.660 |
| Compress::BWTEncode | — | — | — | — | — | — |
| Compress::BWTDecode | — | — | — | — | — | — |
| Compress::HuffEncode | — | — | — | — | — | — |
| Compress::HuffDecode | — | — | — | — | — | — |
| Compress::ArithEncode | — | — | — | — | — | — |
| Compress::ArithDecode | — | — | — | — | — | — |
| Compress::LZWEncode | — | — | — | — | — | — |
| Compress::LZWDecode | — | — | — | — | — | — |
| Distance::Jaro | — | — | — | — | — | — |
| Distance::NGram | — | — | — | — | — | — |

| Published production configuration | Geomean estimated H/reference time | Paired tasks | Geomean estimated H/reference RSS | Cold H/reference | Incremental H/reference |
| --- | ---: | ---: | ---: | ---: | ---: |
| C#/JIT | 1.289 | 7 / 50 | 0.296 | 2.237 | 2.037 |
| C++/Clang++ | 1.495 | 7 / 50 | 1.198 | 1.022 | 5.941 |
| C++/G++ | 1.605 | 7 / 50 | 1.208 | 0.356 | 5.859 |
| C/Clang | 1.555 | 7 / 50 | 2.118 | 1.860 | 85.547 |
| C/Gcc | 1.702 | 7 / 50 | 2.123 | 0.589 | 61.105 |
| C/mycc/LLVM/Final | 1.661 | 7 / 50 | 2.112 | 1.811 | 53.467 |
| C3 | 1.486 | 7 / 50 | 1.923 | 4.854 | 4.277 |
| Crystal | 1.476 | 7 / 50 | 0.905 | 0.223 | 0.196 |
| D/LDC | 1.356 | 7 / 50 | 0.770 | 0.134 | 0.349 |
| Dart/JIT | 0.847 | 7 / 50 | 0.052 | — | — |
| F#/JIT | 1.224 | 7 / 50 | 0.237 | 1.106 | 0.981 |
| Go | 1.730 | 7 / 50 | 0.762 | 1.703 | 14.749 |
| Goose/Clang | 2.446 | 7 / 50 | 2.452 | 1.118 | 0.988 |
| Gossamer | 1.643 | 7 / 50 | 1.767 | 3.763 | 3.422 |
| Java/GraalVM/JIT | 1.282 | 7 / 50 | 0.051 | 1.134 | 0.812 |
| Java/OpenJDK | 1.572 | 7 / 50 | 0.088 | 1.124 | 1.054 |
| Julia/Default | 1.411 | 7 / 50 | 0.036 | — | — |
| Kotlin/JVM/Default | 1.539 | 7 / 50 | 0.089 | 0.278 | 0.248 |
| Mojo | 1.120 | 7 / 50 | 0.360 | 0.344 | 0.309 |
| Nim/Clang | 1.539 | 7 / 50 | 1.607 | 2.177 | 2.970 |
| Odin/Default | 1.575 | 7 / 50 | 2.254 | 0.722 | 0.637 |
| Python/PYPY | 0.403 | 7 / 50 | 0.099 | — | — |
| Rust | 1.608 | 7 / 50 | 1.740 | 0.697 | 3.819 |
| Rust/WASM/Node | 1.703 | 7 / 50 | 0.199 | 0.856 | 1.614 |
| Scala/JVM/Default | 1.486 | 7 / 50 | 0.088 | 0.454 | 0.719 |
| Swift | 1.090 | 7 / 50 | 0.473 | 0.202 | 0.372 |
| TypeScript/Bun/JIT | 1.176 | 7 / 50 | 0.123 | — | — |
| TypeScript/Node/Default | 1.005 | 7 / 50 | 0.078 | 2.960 | 2.910 |
| V/Clang | 1.687 | 7 / 50 | 1.106 | 0.959 | 0.844 |
| Zig | 1.541 | 7 / 50 | 2.188 | 0.239 | 0.219 |

Paired local runtime coverage: 7 / 50. Printed upstream runtime zeros are below its millisecond resolution and excluded from runtime ratios.

## Source / expressiveness

Upstream benchmarks.rb:54-159 concatenates source files, including comments, then measures bytes, gzip bytes and lines. docs/gen.rb:280-321 defines boilerplate = source/gzip and the expressiveness score as (boilerplate / mean boilerplate) × (lines / mean lines)^1.5. Hirsel implements a subset, so its full-suite expressiveness rank is not comparable. Per-task entries measure the owning group file; shared files repeat here but count once in suite totals.

| Task | Hirsel bytes | Gzip bytes | Lines | Local Rust bytes | Gzip bytes | Lines |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Binarytrees::Obj | 2080 | 795 | 90 | 3254 | 835 | 143 |
| Binarytrees::Arena | 2080 | 795 | 90 | 3254 | 835 | 143 |
| Brainfuck::Array | 0 | 0 | 0 | 6917 | 1608 | 303 |
| Brainfuck::Recursion | 0 | 0 | 0 | 6917 | 1608 | 303 |
| Matmul::Single | 0 | 0 | 0 | 5222 | 1077 | 241 |
| Matmul::T4 | 0 | 0 | 0 | 5222 | 1077 | 241 |
| Matmul::T8 | 0 | 0 | 0 | 5222 | 1077 | 241 |
| Matmul::T16 | 0 | 0 | 0 | 5222 | 1077 | 241 |
| Base64::Encode | 0 | 0 | 0 | 2847 | 680 | 105 |
| Base64::Decode | 0 | 0 | 0 | 2847 | 680 | 105 |
| Json::Generate | 0 | 0 | 0 | 5739 | 1291 | 248 |
| Json::ParseDom | 0 | 0 | 0 | 5739 | 1291 | 248 |
| Json::ParseMapping | 0 | 0 | 0 | 5739 | 1291 | 248 |
| CSV::Parse | 0 | 0 | 0 | 3060 | 1000 | 115 |
| Etc::Sieve | 1066 | 521 | 55 | 1253 | 480 | 58 |
| Etc::TextRaytracer | 0 | 0 | 0 | 0 | 0 | 0 |
| Etc::NeuralNet | 0 | 0 | 0 | 0 | 0 | 0 |
| Etc::CacheSimulation | 0 | 0 | 0 | 0 | 0 | 0 |
| Etc::GameOfLife | 0 | 0 | 0 | 0 | 0 | 0 |
| Etc::Words | 0 | 0 | 0 | 1872 | 704 | 73 |
| Etc::LogParser | 0 | 0 | 0 | 0 | 0 | 0 |
| Template::Regex | 0 | 0 | 0 | 5658 | 1636 | 216 |
| Template::Parse | 0 | 0 | 0 | 5658 | 1636 | 216 |
| Sort::Quick | 0 | 0 | 0 | 5491 | 1055 | 222 |
| Sort::Merge | 0 | 0 | 0 | 5491 | 1055 | 222 |
| Sort::Self | 0 | 0 | 0 | 5491 | 1055 | 222 |
| Graph::BFS | 0 | 0 | 0 | 8051 | 1737 | 316 |
| Graph::DFS | 0 | 0 | 0 | 8051 | 1737 | 316 |
| Graph::AStar | 0 | 0 | 0 | 8051 | 1737 | 316 |
| Hash::SHA256 | 0 | 0 | 0 | 0 | 0 | 0 |
| Hash::CRC32 | 0 | 0 | 0 | 0 | 0 | 0 |
| Calculator::Ast | 0 | 0 | 0 | 11930 | 2582 | 430 |
| Calculator::Interpreter | 0 | 0 | 0 | 11930 | 2582 | 430 |
| Maze::Generator | 0 | 0 | 0 | 12711 | 2695 | 468 |
| Maze::BFS | 0 | 0 | 0 | 12711 | 2695 | 468 |
| Maze::AStar | 0 | 0 | 0 | 12711 | 2695 | 468 |
| CLBG::Fannkuchredux | 8152 | 2757 | 352 | 2425 | 722 | 102 |
| CLBG::Mandelbrot | 8152 | 2757 | 352 | 2097 | 708 | 83 |
| CLBG::Nbody | 8152 | 2757 | 352 | 4483 | 1520 | 177 |
| CLBG::Spectralnorm | 8152 | 2757 | 352 | 2065 | 647 | 89 |
| Compress::BWTEncode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::BWTDecode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::HuffEncode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::HuffDecode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::ArithEncode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::ArithDecode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::LZWEncode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Compress::LZWDecode | 0 | 0 | 0 | 25509 | 4752 | 1029 |
| Distance::Jaro | 0 | 0 | 0 | 6387 | 1412 | 243 |
| Distance::NGram | 0 | 0 | 0 | 6387 | 1412 | 243 |

| Whole-suite source | Bytes | Gzip bytes | Lines | Source/gzip |
| --- | ---: | ---: | ---: | ---: |
| Local hirsel | 15393 | 4837 | 646 | 3.182 |
| Local rust | 161244 | 30654 | 6302 | 5.260 |
| Published c | 235991 | 43151 | 9161 | 5.469 |
| Published c3 | 205947 | 40253 | 9260 | 5.116 |
| Published cpp | 151040 | 32061 | 5827 | 4.711 |
| Published golang | 103803 | 26020 | 5319 | 3.989 |
| Published crystal | 97423 | 22548 | 4221 | 4.321 |
| Published ruby | 95785 | 21893 | 4384 | 4.375 |
| Published rust | 161249 | 30659 | 6303 | 5.260 |
| Published csharp | 148480 | 29184 | 5365 | 5.088 |
| Published swift | 122184 | 27587 | 5283 | 4.429 |
| Published java | 153078 | 29051 | 5489 | 5.269 |
| Published kotlin | 126147 | 25149 | 4518 | 5.016 |
| Published typescript | 139121 | 28109 | 5606 | 4.949 |
| Published zig | 285798 | 43704 | 9112 | 6.539 |
| Published d | 148541 | 28641 | 6676 | 5.186 |
| Published v | 125123 | 28713 | 6403 | 4.358 |
| Published julia | 129096 | 27341 | 5430 | 4.722 |
| Published nim | 112312 | 26798 | 4605 | 4.191 |
| Published fsharp | 145029 | 29788 | 4841 | 4.869 |
| Published dart | 118989 | 26143 | 5127 | 4.552 |
| Published python | 128184 | 25057 | 4492 | 5.116 |
| Published odin | 160338 | 35564 | 7474 | 4.508 |
| Published scala | 116275 | 26317 | 4272 | 4.418 |
| Published php | 148040 | 25825 | 5695 | 5.732 |
| Published mojo | 175483 | 29594 | 5760 | 5.930 |
| Published gossamer | 130796 | 27689 | 4725 | 4.724 |
| Published goose | 133468 | 30853 | 4542 | 4.326 |
| Published javascript | 124908 | 26061 | 5364 | 4.793 |

## Missing and blocked

- Brainfuck::Array: missing: unported task.
- Brainfuck::Recursion: missing: unported task.
- Matmul::Single: missing: unported task.
- Matmul::T4: missing: unported task.
- Matmul::T8: missing: unported task.
- Matmul::T16: missing: unported task.
- Base64::Encode: missing: unported task.
- Base64::Decode: missing: unported task.
- Json::Generate: missing: unported task.
- Json::ParseDom: missing: unported task.
- Json::ParseMapping: missing: unported task.
- CSV::Parse: missing: unported task.
- Etc::TextRaytracer: missing: unported task.
- Etc::NeuralNet: missing: unported task.
- Etc::CacheSimulation: missing: unported task.
- Etc::GameOfLife: missing: unported task.
- Etc::Words: missing: unported task.
- Etc::LogParser: missing: unported task.
- Template::Regex: missing: unported task.
- Template::Parse: missing: unported task.
- Sort::Quick: missing: unported task.
- Sort::Merge: missing: unported task.
- Sort::Self: missing: unported task.
- Graph::BFS: missing: unported task.
- Graph::DFS: missing: unported task.
- Graph::AStar: missing: unported task.
- Hash::SHA256: missing: unported task.
- Hash::CRC32: missing: unported task.
- Calculator::Ast: missing: unported task.
- Calculator::Interpreter: missing: unported task.
- Maze::Generator: missing: unported task.
- Maze::BFS: missing: unported task.
- Maze::AStar: missing: unported task.
- Compress::BWTEncode: missing: unported task.
- Compress::BWTDecode: missing: unported task.
- Compress::HuffEncode: missing: unported task.
- Compress::HuffDecode: missing: unported task.
- Compress::ArithEncode: missing: unported task.
- Compress::ArithDecode: missing: unported task.
- Compress::LZWEncode: missing: unported task.
- Compress::LZWDecode: missing: unported task.
- Distance::Jaro: missing: unported task.
- Distance::NGram: missing: unported task.

## Reproduce

```sh
python3 bench/langarena/compare.py --rounds 10 --cpu 31 --session accepted-cpu31
```

Local build commands:

- hirsel: `/workspace/code/hirsel-wt/la-harness/buck-out/v2/art/root/967aa399796c0942/__hirsel-cli-bin-hirsel__/hirsel build --release /workspace/code/hirsel-wt/la-harness/bench/langarena/build/hirsel.json -o /workspace/code/hirsel-wt/la-harness/bench/langarena/build/accepted-cpu31/langarena`
- rust: `/home/sam/.rustup/toolchains/1.99.0-x86_64-unknown-linux-gnu/bin/cargo build --release --locked -j2`

Raw samples, counters, logs and build commands are in ignored `build/accepted-cpu31/measurements.json`.


## Binary identities

| Participant | Compiler SHA-256 | Executable SHA-256 |
| --- | --- | --- |
| hirsel | cd68edba7f192acfe9934587e8e50266870362f4bad3cff9327de07d4c7810e7 | 896ed13a4be8219e80bd32e64d675e389591b98b08d95db7ae0ea49d7c6f2538 |
| rust | f3834d26669b03f6855fa54bd2381443123e860167bc0c7a8dd137e5cf6e5e4f | 34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3 |

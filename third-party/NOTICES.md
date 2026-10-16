# Third-party notices

Hirsel adapts code and test vectors from the sources below. Each is used under
the license named in its row; the license texts follow once each.

| Source | Adapted | Where in this tree | Upstream revision | License |
|---|---|---|---|---|
| Zig standard library | `lib/std/Uri.zig`, `fmt.zig` parseIntWithSign | `stdlib/std@1/uri.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_uri.rs` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Io/net/HostName.zig` validate, fromUri | `stdlib/std@1/net.hir`, `crates/hirsel-run-config` grant validation, their tests | same | MIT |
| Zig standard library | `lib/std/http/Client.zig`, `HeaderIterator.zig`, `ChunkParser.zig`, `http.zig`, `ascii.zig` | `stdlib/std@1/http_wire.hir`, `stdlib/std@1/http.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs`, `crates/hirsel-driver/tests/it/stdlib_http.rs` | same | MIT |
| Zig standard library | `lib/std/unicode.zig` utf8Decode2/3/4 | `stdlib/std@1/graphemes.hir` | same | MIT |
| Zig standard library | `lib/std/fs/path.zig`, `lib/std/time/epoch.zig` | std@1 path and calendar sources and tests | same | MIT |
| Zig standard library | `lib/std/heap/ArenaAllocator.zig` alloc | `crates/hirsel-rt/src/region.rs` | same | MIT |
| Zig standard library | `lib/std/Io/Threaded.zig` netLookup, `lib/std/Io/net.zig` ListenOptions | `crates/hirsel-rt/src/net_host.rs` | same | MIT |
| Zig standard library | `lib/std/json/dynamic.zig` | std@1 JSON ordered-object duplicates | same | MIT |
| Zig standard library | `lib/std/testing.zig`, `lib/std/math.zig` | std@1 test helpers and their vectors | same | MIT |
| Rust standard library | `library/std/src/net/tcp/tests.rs` | `crates/hirsel-interp/src/library/net_tests.rs`, `crates/hirsel-rt/tests/it/net_script.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/net/{parser,ip_addr,socket_addr}.rs`, `library/coretests/tests/net/` | `stdlib/std@1/net.hir`, `crates/hirsel-run-config` grant tests, driver tests | same | MIT |
| Rust standard library | `library/std/src/sys/net/connection/socket/unix.rs`, `net/socket_addr.rs`, `sys/pal/unix/mod.rs` | `crates/hirsel-rt/src/net_host.rs` | same | MIT |
| Rust standard library | `library/coretests/tests/num/{cast,floats}.rs`, `library/core/src/num/{f32,f64}.rs` | integer-to-float boundary tests | same | MIT |
| Rust standard library | `library/core/src/time.rs` | std@1 Duration text and tests | same | MIT |
| Rust standard library | `library/core/src/fmt/num.rs`, `library/coretests/tests/fmt/num.rs` | `stdlib/std@1/decimal.hir`, `conformance/SEM/SEM-1-format-into-appends-run.hir` | same | MIT |
| unicode-segmentation | `src/grapheme.rs`, `src/tables.rs`, `tests/test.rs` | `stdlib/std@1/graphemes.hir` and its tests | unicode-rs/unicode-segmentation `ff47b3187adddb14813e6fb2cfc1a079f58cfddc` | MIT |
| Unicode Character Database 16.0.0 | data files and generated tables | `stdlib/unicode16/`, `stdlib/std@1/{unicode_tables,grapheme_tables}.hir` | Unicode 16.0.0 | Unicode, `stdlib/unicode16/LICENSE.txt` |
| serde_json | `src/{de,read,ser}.rs`, `tests/test.rs` | std@1 JSON parsing, escaping and writing | serde-rs/json `20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8` | MIT |
| JSONTestSuite | `test_parsing/` | std@1 JSON corpus vectors | nst/JSONTestSuite `1ef36fa01286573e846ac449e8683f8833c5b26a` | MIT |
| mimalloc | `src/{alloc,free,page}.c`, `include/mimalloc/types.h`, `test/test-stress.c` | `crates/hirsel-rt/src/alloc.rs` | microsoft/mimalloc `31d034d94cdb8e22f7d7ed55967f581a2d6e831d` | MIT |
| httparse | `src/lib.rs` | `stdlib/std@1/http_wire.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs` | seanmonstar/httparse `a0fa552e4e0fa9a5914a736ef3a556748fef709b` | MIT |
| rustls | `tests/api.rs` vectors, `test-ca/rsa-2048/{ca.cert,end.fullchain,end.key}` | `crates/hirsel-rt/tests/it/tls.rs`, `crates/hirsel-rt/tests/it/tls-data/` | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
| ureq | `src/config.rs` defaults | `stdlib/std@1/http.hir` | algesten/ureq `0ebb046e` | MIT |
| The Computer Language Benchmarks Game | binarytrees C gcc #2 and #3 | `bench/native/c/binarytrees-arena.h` | benchmarksgame-team.pages.debian.net | BSD-3-Clause |
| Valgrind | `include/valgrind.h`, `memcheck/memcheck.h`, vendored unchanged | `third-party/valgrind/`; their amd64 client request sequence and request codes transcribed in `crates/hirsel-rt/src/memcheck.rs` | Valgrind 3.22.0 (Ubuntu `1:3.22.0-0ubuntu3`) | Valgrind header licence (BSD-style, these two files only) |

Where an upstream offers MIT or Apache-2.0, the MIT option is used.

## Copyright notices

- Zig standard library: Copyright (c) Zig contributors
- Rust standard library: Copyright (c) The Rust Project Contributors
- unicode-segmentation: Copyright (c) 2015 The Rust Project Developers
- serde_json: Copyright (c) Serde project contributors
- JSONTestSuite: Copyright (c) 2016 Nicolas Seriot
- mimalloc: Copyright (c) 2018-2025 Microsoft Corporation, Daan Leijen
- httparse: Copyright (c) 2015-2026 Sean McArthur
- rustls: Copyright (c) 2016 Joseph Birr-Pixton
- ureq: Copyright (c) 2019 Martin Algesten
- The Computer Language Benchmarks Game: Copyright (c) 2004-2008 Brent Fulgham,
  2005-2025 Isaac Gouy. The binarytrees programs are by Jeremy Zerfas, based on
  the C++ program by Jon Harrop, Alex Mizrahi and Bruno Coutinho.
- Valgrind `valgrind.h` and `memcheck.h`: Copyright (C) 2000-2017 Julian Seward

## MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## BSD-3-Clause License (The Computer Language Benchmarks Game)

All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name "The Computer Language Benchmarks Game" nor the name "The
   Benchmarks Game" nor the name "The Computer Language Shootout Benchmarks" nor
   the names of its contributors may be used to endorse or promote products
   derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE
OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
OF THE POSSIBILITY OF SUCH DAMAGE.

## Valgrind header licence (valgrind.h, memcheck.h)

It applies to those two files only; the rest of Valgrind is GPL-2.0 and is not
vendored.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. The origin of this software must not be misrepresented; you must
   not claim that you wrote the original software.  If you use this
   software in a product, an acknowledgment in the product
   documentation would be appreciated but is not required.

3. Altered source versions must be plainly marked as such, and must
   not be misrepresented as being the original software.

4. The name of the author may not be used to endorse or promote
   products derived from this software without specific prior written
   permission.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS
OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE
GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

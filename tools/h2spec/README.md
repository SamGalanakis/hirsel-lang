# h2spec

HTTP/2 conformance for std http, http2 and hpack: `//:gate-h2spec` runs
[h2spec](https://github.com/summerwind/h2spec) 2.6.0, the latest release, against
`server.hir`, an HTTP/2 prior-knowledge (h2c) server that answers every request with
200 and a short body.

`run` builds the server natively, serves it on a loopback port, runs every h2spec case
(`--strict` adds the one strict case, http2/5.4.1/2, for 147 in all) and compares the
cases that pass with `passing.txt`. A case that stops passing fails the test, and so does
one that starts passing, until it is recorded. A case id is h2spec's group and the case's
position in it, the form h2spec takes as a selector (`h2spec http2/6.9.1/2`).

```sh
.buck2/bin/buck2 test //:gate-h2spec
# Outside Buck2, with a native toolchain in the environment (docs/building.md):
tools/h2spec/run --hirsel <hirsel> --h2spec <h2spec>
tools/h2spec/run --hirsel <hirsel> --h2spec <h2spec> --update-passing
```

## Results

Recorded 2026-10-20 with the server built natively (LLVM): 147 of 147 cases pass
(generic 44, http2 95, hpack 8). There is no failure list.

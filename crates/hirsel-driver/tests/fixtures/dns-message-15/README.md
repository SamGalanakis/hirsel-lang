The Go x/net dnsmessage model, name vectors and complete message vectors are
ported from `dns/dnsmessage/message.go:332-395,2039-2123` and
`message_test.go:188-312,411-484,1517-1672` at
`7e562009b3884a0845019b5f886f0242a7c0b970` (see GO-LICENSE.txt).

`vectors.hir` uses independently spelled wire bytes for every typed record and
all three record sections; checks preserved header bits, classes and unknown
question types; adapts root, google.com, www.google.com and embedded-dot names.
Raw zero/non-UTF-8 labels replace Go's dotted-name restrictions. `go-large.hir`
ports Go's largeTestMsg, retaining OPT as Unknown and refusing reconstruction
because Unknown pointers cannot safely move. `errors.hir` adds exact-offset
malformed wire vectors. `limits.hir` exercises count, TXT length, exact packet
and name-work budgets and 128/129-hop pointer chains. `summary.hir` is the docs
example: it prints a literal packet summary using stdout, without DNS traffic.

The codec differs from Go by explicit fixed ceilings, packed lossless flags,
strict field/packet consumption and deterministic uncompressed reconstruction.
No heuristic scans opaque bytes for compression pointers. Every Unknown record
is rejected on encoding, including empty and apparently uncompressed data.
The focused conformance selector is `conformance/SEM/SEM-3-dns-message-15-run`.

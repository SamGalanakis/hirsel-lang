# tabfold

Reduce 1–32 tab-separated files into CSV statistics by key:

```sh
hirsel run examples/tabfold/hirsel.json --grants /absolute/path/grants.json -- --jobs 2 first.tsv second.tsv
```

Grant only the input directory, Tasks and stdout. Directory hosts in a grants
file resolve relative to that file:

```json
{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"data","host":"/absolute/path/inputs","access":"read"}]},"args":[]}
```

`--jobs N` accepts 1..4 (default 2), including after file operands; `--` ends
options. Each file is at most 64 KiB and 1024 records. A record is a nonempty
UTF-8 key of at most 128 bytes, a tab, and a signed decimal integer between
-1000000000 and 1000000000. Optional +/-, leading zeros and -0 are accepted;
whitespace, extra tabs, empty records and bare CR are rejected. LF and CRLF
are accepted, with an optional final newline. An empty file has no records.

Output begins `key,count,sum,min,max` followed by one CSV row per key, sorted
by UTF-8 bytes. CSV escaping handles commas, quotes and Unicode. Reads follow
operand order in batches of at most jobs files; pure parse/reduce workers run
in a joined scope. Counts and sums use checked arithmetic. Inputs are read only.
The program buffers the complete output and writes once after all files succeed.
Input or grant failure exits nonzero with empty stdout. Output-device errors can
still fail the final write. Diagnostics retain original flag indices or input
ordinal and line; line 0 denotes a file-level/merge error.

Concurrent failures use the first completed error observed by task.all. Replay
preserves that observation without promising a fixed winner between invalid
files. Cancellation joins every child; bounded pure workers may finish first.
No physical parallelism or speedup is claimed for the cooperative provider.

Build a native executable once per backend, then reuse it with
`--hirsel-grants`, `--hirsel-record` and `--hirsel-replay`; see
[the implementation and verification guide](GUIDE.md) and
[native verification](../../docs/native-verification.md). Keep tapes outside
all input grants. The standalone E2E records successes and failures on H, LLVM
and Cranelift and replays each after moving input and grant paths away.

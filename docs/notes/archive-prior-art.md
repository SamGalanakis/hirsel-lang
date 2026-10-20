# Archive hardening reference and vectors

Reference: golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` (BSD-3-Clause,
credited in third-party/NOTICES.md). Cloned for this change. Paths and line
numbers below refer to that commit.

The ZIP directory parser follows `src/archive/zip/reader.go:416-482`
(`parseExtras`) for NTFS tagged attributes, the Unix and Info-ZIP Unix mtime,
and extended timestamps. Offset checks follow `reader.go:599-610`; entry
header offsets are signed file positions in Go. Check that the base and the
entry offset fit before adding them. `file_entries` passes the tail buffer's
absolute origin to the shared directory parser, so directory and ZIP64 locator
offsets remain file-relative even for files larger than the tail buffer.
The file payload reader also checks the ZIP64 compressed-size addition before
reserving its 16-byte descriptor allowance.

The tar decimal bound follows `src/archive/tar/strconv.go:200-214` and
`src/internal/strconv/atoi.go:172-206` (`ParseInt`): negative magnitude may
reach 2^63, positive magnitude stops at 2^63-1. Check each digit before
multiplication or narrowing. PAX fractions use the first nine digits as Go
does (`tar/strconv.go:218-235`). Padding EOF follows Go's separate data discard
and padding read (`tar/reader.go:88-94`): missing data is Truncated; EOF inside
padding ends the archive successfully.

| Reference vectors | Carried tests |
| --- | --- |
| ZIP `reader_test.go:58-584`, all 15 formerly missing TestReader entries | `SEM-3-zip-go-*`; original big.zip in `zip-big.bin` and `zip_big_directory_executes_std_source`; existing `zip_large_vectors_execute_std_source` decodes readme.zip |
| ZIP `reader_test.go:789-808`, TestInvalidFiles | `SEM-3-zip-invalid-signatures-run` and Rust fixture `zip-invalid-zeros.hir`, original 70 KiB inputs; corpus zeros twin is 1 KiB |
| ZIP `reader_test.go:1838-1880`, TestBaseOffsetPlusOverflow | `SEM-3-zip-read-fuzz-offset-run`, original bytes, plus a minimal ZIP64 entry offset with a one-byte prefix; both memory and file readers |
| ZIP tail origin and absolute ZIP64 locator | `SEM-3-zip-file-tail-zip64-run`, memory/file reader parity beyond 65 KiB |
| tar `reader_test.go:817-851`, first 30 TestReadTruncation rows | `SEM-3-tar-read-truncation-{00..29}-run` (06/07 short-refill twins split for budget), exact original bytes and header counts, auto/manual discard, 512/17-byte refills |
| tar `reader_test.go:252,409`, large PAX and GNU incremental inputs | `tar_large_pax_header_executes_std_source` and `SEM-3-tar-read-gnu-incremental-run` |
| tar signed decimal crash class | `SEM-3-tar-read-fuzz-pax-time*-run`, real PAX archives at and beyond signed bounds |

The recursive ZIP is copied from `reader_test.go:851-889`. The big ZIP is
reconstructed exactly as `reader_test.go:1049-1066` from the two nested
members in `biggestZipBytes` (`reader_test.go:959-1046`). Its 4 GiB member is
listed, not expanded, matching Go's large-file shortcut at
`reader_test.go:737-745`.

Deviations from the Go test harness and representation:

- Hirsel timestamps store whole Unix seconds, with no timezone object or
  nanoseconds. Timestamp vectors compare those seconds; `dos_time` preserves
  the independent DOS fields. This preserves the existing public API.
- A negative PAX fraction below i64 min returns Header: its floor cannot fit
  Hirsel's i64 seconds field. The exact minimum and zero fraction are accepted.
- Tar's ten sparse truncation rows remain excluded: sparse entries are an
  explicit Unsupported surface. Go's six Reader/ReadSeeker/bad-seeker wrappers
  become two refill sizes times auto/manual discard; Hirsel exposes refill
  callbacks rather than seekable tar readers.
- ZIP's negative-size constructor vector cannot be expressed: sizes are u64.
  Both byte-expressible TestInvalidFiles rows are preserved exactly; the
  full all-zero scan uses a larger-budget Rust fixture. Independent tar
  assertions are split across corpus programs to fit their per-program budget.
- Big ZIP uses a binary Rust fixture and the file reader to avoid putting a
  4 MiB literal or payload construction under the corpus step budget.
  The original oversized PAX archive also uses a binary Rust fixture to keep
  the large input out of source literals. Readme ZIP's payload remains in the
  existing larger-budget Rust fixture; the corpus adds directory checks on its original bytes.
- Go's NonUTF8 flag, filesystem mode conversion, simultaneous reads, and raw
  reader API have no direct counterpart in the entry model. New vectors check
  names, sizes, timestamp extras and decoded contents/error where exposed.
  Invalid UTF-8 names still return Encoding because Hirsel String is UTF-8.

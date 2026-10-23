# Reopening an append log

`file.open_append(d, p) -> Result[File of d, FileError]` requires the same write
access and confinement as `file.create`. It creates an absent regular file and
never truncates an existing one. Every `file.write`/`file.append` targets the
current end even after seek; reads on the returned write-only handle fail with
`FileFault`. Existing `file.open`, `file.create` and `file.narrow` modes are
unchanged. Opening alone promises no durability; acknowledge a journal write
only after append and `file.sync`.

The host follows Go `src/os/file.go:403–418` and
`src/os/file_unix.go:246–279` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`, with
`O_WRONLY | O_APPEND | O_CREAT`. Differences are Hirsel's existing descriptor
confinement, explicit directory grants, regular-file check, portable typed
errors, cancellation checks and recorded effects. File creation retains the
existing host permission mode `0600`; simulation retains its existing `0666`
mode. This change introduces no general flags API. Go's BSD license is retained
in third-party/NOTICES.md; there is no vendored Go code.

The native entry `hirsel_rt_v2_file_open_append` is additive. Runtime ABI 50
identifies the expanded export set in the runtime, compiler and C header;
preexisting layouts and entry signatures are unchanged. There is no new TIR or
LIR instruction. The binary recording adds `Request::OpenAppend` as tag 114;
old request tags, including Open=52 and Create=53, keep their exact bytes. An
unknown binary request tag refuses through the ordinary tape decoder. Text
inspection spells it `open_append`. Old tapes retain their decoding; ordinary
semantic program-revision binding still applies to executable replay.

Focused fixtures are in the journal_kv_29 CLI module and
`tests/support/journal_kv_29/open_append.hir`: absent/existing reopen, seek before
append, unchanged read-only mode, create truncation, path and grant refusal,
sync/crash/stale-handle recovery and native/reference replay. Free execution
remains unqualified; its exhaustive request classification only gains the
same namespace-exclusive key as Create. No Free policy is enabled.

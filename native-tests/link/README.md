# native-tests/link

`hosts.txt` is the program set of the two-host link gate (`//:gate-link-hosts`,
`tools/buck2/link_hosts.py`). `//:gate-link-hosts-local` builds every row with
`hirsel build` on this host; the gate builds them again on a pool worker, under
a different checkout, native cache and scratch root, and requires every
executable and `<build-id>.debug` file to be byte-identical. A shipped
executable depends on the program, the toolchain and the link kit only.

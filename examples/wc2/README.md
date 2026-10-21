Count LF, ASCII-whitespace words and UTF-8 bytes across files/`-` stdin; `-u` sorts unique lines. Lines are bounded at 1 MiB; errors go to stderr and exit 1.
`hirsel test examples/wc2/hirsel.json`
`hirsel run examples/wc2/hirsel.json --grants examples/wc2/grants.json --record /tmp/wc2.tape`
`hirsel run --native examples/wc2/hirsel.json --grants examples/wc2/grants.json`
`hirsel run examples/wc2/hirsel.json --replay /tmp/wc2.tape` (also accepts `--native`).

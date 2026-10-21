`FILE REGEX [EVENTS]` prints matching complete lines with UTC timestamps; watches the parent for growth/replacement, retains partial lines and resets on truncation. The sample scans once (`EVENTS=0`).
`hirsel test examples/logtail/hirsel.json`
`hirsel run examples/logtail/hirsel.json --grants examples/logtail/grants.json --record /tmp/logtail.tape`
`hirsel run --native examples/logtail/hirsel.json --grants examples/logtail/grants.json`
`hirsel run examples/logtail/hirsel.json --replay /tmp/logtail.tape` (also accepts `--native`).

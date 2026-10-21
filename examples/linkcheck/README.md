`JOBS URL...` checks URLs with 1–32 live workers, redirects, channels and a two-second deadline. Status 0 reports timeout/fetch failure. The grants fixture scripts a redirect and a 404.
`hirsel test examples/linkcheck/hirsel.json`
`hirsel run examples/linkcheck/hirsel.json --grants examples/linkcheck/grants.json --record /tmp/linkcheck.tape`
`hirsel run --native examples/linkcheck/hirsel.json --grants examples/linkcheck/grants.json`
`hirsel run examples/linkcheck/hirsel.json --replay /tmp/linkcheck.tape` (also accepts `--native`).

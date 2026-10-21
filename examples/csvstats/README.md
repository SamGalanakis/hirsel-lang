Group `data/input.csv` (`group,value` header) and write `data/report.json`; derived JSON reports count, sum and mean using finite binary64 values. Groups keep first-seen order.
`hirsel test examples/csvstats/hirsel.json`
`hirsel run examples/csvstats/hirsel.json --grants examples/csvstats/grants.json --record /tmp/csvstats.tape`
`hirsel run --native examples/csvstats/hirsel.json --grants examples/csvstats/grants.json`
`hirsel run examples/csvstats/hirsel.json --replay /tmp/csvstats.tape` (also accepts `--native`).

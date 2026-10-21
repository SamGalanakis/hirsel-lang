# Bounded log summary with graceful stop

Count lines and the exact byte prefixes `ERROR ` and `WARN `, including a final
line without LF. Scan at most 1 MiB per snapshot through borrowed 4 KiB bufio
views, retaining counters rather than lines. One argument scans once; a second
`--follow` watches the mount and rescans on events until SIGINT or SIGTERM.
A missing file or failed rescan exits with an error.

```sh
mkdir -p examples/logdigest/data
printf 'ERROR broken\nWARN slow\nINFO ok' > examples/logdigest/data/app.log
hirsel test examples/logdigest/hirsel.json
hirsel run examples/logdigest/hirsel.json --grants examples/logdigest/grants.json
hirsel run examples/logdigest/hirsel.json --native --backend llvm --grants examples/logdigest/grants.json
hirsel run examples/logdigest/hirsel.json --native --backend cranelift --grants examples/logdigest/grants.json
python3 examples/verify-dogfood6.py --hirsel "$(command -v hirsel)" --tool logdigest
```

The shipped scripted signal stops after the initial summary. For interactive
following, set `io.signals` to
`{"provider":"signals.host@1","kinds":["interrupt","terminate"]}`.
Three tests cover chunk boundaries, final/empty lines, missing files and confined
paths. The verifier records graceful stop and a missing-file error and compares
stdout, stderr and terminal outcomes under grant-free replay in all engines.

When launching a built executable, supply its program arguments explicitly:
`./logdigest --hirsel-grants examples/logdigest/grants.json app.log --follow`.
Its process arguments replace the template's `args`, including when empty.

Find and replay a lost update between two tasks:

```sh
hirsel test examples/race/hirsel.json                 # fifo@1: passes
hirsel test examples/race/hirsel.json --explore 20    # random@1: fails at seed 1
hirsel test examples/race/hirsel.json --explore 20 --scheduler pct@1
hirsel test examples/race/hirsel.json race@1/both_additions_land --scheduler random@1:1
```

`count` spawns two tasks that each load the shared counter and store one more. Every
atomic operation is a scheduling point. Under the default `fifo@1` schedule the first
task stores before the second loads, so the test passes; a seeded schedule that runs both
loads first loses one addition, and `test.check` fails. `--explore` runs the test under
seeds 0, 1, ... of the policy, stops at the first failing schedule and prints a `replay:`
command that reruns exactly that schedule, interpreted or with `--native`.
